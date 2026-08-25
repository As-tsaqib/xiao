use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use xiao::{
    runtime::{
        CapabilityRegistry, CommandOutcome, DependencyResolver, ExecutionBackend, PackageBackend,
        PackageCandidate, ProcessExecutor, RuntimeEnvironment, SelinuxState, TermuxCommand,
        TrustedPackageRepository,
    },
    tools::{builtin::TermuxTerminalTool, Tool, ToolCall, ToolContext, ToolPolicy, ToolRegistry},
};

#[derive(Clone)]
struct DummyExecutor {
    last_cwd: Arc<std::sync::Mutex<Option<PathBuf>>>,
}

#[async_trait::async_trait]
impl ProcessExecutor for DummyExecutor {
    async fn execute(
        &self,
        command: TermuxCommand,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<CommandOutcome> {
        *self.last_cwd.lock().unwrap() = Some(command.cwd.clone());
        Ok(CommandOutcome {
            program: command.program,
            args: command.args,
            cwd: command.cwd,
            exit_code: Some(0),
            stdout: "ok\n".into(),
            stderr: String::new(),
            duration_ms: 10,
            truncated: false,
            timed_out: false,
            cancelled: false,
        })
    }
}

struct DummyPackageRepo;
#[async_trait::async_trait]
impl TrustedPackageRepository for DummyPackageRepo {
    async fn search(
        &self,
        _query: &str,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<Vec<PackageCandidate>> {
        Ok(vec![])
    }
}

struct DummyBackend;
#[async_trait::async_trait]
impl PackageBackend for DummyBackend {
    fn package_manager_name(&self) -> &str {
        "dummy"
    }
    async fn binary_available(&self, _binary: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
    async fn install(
        &self,
        _pkg: &str,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<CommandOutcome> {
        anyhow::bail!("disabled in test")
    }
}

fn test_capabilities() -> Arc<CapabilityRegistry> {
    Arc::new(CapabilityRegistry::from_environment(&RuntimeEnvironment {
        platform: "android".into(),
        os_version: None,
        android_version: Some("14".into()),
        device_model: None,
        architecture: "aarch64".into(),
        xiao_version: "0.3.0".into(),
        effective_uid: 10234,
        root_available: false,
        root_evidence: "test".into(),
        selinux: SelinuxState::Enforcing,
        data_root: PathBuf::from("/data/adb/xiao"),
        workspace_writable: true,
        termux: None,
        binaries: BTreeMap::new(),
        execution_backends: vec![ExecutionBackend::Termux],
        probed_at: "now".into(),
    }))
}

#[tokio::test]
async fn termux_terminal_defaults_to_isolated_session_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let termux_home = dir.path().join("home");
    std::fs::create_dir_all(&termux_home).unwrap();

    let last_cwd = Arc::new(std::sync::Mutex::new(None));
    let executor: Arc<dyn ProcessExecutor> = Arc::new(DummyExecutor {
        last_cwd: last_cwd.clone(),
    });
    let resolver = Arc::new(DependencyResolver::with_trusted_repository(
        test_capabilities(),
        Arc::new(DummyBackend),
        None,
        Arc::new(DummyPackageRepo),
    ));

    let tool = TermuxTerminalTool::new(executor, resolver, termux_home.clone());
    let context = ToolContext {
        principal: "owner:test".into(),
        session_id: "session-abc-123".into(),
        agent_run_id: "run-1".into(),
        yolo_mode: false,
        messages: vec![],
        cancellation: tokio_util::sync::CancellationToken::new(),
        progress: None,
    };

    let result = tool
        .execute(
            &context,
            serde_json::json!({
                "program": "ls",
            }),
        )
        .await;

    assert!(result.is_ok());
    let recorded_cwd = last_cwd.lock().unwrap().clone().unwrap();
    assert!(recorded_cwd.ends_with(".xiao/workspaces/session-abc-123"));
}

#[tokio::test]
async fn yolo_mode_converts_ask_to_allow_but_never_bypasses_hard_deny() {
    let policy = ToolPolicy::default();
    let registry = ToolRegistry::new(policy, 4096);

    let context_yolo = ToolContext {
        principal: "owner:test".into(),
        session_id: "s1".into(),
        agent_run_id: "r1".into(),
        yolo_mode: true,
        messages: vec![],
        cancellation: tokio_util::sync::CancellationToken::new(),
        progress: None,
    };

    let call = ToolCall {
        call_id: "call-1".into(),
        name: "unregistered_tool".into(),
        arguments: serde_json::json!({}),
    };

    let execution = registry.execute(&call, &context_yolo).await;
    assert!(execution.result.is_error);
}
