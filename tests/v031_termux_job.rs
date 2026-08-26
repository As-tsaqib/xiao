use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use xiao::{
    runtime::{
        CapabilityRegistry, CommandOutcome, DependencyResolver, ExecutionBackend,
        PackageBackend, PackageCandidate, ProcessExecutor, RuntimeEnvironment, SelinuxState,
        TermuxCommand, TrustedPackageRepository,
    },
    tools::{
        builtin::{TermuxJobTool, TermuxTerminalTool},
        Tool, ToolContext,
    },
};

struct DummyExecutor;

#[async_trait]
impl ProcessExecutor for DummyExecutor {
    async fn execute(
        &self,
        command: TermuxCommand,
        _cancellation: CancellationToken,
    ) -> anyhow::Result<CommandOutcome> {
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

#[async_trait]
impl TrustedPackageRepository for DummyPackageRepo {
    async fn search(
        &self,
        _query: &str,
        _cancellation: CancellationToken,
    ) -> anyhow::Result<Vec<PackageCandidate>> {
        Ok(vec![])
    }
}

struct DummyBackend;

#[async_trait]
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
        _cancellation: CancellationToken,
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
        xiao_version: "0.3.1".into(),
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

fn create_test_job_tool(max_steps: usize) -> TermuxJobTool {
    let executor: Arc<dyn ProcessExecutor> = Arc::new(DummyExecutor);
    let resolver = Arc::new(DependencyResolver::with_trusted_repository(
        test_capabilities(),
        Arc::new(DummyBackend),
        None,
        Arc::new(DummyPackageRepo),
    ));
    let terminal = TermuxTerminalTool::new(executor, resolver, "/workspace");
    TermuxJobTool::new(terminal, max_steps)
}

#[test]
fn termux_job_schema_and_bounds() {
    let tool = create_test_job_tool(32);
    let spec = tool.spec();
    assert_eq!(spec.name, "termux_job");
}

#[tokio::test]
async fn termux_job_rejects_empty_or_excessive_steps() {
    let tool = create_test_job_tool(32);

    let ctx = ToolContext {
        principal: "owner-1".into(),
        session_id: "sess-1".into(),
        agent_run_id: "run-1".into(),
        yolo_mode: false,
        messages: vec![],
        cancellation: CancellationToken::new(),
        progress: None,
    };

    // Empty steps
    let res = tool.execute(&ctx, json!({ "steps": [] })).await;
    assert!(res.is_err());

    // Steps exceeding max (e.g. 33 steps)
    let steps: Vec<serde_json::Value> = (0..33)
        .map(|i| json!({ "id": format!("step-{i}"), "program": "echo", "args": ["hi"] }))
        .collect();
    let res = tool.execute(&ctx, json!({ "steps": steps })).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn termux_job_denies_root_escalation() {
    let tool = create_test_job_tool(32);

    let ctx = ToolContext {
        principal: "owner-1".into(),
        session_id: "sess-1".into(),
        agent_run_id: "run-1".into(),
        yolo_mode: false,
        messages: vec![],
        cancellation: CancellationToken::new(),
        progress: None,
    };

    let res = tool
        .execute(
            &ctx,
            json!({
                "steps": [
                    { "id": "step-su", "program": "su", "args": ["-c", "id"] }
                ]
            }),
        )
        .await
        .unwrap();

    assert!(res.contains("denied"));
    assert!(res.contains("failed"));
}
