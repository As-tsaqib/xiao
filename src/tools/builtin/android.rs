use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::{
    runtime::{AndroidBroker, AndroidOperation},
    tools::{Tool, ToolContext, ToolEffect, ToolOrigin, ToolRisk, ToolSpec},
};

pub struct AndroidXiaoStatusTool {
    broker: Arc<dyn AndroidBroker>,
}

impl AndroidXiaoStatusTool {
    pub fn new(broker: Arc<dyn AndroidBroker>) -> Self {
        Self { broker }
    }
}

#[async_trait]
impl Tool for AndroidXiaoStatusTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "android_xiao_status".into(),
            description: "Inspect Xiao's own Android init service through the typed privileged broker. This is not a root shell.".into(),
            parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
            risk: ToolRisk::ReadOnly,
            origin: ToolOrigin::AndroidPrivileged,
            effect: ToolEffect::None,
            required_capabilities: vec!["android.service.inspect".into()],
            timeout_ms: 15_000,
        }
    }

    async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
        ensure_empty(arguments)?;
        Ok(serde_json::to_string(
            &self
                .broker
                .execute(
                    AndroidOperation::InspectXiaoService,
                    context.cancellation.clone(),
                )
                .await?,
        )?)
    }
}

pub struct AndroidXiaoRestartTool {
    broker: Arc<dyn AndroidBroker>,
}

impl AndroidXiaoRestartTool {
    pub fn new(broker: Arc<dyn AndroidBroker>) -> Self {
        Self { broker }
    }
}

#[async_trait]
impl Tool for AndroidXiaoRestartTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "android_xiao_restart".into(),
            description: "Restart only Xiao's own Android init service through a typed privileged operation after explicit owner approval. No arbitrary root command is accepted.".into(),
            parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
            risk: ToolRisk::Privileged,
            origin: ToolOrigin::AndroidPrivileged,
            effect: ToolEffect::NonIdempotent,
            required_capabilities: vec!["android.service.restart".into()],
            timeout_ms: 30_000,
        }
    }

    async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
        ensure_empty(arguments)?;
        Ok(serde_json::to_string(
            &self
                .broker
                .execute(
                    AndroidOperation::RestartXiaoService,
                    context.cancellation.clone(),
                )
                .await?,
        )?)
    }
}

fn ensure_empty(arguments: Value) -> Result<()> {
    if arguments.as_object().is_some_and(serde_json::Map::is_empty) {
        Ok(())
    } else {
        Err(anyhow!(
            "typed Android operation accepts no model-controlled arguments"
        ))
    }
}
