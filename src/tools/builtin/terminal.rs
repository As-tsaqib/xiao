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
        let outcome = self
            .executor
            .execute(
                TermuxCommand {
                    program: arguments.program,
                    args: arguments.args,
                    cwd: arguments.cwd.unwrap_or_else(|| self.default_cwd.clone()),
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
            CapabilityRegistry, CommandOutcome, ExecutionBackend, PackageBackend,
            RuntimeEnvironment, SelinuxState, TermuxEnvironment,
        },
        storage::MessageRecord,
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
}
