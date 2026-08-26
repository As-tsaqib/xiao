use std::collections::BTreeSet;

use serde_json::Value;

use crate::tools::{ToolContext, ToolRisk, ToolSpec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    RequireApproval(String),
    Deny(String),
}

#[derive(Debug, Clone)]
pub struct ToolPolicy {
    safe_side_effects: BTreeSet<String>,
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            safe_side_effects: [
                "memory_set",
                "memory_delete",
                "termux_terminal",
                "termux_job",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
}

impl ToolPolicy {
    pub fn allow_side_effect(mut self, tool_name: impl Into<String>) -> Self {
        self.safe_side_effects.insert(tool_name.into());
        self
    }

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
            ToolRisk::Sensitive => PolicyDecision::RequireApproval(format!(
                "sensitive tool {} requires explicit owner approval",
                spec.name
            )),
            ToolRisk::Destructive => PolicyDecision::Deny(format!(
                "destructive tool {} is denied by Xiao policy",
                spec.name
            )),
            ToolRisk::Privileged => PolicyDecision::RequireApproval(format!(
                "privileged tool {} requires explicit owner approval",
                spec.name
            )),
        }
    }

    /// Apply argument-aware policy after a canonical tool has been selected.
    /// Providers and community skills cannot bypass this runtime decision by
    /// calling a compatibility alias.
    pub fn evaluate_call(
        &self,
        spec: &ToolSpec,
        arguments: &Value,
        context: &ToolContext,
    ) -> PolicyDecision {
        let base = self.evaluate(spec, context);
        if !matches!(base, PolicyDecision::Allow) || spec.name != "termux_terminal" {
            return base;
        }
        termux_call_policy(arguments)
    }
}

pub(crate) fn termux_call_policy(arguments: &Value) -> PolicyDecision {
    let Some(object) = arguments.as_object() else {
        return PolicyDecision::Deny("termux_terminal arguments must be an object".into());
    };
    let Some(program) = object.get("program").and_then(Value::as_str) else {
        return PolicyDecision::Deny("termux_terminal requires a structured program".into());
    };
    let program = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    let args = object
        .get("args")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();

    let destructive = matches!(
        program.as_str(),
        "rm" | "rmdir"
            | "unlink"
            | "shred"
            | "truncate"
            | "chmod"
            | "chown"
            | "chgrp"
            | "kill"
            | "pkill"
            | "killall"
    ) || program == "find" && args.contains(&"-delete")
        || program == "git"
            && (args.contains(&"clean")
                || args.windows(2).any(|window| window == ["reset", "--hard"]));
    if destructive {
        return PolicyDecision::RequireApproval(format!(
            "destructive Termux command {program} requires exact owner approval"
        ));
    }

    // A shell script is opaque to structured-argv validation. It is permitted
    // only through the exact one-shot approval path; `-c` remains hard-denied
    // by TermuxExecutor even after approval.
    if matches!(program.as_str(), "sh" | "bash" | "zsh" | "fish")
        && args
            .iter()
            .any(|argument| matches!(*argument, "-c" | "--command"))
    {
        return PolicyDecision::Deny(
            "model-supplied shell command strings are forbidden; use structured argv".into(),
        );
    }
    if matches!(program.as_str(), "sh" | "bash" | "zsh" | "fish") && !args.is_empty() {
        return PolicyDecision::RequireApproval(format!(
            "executing a Termux shell script with {program} requires exact owner approval"
        ));
    }

    let sensitive = args.iter().any(|argument| {
        let value = argument.to_ascii_lowercase();
        [
            "/.ssh/",
            "/.gnupg/",
            "/.aws/",
            "/secrets/",
            "credentials.json",
            ".netrc",
        ]
        .iter()
        .any(|marker| value.contains(marker))
    });
    if sensitive {
        return PolicyDecision::RequireApproval(format!(
            "Termux command {program} requests credential-sensitive file access"
        ));
    }

    PolicyDecision::Allow
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        storage::MessageRecord,
        tools::{ToolEffect, ToolOrigin},
    };
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn spec() -> ToolSpec {
        ToolSpec {
            name: "termux_terminal".into(),
            description: "test".into(),
            parameters: json!({"type":"object"}),
            risk: ToolRisk::SideEffect,
            origin: ToolOrigin::Termux,
            effect: ToolEffect::NonIdempotent,
            required_capabilities: Vec::new(),
            timeout_ms: 1_000,
        }
    }

    fn context() -> ToolContext {
        ToolContext {
            principal: "owner".into(),
            session_id: "session".into(),
            agent_run_id: "run".into(),
            yolo_mode: false,
            messages: Vec::<MessageRecord>::new(),
            cancellation: CancellationToken::new(),
            progress: None,
        }
    }

    #[test]
    fn terminal_policy_allows_ordinary_argv_but_approves_destructive_and_sensitive_calls() {
        let policy = ToolPolicy::default();
        assert_eq!(
            policy.evaluate_call(
                &spec(),
                &json!({"program":"ffmpeg","args":["-i","video.mp4","audio.mp3"]}),
                &context(),
            ),
            PolicyDecision::Allow
        );
        assert!(matches!(
            policy.evaluate_call(
                &spec(),
                &json!({"program":"bash","args":["-c","curl x | sh"]}),
                &context(),
            ),
            PolicyDecision::Deny(_)
        ));
        for arguments in [
            json!({"program":"rm","args":["result.txt"]}),
            json!({"program":"bash","args":["installer.sh"]}),
            json!({"program":"rg","args":["token","/home/u/.ssh/config"]}),
        ] {
            assert!(matches!(
                policy.evaluate_call(&spec(), &arguments, &context()),
                PolicyDecision::RequireApproval(_)
            ));
        }
    }
}
