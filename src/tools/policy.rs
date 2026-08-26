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
                "pdf_create",
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

pub(crate) fn is_sensitive_path_or_value(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    const SENSITIVE_MARKERS: &[&str] = &[
        "/.ssh/",
        "/.ssh",
        "id_rsa",
        "id_ed25519",
        "id_ecdsa",
        "id_dsa",
        "known_hosts",
        "authorized_keys",
        "/.gnupg/",
        "/.gnupg",
        "secring.gpg",
        "/.aws/",
        "/.aws",
        "/secrets/",
        "credentials.json",
        ".netrc",
        "/.netrc",
        "/etc/shadow",
        "/etc/passwd",
        "/.env",
        "/.config/gcloud",
        "/.azure",
    ];
    if SENSITIVE_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return true;
    }
    crate::security::redact::contains_secret_material(value)
}

pub(crate) fn is_sensitive_env_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    const SENSITIVE_KEYS: &[&str] = &[
        "authorization",
        "access_token",
        "refresh_token",
        "id_token",
        "api_key",
        "api-key",
        "apikey",
        "bot_token",
        "token",
        "secret",
        "password",
        "passcode",
        "ssh_auth_sock",
        "ssh_key",
        "aws_secret_access_key",
        "aws_access_key_id",
        "aws_session_token",
    ];
    SENSITIVE_KEYS.iter().any(|k| lower.contains(k))
}

pub(crate) fn termux_call_policy(arguments: &Value) -> PolicyDecision {
    let Some(object) = arguments.as_object() else {
        return PolicyDecision::Deny("termux_terminal arguments must be an object".into());
    };
    let Some(raw_program) = object.get("program").and_then(Value::as_str) else {
        return PolicyDecision::Deny("termux_terminal requires a structured program".into());
    };
    let trimmed_raw = raw_program.trim();
    if trimmed_raw.is_empty() {
        return PolicyDecision::Deny("termux_terminal requires a non-empty program".into());
    }
    if trimmed_raw.contains(' ')
        || trimmed_raw.contains('\t')
        || trimmed_raw.contains('\n')
        || trimmed_raw.contains('|')
        || trimmed_raw.contains(';')
        || trimmed_raw.contains('&')
    {
        return PolicyDecision::Deny(
            "model-supplied shell command strings are forbidden; provide structured binary and argv in 'program' and 'args' (e.g. program: 'python', args: ['script.py'])".into(),
        );
    }
    let program = std::path::Path::new(trimmed_raw)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(trimmed_raw)
        .to_ascii_lowercase();

    if matches!(program.as_str(), "su" | "tsu" | "sudo" | "doas") {
        return PolicyDecision::Deny(format!(
            "root escalation via {program} is forbidden in Termux unprivileged executor; root operations require typed AndroidBroker tools"
        ));
    }
    let args = object
        .get("args")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();

    if let Some(raw_cwd) = object.get("cwd").and_then(Value::as_str) {
        let cwd_path = std::path::Path::new(raw_cwd);
        if cwd_path.is_absolute()
            || cwd_path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return PolicyDecision::Deny(
                "cwd must be a relative path within the workspace; parent traversal and absolute escapes are forbidden".into(),
            );
        }
    }

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

    let sensitive = is_sensitive_path_or_value(trimmed_raw)
        || object
            .get("cwd")
            .and_then(Value::as_str)
            .map(is_sensitive_path_or_value)
            .unwrap_or(false)
        || args.iter().any(|argument| is_sensitive_path_or_value(argument))
        || object
            .get("environment")
            .and_then(Value::as_object)
            .map(|env| {
                env.iter().any(|(k, v)| {
                    is_sensitive_env_key(k)
                        || v.as_str().map(is_sensitive_path_or_value).unwrap_or(false)
                })
            })
            .unwrap_or(false);
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
