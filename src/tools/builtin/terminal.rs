use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    runtime::{DependencyResolver, ExecutionPurpose, ProcessExecutor, TermuxCommand},
    tools::{Tool, ToolContext, ToolEffect, ToolOrigin, ToolRisk, ToolSpec},
};

pub struct TermuxTerminalTool {
    executor: Arc<dyn ProcessExecutor>,
    dependencies: Arc<DependencyResolver>,
    default_cwd: PathBuf,
}

impl Clone for TermuxTerminalTool {
    fn clone(&self) -> Self {
        Self {
            executor: self.executor.clone(),
            dependencies: self.dependencies.clone(),
            default_cwd: self.default_cwd.clone(),
        }
    }
}

impl TermuxTerminalTool {
    pub fn new(
        executor: Arc<dyn ProcessExecutor>,
        dependencies: Arc<DependencyResolver>,
        default_cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            executor,
            dependencies,
            default_cwd: default_cwd.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    cwd: Option<PathBuf>,
    #[serde(default)]
    environment: BTreeMap<String, String>,
    timeout_ms: Option<u64>,
    #[serde(default)]
    purpose: Option<ExecutionPurpose>,
    #[serde(default)]
    artifacts: Vec<PathBuf>,
}

#[async_trait]
impl Tool for TermuxTerminalTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "termux_terminal".into(),
            description: "Run one structured argv command in the detected unprivileged Termux environment. Missing trusted user-space binaries are resolved and installed through Xiao package policy before the original command resumes. Shell command strings, root escalation, package mutations, and destructive system commands are rejected.".into(),
            parameters: json!({
                "type":"object",
                "properties":{
                    "program":{"type":"string","minLength":1,"maxLength":128},
                    "args":{"type":"array","maxItems":128,"items":{"type":"string","maxLength":8192}},
                    "cwd":{"type":"string"},
                    "environment":{"type":"object","additionalProperties":{"type":"string"}},
                    "timeout_ms":{"type":"integer","minimum":100,"maximum":600000},
                    "purpose":{"type":"string","enum":["user_command","verification"]}
                    ,"artifacts":{"type":"array","maxItems":16,"items":{"type":"string"}}
                },
                "required":["program"],
                "additionalProperties":false
            }),
            risk: ToolRisk::SideEffect,
            origin: ToolOrigin::Termux,
            effect: ToolEffect::NonIdempotent,
            required_capabilities: vec!["execution.termux".into()],
            timeout_ms: 600_000,
        }
    }

    async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
        let arguments: Arguments = serde_json::from_value(arguments)?;
        preflight_validate_command(
            &arguments.program,
            &arguments.args,
            arguments.cwd.as_ref(),
            &arguments.artifacts,
        )?;
        let declared_artifacts = arguments.artifacts.clone();
        let purpose = arguments.purpose.unwrap_or(ExecutionPurpose::UserCommand);
        if purpose == ExecutionPurpose::PackageInstall {
            return Err(anyhow!(
                "package installation purpose is reserved for Xiao DependencyResolver"
            ));
        }
        let dependency = self
            .dependencies
            .ensure_binary(
                &arguments.program,
                Some(&context.agent_run_id),
                context.cancellation.clone(),
                context.progress.as_ref(),
            )
            .await?;
        let session_workspace = if context.session_id.is_empty() {
            self.default_cwd.clone()
        } else {
            let dir = self
                .default_cwd
                .join(".xiao/workspaces")
                .join(&context.session_id);
            let _ = std::fs::create_dir_all(&dir);
            dir
        };
        let effective_cwd = match arguments.cwd {
            Some(custom) => {
                if custom.is_absolute() {
                    custom
                } else {
                    session_workspace.join(custom)
                }
            }
            None => session_workspace.clone(),
        };
        let outcome = self
            .executor
            .execute(
                TermuxCommand {
                    program: arguments.program,
                    args: arguments.args,
                    cwd: effective_cwd,
                    environment: arguments.environment,
                    timeout_ms: arguments.timeout_ms.unwrap_or(120_000),
                    max_output_chars: 16_384,
                    purpose,
                },
                context.cancellation.clone(),
            )
            .await?;
        if !outcome.succeeded() {
            return Err(anyhow!(
                "Termux command failed: {}; stderr: {}",
                outcome.observable_summary(),
                outcome.stderr
            ));
        }
        let artifacts = verified_artifacts(&outcome.cwd, &self.default_cwd, &declared_artifacts)?;
        Ok(serde_json::to_string(&json!({
            "outcome": outcome,
            "dependency": dependency,
            "verification_evidence": purpose == ExecutionPurpose::Verification,
            "artifacts": artifacts,
        }))?)
    }
}

pub struct TermuxJobTool {
    terminal: TermuxTerminalTool,
    max_steps: usize,
    storage: Option<Arc<crate::storage::Storage>>,
}

impl TermuxJobTool {
    pub fn new(terminal: TermuxTerminalTool, max_steps: usize) -> Self {
        Self {
            terminal,
            max_steps: max_steps.clamp(1, 64),
            storage: None,
        }
    }

    pub fn with_storage(
        terminal: TermuxTerminalTool,
        max_steps: usize,
        storage: Arc<crate::storage::Storage>,
    ) -> Self {
        Self {
            terminal,
            max_steps: max_steps.clamp(1, 64),
            storage: Some(storage),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobArguments {
    steps: Vec<JobStep>,
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobStep {
    id: String,
    program: String,
    #[serde(default)]
    args: Vec<String>,
    cwd: Option<PathBuf>,
    #[serde(default)]
    continue_on_error: bool,
}

#[async_trait]
impl Tool for TermuxJobTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "termux_job".into(),
            description: "Run a bounded ordered workflow of structured argv commands under the unprivileged Termux UID. Every step is policy checked; opaque shell strings and root escalation are forbidden.".into(),
            parameters: json!({
                "type":"object",
                "properties":{
                    "steps":{"type":"array","minItems":1,"maxItems":64,"items":{
                        "type":"object","properties":{
                            "id":{"type":"string","minLength":1,"maxLength":64},
                            "program":{"type":"string","minLength":1,"maxLength":128},
                            "args":{"type":"array","maxItems":128,"items":{"type":"string","maxLength":8192}},
                            "cwd":{"type":"string"},
                            "continue_on_error":{"type":"boolean"}
                        },"required":["id","program"],"additionalProperties":false
                    }},
                    "mode":{"type":"string","enum":["auto","sequential"]}
                },"required":["steps"],"additionalProperties":false
            }),
            risk: ToolRisk::SideEffect,
            origin: ToolOrigin::Termux,
            effect: ToolEffect::NonIdempotent,
            required_capabilities: vec!["execution.termux".into()],
            timeout_ms: 600_000,
        }
    }

    async fn execute(&self, context: &ToolContext, arguments: Value) -> Result<String> {
        let job: JobArguments = serde_json::from_value(arguments)?;
        if job.steps.is_empty() || job.steps.len() > self.max_steps || job.steps.len() > 64 {
            return Err(anyhow!("termux_job requires 1..={} steps", self.max_steps));
        }
        if !matches!(
            job.mode.as_deref(),
            None | Some("auto") | Some("sequential")
        ) {
            return Err(anyhow!("termux_job mode must be auto or sequential"));
        }
        let mut results = Vec::with_capacity(job.steps.len());
        for (index, step) in job.steps.into_iter().enumerate() {
            if context.cancellation.is_cancelled() {
                return Err(anyhow!("termux_job cancelled"));
            }
            let call = json!({"program":step.program,"args":step.args,"cwd":step.cwd});
            let audit_id = self
                .storage
                .as_ref()
                .map(|storage| {
                    storage.create_tool_run_step(
                        &context.agent_run_id,
                        index,
                        &step.id,
                        &step.program,
                        &call,
                    )
                })
                .transpose()?;
            match crate::tools::policy::termux_call_policy(&call) {
                crate::tools::PolicyDecision::Allow => {}
                crate::tools::PolicyDecision::Deny(reason)
                | crate::tools::PolicyDecision::RequireApproval(reason) => {
                    if let (Some(storage), Some(id)) = (&self.storage, audit_id.as_deref()) {
                        storage.finish_tool_run_step(id, "denied", None, Some(&reason))?;
                    }
                    results
                        .push(json!({"index":index,"id":step.id,"status":"denied","error":reason}));
                    if !step.continue_on_error {
                        break;
                    }
                    continue;
                }
            }
            if context.cancellation.is_cancelled() {
                results.push(serde_json::json!({"index":index,"id":step.id,"status":"cancelled","error":"job cancelled"}));
                break;
            }
            match self.terminal.execute(context, call).await {
                Ok(output) => {
                    if let (Some(storage), Some(id)) = (&self.storage, audit_id.as_deref()) {
                        storage.finish_tool_run_step(id, "succeeded", Some(&output), None)?;
                    }
                    results.push(
                        json!({"index":index,"id":step.id,"status":"succeeded","summary":output}),
                    );
                }
                Err(error) => {
                    if let (Some(storage), Some(id)) = (&self.storage, audit_id.as_deref()) {
                        storage.finish_tool_run_step(
                            id,
                            if context.cancellation.is_cancelled() {
                                "interrupted"
                            } else {
                                "failed"
                            },
                            None,
                            Some(&error.to_string()),
                        )?;
                    }
                    results.push(json!({"index":index,"id":step.id,"status":"failed","error":error.to_string()}));
                    if !step.continue_on_error {
                        break;
                    }
                }
            }
        }
        Ok(serde_json::to_string(&json!({
            "job_status": if results.iter().all(|item| item["status"] == "succeeded") { "succeeded" } else { "failed" },
            "steps": results,
            "verification_evidence": true
        }))?)
    }
}

fn preflight_validate_command(
    program: &str,
    args: &[String],
    cwd: Option<&PathBuf>,
    artifacts: &[PathBuf],
) -> Result<()> {
    let trimmed_prog = program.trim();
    if trimmed_prog.is_empty() {
        return Err(anyhow!("program must not be empty"));
    }
    if trimmed_prog.contains(' ')
        || trimmed_prog.contains('\t')
        || trimmed_prog.contains('\n')
        || trimmed_prog.contains('|')
        || trimmed_prog.contains(';')
        || trimmed_prog.contains('&')
    {
        return Err(anyhow!(
            "model-supplied shell command strings are forbidden; provide structured binary and argv in 'program' and 'args' (e.g. program: 'python', args: ['script.py'])"
        ));
    }
    if ["su", "tsu", "sudo", "doas"].contains(&trimmed_prog) {
        return Err(anyhow!(
            "root escalation via {trimmed_prog} is forbidden in Termux unprivileged executor; root operations require typed AndroidBroker tools"
        ));
    }
    if ["sh", "bash", "zsh", "fish", "dash", "ksh"].contains(&trimmed_prog)
        && args
            .iter()
            .any(|arg| matches!(arg.as_str(), "-c" | "--command" | "-ic" | "-c;"))
    {
        return Err(anyhow!(
            "model-supplied shell command strings are forbidden; use structured argv"
        ));
    }
    if let Some(cwd) = cwd {
        if cwd
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(anyhow!(
                "cwd must be within workspace; parent directory traversal ('..') is forbidden"
            ));
        }
    }
    for artifact in artifacts {
        if artifact.is_absolute()
            || artifact
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(anyhow!(
                "artifact paths must be relative paths within the workspace; parent traversal ('..') and absolute escapes are forbidden"
            ));
        }
    }
    Ok(())
}

fn verified_artifacts(
    cwd: &std::path::Path,
    workspace: &std::path::Path,
    paths: &[PathBuf],
) -> Result<Vec<Value>> {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    paths
        .iter()
        .map(|path| {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            };
            let canonical = path.canonicalize().map_err(|_| {
                anyhow!(
                    "declared result artifact does not exist: {}",
                    path.display()
                )
            })?;
            if !canonical.starts_with(cwd) && !canonical.starts_with(&workspace) {
                return Err(anyhow!(
                    "result artifact is outside the controlled task workspace"
                ));
            }
            let metadata = std::fs::metadata(&canonical)?;
            if !metadata.is_file() || metadata.len() > 50 * 1024 * 1024 {
                return Err(anyhow!("result artifact is not a bounded regular file"));
            }
            Ok(json!({
                "path": canonical,
                "name": canonical.file_name().and_then(|name| name.to_str()).unwrap_or("result"),
                "size_bytes": metadata.len(),
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        runtime::{
            CapabilityRegistry, CommandOutcome, ExecutionBackend, PackageBackend, PackageCandidate,
            RuntimeEnvironment, SelinuxState, TermuxEnvironment, TrustedPackageRepository,
        },
        storage::{MessageRecord, Storage},
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        path::PathBuf,
        sync::Mutex,
    };
    use tokio_util::sync::CancellationToken;

    struct FakePackages {
        available: Mutex<BTreeSet<String>>,
        installs: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl PackageBackend for FakePackages {
        fn package_manager_name(&self) -> &str {
            "pkg"
        }
        async fn binary_available(&self, binary: &str) -> Result<bool> {
            Ok(self.available.lock().unwrap().contains(binary))
        }
        async fn install(&self, package: &str, _: CancellationToken) -> Result<CommandOutcome> {
            self.installs.lock().unwrap().push(package.into());
            self.available.lock().unwrap().insert(package.into());
            Ok(outcome("pkg", "installed"))
        }
    }

    struct FakeExecutor {
        commands: Mutex<Vec<TermuxCommand>>,
    }

    struct FakeRepository;

    #[async_trait]
    impl TrustedPackageRepository for FakeRepository {
        async fn search(
            &self,
            binary: &str,
            _: CancellationToken,
        ) -> Result<Vec<PackageCandidate>> {
            Ok(vec![PackageCandidate {
                package: binary.into(),
                source: "termux_repository_fake_index".into(),
                provided_binaries: vec![binary.into()],
            }])
        }
    }

    #[async_trait]
    impl ProcessExecutor for FakeExecutor {
        async fn execute(
            &self,
            command: TermuxCommand,
            _: CancellationToken,
        ) -> Result<CommandOutcome> {
            self.commands.lock().unwrap().push(command.clone());
            Ok(outcome(&command.program, "audio.mp3"))
        }
    }

    fn outcome(program: &str, stdout: &str) -> CommandOutcome {
        CommandOutcome {
            program: program.into(),
            args: Vec::new(),
            cwd: PathBuf::from("/workspace"),
            exit_code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
            cancelled: false,
            truncated: false,
            duration_ms: 1,
        }
    }

    fn capabilities() -> Arc<CapabilityRegistry> {
        Arc::new(CapabilityRegistry::from_environment(&RuntimeEnvironment {
            platform: "android".into(),
            os_version: None,
            android_version: Some("14".into()),
            device_model: None,
            architecture: "aarch64".into(),
            xiao_version: crate::VERSION.into(),
            effective_uid: 10234,
            root_available: false,
            root_evidence: "none".into(),
            selinux: SelinuxState::Enforcing,
            termux: Some(TermuxEnvironment {
                prefix: "/termux/usr".into(),
                home: "/termux/home".into(),
                path: "/termux/usr/bin".into(),
                shell: "/termux/usr/bin/sh".into(),
                package_manager: Some("/termux/usr/bin/pkg".into()),
                uid: Some(10234),
                gid: Some(10234),
            }),
            data_root: "/workspace".into(),
            workspace_writable: true,
            binaries: BTreeMap::from([("ffmpeg".into(), None)]),
            execution_backends: vec![ExecutionBackend::Termux],
            probed_at: "now".into(),
        }))
    }

    #[tokio::test]
    async fn missing_dependency_installs_reprobes_and_resumes_original_command() {
        let packages = Arc::new(FakePackages {
            available: Mutex::new(BTreeSet::new()),
            installs: Mutex::new(Vec::new()),
        });
        let executor = Arc::new(FakeExecutor {
            commands: Mutex::new(Vec::new()),
        });
        let resolver = Arc::new(DependencyResolver::new(
            capabilities(),
            packages.clone(),
            None,
        ));
        let tool = TermuxTerminalTool::new(executor.clone(), resolver, "/workspace");
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let result = tool
            .execute(
                &ToolContext {
                    principal: "owner".into(),
                    session_id: "session".into(),
                    agent_run_id: "run".into(),
                    yolo_mode: false,
                    messages: vec![MessageRecord {
                        role: "user".into(),
                        content: "Extract audio".into(),
                        created_at: "now".into(),
                    }],
                    cancellation: CancellationToken::new(),
                    progress: Some(progress_tx),
                },
                json!({
                    "program":"ffmpeg",
                    "args":["-i","video.mp4","audio.mp3"]
                }),
            )
            .await
            .unwrap();
        assert!(result.contains("audio.mp3"));
        assert_eq!(&*packages.installs.lock().unwrap(), &["ffmpeg"]);
        assert_eq!(executor.commands.lock().unwrap().len(), 1);
        let statuses = std::iter::from_fn(|| progress_rx.try_recv().ok()).collect::<Vec<_>>();
        assert!(statuses.iter().any(|status| status.contains("installing")));
        assert!(statuses.iter().any(|status| status.contains("resuming")));
    }

    #[tokio::test]
    async fn unknown_binary_uses_validated_trusted_repository_then_resumes() {
        let packages = Arc::new(FakePackages {
            available: Mutex::new(BTreeSet::new()),
            installs: Mutex::new(Vec::new()),
        });
        let executor = Arc::new(FakeExecutor {
            commands: Mutex::new(Vec::new()),
        });
        let storage = Arc::new(Storage::open_memory().unwrap());
        let session = storage
            .create_session("owner", "task", "custom", None, "m", false, None)
            .unwrap();
        let run = storage
            .create_agent_run("owner", &session.id, "custom", "m", Some("inspect media"))
            .unwrap();
        let resolver = Arc::new(DependencyResolver::with_trusted_repository(
            capabilities(),
            packages.clone(),
            Some(storage.clone()),
            Arc::new(FakeRepository),
        ));
        let tool = TermuxTerminalTool::new(executor.clone(), resolver, "/workspace");
        let result = tool
            .execute(
                &ToolContext {
                    principal: "owner".into(),
                    session_id: session.id,
                    agent_run_id: run.clone(),
                    yolo_mode: false,
                    messages: Vec::new(),
                    cancellation: CancellationToken::new(),
                    progress: None,
                },
                json!({"program":"xiao-media-probe","args":["clip.mp4"]}),
            )
            .await
            .unwrap();
        assert!(result.contains("audio.mp3"));
        assert_eq!(&*packages.installs.lock().unwrap(), &["xiao-media-probe"]);
        assert_eq!(executor.commands.lock().unwrap().len(), 1);
        let audit = storage.dependency_installs(&run).unwrap();
        assert_eq!(audit.len(), 1);
        assert!(audit[0].validated);
        assert_eq!(audit[0].source, "termux_repository_fake_index");
        assert_eq!(
            audit[0].requested_capability.as_deref(),
            Some("binary.xiao-media-probe")
        );
        assert_eq!(audit[0].status, "succeeded");
    }

    #[test]
    fn declared_artifacts_must_be_bounded_regular_files_in_controlled_space() {
        let workspace = tempfile::tempdir().unwrap();
        let task = workspace.path().join("task");
        std::fs::create_dir(&task).unwrap();
        std::fs::write(task.join("result.bin"), b"verified result").unwrap();
        let accepted =
            verified_artifacts(&task, workspace.path(), &[PathBuf::from("result.bin")]).unwrap();
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0]["name"], "result.bin");

        let outside = tempfile::NamedTempFile::new().unwrap();
        assert!(
            verified_artifacts(&task, workspace.path(), &[outside.path().to_path_buf()]).is_err()
        );
        assert!(verified_artifacts(&task, workspace.path(), &[PathBuf::from("missing")]).is_err());
    }

    #[tokio::test]
    async fn terminal_preflight_rejects_shell_strings_and_artifact_escapes() {
        let packages = Arc::new(FakePackages {
            available: Mutex::new(BTreeSet::new()),
            installs: Mutex::new(Vec::new()),
        });
        let executor = Arc::new(FakeExecutor {
            commands: Mutex::new(Vec::new()),
        });
        let resolver = Arc::new(DependencyResolver::new(
            capabilities(),
            packages.clone(),
            None,
        ));
        let terminal = TermuxTerminalTool::new(executor.clone(), resolver, "/workspace");
        let context = ToolContext {
            principal: "owner".into(),
            session_id: "session".into(),
            agent_run_id: "run".into(),
            yolo_mode: false,
            messages: Vec::new(),
            cancellation: CancellationToken::new(),
            progress: None,
        };

        // Rejects compound shell string
        let err = terminal
            .execute(&context, json!({"program": "python3 -c 'print(1)'"}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("shell command strings are forbidden"));

        // Rejects bash -c
        let err2 = terminal
            .execute(&context, json!({"program": "bash", "args": ["-c", "id"]}))
            .await
            .unwrap_err();
        assert!(err2.to_string().contains("shell command strings are forbidden"));

        // Rejects artifact escape
        let err3 = terminal
            .execute(&context, json!({"program": "ls", "artifacts": ["../../etc/passwd"]}))
            .await
            .unwrap_err();
        assert!(err3.to_string().contains("relative paths within the workspace"));
    }

    #[tokio::test]
    async fn termux_job_rejects_su_and_root_escalation() {
        let packages = Arc::new(FakePackages {
            available: Mutex::new(BTreeSet::new()),
            installs: Mutex::new(Vec::new()),
        });
        let executor = Arc::new(FakeExecutor {
            commands: Mutex::new(Vec::new()),
        });
        let resolver = Arc::new(DependencyResolver::new(
            capabilities(),
            packages.clone(),
            None,
        ));
        let terminal = TermuxTerminalTool::new(executor.clone(), resolver, "/workspace");
        let job = TermuxJobTool::new(terminal, 16);
        let result = job
            .execute(
                &ToolContext {
                    principal: "owner".into(),
                    session_id: "session".into(),
                    agent_run_id: "run".into(),
                    yolo_mode: false,
                    messages: Vec::new(),
                    cancellation: CancellationToken::new(),
                    progress: None,
                },
                json!({
                    "steps": [{
                        "id": "step-root",
                        "program": "su",
                        "args": ["-c", "id"]
                    }]
                }),
            )
            .await
            .unwrap();
        assert!(result.contains(r#""status":"denied"#));
    }

    #[tokio::test]
    async fn termux_job_enforces_max_steps() {
        let packages = Arc::new(FakePackages {
            available: Mutex::new(BTreeSet::new()),
            installs: Mutex::new(Vec::new()),
        });
        let executor = Arc::new(FakeExecutor {
            commands: Mutex::new(Vec::new()),
        });
        let resolver = Arc::new(DependencyResolver::new(
            capabilities(),
            packages.clone(),
            None,
        ));
        let terminal = TermuxTerminalTool::new(executor.clone(), resolver, "/workspace");
        let job = TermuxJobTool::new(terminal, 2);
        let steps = (0..5)
            .map(|i| json!({ "id": format!("step-{i}"), "program": "echo", "args": ["hi"] }))
            .collect::<Vec<_>>();
        let result = job
            .execute(
                &ToolContext {
                    principal: "owner".into(),
                    session_id: "session".into(),
                    agent_run_id: "run".into(),
                    yolo_mode: false,
                    messages: Vec::new(),
                    cancellation: CancellationToken::new(),
                    progress: None,
                },
                json!({ "steps": steps }),
            )
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("requires 1..=2 steps"));
    }
}
