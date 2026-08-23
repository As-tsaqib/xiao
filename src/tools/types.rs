use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::storage::MessageRecord;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub risk: ToolRisk,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolRisk {
    ReadOnly,
    SideEffect,
    Sensitive,
    Destructive,
    Privileged,
}

impl ToolRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::SideEffect => "side_effect",
            Self::Sensitive => "sensitive",
            Self::Destructive => "destructive",
            Self::Privileged => "privileged",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub principal: String,
    pub session_id: String,
    pub agent_run_id: String,
    /// Already-bounded context, supplied only for semantic context statistics.
    pub messages: Vec<MessageRecord>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> Result<String>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    pub output: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolRunStatus {
    Succeeded,
    Failed,
    Denied,
}

impl ToolRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Denied => "denied",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecution {
    pub result: ToolResult,
    pub status: ToolRunStatus,
}
