use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    context::SessionHistoryStore,
    tools::{Tool, ToolContext, ToolRisk, ToolSpec},
};

pub struct SessionSearchTool {
    history: Arc<SessionHistoryStore>,
}

impl SessionSearchTool {
    pub fn new(history: Arc<SessionHistoryStore>) -> Self {
        Self { history }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for SessionSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "session_search".into(),
            description: "Search this principal's durable conversation history using bounded SQLite full-text retrieval. Use when prior work is referenced and current context is insufficient.".into(),
            parameters: json!({
                "type":"object",
                "properties":{
                    "query":{"type":"string","minLength":1},
                    "limit":{"type":"integer","minimum":1,"maximum":20}
                },
                "required":["query"],
                "additionalProperties":false
            }),
            risk: ToolRisk::ReadOnly,
            timeout_ms: 5_000,
        }
    }

    async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
        let arguments: Arguments = serde_json::from_value(arguments)?;
        let rows = self.history.search(
            &context.principal,
            &arguments.query,
            arguments.limit.unwrap_or(8),
        )?;
        Ok(serde_json::to_string(&rows)?)
    }
}
