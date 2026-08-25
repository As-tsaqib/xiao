import re

with open('src/agent/mod.rs', 'r') as f:
    text = f.read()

ping_pong = """
                            let audit = self.storage.tool_runs(principal, &agent_run_id)?;
                            let mut recent_calls: Vec<String> = audit.into_iter().map(|r| r.name).collect();
                            recent_calls.push(call.name.clone());
                            if recent_calls.len() >= 6 {
                                let len = recent_calls.len();
                                if recent_calls[len-2] == recent_calls[len-4] && recent_calls[len-4] == recent_calls[len-6] &&
                                   recent_calls[len-1] == recent_calls[len-3] && recent_calls[len-3] == recent_calls[len-5] {
                                    return Ok(LoopOutcome {
                                        final_answer: "Blocked: Ping-pong repeating tool sequence detected".into(),
                                        verification: VerificationState::Blocked.into(), // Wait, just return something
                                    });
                                }
                            }
"""

