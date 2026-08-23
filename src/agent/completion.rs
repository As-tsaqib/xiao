use serde::{Deserialize, Serialize};

use crate::storage::ToolRunRecord;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletionEvidence {
    pub verified: bool,
    pub summary: String,
    pub succeeded_tools: usize,
    pub unresolved_tool_failures: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CompletionVerifier;

impl CompletionVerifier {
    /// This deliberately evaluates observable state only. It never consumes
    /// hidden model reasoning or trusts a bare textual claim of completion.
    pub fn verify(&self, final_answer: &str, tool_runs: &[ToolRunRecord]) -> CompletionEvidence {
        if final_answer.trim().is_empty() {
            return CompletionEvidence {
                verified: false,
                summary: "provider produced no final observable result".into(),
                succeeded_tools: 0,
                unresolved_tool_failures: 0,
            };
        }

        let succeeded_tools = tool_runs
            .iter()
            .filter(|run| run.status == "succeeded")
            .count();
        let unresolved_tool_failures = tool_runs
            .iter()
            .enumerate()
            .filter(|(index, run)| {
                matches!(run.status.as_str(), "failed" | "denied" | "interrupted")
                    && !tool_runs[index + 1..].iter().any(|later| {
                        later.tool_name == run.tool_name && later.status == "succeeded"
                    })
            })
            .count();
        let unfinished = tool_runs
            .iter()
            .any(|run| matches!(run.status.as_str(), "requested" | "running"));
        let verified = unresolved_tool_failures == 0 && !unfinished;
        CompletionEvidence {
            verified,
            summary: if verified {
                if tool_runs.is_empty() {
                    "final answer is present; no tool side effects required".into()
                } else {
                    format!(
                        "{succeeded_tools} tool observations completed without unresolved failure"
                    )
                }
            } else if unfinished {
                "tool audit contains unfinished execution".into()
            } else {
                format!("{unresolved_tool_failures} tool failure(s) remain unresolved")
            },
            succeeded_tools,
            unresolved_tool_failures,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(name: &str, status: &str) -> ToolRunRecord {
        ToolRunRecord {
            id: format!("{name}-{status}"),
            agent_run_id: "r".into(),
            call_id: "c".into(),
            tool_name: name.into(),
            arguments_json: "{}".into(),
            risk: "read_only".into(),
            status: status.into(),
            output: None,
            error: None,
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn unresolved_failure_is_not_verified_but_successful_recovery_is() {
        let verifier = CompletionVerifier;
        assert!(!verifier.verify("done", &[run("search", "failed")]).verified);
        let recovered = verifier.verify(
            "done",
            &[run("search", "failed"), run("search", "succeeded")],
        );
        assert!(recovered.verified);
        assert_eq!(recovered.succeeded_tools, 1);
    }

    #[test]
    fn information_answer_can_complete_without_creating_verification_claims() {
        let evidence = CompletionVerifier.verify("Here is the answer", &[]);
        assert!(evidence.verified);
        assert_eq!(evidence.succeeded_tools, 0);
    }
}
