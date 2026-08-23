use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{
    memory::MemoryEvaluator,
    skills::{canonical_skill_name, SkillCandidate, SkillMutation, SkillRegistry},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SafeToolObservation {
    pub tool: String,
    pub risk: String,
    pub status: String,
    pub observable_summary: String,
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
}

impl LearningEvaluator {
    pub fn new(skills: Arc<SkillRegistry>, memory: Arc<MemoryEvaluator>) -> Self {
        Self { skills, memory }
    }

    pub fn evaluate(&self, owner: &str, trace: &LearningTrace) -> Result<LearningOutcome> {
        if trace.run_status != "completed" || !trace.verified {
            return Ok(LearningOutcome {
                skill: SkillLearningAction::None,
                memory_mutations: 0,
            });
        }

        let memory_mutations = self
            .memory
            .apply_implicit(owner, &trace.session_id, &trace.user_goal)?
            .len();
        if !trace.meaningful || !trace.reusable {
            return Ok(LearningOutcome {
                skill: SkillLearningAction::None,
                memory_mutations,
            });
        }
        if trace.skill_candidate.is_none() && !reusable_trace(trace) {
            return Ok(LearningOutcome {
                skill: SkillLearningAction::None,
                memory_mutations,
            });
        }
        let Some(candidate) = trace
            .skill_candidate
            .clone()
            .or_else(|| derive_candidate(trace))
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
    let checkpoints = successful
        .iter()
        .map(|observation| compact(&observation.observable_summary, 220))
        .filter(|summary| summary != "no output")
        .take(4)
        .map(|summary| format!("   - Expected checkpoint: {summary}"))
        .collect::<Vec<_>>()
        .join("\n");
    let prerequisites = if trace
        .tool_observations
        .iter()
        .any(|observation| observation.risk == "privileged")
    {
        "Confirm the typed privileged capability is available and obtain owner approval before the sensitive step."
    } else {
        "Confirm required runtime capabilities and inputs before changing state."
    };
    let procedure = format!(
        "1. Clarify the desired observable end state and inspect the current state.\n2. {prerequisites}\n3. Perform the smallest scoped action that advances the task, then observe its bounded result before continuing.\n{checkpoints}\n4. If an attempt fails, diagnose its observable error and choose a materially different action.\n5. Run an independent verification appropriate to the artifact, service, or configuration before reporting success."
    );
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
        skills::{SkillRegistry, SkillStore},
        storage::Storage,
    };

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
                observable_summary: "service healthy".into(),
            }],
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
                observable_summary: "service socket was stale".into(),
            },
            SafeToolObservation {
                tool: "scoped_repair".into(),
                risk: "side_effect".into(),
                status: "succeeded".into(),
                observable_summary: "stale socket replaced".into(),
            },
            SafeToolObservation {
                tool: "health_check".into(),
                risk: "read_only".into(),
                status: "succeeded".into(),
                observable_summary: "service healthy after repair".into(),
            },
        ];
        let first = evaluator.evaluate("p", &trace).unwrap();
        assert!(matches!(first.skill, SkillLearningAction::Created { .. }));
        let skill = store.list("p", 10).unwrap().remove(0);
        assert!(skill.pitfalls.contains("stale"));
        assert!(skill.procedure.contains("materially different"));
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
                observable_summary: "file written".into(),
            },
            SafeToolObservation {
                tool: "check".into(),
                risk: "read_only".into(),
                status: "succeeded".into(),
                observable_summary: "file exists".into(),
            },
        ];
        assert_eq!(
            evaluator.evaluate("p", &trace).unwrap().skill,
            SkillLearningAction::None
        );
        assert!(store.list("p", 10).unwrap().is_empty());
    }
}
