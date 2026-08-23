use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{
    memory::MemoryEvaluator,
    semantic::{SemanticEvaluator, SemanticResult},
    skills::{canonical_skill_name, SkillCandidate, SkillMutation, SkillRegistry},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SafeToolObservation {
    pub tool: String,
    pub risk: String,
    pub status: String,
    /// Bounded redacted arguments/operation description, not hidden reasoning.
    #[serde(default)]
    pub operation: String,
    pub observable_summary: String,
    #[serde(default)]
    pub verification: bool,
}

/// A bounded observable trace. There is intentionally no reasoning or
/// chain-of-thought field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LearningTrace {
    pub run_status: String,
    pub verified: bool,
    pub meaningful: bool,
    pub reusable: bool,
    pub user_goal: String,
    pub session_id: String,
    pub tool_observations: Vec<SafeToolObservation>,
    #[serde(default)]
    pub installed_dependencies: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<String>,
    pub final_observable_result: String,
    pub verification_evidence: String,
    pub skill_candidate: Option<SkillCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SkillLearningAction {
    None,
    Created { id: String, name: String },
    Updated { id: String, name: String },
    Unchanged { id: String, name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LearningOutcome {
    pub skill: SkillLearningAction,
    pub memory_mutations: usize,
}

#[derive(Clone)]
pub struct LearningEvaluator {
    skills: Arc<SkillRegistry>,
    memory: Arc<MemoryEvaluator>,
    semantic: Arc<SemanticEvaluator>,
}

impl LearningEvaluator {
    pub fn new(skills: Arc<SkillRegistry>, memory: Arc<MemoryEvaluator>) -> Self {
        Self {
            skills,
            memory,
            semantic: Arc::new(SemanticEvaluator::deterministic()),
        }
    }

    pub fn with_semantic(
        skills: Arc<SkillRegistry>,
        memory: Arc<MemoryEvaluator>,
        semantic: Arc<SemanticEvaluator>,
    ) -> Self {
        Self {
            skills,
            memory,
            semantic,
        }
    }

    pub fn evaluate(&self, owner: &str, trace: &LearningTrace) -> Result<LearningOutcome> {
        if trace.run_status != "completed" || !trace.verified {
            return Ok(LearningOutcome {
                skill: SkillLearningAction::None,
                memory_mutations: 0,
            });
        }

        let sanitized_trace = serde_json::to_value(trace)?;
        let memory_mutations = self
            .memory
            .apply_implicit_trace(owner, &trace.session_id, &sanitized_trace)?
            .len();
        if !trace.meaningful
            || !trace.reusable
            || trace.skill_candidate.is_none() && !self.is_reusable(trace)
        {
            return Ok(LearningOutcome {
                skill: SkillLearningAction::None,
                memory_mutations,
            });
        }
        let Some(candidate) = trace
            .skill_candidate
            .clone()
            .or_else(|| self.synthesize(trace))
        else {
            return Ok(LearningOutcome {
                skill: SkillLearningAction::None,
                memory_mutations,
            });
        };
        if trace.verification_evidence.trim().is_empty() {
            return Ok(LearningOutcome {
                skill: SkillLearningAction::None,
                memory_mutations,
            });
        }

        let exact = self.skills.view(owner, &candidate.name)?;
        let retrieved = if exact.is_some() {
            Vec::new()
        } else {
            self.skills.search(
                owner,
                &format!("{} {}", candidate.name, candidate.summary),
                5,
            )?
        };
        let (candidate, unchanged) = self.resolve_equivalence(candidate, exact, retrieved);
        if let Some(skill) = unchanged {
            return Ok(LearningOutcome {
                skill: SkillLearningAction::Unchanged {
                    id: skill.id,
                    name: skill.name,
                },
                memory_mutations,
            });
        }
        let (mutation, skill) = self
            .skills
            .learn(owner, candidate, Some(&trace.session_id))?;
        let skill = match mutation {
            SkillMutation::Created => SkillLearningAction::Created {
                id: skill.id,
                name: skill.name,
            },
            SkillMutation::Updated => SkillLearningAction::Updated {
                id: skill.id,
                name: skill.name,
            },
            SkillMutation::Unchanged => SkillLearningAction::Unchanged {
                id: skill.id,
                name: skill.name,
            },
        };
        Ok(LearningOutcome {
            skill,
            memory_mutations,
        })
    }

    /// Post-success semantic learning may involve several bounded provider
    /// decisions. Run it outside Tokio's async workers so Telegram remains
    /// responsive while the observable trace is evaluated.
    pub async fn evaluate_async(
        &self,
        owner: &str,
        trace: &LearningTrace,
    ) -> Result<LearningOutcome> {
        let evaluator = self.clone();
        let owner = owner.to_owned();
        let trace = trace.clone();
        tokio::task::spawn_blocking(move || evaluator.evaluate(&owner, &trace))
            .await
            .map_err(|error| anyhow::anyhow!("learning evaluator worker failed: {error}"))?
    }

    fn is_reusable(&self, trace: &LearningTrace) -> bool {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Decision {
            reusable: bool,
            reason: String,
        }
        match self.semantic.evaluate::<Decision>(
            "task_reusability",
            serde_json::json!({
                "type":"object","additionalProperties":false,
                "required":["reusable","reason"],
                "properties":{"reusable":{"type":"boolean"},"reason":{"type":"string","maxLength":800}}
            }),
            serde_json::to_value(trace).unwrap_or(serde_json::Value::Null),
        ) {
            SemanticResult::Valid(decision) if decision.reason.chars().count() <= 800 => {
                decision.reusable
            }
            SemanticResult::Malformed => false,
            SemanticResult::Unavailable | SemanticResult::Valid(_) => reusable_trace(trace),
        }
    }

    fn synthesize(&self, trace: &LearningTrace) -> Option<SkillCandidate> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Synthesis {
            candidate: Option<SkillCandidate>,
        }
        match self.semantic.evaluate::<Synthesis>(
            "skill_synthesis",
            serde_json::json!({
                "type":"object","additionalProperties":false,"required":["candidate"],
                "properties":{"candidate":{"oneOf":[{"type":"null"},{"type":"object","additionalProperties":false,
                    "required":["name","summary","when_to_use","prerequisites","procedure","pitfalls","verification"],
                    "properties":{
                        "name":{"type":"string","maxLength":120},"summary":{"type":"string","maxLength":2000},
                        "when_to_use":{"type":"string","maxLength":3000},"prerequisites":{"type":"string","maxLength":6000},
                        "procedure":{"type":"string","maxLength":12000},"pitfalls":{"type":"string","maxLength":6000},
                        "verification":{"type":"string","maxLength":6000}
                    }}]}}
            }),
            serde_json::json!({
                "verified_sanitized_trace":trace,
                "rules":[
                    "Reflect only operations that actually succeeded.",
                    "Generalize when-to-use, preserve actual dependencies, corrections, and observable verification.",
                    "Do not emit a generic clarify-act-verify template or literal hidden reasoning."
                ]
            }),
        ) {
            SemanticResult::Valid(value) => value.candidate,
            SemanticResult::Malformed => None,
            SemanticResult::Unavailable => derive_candidate(trace),
        }
    }

    fn resolve_equivalence(
        &self,
        mut candidate: SkillCandidate,
        exact: Option<crate::skills::SkillRecord>,
        retrieved: Vec<crate::skills::SkillRecord>,
    ) -> (SkillCandidate, Option<crate::skills::SkillRecord>) {
        if let Some(exact) = exact {
            candidate.name = exact.name;
            return (candidate, None);
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum EquivalenceAction {
            CreateNew,
            UpdateExisting,
            MergeInto,
            Unchanged,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Decision {
            action: EquivalenceAction,
            skill_id: Option<String>,
            reason: String,
        }
        let result = self.semantic.evaluate::<Decision>(
            "skill_equivalence",
            serde_json::json!({
                "type":"object","additionalProperties":false,
                "required":["action","skill_id","reason"],
                "properties":{
                    "action":{"enum":["create_new","update_existing","merge_into","unchanged"]},
                    "skill_id":{"type":["string","null"],"maxLength":128},
                    "reason":{"type":"string","maxLength":800}
                }
            }),
            serde_json::json!({"candidate":candidate,"retrieved_candidates":retrieved}),
        );
        match result {
            SemanticResult::Valid(decision) if decision.reason.chars().count() <= 800 => {
                let target = decision
                    .skill_id
                    .as_deref()
                    .and_then(|id| retrieved.iter().find(|skill| skill.id == id))
                    .cloned();
                match (decision.action, target) {
                    (EquivalenceAction::Unchanged, Some(target)) => (candidate, Some(target)),
                    (
                        EquivalenceAction::UpdateExisting | EquivalenceAction::MergeInto,
                        Some(target),
                    ) => {
                        candidate.name = target.name;
                        (candidate, None)
                    }
                    (EquivalenceAction::CreateNew, _) => (candidate, None),
                    _ => (candidate, None),
                }
            }
            SemanticResult::Malformed | SemanticResult::Unavailable | SemanticResult::Valid(_) => {
                (candidate, None)
            }
        }
    }
}

fn derive_candidate(trace: &LearningTrace) -> Option<SkillCandidate> {
    let successful = trace
        .tool_observations
        .iter()
        .filter(|observation| observation.status == "succeeded")
        .collect::<Vec<_>>();
    if successful.len() < 2 || trace.verification_evidence.trim().is_empty() {
        return None;
    }
    let name_words = trace
        .user_goal
        .split_whitespace()
        .filter(|word| {
            !matches!(
                word.to_ascii_lowercase().as_str(),
                "a" | "an" | "the" | "please" | "to" | "for" | "my" | "our"
            )
        })
        .take(8)
        .collect::<Vec<_>>();
    if name_words.is_empty() {
        return None;
    }
    let name = canonical_skill_name(&name_words.join(" "));
    let mut prerequisites = trace
        .installed_dependencies
        .iter()
        .map(|dependency| {
            format!(
                "- Installed/available dependency: {}",
                compact(dependency, 180)
            )
        })
        .collect::<Vec<_>>();
    if trace
        .tool_observations
        .iter()
        .any(|observation| observation.risk == "privileged")
    {
        prerequisites.push(
            "- Typed privileged Android capability plus owner approval when ToolPolicy returns ASK."
                .into(),
        );
    }
    if prerequisites.is_empty() {
        prerequisites
            .push("- Runtime capabilities used by the successful observations below.".into());
    }
    let procedure = successful
        .iter()
        .enumerate()
        .map(|(index, observation)| {
            let operation = if observation.operation.trim().is_empty() {
                observation.tool.clone()
            } else {
                format!(
                    "{} with {}",
                    observation.tool,
                    compact(&observation.operation, 500)
                )
            };
            format!(
                "{}. Execute {}. Observe: {}",
                index + 1,
                operation,
                compact(&observation.observable_summary, 500)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let pitfalls = trace
        .tool_observations
        .iter()
        .filter(|observation| observation.status != "succeeded")
        .map(|observation| {
            format!(
                "- Avoid repeating the failed {} approach unchanged: {}",
                observation.tool,
                compact(&observation.observable_summary, 300)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(SkillCandidate {
        name,
        summary: compact(&trace.user_goal, 1_000),
        when_to_use: format!(
            "Use for future tasks with the same intended outcome as: {}",
            compact(&trace.user_goal, 800)
        ),
        prerequisites: prerequisites.join("\n"),
        procedure,
        pitfalls: if pitfalls.is_empty() {
            "- Do not treat a successful command exit or a model statement as sufficient proof; verify the requested outcome.".into()
        } else {
            pitfalls
        },
        verification: compact(&trace.verification_evidence, 4_000),
    })
}

fn reusable_trace(trace: &LearningTrace) -> bool {
    if trace.final_observable_result.trim().is_empty()
        || trace.verification_evidence.trim().is_empty()
    {
        return false;
    }
    let successful = trace
        .tool_observations
        .iter()
        .filter(|observation| observation.status == "succeeded")
        .collect::<Vec<_>>();
    let has_observable_action = successful.iter().any(|observation| {
        !matches!(observation.risk.as_str(), "read_only" | "unknown")
            && !observation.observable_summary.trim().is_empty()
            && observation.observable_summary != "no output"
    });
    let distinct_observations = trace
        .tool_observations
        .iter()
        .filter(|observation| observation.status == "succeeded")
        .map(|observation| format!("{}:{}", observation.tool, observation.observable_summary))
        .collect::<BTreeSet<_>>()
        .len();
    // Reusability is based on a semantically non-trivial goal plus an actual
    // action/verification trace. It is not inferred from a hard-coded action
    // verb or from tool-call count alone.
    let goal_concepts = trace
        .user_goal
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|token| token.chars().count() >= 3 && !goal_stop_word(token))
        .collect::<BTreeSet<_>>()
        .len();
    has_observable_action
        && successful.len() >= 2
        && distinct_observations >= 2
        && goal_concepts >= 4
}

fn goal_stop_word(word: &str) -> bool {
    matches!(
        word,
        "and"
            | "the"
            | "for"
            | "from"
            | "with"
            | "this"
            | "that"
            | "please"
            | "tolong"
            | "untuk"
            | "dengan"
            | "yang"
            | "ini"
            | "itu"
            | "saya"
    )
}

fn compact(value: &str, max_chars: usize) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() <= max_chars {
        value
    } else {
        value.chars().take(max_chars).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        memory::{MemoryEvaluator, MemoryStore},
        semantic::{SemanticBackend, SemanticRequest},
        skills::{SkillRegistry, SkillStore},
        storage::Storage,
    };

    struct EquivalenceSemantic {
        target_id: String,
    }

    impl SemanticBackend for EquivalenceSemantic {
        fn evaluate(&self, request: &SemanticRequest) -> Result<String> {
            Ok(match request.purpose.as_str() {
                "durable_trace_memory" => r#"{"decisions":[]}"#.into(),
                "skill_equivalence" => serde_json::json!({
                    "action":"merge_into",
                    "skill_id":self.target_id,
                    "reason":"both workflows recover the same xiaod boot-start failure"
                })
                .to_string(),
                _ => return Err(anyhow::anyhow!("unexpected semantic purpose")),
            })
        }
    }

    fn evaluator() -> (LearningEvaluator, Arc<SkillStore>) {
        let storage = Arc::new(Storage::open_memory().unwrap());
        let skill_store = Arc::new(SkillStore::new(storage.clone()));
        let memory_store = Arc::new(MemoryStore::new(storage));
        (
            LearningEvaluator::new(
                Arc::new(SkillRegistry::new(skill_store.clone())),
                Arc::new(MemoryEvaluator::new(memory_store)),
            ),
            skill_store,
        )
    }

    fn completed(candidate: SkillCandidate) -> LearningTrace {
        LearningTrace {
            run_status: "completed".into(),
            verified: true,
            meaningful: true,
            reusable: true,
            user_goal: "Diagnose the Xiao service failure".into(),
            session_id: "s".into(),
            tool_observations: vec![SafeToolObservation {
                tool: "service_status".into(),
                risk: "read_only".into(),
                status: "succeeded".into(),
                operation: "inspect xiaod state".into(),
                observable_summary: "service healthy".into(),
                verification: true,
            }],
            installed_dependencies: Vec::new(),
            artifacts: Vec::new(),
            final_observable_result: "xiaod is healthy".into(),
            verification_evidence: "health check passed and no fatal error recurred".into(),
            skill_candidate: Some(candidate),
        }
    }

    fn candidate(name: &str, extra: &str) -> SkillCandidate {
        SkillCandidate {
            name: name.into(),
            summary: "Diagnose Xiao service failures".into(),
            when_to_use: "When xiaod is unhealthy".into(),
            prerequisites: "Service status/log access.".into(),
            procedure: format!("1. Check status.\n2. Inspect logs.{extra}"),
            pitfalls: "Do not leak secrets.".into(),
            verification: "Health check passes.".into(),
        }
    }

    #[test]
    fn verified_work_creates_then_updates_one_canonical_skill() {
        let (evaluator, store) = evaluator();
        let first = evaluator
            .evaluate("p", &completed(candidate("diagnose-xiao-service", "")))
            .unwrap();
        assert!(matches!(first.skill, SkillLearningAction::Created { .. }));
        let second = evaluator
            .evaluate(
                "p",
                &completed(candidate(
                    "fix-xiao-service-v2",
                    "\n3. Check file ownership.",
                )),
            )
            .unwrap();
        assert!(matches!(second.skill, SkillLearningAction::Updated { .. }));
        let rows = store.list("p", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "diagnose-xiao-service");
        assert!(rows[0].procedure.contains("ownership"));
    }

    #[test]
    fn trivial_failed_cancelled_and_unverified_work_never_creates_skill() {
        let (evaluator, store) = evaluator();
        for (status, verified, meaningful) in [
            ("failed", false, true),
            ("cancelled", false, true),
            ("interrupted", false, true),
            ("completed", true, false),
            ("completed", false, true),
        ] {
            let mut trace = completed(candidate("must-not-exist", ""));
            trace.run_status = status.into();
            trace.verified = verified;
            trace.meaningful = meaningful;
            let result = evaluator.evaluate("p", &trace).unwrap();
            assert_eq!(result.skill, SkillLearningAction::None);
        }
        assert!(store.list("p", 10).unwrap().is_empty());
    }

    #[test]
    fn observable_trace_creates_generalized_skill_with_pitfall_then_updates_same_skill() {
        let (evaluator, store) = evaluator();
        let mut trace = completed(candidate("ignored", ""));
        trace.skill_candidate = None;
        trace.tool_observations = vec![
            SafeToolObservation {
                tool: "first_probe".into(),
                risk: "read_only".into(),
                status: "failed".into(),
                operation: "probe stale socket".into(),
                observable_summary: "service socket was stale".into(),
                verification: false,
            },
            SafeToolObservation {
                tool: "scoped_repair".into(),
                risk: "side_effect".into(),
                status: "succeeded".into(),
                operation: "replace the observed stale socket".into(),
                observable_summary: "stale socket replaced".into(),
                verification: false,
            },
            SafeToolObservation {
                tool: "health_check".into(),
                risk: "read_only".into(),
                status: "succeeded".into(),
                operation: "probe service health".into(),
                observable_summary: "service healthy after repair".into(),
                verification: true,
            },
        ];
        let first = evaluator.evaluate("p", &trace).unwrap();
        assert!(matches!(first.skill, SkillLearningAction::Created { .. }));
        let skill = store.list("p", 10).unwrap().remove(0);
        assert!(skill.pitfalls.contains("stale"));
        assert!(skill.procedure.contains("scoped_repair"));
        assert!(!skill.procedure.contains("Use the typed"));

        trace.user_goal = "Repair the Xiao service crash safely".into();
        trace.tool_observations[2].observable_summary =
            "service healthy and stable on a second probe".into();
        let second = evaluator.evaluate("p", &trace).unwrap();
        assert!(matches!(second.skill, SkillLearningAction::Updated { .. }));
        assert_eq!(store.list("p", 10).unwrap().len(), 1);
    }

    #[test]
    fn tool_counts_without_reusable_semantics_do_not_create_a_skill() {
        let (evaluator, store) = evaluator();
        let mut trace = completed(candidate("ignored", ""));
        trace.skill_candidate = None;
        trace.user_goal = "Create file".into();
        trace.tool_observations = vec![
            SafeToolObservation {
                tool: "write".into(),
                risk: "side_effect".into(),
                status: "succeeded".into(),
                operation: "write requested file".into(),
                observable_summary: "file written".into(),
                verification: false,
            },
            SafeToolObservation {
                tool: "check".into(),
                risk: "read_only".into(),
                status: "succeeded".into(),
                operation: "inspect requested file".into(),
                observable_summary: "file exists".into(),
                verification: true,
            },
        ];
        assert_eq!(
            evaluator.evaluate("p", &trace).unwrap().skill,
            SkillLearningAction::None
        );
        assert!(store.list("p", 10).unwrap().is_empty());
    }

    #[test]
    fn media_dependency_workflow_synthesizes_concrete_verified_skill() {
        let (evaluator, store) = evaluator();
        let trace = LearningTrace {
            run_status: "completed".into(),
            verified: true,
            meaningful: true,
            reusable: true,
            user_goal: "Extract audio from the owner video and verify the output".into(),
            session_id: "media-session".into(),
            tool_observations: vec![
                SafeToolObservation {
                    tool: "termux_terminal".into(),
                    risk: "side_effect".into(),
                    status: "failed".into(),
                    operation: r#"{"argv":["ffmpeg","-i","clip.mp4","audio.m4a"]}"#.into(),
                    observable_summary: "ffmpeg was initially missing".into(),
                    verification: false,
                },
                SafeToolObservation {
                    tool: "dependency_installer".into(),
                    risk: "side_effect".into(),
                    status: "succeeded".into(),
                    operation: "install validated Termux package ffmpeg".into(),
                    observable_summary: "trusted Termux repository installed ffmpeg".into(),
                    verification: false,
                },
                SafeToolObservation {
                    tool: "termux_terminal".into(),
                    risk: "side_effect".into(),
                    status: "succeeded".into(),
                    operation: r#"{"argv":["ffmpeg","-i","clip.mp4","audio.m4a"]}"#.into(),
                    observable_summary: "audio.m4a was produced".into(),
                    verification: false,
                },
                SafeToolObservation {
                    tool: "artifact_probe".into(),
                    risk: "read_only".into(),
                    status: "succeeded".into(),
                    operation: "inspect audio.m4a metadata and size".into(),
                    observable_summary: "audio stream exists and output is non-empty".into(),
                    verification: true,
                },
            ],
            installed_dependencies: vec!["ffmpeg (Termux package ffmpeg)".into()],
            artifacts: vec!["audio.m4a".into()],
            final_observable_result: "audio.m4a delivered".into(),
            verification_evidence: "artifact probe confirmed a non-empty audio stream".into(),
            skill_candidate: None,
        };
        let outcome = evaluator.evaluate("p", &trace).unwrap();
        assert!(matches!(outcome.skill, SkillLearningAction::Created { .. }));
        let skill = store.list("p", 10).unwrap().remove(0);
        assert!(skill.prerequisites.contains("ffmpeg"));
        assert!(skill.procedure.contains("ffmpeg"));
        assert!(skill.procedure.contains("artifact_probe"));
        assert!(skill.pitfalls.contains("initially missing"));
        assert!(skill.verification.contains("non-empty audio stream"));
    }

    #[test]
    fn semantic_equivalence_merges_differently_named_workflow_into_existing_skill() {
        let storage = Arc::new(Storage::open_memory().unwrap());
        let store = Arc::new(SkillStore::new(storage.clone()));
        let existing = store
            .create_or_update(
                "p",
                candidate("repair-xiaod-startup", ""),
                Some("earlier-session"),
            )
            .unwrap()
            .1;
        let semantic = Arc::new(SemanticEvaluator::with_backend(Arc::new(
            EquivalenceSemantic {
                target_id: existing.id.clone(),
            },
        )));
        let memory = Arc::new(MemoryEvaluator::with_semantic(
            Arc::new(MemoryStore::new(storage)),
            semantic.clone(),
        ));
        let evaluator = LearningEvaluator::with_semantic(
            Arc::new(SkillRegistry::new(store.clone())),
            memory,
            semantic,
        );
        let new_candidate = SkillCandidate {
            name: "recover-daemon-after-boot-failure".into(),
            summary: "Diagnose Xiao service failures after boot".into(),
            when_to_use: "When xiaod does not start after Android boot".into(),
            prerequisites: "Service status and bounded log access.".into(),
            procedure: "1. Check status.\n2. Inspect logs.\n3. Correct boot ownership.".into(),
            pitfalls: "Do not repeat a failed restart without new evidence.".into(),
            verification: "Health check passes after a fresh boot.".into(),
        };
        let outcome = evaluator.evaluate("p", &completed(new_candidate)).unwrap();
        assert!(matches!(outcome.skill, SkillLearningAction::Updated { .. }));
        let records = store.list("p", 10).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, existing.id);
        assert_eq!(records[0].name, "repair-xiaod-startup");
        assert!(records[0].procedure.contains("boot ownership"));
    }
}
