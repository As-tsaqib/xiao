use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::{anyhow, Result};
use tokio::time::timeout;

use crate::{
    security::redact::redact_text,
    tools::{
        PolicyDecision, Tool, ToolCall, ToolContext, ToolExecution, ToolPolicy, ToolResult,
        ToolRunStatus, ToolSpec,
    },
};

pub struct ToolRegistry {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    policy: ToolPolicy,
    max_output_chars: usize,
}

impl ToolRegistry {
    pub fn new(policy: ToolPolicy, max_output_chars: usize) -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
            policy,
            max_output_chars: max_output_chars.max(1),
        }
    }

    pub fn register<T: Tool + 'static>(&self, tool: T) -> Result<()> {
        self.register_arc(Arc::new(tool))
    }

    pub fn register_arc(&self, tool: Arc<dyn Tool>) -> Result<()> {
        let spec = tool.spec();
        validate_spec(&spec)?;
        let mut tools = self
            .tools
            .write()
            .map_err(|_| anyhow!("tool registry lock poisoned"))?;
        if tools.contains_key(&spec.name) {
            return Err(anyhow!("tool {} is already registered", spec.name));
        }
        tools.insert(spec.name, tool);
        Ok(())
    }

    pub fn spec(&self, name: &str) -> Option<ToolSpec> {
        self.tools.read().ok()?.get(name).map(|tool| tool.spec())
    }

    /// Only policy-allowed tools are advertised to providers. Registration is
    /// not itself a grant of model-visible capability.
    pub fn available_specs(&self, context: &ToolContext) -> Vec<ToolSpec> {
        let Ok(tools) = self.tools.read() else {
            return Vec::new();
        };
        let mut specs = tools
            .values()
            .map(|tool| tool.spec())
            .filter(|spec| matches!(self.policy.evaluate(spec, context), PolicyDecision::Allow))
            .collect::<Vec<_>>();
        specs.sort_by(|left, right| left.name.cmp(&right.name));
        specs
    }

    pub async fn execute(&self, call: &ToolCall, context: &ToolContext) -> ToolExecution {
        let tool = self
            .tools
            .read()
            .ok()
            .and_then(|tools| tools.get(&call.name).cloned());
        let Some(tool) = tool else {
            return self.error(call, ToolRunStatus::Denied, "unknown or unavailable tool");
        };
        let spec = tool.spec();
        if let PolicyDecision::Deny(reason) = self.policy.evaluate(&spec, context) {
            return self.error(call, ToolRunStatus::Denied, &reason);
        }

        // Tool implementations never hold the registry lock while awaiting.
        let timeout_ms = spec.timeout_ms.clamp(1, 120_000);
        match timeout(
            Duration::from_millis(timeout_ms),
            tool.execute(context, call.arguments.clone()),
        )
        .await
        {
            Ok(Ok(output)) => ToolExecution {
                result: ToolResult {
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                    output: bound(redact_text(&output), self.max_output_chars),
                    is_error: false,
                },
                status: ToolRunStatus::Succeeded,
            },
            Ok(Err(error)) => self.error(call, ToolRunStatus::Failed, &error.to_string()),
            Err(_) => self.error(call, ToolRunStatus::Failed, "tool timed out"),
        }
    }

    fn error(&self, call: &ToolCall, status: ToolRunStatus, message: &str) -> ToolExecution {
        ToolExecution {
            result: ToolResult {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                output: bound(redact_text(message), self.max_output_chars),
                is_error: true,
            },
            status,
        }
    }
}

fn validate_spec(spec: &ToolSpec) -> Result<()> {
    let valid_name = !spec.name.is_empty()
        && spec.name.len() <= 64
        && spec.name.chars().enumerate().all(|(index, character)| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit() && index > 0
                || character == '_' && index > 0
        });
    if !valid_name {
        return Err(anyhow!("tool name must be canonical snake_case"));
    }
    if spec.description.trim().is_empty() || spec.description.chars().count() > 1_000 {
        return Err(anyhow!("tool description is empty or too long"));
    }
    if !spec.parameters.is_object() {
        return Err(anyhow!("tool parameters must be a JSON schema object"));
    }
    if spec.timeout_ms == 0 {
        return Err(anyhow!("tool timeout must be positive"));
    }
    Ok(())
}

fn bound(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }
    value.chars().take(max_chars).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::{json, Value};

    use crate::{storage::MessageRecord, tools::ToolRisk};

    struct FakeTool {
        name: &'static str,
        risk: ToolRisk,
        output: String,
        delay_ms: u64,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.name.into(),
                description: "A bounded test tool".into(),
                parameters: json!({"type":"object"}),
                risk: self.risk,
                timeout_ms: 10,
            }
        }

        async fn execute(&self, _: &ToolContext, _: Value) -> Result<String> {
            if self.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            }
            Ok(self.output.clone())
        }
    }

    fn context() -> ToolContext {
        ToolContext {
            principal: "p".into(),
            session_id: "s".into(),
            agent_run_id: "r".into(),
            messages: vec![MessageRecord {
                role: "user".into(),
                content: "hello".into(),
                created_at: "now".into(),
            }],
        }
    }

    #[test]
    fn duplicate_registration_is_rejected() {
        let registry = ToolRegistry::new(ToolPolicy::default(), 64);
        for expected in [true, false] {
            let result = registry.register(FakeTool {
                name: "duplicate",
                risk: ToolRisk::ReadOnly,
                output: "ok".into(),
                delay_ms: 0,
            });
            assert_eq!(result.is_ok(), expected);
        }
    }

    #[tokio::test]
    async fn unknown_and_policy_denied_tools_fail_safely() {
        let registry = ToolRegistry::new(ToolPolicy::default(), 64);
        registry
            .register(FakeTool {
                name: "dangerous",
                risk: ToolRisk::Destructive,
                output: "must not run".into(),
                delay_ms: 0,
            })
            .unwrap();
        assert!(registry.available_specs(&context()).is_empty());
        for name in ["missing", "dangerous"] {
            let result = registry
                .execute(
                    &ToolCall {
                        call_id: name.into(),
                        name: name.into(),
                        arguments: json!({}),
                    },
                    &context(),
                )
                .await;
            assert!(result.result.is_error);
            assert_eq!(result.status, ToolRunStatus::Denied);
        }
    }

    #[tokio::test]
    async fn timeout_and_output_are_bounded() {
        let registry = ToolRegistry::new(ToolPolicy::default(), 16);
        registry
            .register(FakeTool {
                name: "large",
                risk: ToolRisk::ReadOnly,
                output: "x".repeat(100),
                delay_ms: 0,
            })
            .unwrap();
        registry
            .register(FakeTool {
                name: "slow",
                risk: ToolRisk::ReadOnly,
                output: "late".into(),
                delay_ms: 100,
            })
            .unwrap();
        let large = registry
            .execute(
                &ToolCall {
                    call_id: "1".into(),
                    name: "large".into(),
                    arguments: json!({}),
                },
                &context(),
            )
            .await;
        assert_eq!(large.result.output.chars().count(), 17);
        let slow = registry
            .execute(
                &ToolCall {
                    call_id: "2".into(),
                    name: "slow".into(),
                    arguments: json!({}),
                },
                &context(),
            )
            .await;
        assert!(slow.result.is_error);
        assert!(slow.result.output.contains("timed out"));
    }
}
