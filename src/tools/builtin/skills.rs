use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    skills::SkillRegistry,
    tools::{Tool, ToolContext, ToolRisk, ToolSpec},
};

pub struct SkillSearchTool {
    skills: Arc<SkillRegistry>,
}

impl SkillSearchTool {
    pub fn new(skills: Arc<SkillRegistry>) -> Self {
        Self { skills }
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
impl Tool for SkillSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_search".into(),
            description: "Search summaries of reusable procedures learned for this principal. Use progressive disclosure and view only a relevant skill.".into(),
            parameters: json!({
                "type":"object",
                "properties":{
                    "query":{"type":"string","minLength":1},
                    "limit":{"type":"integer","minimum":1,"maximum":10}
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
        let rows = self.skills.search(
            &context.principal,
            &arguments.query,
            arguments.limit.unwrap_or(5),
        )?;
        let summaries = rows
            .into_iter()
            .map(|skill| {
                json!({
                    "id":skill.id,
                    "name":skill.name,
                    "summary":skill.summary,
                    "when_to_use":skill.when_to_use,
                    "updated_at":skill.updated_at
                })
            })
            .collect::<Vec<_>>();
        Ok(serde_json::to_string(&summaries)?)
    }
}

pub struct SkillViewTool {
    skills: Arc<SkillRegistry>,
}

impl SkillViewTool {
    pub fn new(skills: Arc<SkillRegistry>) -> Self {
        Self { skills }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewArguments {
    name_or_id: String,
}

#[async_trait]
impl Tool for SkillViewTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_view".into(),
            description: "View one full reusable skill owned by this principal after selecting it with skill_search. A skill is guidance, never permission.".into(),
            parameters: json!({
                "type":"object",
                "properties":{"name_or_id":{"type":"string","minLength":1}},
                "required":["name_or_id"],
                "additionalProperties":false
            }),
            risk: ToolRisk::ReadOnly,
            timeout_ms: 5_000,
        }
    }

    async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
        let arguments: ViewArguments = serde_json::from_value(arguments)?;
        let skill = self
            .skills
            .view(&context.principal, &arguments.name_or_id)?
            .ok_or_else(|| anyhow!("skill not found for principal"))?;
        Ok(serde_json::to_string(&skill)?)
    }
}
