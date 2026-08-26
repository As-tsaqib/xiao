use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    runtime::{DependencyResolver, ExecutionPurpose, ProcessExecutor, TermuxCommand},
    tools::{
        policy::{is_sensitive_env_key, is_sensitive_path_or_value},
        Tool, ToolContext, ToolEffect, ToolOrigin, ToolRisk, ToolSpec,
    },
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
            &arguments.environment,
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
            dir
        };
        std::fs::create_dir_all(&session_workspace)?;
        let canonical_workspace = session_workspace.canonicalize()?;

        let effective_cwd = match &arguments.cwd {
            Some(custom) => {
                if custom.is_absolute() {
                    return Err(anyhow!(
                        "cwd must be a relative path within workspace; absolute paths are forbidden"
                    ));
                }
                let mut current = canonical_workspace.clone();
                for comp in custom.components() {
                    match comp {
                        std::path::Component::Normal(part) => {
                            current = current.join(part);
                            if current.is_symlink() {
                                return Err(anyhow!(
                                    "symlink cwd components are forbidden: {}",
                                    current.display()
                                ));
                            }
                        }
                        std::path::Component::CurDir => {}
                        _ => {
                            return Err(anyhow!("invalid cwd component"));
                        }
                    }
                }
                std::fs::create_dir_all(&current)?;
                let canonical_target = current.canonicalize()?;
                if !canonical_target.starts_with(&canonical_workspace) {
                    return Err(anyhow!(
                        "cwd escapes workspace: target is outside canonical session workspace"
                    ));
                }
                canonical_target
            }
            None => canonical_workspace.clone(),
        };

        let outcome = self
            .executor
            .execute(
                TermuxCommand {
                    program: arguments.program,
                    args: arguments.args,
                    cwd: effective_cwd.clone(),
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
                crate::tools::PolicyDecision::Deny(reason) => {
                    if let (Some(storage), Some(id)) = (&self.storage, audit_id.as_deref()) {
                        storage.finish_tool_run_step(id, "denied", None, Some(&reason))?;
                    }
                    results.push(json!({"index":index,"id":step.id,"status":"denied","error":reason}));
                    if !step.continue_on_error {
                        break;
                    }
                    continue;
                }
                crate::tools::PolicyDecision::RequireApproval(reason) => {
                    let msg = format!(
                        "unsupported inside termux_job; call termux_terminal separately for exact approval: {reason}"
                    );
                    if let (Some(storage), Some(id)) = (&self.storage, audit_id.as_deref()) {
                        storage.finish_tool_run_step(id, "approval_required", None, Some(&msg))?;
                    }
                    results.push(json!({
                        "index": index,
                        "id": step.id,
                        "status": "approval_required",
                        "error": msg,
                    }));
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
    environment: &BTreeMap<String, String>,
) -> Result<()> {
    let trimmed_prog = program.trim();
    if trimmed_prog.is_empty() {
        return Err(anyhow!("program must not be empty"));
    }
    if trimmed_prog.contains(' ')
        || trimmed_prog.contains('	')
        || trimmed_prog.contains('
')
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
        if cwd.is_absolute() {
            return Err(anyhow!(
                "cwd must be a relative path within workspace; absolute paths are forbidden"
            ));
        }
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

    if is_sensitive_path_or_value(trimmed_prog) {
        return Err(anyhow!(
            "sensitive program path is forbidden in unprivileged terminal execution: {trimmed_prog}"
        ));
    }
    if let Some(cwd) = cwd {
        let cwd_str = cwd.to_string_lossy();
        if is_sensitive_path_or_value(&cwd_str) {
            return Err(anyhow!(
                "sensitive cwd is forbidden in unprivileged terminal execution: {cwd_str}"
            ));
        }
    }
    for arg in args {
        if is_sensitive_path_or_value(arg) {
            return Err(anyhow!(
                "sensitive argv is forbidden in unprivileged terminal execution"
            ));
        }
    }
    for (k, v) in environment {
        if is_sensitive_env_key(k) || is_sensitive_path_or_value(v) {
            return Err(anyhow!(
                "sensitive environment key or value is forbidden in unprivileged terminal execution: {k}"
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
    let cwd = cwd
        .canonicalize()
        .unwrap_or_else(|_| cwd.to_path_buf());
    paths
        .iter()
        .map(|path| {
            if path.is_absolute()
                || path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(anyhow!(
                    "artifact path must be relative without parent traversal: {}",
                    path.display()
                ));
            }
            let candidate = cwd.join(path);
            if candidate.is_symlink() {
                return Err(anyhow!("artifact cannot be a symlink"));
            }
            let canonical = candidate.canonicalize().map_err(|_| {
                anyhow!(
                    "declared result artifact does not exist: {}",
                    path.display()
                )
            })?;
            if !canonical.starts_with(&cwd) && !canonical.starts_with(&workspace) {
                return Err(anyhow!(
                    "result artifact is outside the controlled task workspace"
                ));
            }
            let metadata = std::fs::symlink_metadata(&canonical)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 50 * 1024 * 1024 {
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
    use tempfile::tempdir;
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
            duration_ms: 1,
            truncated: false,
            timed_out: false,
            cancelled: false,
        }
    }

    fn capabilities() -> Arc<CapabilityRegistry> {
        Arc::new(CapabilityRegistry::from_environment(&RuntimeEnvironment {
            platform: "android".into(),
            os_version: Some("14".into()),
            android_version: Some("14".into()),
            device_model: Some("Pixel".into()),
            architecture: "aarch64".into(),
            xiao_version: "0.3.1".into(),
            effective_uid: 10234,
            root_available: false,
            root_evidence: "unrooted".into(),
            selinux: SelinuxState::Enforcing,
            data_root: PathBuf::from("/data/data/com.termux/files/home/.xiao"),
            workspace_writable: true,
            termux: Some(TermuxEnvironment {
                prefix: PathBuf::from("/data/data/com.termux/files/usr"),
                home: PathBuf::from("/data/data/com.termux/files/home"),
                app_data: PathBuf::from("/data/data/com.termux"),
                is_shared_uid: false,
            }),
            binaries: BTreeMap::new(),
            execution_backends: vec![ExecutionBackend::Termux],
            probed_at: "2026-08-26T00:00:00Z".into(),
        }))
    }

    #[tokio::test]
    async fn terminal_auto_resolves_and_installs_trusted_package() {
        let packages = Arc::new(FakePackages {
            available: Mutex::new(BTreeSet::new()),
            installs: Mutex::new(Vec::new()),
        });
        let executor = Arc::new(FakeExecutor {
            commands: Mutex::new(Vec::new()),
        });
        let resolver = Arc::new(DependencyResolver::with_trusted_repository(
            capabilities(),
            packages.clone(),
            None,
            Arc::new(FakeRepository),
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

        let result = terminal
            .execute(
                &context,
                json!({
                    "program": "ffmpeg",
                    "args": ["-i", "video.mp4", "audio.mp3"],
                }),
            )
            .await
            .unwrap();

        assert!(result.contains("ffmpeg"));
        assert_eq!(
            packages.installs.lock().unwrap().as_slice(),
            &["ffmpeg".to_string()]
        );
        assert_eq!(executor.commands.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn terminal_rejects_absolute_outside_symlink_cwd_and_sensitive_env_and_never_calls_executor() {
        let temp = tempdir().unwrap();
        let outside = tempdir().unwrap();
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
        let terminal = TermuxTerminalTool::new(executor.clone(), resolver, temp.path());
        let context = ToolContext {
            principal: "owner".into(),
            session_id: "sec-session".into(),
            agent_run_id: "sec-run".into(),
            yolo_mode: false,
            messages: Vec::new(),
            cancellation: CancellationToken::new(),
            progress: None,
        };

        // 1. Absolute cwd is rejected
        let err1 = terminal
            .execute(&context, json!({"program": "ls", "cwd": "/etc"}))
            .await
            .unwrap_err();
        assert!(err1.to_string().contains("relative path"));
        assert_eq!(executor.commands.lock().unwrap().len(), 0);

        // 2. Traversal cwd is rejected
        let err2 = terminal
            .execute(&context, json!({"program": "ls", "cwd": "../../outside"}))
            .await
            .unwrap_err();
        assert!(err2.to_string().contains("parent directory traversal"));
        assert_eq!(executor.commands.lock().unwrap().len(), 0);

        // 3. Symlink cwd is rejected
        let workspace = temp.path().join(".xiao/workspaces/sec-session");
        std::fs::create_dir_all(&workspace).unwrap();
        #[cfg(unix)]
        {
            let symlink_dir = workspace.join("escaped_symlink");
            std::os::unix::fs::symlink(outside.path(), &symlink_dir).unwrap();
            let err3 = terminal
                .execute(&context, json!({"program": "ls", "cwd": "escaped_symlink"}))
                .await
                .unwrap_err();
            assert!(err3.to_string().contains("symlink") || err3.to_string().contains("escapes workspace"));
            assert_eq!(executor.commands.lock().unwrap().len(), 0);
        }

        // 4. Sensitive cwd is rejected
        let err4 = terminal
            .execute(&context, json!({"program": "ls", "cwd": ".ssh"}))
            .await
            .unwrap_err();
        assert!(err4.to_string().contains("sensitive"));
        assert_eq!(executor.commands.lock().unwrap().len(), 0);

        // 5. Sensitive environment key is rejected
        let err5 = terminal
            .execute(
                &context,
                json!({"program": "ls", "environment": {"SSH_AUTH_SOCK": "/tmp/sock"}}),
            )
            .await
            .unwrap_err();
        assert!(err5.to_string().contains("sensitive"));
        assert_eq!(executor.commands.lock().unwrap().len(), 0);

        // 6. Sensitive environment value is rejected
        let err6 = terminal
            .execute(
                &context,
                json!({"program": "ls", "environment": {"CUSTOM_KEY": "/root/.ssh/id_rsa"}}),
            )
            .await
            .unwrap_err();
        assert!(err6.to_string().contains("sensitive"));
        assert_eq!(executor.commands.lock().unwrap().len(), 0);

        // 7. Sensitive argv is rejected
        let err7 = terminal
            .execute(
                &context,
                json!({"program": "cat", "args": ["/home/u/.ssh/id_rsa"]}),
            )
            .await
            .unwrap_err();
        assert!(err7.to_string().contains("sensitive"));
        assert_eq!(executor.commands.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn termux_job_rejects_approval_requiring_substeps_with_approval_required_status() {
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
                    "steps": [
                        {
                            "id": "step-rm",
                            "program": "rm",
                            "args": ["result.txt"]
                        }
                    ]
                }),
            )
            .await
            .unwrap();

        assert!(result.contains(r#""status":"approval_required""#));
        assert!(result.contains("unsupported inside termux_job; call termux_terminal separately for exact approval"));
        assert_eq!(executor.commands.lock().unwrap().len(), 0);
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
        assert!(err
            .to_string()
            .contains("shell command strings are forbidden"));

        // Rejects bash -c
        let err2 = terminal
            .execute(&context, json!({"program": "bash", "args": ["-c", "id"]}))
            .await
            .unwrap_err();
        assert!(err2
            .to_string()
            .contains("shell command strings are forbidden"));

        // Rejects artifact escape
        let err3 = terminal
            .execute(
                &context,
                json!({"program": "ls", "artifacts": ["../../etc/passwd"]}),
            )
            .await
            .unwrap_err();
        assert!(err3
            .to_string()
            .contains("relative paths within the workspace"));
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
