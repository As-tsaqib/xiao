use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::{timeout, Duration};

use crate::providers::ProviderRequest;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    pub output: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ToolRouter;

impl ToolRouter {
    pub fn definitions(&self) -> Vec<Value> {
        vec![json!({
            "type":"function",
            "name":"context_stats",
            "description":"Return bounded statistics about the current xiao conversation context. This tool never reads files or executes processes.",
            "parameters":{"type":"object","properties":{},"additionalProperties":false},
            "strict":true
        })]
    }

    pub async fn execute(&self, call: &ToolCall, req: &ProviderRequest) -> ToolResult {
        let run = async {
            match call.name.as_str() {
                "context_stats" => {
                    let chars: usize = req.messages.iter().map(|m| m.content.chars().count()).sum();
                    Ok(serde_json::to_string(&json!({
                        "messages": req.messages.len(),
                        "characters": chars,
                        "session_id": req.session_id,
                    }))?)
                }
                _ => Err(anyhow!("tool is not allowed by v0.1.0 policy")),
            }
        };
        match timeout(Duration::from_secs(5), run).await {
            Ok(Ok(output)) => ToolResult {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                output: bound(output),
                is_error: false,
            },
            Ok(Err(error)) => ToolResult {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                output: bound(error.to_string()),
                is_error: true,
            },
            Err(_) => ToolResult {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                output: "tool timed out".into(),
                is_error: true,
            },
        }
    }
}

fn bound(value: String) -> String {
    const MAX: usize = 4096;
    if value.chars().count() <= MAX {
        return value;
    }
    value.chars().take(MAX).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MessageRecord;
    #[tokio::test]
    async fn tool_router_is_typed_bounded_and_does_not_offer_shell() {
        let router = ToolRouter;
        assert!(router.definitions().iter().all(|d| d["name"] != "shell"));
        let req = ProviderRequest {
            session_id: "s".into(),
            account_id: None,
            model: "m".into(),
            messages: vec![MessageRecord {
                role: "user".into(),
                content: "hello".into(),
                created_at: "now".into(),
            }],
        };
        let result = router
            .execute(
                &ToolCall {
                    call_id: "1".into(),
                    name: "context_stats".into(),
                    arguments: json!({}),
                },
                &req,
            )
            .await;
        assert!(!result.is_error);
        assert!(result.output.contains("messages"));
        let denied = router
            .execute(
                &ToolCall {
                    call_id: "2".into(),
                    name: "shell".into(),
                    arguments: json!({"cmd":"id"}),
                },
                &req,
            )
            .await;
        assert!(denied.is_error);
    }
}
