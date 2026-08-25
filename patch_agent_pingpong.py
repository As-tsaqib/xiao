import re

with open('src/agent/mod.rs', 'r') as f:
    text = f.read()

replacement = """
                            tool_calls += 1;
                            let audit = self.storage.tool_runs(principal, &agent_run_id)?;
                            let mut recent_calls: Vec<String> = audit.iter().map(|r| r.tool.clone()).collect();
                            recent_calls.push(call.name.clone());
                            if recent_calls.len() >= 6 {
                                let len = recent_calls.len();
                                if recent_calls[len-2] == recent_calls[len-4] && recent_calls[len-4] == recent_calls[len-6] &&
                                   recent_calls[len-1] == recent_calls[len-3] && recent_calls[len-3] == recent_calls[len-5] {
                                    let mut blocked = completion.verify_for_task_async(prompt, "ping-pong sequence", &audit).await;
                                    blocked.state = VerificationState::Blocked;
                                    blocked.verified = false;
                                    blocked.summary = "ping-pong repeating tool sequence detected".into();
                                    return Ok(LoopOutcome { final_answer: format!("Blocked: {}", blocked.summary), verification: blocked });
                                }
                            }
                            if tool_calls > self.config.max_tool_calls {
"""

text = text.replace("                            tool_calls += 1;\n                            if tool_calls > self.config.max_tool_calls {", replacement.strip("\n"))

with open('src/agent/mod.rs', 'w') as f:
    f.write(text)
