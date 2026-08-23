use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tools::{Tool, ToolContext, ToolEffect, ToolOrigin, ToolRisk, ToolSpec};

#[derive(Debug, Default)]
pub struct ContextStatsTool;

#[async_trait]
impl Tool for ContextStatsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "context_stats".into(),
            description: "Return bounded statistics about the current Xiao model context. This tool never reads files or executes processes.".into(),
            parameters: json!({
                "type":"object",
                "properties":{},
                "additionalProperties":false
            }),
            risk: ToolRisk::ReadOnly,
            origin: ToolOrigin::Builtin,
            effect: ToolEffect::None,
            required_capabilities: vec!["xiao.tool_registry".into()],
            timeout_ms: 5_000,
        }
    }

    async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct EmptyArguments {}
        let _: EmptyArguments = serde_json::from_value(arguments)?;
        let characters = context
            .messages
            .iter()
            .map(|message| message.content.chars().count())
            .sum::<usize>();
        Ok(serde_json::to_string(&json!({
            "messages": context.messages.len(),
            "characters": characters,
            "session_id": context.session_id,
        }))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{storage::MessageRecord, tools::ToolPolicy, tools::ToolRegistry};

    #[tokio::test]
    async fn context_stats_works_through_registry_and_no_shell_is_exposed() {
        let registry = ToolRegistry::new(ToolPolicy::default(), 4_096);
        registry.register(ContextStatsTool).unwrap();
        let context = ToolContext {
            principal: "p".into(),
            session_id: "s".into(),
            agent_run_id: "r".into(),
            yolo_mode: false,
            messages: vec![MessageRecord {
                role: "user".into(),
                content: "hello".into(),
                created_at: "now".into(),
            }],
            cancellation: tokio_util::sync::CancellationToken::new(),
            progress: None,
        };
        let specs = registry.available_specs(&context);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "context_stats");
        assert!(specs.iter().all(|spec| spec.name != "shell"));
        let result = registry
            .execute(
                &crate::tools::ToolCall {
                    call_id: "1".into(),
                    name: "context_stats".into(),
                    arguments: json!({}),
                },
                &context,
            )
            .await;
        assert!(!result.result.is_error);
        assert!(result.result.output.contains("characters"));
        let malformed = registry
            .execute(
                &crate::tools::ToolCall {
                    call_id: "2".into(),
                    name: "context_stats".into(),
                    arguments: json!({"unexpected":true}),
                },
                &context,
            )
            .await;
        assert!(malformed.result.is_error);
    }
}
