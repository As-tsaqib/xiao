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
    if successful.len() < 2 {
        return None;
    }
    let mut name_words = trace
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
    name_words.clear();
    let procedure = successful
        .iter()
        .enumerate()
        .map(|(index, observation)| {
            format!(
                "{}. Use the typed {} operation and require a successful observable result.",
                index + 1,
                observation.tool
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
                "- {} did not succeed: {}",
                observation.tool, observation.observable_summary
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(SkillCandidate {
        name,
        summary: trace.user_goal.chars().take(1_000).collect(),
        when_to_use: format!("When a future task has this goal: {}", trace.user_goal),
        procedure,
        pitfalls,
        verification: trace.verification_evidence.clone(),
    })
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
}
