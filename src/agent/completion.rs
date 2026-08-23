use serde::{Deserialize, Serialize};

use crate::storage::ToolRunRecord;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Informational,
    Action,
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

#[derive(Debug, Clone, Copy, Default)]
pub struct CompletionVerifier;

impl CompletionVerifier {
    pub fn classify(&self, goal: &str, tool_runs: &[ToolRunRecord]) -> TaskKind {
        let normalized = goal.to_ascii_lowercase();
        let action_marker = [
            "implement",
            "build",
            "create",
            "write",
            "edit",
            "change",
            "update",
            "delete",
            "remove",
            "install",
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
        let observed_side_effect = tool_runs
            .iter()
            .any(|run| run.risk != "read_only" && run.risk != "unknown");
        if action_marker || observed_side_effect {
            TaskKind::Action
        } else {
            TaskKind::Informational
        }
    }

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
                        .any(|later| later.status == "succeeded")
            });
            return evidence(
                if terminal_denial {
                    VerificationState::Failed
                } else {
                    VerificationState::NotYetVerified
                },
                task_kind,
                format!("{unresolved} tool failure(s) remain unresolved"),
                succeeded_tools,
                unresolved,
                Vec::new(),
            );
        }
        if task_kind == TaskKind::Informational {
            return evidence(
                VerificationState::VerifiedSuccess,
                task_kind,
                if tool_runs.is_empty() {
                    "informational answer is present; no external action was requested".into()
                } else {
                    format!("{succeeded_tools} read-only observation(s) support the answer")
                },
                succeeded_tools,
                0,
                tool_runs
                    .iter()
                    .filter(|run| run.status == "succeeded")
                    .map(observation)
                    .collect(),
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
            verification_runs.into_iter().map(observation).collect(),
        )
    }
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

fn unresolved_failures(tool_runs: &[ToolRunRecord]) -> usize {
    tool_runs
        .iter()
        .enumerate()
        .filter(|(index, run)| {
            matches!(run.status.as_str(), "failed" | "denied" | "interrupted")
                && !tool_runs[index + 1..]
                    .iter()
                    .any(|later| later.status == "succeeded")
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

    fn run(name: &str, risk: &str, status: &str, output: Option<&str>) -> ToolRunRecord {
        ToolRunRecord {
            id: format!("{name}-{status}"),
            agent_run_id: "r".into(),
            call_id: "c".into(),
            tool_name: name.into(),
            arguments_json: "{}".into(),
            risk: risk.into(),
            status: status.into(),
            output: output.map(str::to_owned),
            error: None,
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn unresolved_failure_is_not_yet_verified_and_changed_strategy_recovers() {
        let verifier = CompletionVerifier;
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
        let evidence =
            CompletionVerifier.verify_for_task("Explain Rust", "Here is the answer", &[]);
        assert_eq!(evidence.state, VerificationState::VerifiedSuccess);
        assert_eq!(evidence.task_kind, TaskKind::Informational);
    }

    #[test]
    fn action_claim_without_evidence_is_not_yet_verified() {
        let evidence = CompletionVerifier.verify_for_task("Create the file", "Done", &[]);
        assert_eq!(evidence.state, VerificationState::NotYetVerified);
        assert!(!evidence.verified);
        let action_only = CompletionVerifier.verify_for_task(
            "Create the file",
            "Done",
            &[run("write_file", "side_effect", "succeeded", Some("ok"))],
        );
        assert_eq!(action_only.state, VerificationState::NotYetVerified);
        let same_call_claim = CompletionVerifier.verify_for_task(
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
        let verified = CompletionVerifier.verify_for_task(
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
    fn approval_is_a_blocker_not_success_or_generic_failure() {
        let mut approval = run("android_restart", "privileged", "awaiting_approval", None);
        approval.error = Some("approval request 123".into());
        let evidence = CompletionVerifier.verify_for_task("Restart Xiao", "Waiting", &[approval]);
        assert_eq!(evidence.state, VerificationState::Blocked);
    }
}
