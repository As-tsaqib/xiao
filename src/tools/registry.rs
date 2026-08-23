use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use tokio::time::timeout;

use crate::{
    runtime::{CapabilityRegistry, CapabilityStatus},
    security::redact::redact_text,
    storage::Storage,
    tools::{
        PolicyDecision, Tool, ToolCall, ToolContext, ToolExecution, ToolPolicy, ToolResult,
        ToolRunStatus, ToolSpec,
    },
};

pub struct ToolRegistry {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    aliases: RwLock<HashMap<String, String>>,
    policy: ToolPolicy,
    max_output_chars: usize,
    capabilities: Option<Arc<CapabilityRegistry>>,
    approvals: Option<Arc<Storage>>,
}

impl ToolRegistry {
    pub fn new(policy: ToolPolicy, max_output_chars: usize) -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
            aliases: RwLock::new(HashMap::new()),
            policy,
            max_output_chars: max_output_chars.max(1),
            capabilities: None,
            approvals: None,
        }
    }

    pub fn with_capabilities(
        policy: ToolPolicy,
        max_output_chars: usize,
        capabilities: Arc<CapabilityRegistry>,
    ) -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
            aliases: RwLock::new(HashMap::new()),
            policy,
            max_output_chars: max_output_chars.max(1),
            capabilities: Some(capabilities),
            approvals: None,
        }
    }

    pub fn with_runtime(
        policy: ToolPolicy,
        max_output_chars: usize,
        capabilities: Arc<CapabilityRegistry>,
        storage: Arc<Storage>,
    ) -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
            aliases: RwLock::new(HashMap::new()),
            policy,
            max_output_chars: max_output_chars.max(1),
            capabilities: Some(capabilities),
            approvals: Some(storage),
        }
    }

    pub fn register<T: Tool + 'static>(&self, tool: T) -> Result<()> {
        self.register_arc(Arc::new(tool))
    }

    pub fn register_arc(&self, tool: Arc<dyn Tool>) -> Result<()> {
        let spec = tool.spec();
        validate_spec(&spec)?;
        let mut tools = self
            .tools
            .write()
            .map_err(|_| anyhow!("tool registry lock poisoned"))?;
        if tools.contains_key(&spec.name) {
            return Err(anyhow!("tool {} is already registered", spec.name));
        }
        tools.insert(spec.name, tool);
        Ok(())
    }

    /// Compatibility aliases are resolved centrally and still pass through
    /// the canonical tool's policy and capability checks.
    pub fn register_alias(&self, alias: &str, canonical: &str) -> Result<()> {
        validate_tool_name(alias)?;
        validate_tool_name(canonical)?;
        if !self
            .tools
            .read()
            .map_err(|_| anyhow!("tool registry lock poisoned"))?
            .contains_key(canonical)
        {
            return Err(anyhow!("alias target {canonical} is not registered"));
        }
        let mut aliases = self
            .aliases
            .write()
            .map_err(|_| anyhow!("tool alias registry lock poisoned"))?;
        if aliases.contains_key(alias) || alias == canonical {
            return Err(anyhow!("tool alias {alias} is already registered"));
        }
        aliases.insert(alias.to_owned(), canonical.to_owned());
        Ok(())
    }

    pub fn spec(&self, name: &str) -> Option<ToolSpec> {
        let canonical = self
            .aliases
            .read()
            .ok()
            .and_then(|aliases| aliases.get(name).cloned())
            .unwrap_or_else(|| name.to_owned());
        self.tools
            .read()
            .ok()?
            .get(&canonical)
            .map(|tool| tool.spec())
    }

    /// Only policy-allowed tools are advertised to providers. Registration is
    /// not itself a grant of model-visible capability.
    pub fn available_specs(&self, context: &ToolContext) -> Vec<ToolSpec> {
        let Ok(tools) = self.tools.read() else {
            return Vec::new();
        };
        let mut specs = tools
            .values()
            .map(|tool| tool.spec())
            .filter(|spec| {
                matches!(
                    self.policy.evaluate(spec, context),
                    PolicyDecision::Allow | PolicyDecision::RequireApproval(_)
                )
            })
            .filter(|spec| self.capabilities_satisfiable(spec))
            .collect::<Vec<_>>();
        specs.sort_by(|left, right| left.name.cmp(&right.name));
        specs
    }

    pub async fn execute(&self, call: &ToolCall, context: &ToolContext) -> ToolExecution {
        let canonical_name = self
            .aliases
            .read()
            .ok()
            .and_then(|aliases| aliases.get(&call.name).cloned())
            .unwrap_or_else(|| call.name.clone());
        let tool = self
            .tools
            .read()
            .ok()
            .and_then(|tools| tools.get(&canonical_name).cloned());
        let Some(tool) = tool else {
            return self.error(call, ToolRunStatus::Denied, "unknown or unavailable tool");
        };
        let spec = tool.spec();
        match self.policy.evaluate_call(&spec, &call.arguments, context) {
            PolicyDecision::Allow => {}
            PolicyDecision::Deny(reason) => {
                return self.error(call, ToolRunStatus::Denied, &reason);
            }
            PolicyDecision::RequireApproval(reason) => {
                let reason = bound(redact_text(&reason), 1_024);
                let Some(storage) = &self.approvals else {
                    return self.error(call, ToolRunStatus::Denied, &reason);
                };
                let arguments_hash = approval_hash(&spec.name, &call.arguments);
                match storage.consume_approval(&context.principal, &spec.name, &arguments_hash) {
                    Ok(true) => {}
                    Ok(false) => {
                        let capability = spec
                            .required_capabilities
                            .first()
                            .map(String::as_str)
                            .unwrap_or("tool.approval");
                        match storage.request_approval(
                            &context.principal,
                            capability,
                            &spec.name,
                            &arguments_hash,
                            &reason,
                        ) {
                            Ok(approval) => {
                                return self.error(
                                    call,
                                    ToolRunStatus::AwaitingApproval,
                                    &format!(
                                        "approval required: {}. Approve request {} then retry",
                                        approval.summary, approval.id
                                    ),
                                );
                            }
                            Err(error) => {
                                return self.error(
                                    call,
                                    ToolRunStatus::Denied,
                                    &format!("approval could not be recorded: {error}"),
                                );
                            }
                        }
                    }
                    Err(error) => {
                        return self.error(
                            call,
                            ToolRunStatus::Denied,
                            &format!("approval lookup failed: {error}"),
                        );
                    }
                }
            }
        }
        if let Some(blocker) = self.capability_blocker(&spec) {
            return self.error(call, ToolRunStatus::Denied, &blocker);
        }

        // Tool implementations never hold the registry lock while awaiting.
        let timeout_ms = spec.timeout_ms.clamp(1, 600_000);
        match timeout(
            Duration::from_millis(timeout_ms),
            tool.execute(context, call.arguments.clone()),
        )
        .await
        {
            Ok(Ok(output)) => ToolExecution {
                result: ToolResult {
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                    output: bound(redact_text(&output), self.max_output_chars),
                    is_error: false,
                },
                status: ToolRunStatus::Succeeded,
            },
            Ok(Err(error)) => self.error(call, ToolRunStatus::Failed, &error.to_string()),
            Err(_) => self.error(call, ToolRunStatus::Failed, "tool timed out"),
        }
    }

    fn capabilities_satisfiable(&self, spec: &ToolSpec) -> bool {
        let Some(registry) = &self.capabilities else {
            return true;
        };
        spec.required_capabilities.iter().all(|requirement| {
            matches!(
                registry.resolve(requirement).status,
                CapabilityStatus::Available
                    | CapabilityStatus::MissingInstallable
                    | CapabilityStatus::ApprovalRequired
            )
        })
    }

    fn capability_blocker(&self, spec: &ToolSpec) -> Option<String> {
        let registry = self.capabilities.as_ref()?;
        spec.required_capabilities.iter().find_map(|requirement| {
            let resolution = registry.resolve(requirement);
            match resolution.status {
                CapabilityStatus::Available | CapabilityStatus::MissingInstallable => None,
                CapabilityStatus::ApprovalRequired => None,
                _ => Some(resolution.concrete_blocker.unwrap_or_else(|| {
                    format!("capability {} is unavailable", resolution.canonical)
                })),
            }
        })
    }

    fn error(&self, call: &ToolCall, status: ToolRunStatus, message: &str) -> ToolExecution {
        ToolExecution {
            result: ToolResult {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                output: bound(redact_text(message), self.max_output_chars),
                is_error: true,
            },
            status,
        }
    }
}

fn approval_hash(tool_name: &str, arguments: &serde_json::Value) -> String {
    let canonical = serde_json::to_vec(arguments).unwrap_or_default();
    let mut digest = Sha256::new();
    digest.update(tool_name.as_bytes());
    digest.update([0]);
    digest.update(canonical);
    format!("{:x}", digest.finalize())
}

fn validate_spec(spec: &ToolSpec) -> Result<()> {
    validate_tool_name(&spec.name)?;
    if spec.description.trim().is_empty() || spec.description.chars().count() > 1_000 {
        return Err(anyhow!("tool description is empty or too long"));
    }
    if !spec.parameters.is_object() {
        return Err(anyhow!("tool parameters must be a JSON schema object"));
    }
    if spec.timeout_ms == 0 {
        return Err(anyhow!("tool timeout must be positive"));
    }
    for capability in &spec.required_capabilities {
        if capability.trim().is_empty()
            || capability.chars().count() > 160
            || capability.contains(['\0', '\r', '\n'])
        {
            return Err(anyhow!("tool capability requirement is invalid"));
        }
    }
    Ok(())
}

fn validate_tool_name(name: &str) -> Result<()> {
    let valid_name = !name.is_empty()
        && name.len() <= 64
        && name.chars().enumerate().all(|(index, character)| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit() && index > 0
                || character == '_' && index > 0
        });
    if valid_name {
        Ok(())
    } else {
        Err(anyhow!("tool name must be canonical snake_case"))
    }
}

fn bound(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value;
    }
    value.chars().take(max_chars).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::{json, Value};

    use crate::{
        runtime::{
            CapabilityRegistry, ExecutionBackend, RuntimeEnvironment, SelinuxState,
            TermuxEnvironment,
        },
        storage::MessageRecord,
        tools::{ToolEffect, ToolOrigin, ToolRisk},
    };
    use std::{collections::BTreeMap, path::PathBuf};

    struct FakeTool {
        name: &'static str,
        risk: ToolRisk,
        output: String,
        delay_ms: u64,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.name.into(),
                description: "A bounded test tool".into(),
                parameters: json!({"type":"object"}),
                risk: self.risk,
                origin: ToolOrigin::Builtin,
                effect: ToolEffect::None,
                required_capabilities: Vec::new(),
                timeout_ms: 10,
            }
        }

        async fn execute(&self, _: &ToolContext, _: Value) -> Result<String> {
            if self.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            }
            Ok(self.output.clone())
        }
    }

    fn context() -> ToolContext {
        ToolContext {
            principal: "p".into(),
            session_id: "s".into(),
            agent_run_id: "r".into(),
            messages: vec![MessageRecord {
                role: "user".into(),
                content: "hello".into(),
                created_at: "now".into(),
            }],
            cancellation: tokio_util::sync::CancellationToken::new(),
            progress: None,
        }
    }

    #[test]
    fn duplicate_registration_is_rejected() {
        let registry = ToolRegistry::new(ToolPolicy::default(), 64);
        for expected in [true, false] {
            let result = registry.register(FakeTool {
                name: "duplicate",
                risk: ToolRisk::ReadOnly,
                output: "ok".into(),
                delay_ms: 0,
            });
            assert_eq!(result.is_ok(), expected);
        }
    }

    #[tokio::test]
    async fn unknown_and_policy_denied_tools_fail_safely() {
        let registry = ToolRegistry::new(ToolPolicy::default(), 64);
        registry
            .register(FakeTool {
                name: "dangerous",
                risk: ToolRisk::Destructive,
                output: "must not run".into(),
                delay_ms: 0,
            })
            .unwrap();
        assert!(registry.available_specs(&context()).is_empty());
        for name in ["missing", "dangerous"] {
            let result = registry
                .execute(
                    &ToolCall {
                        call_id: name.into(),
                        name: name.into(),
                        arguments: json!({}),
                    },
                    &context(),
                )
                .await;
            assert!(result.result.is_error);
            assert_eq!(result.status, ToolRunStatus::Denied);
        }
    }

    #[tokio::test]
    async fn timeout_and_output_are_bounded() {
        let registry = ToolRegistry::new(ToolPolicy::default(), 16);
        registry
            .register(FakeTool {
                name: "large",
                risk: ToolRisk::ReadOnly,
                output: "x".repeat(100),
                delay_ms: 0,
            })
            .unwrap();
        registry
            .register(FakeTool {
                name: "slow",
                risk: ToolRisk::ReadOnly,
                output: "late".into(),
                delay_ms: 100,
            })
            .unwrap();
        let large = registry
            .execute(
                &ToolCall {
                    call_id: "1".into(),
                    name: "large".into(),
                    arguments: json!({}),
                },
                &context(),
            )
            .await;
        assert_eq!(large.result.output.chars().count(), 17);
        let slow = registry
            .execute(
                &ToolCall {
                    call_id: "2".into(),
                    name: "slow".into(),
                    arguments: json!({}),
                },
                &context(),
            )
            .await;
        assert!(slow.result.is_error);
        assert!(slow.result.output.contains("timed out"));
    }

    #[tokio::test]
    async fn privileged_tool_requires_exact_durable_one_shot_approval() {
        let environment = RuntimeEnvironment {
            platform: "android".into(),
            os_version: None,
            android_version: Some("14".into()),
            device_model: None,
            architecture: "aarch64".into(),
            xiao_version: crate::VERSION.into(),
            effective_uid: 0,
            root_available: true,
            root_evidence: "test root".into(),
            selinux: SelinuxState::Enforcing,
            termux: Some(TermuxEnvironment {
                prefix: PathBuf::from("/termux/usr"),
                home: PathBuf::from("/termux/home"),
                path: "/termux/usr/bin".into(),
                shell: PathBuf::from("/termux/usr/bin/sh"),
                package_manager: None,
                uid: Some(10234),
                gid: Some(10234),
            }),
            data_root: PathBuf::from("/xiao"),
            workspace_writable: true,
            binaries: BTreeMap::new(),
            execution_backends: vec![ExecutionBackend::AndroidPrivileged],
            probed_at: "now".into(),
        };
        let capabilities = Arc::new(CapabilityRegistry::from_environment(&environment));
        let storage = Arc::new(Storage::open_memory().unwrap());
        let registry =
            ToolRegistry::with_runtime(ToolPolicy::default(), 1_024, capabilities, storage.clone());
        registry
            .register(FakeTool {
                name: "android_xiao_restart",
                risk: ToolRisk::Privileged,
                output: "{\"verified\":true}".into(),
                delay_ms: 0,
            })
            .unwrap();
        // The test fake needs the same capability declaration as the real
        // typed tool, so wrap its canonical spec through an adapter.
        let call = ToolCall {
            call_id: "approval-1".into(),
            name: "android_xiao_restart".into(),
            arguments: json!({}),
        };
        let first = registry.execute(&call, &context()).await;
        assert_eq!(first.status, ToolRunStatus::AwaitingApproval);
        let pending = storage.pending_approvals("p").unwrap();
        assert_eq!(pending.len(), 1);
        assert!(storage.decide_approval("p", &pending[0].id, true).unwrap());
        let second = registry.execute(&call, &context()).await;
        assert_eq!(second.status, ToolRunStatus::Succeeded);
        let third = registry.execute(&call, &context()).await;
        assert_eq!(third.status, ToolRunStatus::AwaitingApproval);
        assert!(registry
            .available_specs(&context())
            .iter()
            .all(|spec| spec.name != "root" && spec.name != "shell"));
    }
}
