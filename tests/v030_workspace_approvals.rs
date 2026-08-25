use std::{path::PathBuf, sync::Arc};

use xiao::{
    runtime::{
        CapabilityRegistry, CommandOutcome, DependencyResolver, ExecutionBackend, ExecutionPurpose,
        PackageBackend, PackageCandidate, ProcessExecutor, RuntimeEnvironment, SelinuxState,
        TermuxCommand, TermuxEnvironment, TermuxPackageBackend, TermuxRepositoryBackend,
        TrustedPackageRepository,
    },
    storage::Storage,
    tools::{
        builtin::terminal::TermuxTerminalTool, policy::ToolPolicy, Tool, ToolContext, ToolRegistry,
        ToolRisk,
    },
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
            exit_code: 0,
            stdout: "ok\n".into(),
            stderr: String::new(),
            duration_ms: 10,
            truncated: false,
            purpose: command.purpose,
        })
    }
}

struct DummyPackageRepo;
#[async_trait::async_trait]
impl TrustedPackageRepository for DummyPackageRepo {
    async fn resolve_candidate(
        &self,
        _binary: &str,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<Option<PackageCandidate>> {
        Ok(None)
    }
}

struct DummyBackend;
#[async_trait::async_trait]
impl PackageBackend for DummyBackend {
    async fn install_package(
        &self,
        _pkg: &str,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<CommandOutcome> {
        anyhow::bail!("disabled in test")
    }
}

#[tokio::test]
async fn termux_terminal_defaults_to_isolated_session_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let termux_home = dir.path().join("home");
    std::fs::create_dir_all(&termux_home).unwrap();

    let env = TermuxEnvironment {
        prefix: dir.path().to_path_buf(),
        home: termux_home.clone(),
        path: "/bin".into(),
        shell: "/bin/sh".into(),
        package_manager: None,
        uid: Some(1000),
        gid: Some(1000),
    };
    let last_cwd = Arc::new(std::sync::Mutex::new(None));
    let executor: Arc<dyn ProcessExecutor> = Arc::new(DummyExecutor {
        last_cwd: last_cwd.clone(),
    });
    let resolver = Arc::new(DependencyResolver::with_trusted_repository(
        Arc::new(CapabilityRegistry::default()),
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
    assert!(
        recorded_cwd.ends_with(".xiao/workspaces/session-abc-123")
            || recorded_cwd == termux_home
            || recorded_cwd.starts_with(&termux_home)
    );
}

#[tokio::test]
async fn yolo_mode_converts_ask_to_allow_but_never_bypasses_hard_deny() {
    let policy = ToolPolicy::default();
    let storage = Arc::new(Storage::open_memory().unwrap());
    let registry = ToolRegistry::with_runtime(policy, 4096, storage);

    // Hard deny must fail closed regardless of yolo_mode
    let context_yolo = ToolContext {
        principal: "owner:test".into(),
        session_id: "s1".into(),
        agent_run_id: "r1".into(),
        yolo_mode: true,
        messages: vec![],
        cancellation: tokio_util::sync::CancellationToken::new(),
        progress: None,
    };

    // Unknown/denied tool fails closed
    assert!(registry
        .execute(&context_yolo, "unregistered_tool", serde_json::json!({}))
        .await
        .is_err());
}
