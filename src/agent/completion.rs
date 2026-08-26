use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    semantic::{SemanticEvaluator, SemanticResult},
    storage::ToolRunRecord,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Informational,
    Action,
    Inspection,
    Modification,
    Installation,
    Verification,
    Mixed,
}

impl TaskKind {
    pub fn is_action_like(self) -> bool {
        self != Self::Informational
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    VerifiedSuccess,
    NotYetVerified,
    Blocked,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletionEvidence {
    pub state: VerificationState,
    pub task_kind: TaskKind,
    /// Compatibility projection for older callers. New control flow uses
    /// `state`, never this boolean alone.
    pub verified: bool,
    pub summary: String,
    pub succeeded_tools: usize,
    pub unresolved_tool_failures: usize,
    pub observable_evidence: Vec<String>,
}

#[derive(Clone)]
pub struct CompletionVerifier {
    semantic: Arc<SemanticEvaluator>,
}

impl Default for CompletionVerifier {
    fn default() -> Self {
        Self {
            semantic: Arc::new(SemanticEvaluator::deterministic()),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskIntentDecision {
    task_kind: TaskKind,
    #[serde(default)]
    required_evidence: Vec<String>,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SemanticCompletionDecision {
    satisfied: bool,
    #[serde(default)]
    missing_evidence: Vec<String>,
    #[serde(default)]
    reason: String,
}

impl CompletionVerifier {
    pub fn with_semantic(semantic: Arc<SemanticEvaluator>) -> Self {
        Self { semantic }
    }

    pub fn classify(&self, goal: &str, tool_runs: &[ToolRunRecord]) -> TaskKind {
        let clean_goal = strip_attachment_envelope(goal);
        match self.semantic.evaluate::<TaskIntentDecision>(
            "task_intent",
            serde_json::json!({
                "type":"object",
                "additionalProperties":false,
                "required":["task_kind","required_evidence","reason"],
                "properties":{
                    "task_kind":{"enum":["informational","inspection","action","modification","installation","verification","mixed"]},
                    "required_evidence":{"type":"array","maxItems":8,"items":{"type":"string","maxLength":300}},
                    "reason":{"type":"string","maxLength":500}
                }
            }),
            serde_json::json!({
                "goal": clean_goal,
                "observed_tools": tool_runs.iter().take(64).map(|run| serde_json::json!({
                    "tool":run.tool_name,"risk":run.risk,"status":run.status
                })).collect::<Vec<_>>()
            }),
        ) {
            SemanticResult::Valid(decision)
                if decision.required_evidence.len() <= 8
                    && decision.reason.chars().count() <= 500 =>
            {
                return decision.task_kind;
            }
            SemanticResult::Malformed | SemanticResult::Unavailable | SemanticResult::Valid(_) => {}
        }
        deterministic_task_kind(clean_goal, tool_runs)
    }

    /// Provider-backed semantic evaluation bridges an async provider through
    /// a synchronous, schema-validating boundary. Keep that work off Tokio's
    /// async workers so a slow evaluator cannot stall Telegram cancellation,
    /// callbacks, or another owner's request.
    pub async fn classify_async(&self, goal: &str, tool_runs: &[ToolRunRecord]) -> TaskKind {
        let evaluator = self.clone();
        let goal = goal.to_owned();
        let tool_runs = tool_runs.to_vec();
        tokio::task::spawn_blocking(move || evaluator.classify(&goal, &tool_runs))
            .await
            .unwrap_or(TaskKind::Action)
    }

    fn semantic_completion(
        &self,
        goal: &str,
        task_kind: TaskKind,
        final_answer: &str,
        evidence: &[String],
    ) -> SemanticResult<SemanticCompletionDecision> {
        self.semantic.evaluate(
            "completion_interpretation",
            serde_json::json!({
                "type":"object",
                "additionalProperties":false,
                "required":["satisfied","missing_evidence","reason"],
                "properties":{
                    "satisfied":{"type":"boolean"},
                    "missing_evidence":{"type":"array","maxItems":8,"items":{"type":"string","maxLength":400}},
                    "reason":{"type":"string","maxLength":800}
                }
            }),
            serde_json::json!({
                "goal":goal,
                "task_kind":task_kind,
                "candidate_final_answer":final_answer,
                "hard_validated_observations":evidence,
                "hard_rule":"Action-like tasks cannot succeed from model text alone. Treat supplied observations as facts; do not invent evidence."
            }),
        )
    }

    /// Deterministic fallback remains security conservative. Semantic output
    /// may refine intent and missing evidence, but can never manufacture tool
    /// evidence or override approval/policy failures.
    pub fn deterministic_classify(&self, goal: &str, tool_runs: &[ToolRunRecord]) -> TaskKind {
        deterministic_task_kind(goal, tool_runs)
    }
}

fn is_informational_or_code_example(clean_goal: &str) -> bool {
    let lower = clean_goal.to_ascii_lowercase();
    let question_or_example_markers = [
        "how to",
        "how do",
        "how can",
        "how does",
        "what is",
        "what are",
        "what's",
        "why",
        "explain",
        "describe",
        "tell me",
        "show me",
        "give me",
        "code example",
        "example",
        "sample",
        "tutorial",
        "guide",
        "contoh",
        "jelaskan",
        "bagaimana",
        "apa itu",
        "mengapa",
        "tunjukkan cara",
        "berikan contoh",
    ];
    let is_query = question_or_example_markers
        .iter()
        .any(|m| lower.contains(m))
        || lower.trim().ends_with('?');
    let explicit_actions = [
        "run this",
        "execute this",
        "compile this",
        "install this",
        "build the project",
        "create file",
        "write to file",
        "save to file",
        "save as",
        "save it to",
        "edit file",
        "delete file",
        "update the file",
        "fix the bug in",
        "jalankan script",
        "buat file",
        "simpan ke file",
        "tulis ke file",
        "pasang di sistem",
        "perbaiki file",
    ];
    let has_explicit_action = explicit_actions.iter().any(|a| lower.contains(a));
    is_query && !has_explicit_action
}

fn deterministic_task_kind(goal: &str, tool_runs: &[ToolRunRecord]) -> TaskKind {
    let clean_goal = strip_attachment_envelope(goal);
    let observed_side_effect = tool_runs
        .iter()
        .any(|run| run.risk != "read_only" && run.risk != "unknown");
    if is_informational_or_code_example(clean_goal) && !observed_side_effect {
        return TaskKind::Informational;
    }
    let normalized = clean_goal.to_ascii_lowercase();
    let installation = ["install", "package", "dependency", "pasang"]
        .iter()
        .any(|marker| normalized.contains(marker));
    let modification = [
        "implement",
        "build",
        "create",
        "write",
        "edit",
        "change",
        "update",
        "delete",
        "remove",
        "configure",
        "restart",
        "run ",
        "execute",
        "extract",
        "convert",
        "download",
        "upload",
        "send ",
        "fix ",
        "repair",
        "buat",
        "ubah",
        "hapus",
        "pasang",
        "jalankan",
        "perbaiki",
    ]
    .iter()
    .any(|marker| normalized.contains(marker));
    let inspection = [
        "inspect",
        "diagnose",
        "investigate",
        "check",
        "status",
        "periksa",
        "telusuri",
    ]
    .iter()
    .any(|marker| normalized.contains(marker));
    let verification = ["verify", "validate", "prove", "pastikan", "verifikasi"]
        .iter()
        .any(|marker| normalized.contains(marker));
    let kinds = usize::from(installation)
        + usize::from(modification || observed_side_effect)
        + usize::from(inspection)
        + usize::from(verification);
    if kinds > 1 {
        TaskKind::Mixed
    } else if installation {
        TaskKind::Installation
    } else if modification || observed_side_effect {
        TaskKind::Modification
    } else if verification {
        TaskKind::Verification
    } else if inspection {
        TaskKind::Inspection
    } else {
        TaskKind::Informational
    }
}

impl CompletionVerifier {
    /// Compatibility entry point. With no explicit goal, side-effect audit
    /// records still classify a task as an action.
    pub fn verify(&self, final_answer: &str, tool_runs: &[ToolRunRecord]) -> CompletionEvidence {
        self.verify_for_task("", final_answer, tool_runs)
    }

    /// Evaluate observable state only. Hidden reasoning and a model's bare
    /// textual claim of completion are never accepted as action evidence.
    pub fn verify_for_task(
        &self,
        goal: &str,
        final_answer: &str,
        tool_runs: &[ToolRunRecord],
    ) -> CompletionEvidence {
        self.verify_for_task_with_images(
            goal,
            final_answer,
            tool_runs,
            has_attachment_context(goal),
        )
    }

    pub fn verify_for_task_with_images(
        &self,
        goal: &str,
        final_answer: &str,
        tool_runs: &[ToolRunRecord],
        has_images: bool,
    ) -> CompletionEvidence {
        let task_kind = self.classify(goal, tool_runs);
        let succeeded_tools = tool_runs
            .iter()
            .filter(|run| run.status == "succeeded")
            .count();
        let unresolved = unresolved_failures(tool_runs);
        let unfinished = tool_runs.iter().any(|run| {
            matches!(
                run.status.as_str(),
                "requested" | "policy_check" | "installing_dependency" | "running"
            )
        });
        let approval = tool_runs
            .iter()
            .rev()
            .find(|run| run.status == "awaiting_approval");

        if let Some(approval) = approval {
            return evidence(
                VerificationState::Blocked,
                task_kind,
                format!(
                    "owner approval is required for {}: {}",
                    approval.tool_name,
                    approval.error.as_deref().unwrap_or("approval pending")
                ),
                succeeded_tools,
                unresolved,
                Vec::new(),
            );
        }
        if final_answer.trim().is_empty() {
            return evidence(
                VerificationState::NotYetVerified,
                task_kind,
                "provider produced no final observable result".into(),
                succeeded_tools,
                unresolved,
                Vec::new(),
            );
        }
        if unfinished {
            return evidence(
                VerificationState::NotYetVerified,
                task_kind,
                "tool audit contains unfinished execution".into(),
                succeeded_tools,
                unresolved,
                Vec::new(),
            );
        }
        if unresolved > 0 {
            let terminal_denial = tool_runs.iter().any(|run| {
                matches!(run.status.as_str(), "denied" | "interrupted")
                    && !tool_runs
                        .iter()
                        .skip_while(|candidate| candidate.id != run.id)
                        .skip(1)
                        .any(|later| is_relevant_recovery(run, later))
            });
            if terminal_denial {
                return evidence(
                    VerificationState::Failed,
                    task_kind,
                    format!("{unresolved} tool failure(s) remain unresolved (denied/interrupted)"),
                    succeeded_tools,
                    unresolved,
                    Vec::new(),
                );
            }
        }
        if !task_kind.is_action_like() {
            return evidence(
                VerificationState::VerifiedSuccess,
                task_kind,
                if tool_runs.is_empty() {
                    if has_images {
                        "informational vision answer is present; no external action was requested"
                            .into()
                    } else {
                        "informational answer is present; no external action was requested".into()
                    }
                } else if unresolved > 0 {
                    format!("informational answer is present; {succeeded_tools} observation(s) recorded, {unresolved} exploratory tool failure(s) did not block answer")
                } else {
                    format!("{succeeded_tools} read-only observation(s) support the answer")
                },
                succeeded_tools,
                unresolved,
                tool_runs
                    .iter()
                    .filter(|run| run.status == "succeeded")
                    .map(observation)
                    .collect(),
            );
        }
        if unresolved > 0 {
            return evidence(
                VerificationState::NotYetVerified,
                task_kind,
                format!("{unresolved} tool failure(s) remain unresolved"),
                succeeded_tools,
                unresolved,
                Vec::new(),
            );
        }

        if matches!(task_kind, TaskKind::Inspection | TaskKind::Verification) {
            let observations = tool_runs
                .iter()
                .filter(|run| run.status == "succeeded" && run.risk == "read_only")
                .map(observation)
                .collect::<Vec<_>>();
            if observations.is_empty() {
                if has_images {
                    return evidence(
                        VerificationState::VerifiedSuccess,
                        task_kind,
                        "visual inspection completed directly from input image(s)".into(),
                        succeeded_tools,
                        0,
                        Vec::new(),
                    );
                }
                return evidence(
                    VerificationState::NotYetVerified,
                    task_kind,
                    "inspection/verification task has no successful observable probe".into(),
                    succeeded_tools,
                    0,
                    Vec::new(),
                );
            }
            if let SemanticResult::Valid(semantic) =
                self.semantic_completion(goal, task_kind, final_answer, &observations)
            {
                if !semantic.satisfied {
                    return evidence(
                        VerificationState::NotYetVerified,
                        task_kind,
                        format!(
                            "inspection evidence is incomplete: {}",
                            semantic.missing_evidence.join("; ")
                        ),
                        succeeded_tools,
                        0,
                        observations,
                    );
                }
            }
            return evidence(
                VerificationState::VerifiedSuccess,
                task_kind,
                format!(
                    "inspection completed with {} observation(s)",
                    observations.len()
                ),
                succeeded_tools,
                0,
                observations,
            );
        }

        let side_effect_positions = tool_runs
            .iter()
            .enumerate()
            .filter(|(_, run)| {
                run.status == "succeeded" && !matches!(run.risk.as_str(), "read_only" | "unknown")
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if side_effect_positions.is_empty() {
            return evidence(
                VerificationState::NotYetVerified,
                task_kind,
                "action task has no successful observable action yet".into(),
                succeeded_tools,
                0,
                Vec::new(),
            );
        }
        let first_action = side_effect_positions[0];
        let verification_runs = tool_runs
            .iter()
            .enumerate()
            .filter(|(index, run)| {
                run.status == "succeeded"
                    && (self_verified_operation(run)
                        || *index > first_action
                            && (declared_verification(run) || verification_tool(&run.tool_name)))
            })
            .map(|(_, run)| run)
            .collect::<Vec<_>>();
        if verification_runs.is_empty() {
            return evidence(
                VerificationState::NotYetVerified,
                task_kind,
                "the action ran, but no independent observable verification was recorded".into(),
                succeeded_tools,
                0,
                side_effect_positions
                    .iter()
                    .map(|index| observation(&tool_runs[*index]))
                    .collect(),
            );
        }
        let observations = verification_runs
            .iter()
            .map(|run| observation(run))
            .collect::<Vec<_>>();
        if let SemanticResult::Valid(semantic) =
            self.semantic_completion(goal, task_kind, final_answer, &observations)
        {
            if !semantic.satisfied {
                let missing = semantic
                    .missing_evidence
                    .into_iter()
                    .take(8)
                    .collect::<Vec<_>>()
                    .join("; ");
                return evidence(
                    VerificationState::NotYetVerified,
                    task_kind,
                    if missing.is_empty() {
                        format!(
                            "semantic completion check needs more evidence: {}",
                            semantic.reason
                        )
                    } else {
                        format!("missing semantic completion evidence: {missing}")
                    },
                    succeeded_tools,
                    0,
                    observations,
                );
            }
        }
        evidence(
            VerificationState::VerifiedSuccess,
            task_kind,
            format!(
                "action completed with {} successful action(s) and {} verification observation(s)",
                side_effect_positions.len(),
                verification_runs.len()
            ),
            succeeded_tools,
            0,
            observations,
        )
    }

    /// Async agent-loop entry point. A failed blocking worker is treated
    /// conservatively using the deterministic verifier rather than promoting
    /// an unobserved action to success.
    pub async fn verify_for_task_async(
        &self,
        goal: &str,
        final_answer: &str,
        tool_runs: &[ToolRunRecord],
    ) -> CompletionEvidence {
        self.verify_for_task_with_images_async(
            goal,
            final_answer,
            tool_runs,
            has_attachment_context(goal),
        )
        .await
    }

    pub async fn verify_for_task_with_images_async(
        &self,
        goal: &str,
        final_answer: &str,
        tool_runs: &[ToolRunRecord],
        has_images: bool,
    ) -> CompletionEvidence {
        let evaluator = self.clone();
        let owned_goal = goal.to_owned();
        let owned_answer = final_answer.to_owned();
        let owned_runs = tool_runs.to_vec();
        match tokio::task::spawn_blocking(move || {
            evaluator.verify_for_task_with_images(
                &owned_goal,
                &owned_answer,
                &owned_runs,
                has_images,
            )
        })
        .await
        {
            Ok(evidence) => evidence,
            Err(_) => CompletionVerifier::default().verify_for_task_with_images(
                goal,
                final_answer,
                tool_runs,
                has_images,
            ),
        }
    }
}

fn strip_attachment_envelope(goal: &str) -> &str {
    let trimmed = goal.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("attachment received:") {
        if let Some(pos) = trimmed.find("). ") {
            return trimmed[pos + 3..].trim();
        }
        if let Some(pos) = trimmed.find(").") {
            return trimmed[pos + 2..].trim();
        }
    }
    trimmed
}

fn has_attachment_context(goal: &str) -> bool {
    let lower = goal.to_ascii_lowercase();
    lower.starts_with("attachment received:")
        || lower.contains("gambar ini")
        || lower.contains("gambar tadi")
        || lower.contains("foto ini")
        || lower.contains("foto tadi")
        || lower.contains("this image")
        || lower.contains("this photo")
        || lower.contains("attached image")
        || lower.contains("screenshot")
}

fn evidence(
    state: VerificationState,
    task_kind: TaskKind,
    summary: String,
    succeeded_tools: usize,
    unresolved_tool_failures: usize,
    observable_evidence: Vec<String>,
) -> CompletionEvidence {
    CompletionEvidence {
        verified: state == VerificationState::VerifiedSuccess,
        state,
        task_kind,
        summary,
        succeeded_tools,
        unresolved_tool_failures,
        observable_evidence,
    }
}

fn is_relevant_recovery(failed: &ToolRunRecord, later: &ToolRunRecord) -> bool {
    if later.status != "succeeded" {
        return false;
    }
    if failed.status == "denied" {
        return false;
    }
    if failed.tool_name == later.tool_name {
        return true;
    }
    let is_terminal = |name: &str| matches!(name, "termux_terminal" | "termux_job");
    if is_terminal(&failed.tool_name) && is_terminal(&later.tool_name) {
        return true;
    }
    let is_read_or_inspect =
        |run: &ToolRunRecord| run.risk == "read_only" || verification_tool(&run.tool_name);
    if is_read_or_inspect(failed) && is_read_or_inspect(later) {
        return true;
    }
    if failed.risk == "side_effect" && later.risk == "side_effect" {
        return true;
    }
    false
}

fn unresolved_failures(tool_runs: &[ToolRunRecord]) -> usize {
    tool_runs
        .iter()
        .enumerate()
        .filter(|(index, run)| {
            matches!(run.status.as_str(), "failed" | "denied" | "interrupted")
                && !tool_runs[index + 1..]
                    .iter()
                    .any(|later| is_relevant_recovery(run, later))
        })
        .count()
}

/// A typed tool may perform and report its own postcondition check (for
/// example, AndroidBroker re-probing service state after restart).
fn self_verified_operation(run: &ToolRunRecord) -> bool {
    let output = run.output.as_deref().unwrap_or_default();
    output.contains("\"verified\":true")
}

/// General executors can label a later, separate observation as verification,
/// but that label cannot make the same call count as both action and proof.
fn declared_verification(run: &ToolRunRecord) -> bool {
    let output = run.output.as_deref().unwrap_or_default();
    output.contains("\"verification_evidence\":true") || output.contains("\"exists\":true")
}

fn verification_tool(name: &str) -> bool {
    [
        "verify", "check", "status", "test", "inspect", "probe", "stat", "view", "search",
    ]
    .iter()
    .any(|marker| name.contains(marker))
}

fn observation(run: &ToolRunRecord) -> String {
    let detail = run
        .output
        .as_deref()
        .or(run.error.as_deref())
        .unwrap_or("no bounded output");
    let detail = if detail.chars().count() > 240 {
        detail.chars().take(240).collect::<String>() + "…"
    } else {
        detail.to_owned()
    };
    format!("{}: {}", run.tool_name, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::{SemanticBackend, SemanticRequest};

    struct FixedSemantic(&'static str);

    impl SemanticBackend for FixedSemantic {
        fn evaluate(&self, _: &SemanticRequest) -> anyhow::Result<String> {
            Ok(self.0.into())
        }
    }

    fn run(name: &str, risk: &str, status: &str, output: Option<&str>) -> ToolRunRecord {
        ToolRunRecord {
            id: format!("{name}-{status}"),
            agent_run_id: "r".into(),
            call_id: "c".into(),
            tool_name: name.into(),
            arguments_json: "{}".into(),
            risk: risk.into(),
            approval_mode: None,
            policy_original: None,
            status: status.into(),
            output: output.map(str::to_owned),
            error: None,
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn unresolved_failure_is_not_yet_verified_and_changed_strategy_recovers() {
        let verifier = CompletionVerifier::default();
        assert_eq!(
            verifier
                .verify("done", &[run("search", "read_only", "failed", None)])
                .state,
            VerificationState::NotYetVerified
        );
        let recovered = verifier.verify(
            "done",
            &[
                run("search", "read_only", "failed", None),
                run("inspect", "read_only", "succeeded", Some("found")),
            ],
        );
        assert_eq!(recovered.state, VerificationState::VerifiedSuccess);
    }

    #[test]
    fn information_answer_can_complete_without_action_evidence() {
        let evidence = CompletionVerifier::default().verify_for_task(
            "Explain Rust",
            "Here is the answer",
            &[],
        );
        assert_eq!(evidence.state, VerificationState::VerifiedSuccess);
        assert_eq!(evidence.task_kind, TaskKind::Informational);
    }

    #[test]
    fn action_claim_without_evidence_is_not_yet_verified() {
        let evidence =
            CompletionVerifier::default().verify_for_task("Create the file", "Done", &[]);
        assert_eq!(evidence.state, VerificationState::NotYetVerified);
        assert!(!evidence.verified);
        let action_only = CompletionVerifier::default().verify_for_task(
            "Create the file",
            "Done",
            &[run("write_file", "side_effect", "succeeded", Some("ok"))],
        );
        assert_eq!(action_only.state, VerificationState::NotYetVerified);
        let same_call_claim = CompletionVerifier::default().verify_for_task(
            "Create the file",
            "Done",
            &[run(
                "termux_terminal",
                "side_effect",
                "succeeded",
                Some("{\"verification_evidence\":true}"),
            )],
        );
        assert_eq!(same_call_claim.state, VerificationState::NotYetVerified);
        let verified = CompletionVerifier::default().verify_for_task(
            "Create the file",
            "Done",
            &[
                run("write_file", "side_effect", "succeeded", Some("ok")),
                run(
                    "file_check",
                    "read_only",
                    "succeeded",
                    Some("{\"exists\":true}"),
                ),
            ],
        );
        assert_eq!(verified.state, VerificationState::VerifiedSuccess);
    }

    #[test]
    fn semantic_intent_handles_action_wording_outside_deterministic_markers() {
        let goal = "I want a fresh manifest to materialize beside the release artifact";
        assert_eq!(
            CompletionVerifier::default().deterministic_classify(goal, &[]),
            TaskKind::Informational
        );
        let semantic = Arc::new(SemanticEvaluator::with_backend(Arc::new(FixedSemantic(
            r#"{"task_kind":"modification","required_evidence":["manifest exists"],"reason":"the owner requests a new artifact state"}"#,
        ))));
        assert_eq!(
            CompletionVerifier::with_semantic(semantic).classify(goal, &[]),
            TaskKind::Modification
        );
    }

    #[test]
    fn approval_is_a_blocker_not_success_or_generic_failure() {
        let mut approval = run("android_restart", "privileged", "awaiting_approval", None);
        approval.error = Some("approval request 123".into());
        let evidence =
            CompletionVerifier::default().verify_for_task("Restart Xiao", "Waiting", &[approval]);
        assert_eq!(evidence.state, VerificationState::Blocked);
    }

    #[test]
    fn photo_caption_apa_ini_with_envelope_is_informational_and_verified() {
        let goal =
            "Attachment received: photo-1.jpg (id=att-1, type=image/jpeg, status=ready). Apa ini";
        let verifier = CompletionVerifier::default();
        assert_eq!(verifier.classify(goal, &[]), TaskKind::Informational);
        let evidence = verifier.verify_for_task(goal, "Ini adalah gambar bunga mawar merah.", &[]);
        assert_eq!(evidence.state, VerificationState::VerifiedSuccess);
        assert_eq!(evidence.task_kind, TaskKind::Informational);
        assert!(evidence.verified);
    }

    #[test]
    fn photo_caption_visual_inspection_with_images_is_verified() {
        let goal =
            "Attachment received: photo-1.jpg (id=att-1, type=image/jpeg, status=ready). Periksa gambar ini";
        let verifier = CompletionVerifier::default();
        let evidence = verifier.verify_for_task_with_images(
            goal,
            "Gambar ini menunjukkan log sistem tanpa kesalahan.",
            &[],
            true,
        );
        assert_eq!(evidence.state, VerificationState::VerifiedSuccess);
        assert_eq!(evidence.task_kind, TaskKind::Inspection);
        assert!(evidence.verified);
    }

    #[test]
    fn inspection_without_images_or_tools_remains_not_yet_verified() {
        let goal = "Periksa log sistem";
        let verifier = CompletionVerifier::default();
        let evidence = verifier.verify_for_task_with_images(goal, "Log terlihat baik.", &[], false);
        assert_eq!(evidence.state, VerificationState::NotYetVerified);
        assert!(!evidence.verified);
    }

    #[test]
    fn action_with_images_still_requires_action_and_verification_tools() {
        let goal = "Attachment received: photo-1.jpg (id=att-1, type=image/jpeg, status=ready). Create the file result.txt with summary";
        let verifier = CompletionVerifier::default();
        let evidence = verifier.verify_for_task_with_images(goal, "Done", &[], true);
        assert_eq!(evidence.state, VerificationState::NotYetVerified);
        assert!(!evidence.verified);
    }

    #[test]
    fn informational_code_example_with_exploratory_failure_succeeds_without_blocked() {
        let verifier = CompletionVerifier::default();
        let goal = "Show me a code example for binary search in Rust";
        assert_eq!(verifier.classify(goal, &[]), TaskKind::Informational);
        let failed_search = run("search", "read_only", "failed", Some("file not found"));
        let evidence = verifier.verify_for_task(
            goal,
            "```rust
fn binary_search<T: Ord>(slice: &[T], target: &T) -> Option<usize> { ... }
```",
            &[failed_search],
        );
        assert_eq!(evidence.state, VerificationState::VerifiedSuccess);
        assert_eq!(evidence.task_kind, TaskKind::Informational);
        assert!(evidence.verified);
    }

    #[test]
    fn informational_with_privileged_denial_remains_failed() {
        let verifier = CompletionVerifier::default();
        let goal = "Explain how to restart service";
        let denied_run = run(
            "android_restart",
            "privileged",
            "denied",
            Some("denied by owner"),
        );
        let evidence =
            verifier.verify_for_task(goal, "To restart service, run the command.", &[denied_run]);
        assert_eq!(evidence.state, VerificationState::Failed);
    }

    #[test]
    fn action_failure_not_resolved_by_unrelated_readonly_tool() {
        let verifier = CompletionVerifier::default();
        let goal = "Build the binary";
        let build_failed = run(
            "termux_terminal",
            "side_effect",
            "failed",
            Some("syntax error"),
        );
        let file_check = run(
            "file_check",
            "read_only",
            "succeeded",
            Some(r#"{"exists":false}"#),
        );
        let evidence = verifier.verify_for_task(goal, "Done", &[build_failed, file_check]);
        assert_eq!(evidence.state, VerificationState::NotYetVerified);
        assert!(!evidence.verified);
        assert_eq!(evidence.unresolved_tool_failures, 1);

        let build_fixed = run(
            "termux_terminal",
            "side_effect",
            "succeeded",
            Some("compiled successfully"),
        );
        let evidence_recovered = verifier.verify_for_task(
            goal,
            "Done",
            &[
                run("termux_terminal", "side_effect", "failed", None),
                build_fixed,
                run(
                    "file_check",
                    "read_only",
                    "succeeded",
                    Some(r#"{"exists":true}"#),
                ),
            ],
        );
        assert_eq!(evidence_recovered.state, VerificationState::VerifiedSuccess);
    }
}
