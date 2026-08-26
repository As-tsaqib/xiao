use std::{collections::BTreeMap, path::PathBuf, sync::Arc, sync::Mutex};

use async_trait::async_trait;
use serde_json::json;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use xiao::{
    runtime::{
        CapabilityRegistry, CommandOutcome, DependencyResolver, ExecutionBackend, PackageBackend,
        PackageCandidate, ProcessExecutor, RuntimeEnvironment, SelinuxState, TermuxCommand,
        TrustedPackageRepository,
    },
    storage::Storage,
    tools::{
        builtin::{PdfCreateTool, TermuxJobTool, TermuxTerminalTool},
        PolicyDecision, Tool, ToolContext, ToolPolicy,
    },
};

struct RecordingExecutor {
    commands: Mutex<Vec<TermuxCommand>>,
}

#[async_trait]
impl ProcessExecutor for RecordingExecutor {
    async fn execute(
        &self,
        command: TermuxCommand,
        _cancellation: CancellationToken,
    ) -> anyhow::Result<CommandOutcome> {
        self.commands.lock().unwrap().push(command.clone());
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

fn create_test_job_tool(
    max_steps: usize,
    executor: Arc<dyn ProcessExecutor>,
    default_cwd: impl Into<PathBuf>,
    storage: Option<Arc<Storage>>,
) -> TermuxJobTool {
    let resolver = Arc::new(DependencyResolver::with_trusted_repository(
        test_capabilities(),
        Arc::new(DummyBackend),
        None,
        Arc::new(DummyPackageRepo),
    ));
    let terminal = TermuxTerminalTool::new(executor, resolver, default_cwd);
    if let Some(storage) = storage {
        TermuxJobTool::with_storage(terminal, max_steps, storage)
    } else {
        TermuxJobTool::new(terminal, max_steps)
    }
}

#[test]
fn termux_job_schema_and_bounds() {
    let executor = Arc::new(RecordingExecutor {
        commands: Mutex::new(Vec::new()),
    });
    let tool = create_test_job_tool(32, executor, "/workspace", None);
    let spec = tool.spec();
    assert_eq!(spec.name, "termux_job");
}

#[tokio::test]
async fn termux_job_rejects_empty_or_excessive_steps() {
    let executor = Arc::new(RecordingExecutor {
        commands: Mutex::new(Vec::new()),
    });
    let tool = create_test_job_tool(32, executor, "/workspace", None);

    let ctx = ToolContext {
        principal: "owner-1".into(),
        session_id: "sess-1".into(),
        agent_run_id: "run-1".into(),
        yolo_mode: false,
        messages: vec![],
        cancellation: CancellationToken::new(),
        progress: None,
    };

    let res = tool.execute(&ctx, json!({ "steps": [] })).await;
    assert!(res.is_err());

    let steps: Vec<serde_json::Value> = (0..33)
        .map(|i| json!({ "id": format!("step-{i}"), "program": "echo", "args": ["hi"] }))
        .collect();
    let res = tool.execute(&ctx, json!({ "steps": steps })).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn termux_job_denies_root_escalation_and_shell_strings() {
    let executor = Arc::new(RecordingExecutor {
        commands: Mutex::new(Vec::new()),
    });
    let tool = create_test_job_tool(32, executor.clone(), "/workspace", None);

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

    assert!(res.contains(r#""status":"denied"#));
    assert_eq!(executor.commands.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn termux_job_rejects_approval_requiring_substeps_with_distinct_status_and_audit() {
    let temp = tempdir().unwrap();
    let db_path = temp.path().join("test.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let run_id = storage
        .create_agent_run("owner-1", "sess-1", "custom", "test-model", Some("test goal"))
        .unwrap();

    let tool_run_id = storage
        .create_tool_run(&run_id, "call-job-1", "termux_job", "{}", "side_effect")
        .unwrap();

    let executor = Arc::new(RecordingExecutor {
        commands: Mutex::new(Vec::new()),
    });
    let tool = create_test_job_tool(16, executor.clone(), temp.path(), Some(storage.clone()));

    let ctx = ToolContext {
        principal: "owner-1".into(),
        session_id: "sess-1".into(),
        agent_run_id: run_id.clone(),
        yolo_mode: false,
        messages: vec![],
        cancellation: CancellationToken::new(),
        progress: None,
    };

    let output = tool
        .execute(
            &ctx,
            json!({
                "steps": [
                    {
                        "id": "step-destructive",
                        "program": "rm",
                        "args": ["result.txt"]
                    },
                    {
                        "id": "step-safe",
                        "program": "echo",
                        "args": ["hello"]
                    }
                ]
            }),
        )
        .await
        .unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["job_status"], "failed");
    let step0 = &parsed["steps"][0];
    assert_eq!(step0["status"], "approval_required");
    assert!(step0["error"].as_str().unwrap().contains(
        "unsupported inside termux_job; call termux_terminal separately for exact approval"
    ));
    // Since continue_on_error defaults to false, step 2 was not reached
    assert_eq!(parsed["steps"].as_array().unwrap().len(), 1);
    assert_eq!(executor.commands.lock().unwrap().len(), 0);

    // Verify audit record in SQLite storage has status approval_required
    let steps = storage.tool_run_steps(&tool_run_id).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].status, "approval_required");
    assert!(steps[0].error.as_deref().unwrap().contains(
        "unsupported inside termux_job; call termux_terminal separately for exact approval"
    ));
}

#[tokio::test]
async fn pdf_create_tool_policy_and_symlink_containment() {
    let temp = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let tool = PdfCreateTool::new(temp.path());

    // 1. Tool policy allows pdf_create as safe side-effect
    let policy = ToolPolicy::default();
    let ctx = ToolContext {
        principal: "owner-1".into(),
        session_id: "sess-pdf".into(),
        agent_run_id: "run-pdf".into(),
        yolo_mode: false,
        messages: vec![],
        cancellation: CancellationToken::new(),
        progress: None,
    };
    let decision = policy.evaluate(&tool.spec(), &ctx);
    assert_eq!(decision, PolicyDecision::Allow);

    // 2. Symlink escape is rejected
    let workspace = temp.path().join(".xiao/workspaces/sess-pdf");
    std::fs::create_dir_all(&workspace).unwrap();
    #[cfg(unix)]
    {
        let symlink_dir = workspace.join("symlink_folder");
        std::os::unix::fs::symlink(outside.path(), &symlink_dir).unwrap();

        let err = tool
            .execute(
                &ctx,
                json!({
                    "path": "symlink_folder/malicious.pdf",
                    "content": "test payload"
                }),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("symlink") || err.to_string().contains("escapes"));
    }

    // 3. Valid parseable PDF succeeds
    let output = tool
        .execute(
            &ctx,
            json!({
                "path": "docs/valid.pdf",
                "title": "Verified Document",
                "content": "Content inside bounded per-session workspace."
            }),
        )
        .await
        .unwrap();

    assert!(output.contains(r#""status":"succeeded""#));
    let pdf_path = workspace.join("docs/valid.pdf");
    assert!(pdf_path.exists());
    let pdf_bytes = std::fs::read(&pdf_path).unwrap();
    assert!(pdf_bytes.starts_with(b"%PDF-1.4"));
    let extracted = pdf_extract::extract_text_from_mem(&pdf_bytes).unwrap();
    assert!(extracted.contains("Verified Document"));
    assert!(extracted.contains("Content inside bounded per-session workspace."));
}
