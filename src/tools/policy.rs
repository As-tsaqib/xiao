use std::collections::BTreeSet;

use crate::tools::{ToolContext, ToolRisk, ToolSpec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny(String),
}

#[derive(Debug, Clone)]
pub struct ToolPolicy {
    safe_side_effects: BTreeSet<String>,
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            safe_side_effects: ["memory_set", "memory_delete"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }
}

impl ToolPolicy {
    pub fn evaluate(&self, spec: &ToolSpec, _context: &ToolContext) -> PolicyDecision {
        match spec.risk {
            ToolRisk::ReadOnly => PolicyDecision::Allow,
            ToolRisk::SideEffect if self.safe_side_effects.contains(&spec.name) => {
                PolicyDecision::Allow
            }
            ToolRisk::SideEffect => PolicyDecision::Deny(format!(
                "side-effect tool {} is not approved by Xiao policy",
                spec.name
            )),
            ToolRisk::Sensitive => PolicyDecision::Deny(format!(
                "sensitive tool {} requires approval unavailable in v0.2.0",
                spec.name
            )),
            ToolRisk::Destructive => PolicyDecision::Deny(format!(
                "destructive tool {} is denied by Xiao policy",
                spec.name
            )),
            ToolRisk::Privileged => PolicyDecision::Deny(format!(
                "privileged tool {} is denied by Xiao policy",
                spec.name
            )),
        }
    }
}
