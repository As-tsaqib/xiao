use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    memory::{MemoryScope, MemoryStore},
    tools::{Tool, ToolContext, ToolRisk, ToolSpec},
};

pub struct MemorySearchTool {
    store: Arc<MemoryStore>,
}

impl MemorySearchTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArguments {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_search".into(),
            description: "Search this principal's current durable user and agent memories. Results never cross principal boundaries.".into(),
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
        let arguments: SearchArguments = serde_json::from_value(arguments)?;
        let rows = self.store.search(
            &context.principal,
            &arguments.query,
            arguments.limit.unwrap_or(8),
        )?;
        Ok(serde_json::to_string(&rows)?)
    }
}

pub struct MemorySetTool {
    store: Arc<MemoryStore>,
}

impl MemorySetTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetArguments {
    scope: String,
    category: String,
    key: String,
    value: String,
    #[serde(default)]
    confidence: Option<f64>,
}

#[async_trait]
impl Tool for MemorySetTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_set".into(),
            description: "Create or update one canonical current memory for this principal. Prefer an existing category/key when meanings overlap; never store secrets.".into(),
            parameters: json!({
                "type":"object",
                "properties":{
                    "scope":{"type":"string","enum":["user","agent"]},
                    "category":{"type":"string","minLength":1,"maxLength":80},
                    "key":{"type":"string","minLength":1,"maxLength":120},
                    "value":{"type":"string","minLength":1,"maxLength":8192},
                    "confidence":{"type":"number","minimum":0,"maximum":1}
                },
                "required":["scope","category","key","value"],
                "additionalProperties":false
            }),
            risk: ToolRisk::SideEffect,
            timeout_ms: 5_000,
        }
    }

    async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
        let arguments: SetArguments = serde_json::from_value(arguments)?;
        let scope = MemoryScope::try_from(arguments.scope.as_str())?;
        let (outcome, memory) = self.store.upsert(
            &context.principal,
            scope,
            &arguments.category,
            &arguments.key,
            &arguments.value,
            arguments.confidence.unwrap_or(1.0),
            "model_tool",
            Some(&context.session_id),
        )?;
        Ok(serde_json::to_string(&json!({
            "outcome": outcome,
            "memory": memory
        }))?)
    }
}

pub struct MemoryDeleteTool {
    store: Arc<MemoryStore>,
}

impl MemoryDeleteTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteArguments {
    scope: String,
    category: String,
    key: String,
}

#[async_trait]
impl Tool for MemoryDeleteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_delete".into(),
            description: "Forget one canonical active memory owned by this principal. The mutation remains visible only in audit history.".into(),
            parameters: json!({
                "type":"object",
                "properties":{
                    "scope":{"type":"string","enum":["user","agent"]},
                    "category":{"type":"string","minLength":1,"maxLength":80},
                    "key":{"type":"string","minLength":1,"maxLength":120}
                },
                "required":["scope","category","key"],
                "additionalProperties":false
            }),
            risk: ToolRisk::SideEffect,
            timeout_ms: 5_000,
        }
    }

    async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
        let arguments: DeleteArguments = serde_json::from_value(arguments)?;
        let scope = MemoryScope::try_from(arguments.scope.as_str())?;
        let deleted = self.store.delete(
            &context.principal,
            scope,
            &arguments.category,
            &arguments.key,
            Some(&context.session_id),
        )?;
        Ok(serde_json::to_string(&json!({"deleted":deleted}))?)
    }
}
