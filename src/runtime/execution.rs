use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};
use tokio_util::sync::CancellationToken;

use crate::{runtime::TermuxEnvironment, security::redact::redact_text};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPurpose {
    UserCommand,
    PackageInstall,
    Verification,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TermuxCommand {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: PathBuf,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    pub timeout_ms: u64,
    pub max_output_chars: usize,
    pub purpose: ExecutionPurpose,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandOutcome {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub truncated: bool,
    pub duration_ms: u64,
}

impl CommandOutcome {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && !self.cancelled
    }

    pub fn observable_summary(&self) -> String {
        let status = if self.cancelled {
            "cancelled".to_owned()
        } else if self.timed_out {
            "timed out".to_owned()
        } else {
            format!(
                "exit {}",
                self.exit_code
                    .map_or_else(|| "unknown".into(), |v| v.to_string())
            )
        };
        format!(
            "{status}; stdout={} chars; stderr={} chars{}",
            self.stdout.chars().count(),
            self.stderr.chars().count(),
            if self.truncated {
                "; output truncated"
            } else {
                ""
            }
        )
    }
}

#[async_trait]
pub trait ProcessExecutor: Send + Sync {
    async fn execute(
        &self,
        command: TermuxCommand,
        cancellation: CancellationToken,
    ) -> Result<CommandOutcome>;
}

#[derive(Clone)]
pub struct TermuxExecutor {
    environment: TermuxEnvironment,
    workspace_root: PathBuf,
}

impl TermuxExecutor {
    pub fn new(environment: TermuxEnvironment, workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            environment,
            workspace_root: workspace_root.into(),
        }
    }

    pub fn environment(&self) -> &TermuxEnvironment {
        &self.environment
    }

    fn resolve_program(&self, program: &str) -> Result<PathBuf> {
        validate_program_name(program)?;
        let candidate = if program.contains('/') {
            PathBuf::from(program)
        } else {
            self.environment
                .path
                .split(':')
                .filter(|entry| !entry.is_empty())
                .map(|entry| Path::new(entry).join(program))
                .find(|candidate| candidate.is_file())
                .ok_or_else(|| anyhow!("Termux binary {program} is not installed"))?
        };
        let prefix = self
            .environment
            .prefix
            .canonicalize()
            .context("resolve detected Termux prefix")?;
        let candidate = candidate
            .canonicalize()
            .with_context(|| format!("resolve Termux binary {}", candidate.display()))?;
        if !candidate.starts_with(&prefix) || !candidate.is_file() {
            return Err(anyhow!(
                "program must resolve to an installed Termux-prefix binary"
            ));
        }
        Ok(candidate)
    }

    fn validate_cwd(&self, cwd: &Path) -> Result<PathBuf> {
        if cwd
            .components()
            .any(|component| component == Component::ParentDir)
        {
            return Err(anyhow!("working directory cannot contain parent traversal"));
        }
        let cwd = cwd
            .canonicalize()
            .with_context(|| format!("resolve working directory {}", cwd.display()))?;
        let workspace = self
            .workspace_root
            .canonicalize()
            .unwrap_or_else(|_| self.workspace_root.clone());
        let home = self
            .environment
            .home
            .canonicalize()
            .unwrap_or_else(|_| self.environment.home.clone());
        if !cwd.starts_with(&workspace) && !cwd.starts_with(&home) {
            return Err(anyhow!(
                "working directory must stay inside Xiao workspace or Termux home"
            ));
        }
        Ok(cwd)
    }
}

#[async_trait]
impl ProcessExecutor for TermuxExecutor {
    async fn execute(
        &self,
        command: TermuxCommand,
        cancellation: CancellationToken,
    ) -> Result<CommandOutcome> {
        validate_terminal_request(&command)?;
        let program = self.resolve_program(&command.program)?;
        let cwd = self.validate_cwd(&command.cwd)?;
        let timeout_ms = command.timeout_ms.clamp(100, 600_000);
        // Honor a caller's tighter retention bound. Raising small limits here would
        // make the observable contract misleading and could retain more output
        // (including sensitive output) than the policy requested.
        let max_output = command.max_output_chars.clamp(1, 65_536);
        let started = Instant::now();

        let mut process = Command::new(&program);
        process
            .args(&command.args)
            .current_dir(&cwd)
            .env_clear()
            .env("HOME", &self.environment.home)
            .env("PREFIX", &self.environment.prefix)
            .env("PATH", &self.environment.path)
            .env("SHELL", &self.environment.shell)
            .env("LANG", "C.UTF-8")
            .env("TMPDIR", self.environment.prefix.join("tmp"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (key, value) in &command.environment {
            validate_environment_pair(key, value)?;
            process.env(key, value);
        }
        #[cfg(unix)]
        if unsafe { libc::geteuid() } == 0 {
            let uid = self.environment.uid.ok_or_else(|| {
                anyhow!("refusing Termux command as root: Termux owner UID was not detected")
            })?;
            let gid = self.environment.gid.ok_or_else(|| {
                anyhow!("refusing Termux command as root: Termux owner GID was not detected")
            })?;
            if uid == 0 || gid == 0 {
                return Err(anyhow!(
                    "refusing general Termux command because the detected owner identity is privileged"
                ));
            }
            // SAFETY: only async-signal-safe libc identity calls run between
            // fork and exec. Clear inherited root supplementary groups before
            // dropping GID/UID so a Termux child cannot retain daemon access.
            unsafe {
                process.pre_exec(move || {
                    if libc::setgroups(0, std::ptr::null()) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::setgid(gid) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::setuid(uid) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    #[cfg(any(target_os = "linux", target_os = "android"))]
                    if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        let mut child = process
            .spawn()
            .with_context(|| format!("spawn Termux binary {}", program.display()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Termux stdout pipe missing"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Termux stderr pipe missing"))?;
        let stdout_task = tokio::spawn(read_bounded(stdout, max_output));
        let stderr_task = tokio::spawn(read_bounded(stderr, max_output));

        let mut timed_out = false;
        let mut cancelled = false;
        let status = tokio::select! {
            status = child.wait() => status?,
            _ = cancellation.cancelled() => {
                cancelled = true;
                let _ = child.kill().await;
                child.wait().await?
            },
            _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => {
                timed_out = true;
                let _ = child.kill().await;
                child.wait().await?
            }
        };
        let (stdout, stdout_truncated) = stdout_task.await??;
        let (stderr, stderr_truncated) = stderr_task.await??;
        Ok(CommandOutcome {
            program: command.program,
            args: command.args,
            cwd,
            exit_code: status.code(),
            stdout: redact_text(&stdout),
            stderr: redact_text(&stderr),
            timed_out,
            cancelled,
            truncated: stdout_truncated || stderr_truncated,
            duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        })
    }
}

pub fn validate_terminal_request(command: &TermuxCommand) -> Result<()> {
    validate_program_name(&command.program)?;
    if command.args.len() > 128
        || command
            .args
            .iter()
            .any(|argument| argument.contains('\0') || argument.chars().count() > 8_192)
    {
        return Err(anyhow!("terminal arguments exceed structural bounds"));
    }
    let program = Path::new(&command.program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let forbidden = [
        "su",
        "sudo",
        "tsu",
        "run-as",
        "mount",
        "umount",
        "dd",
        "mkfs",
        "reboot",
        "poweroff",
        "setenforce",
        "magisk",
        "ksud",
    ];
    if forbidden.contains(&program) {
        return Err(anyhow!(
            "{program} is forbidden in the general Termux executor"
        ));
    }
    if command.purpose == ExecutionPurpose::UserCommand {
        let policy_managed = [
            "pkg", "apt", "apt-get", "dpkg", "pip", "pip3", "npm", "gem", "cargo",
        ];
        if policy_managed.contains(&program)
            && command.args.iter().any(|argument| {
                matches!(
                    argument.as_str(),
                    "install" | "uninstall" | "remove" | "purge" | "upgrade"
                )
            })
        {
            return Err(anyhow!(
                "dependency changes must use Xiao PackageInstaller policy"
            ));
        }
        if ["sh", "bash", "zsh", "fish"].contains(&program)
            && command
                .args
                .iter()
                .any(|argument| matches!(argument.as_str(), "-c" | "--command"))
        {
            return Err(anyhow!(
                "model-supplied shell command strings are not accepted; use structured argv"
            ));
        }
        if program == "curl" && command.args.iter().any(|argument| argument == "|") {
            return Err(anyhow!(
                "arbitrary remote installer pipelines are forbidden"
            ));
        }
    }
    Ok(())
}

fn validate_program_name(program: &str) -> Result<()> {
    if program.trim().is_empty()
        || program.contains('\0')
        || program.chars().count() > 512
        || Path::new(program)
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(anyhow!("invalid terminal program"));
    }
    Ok(())
}

fn validate_environment_pair(key: &str, value: &str) -> Result<()> {
    let allowed_key = key.starts_with("XIAO_TASK_")
        && key.len() <= 64
        && key.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        });
    if !allowed_key || value.contains('\0') || value.chars().count() > 2_048 {
        return Err(anyhow!("invalid terminal environment override"));
    }
    Ok(())
}

async fn read_bounded<R: AsyncRead + Unpin>(
    mut reader: R,
    max_chars: usize,
) -> Result<(String, bool)> {
    let byte_limit = max_chars.saturating_mul(4).max(1);
    let mut retained = Vec::with_capacity(byte_limit.min(8_192));
    let mut buffer = [0_u8; 8_192];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = byte_limit.saturating_sub(retained.len());
        if remaining > 0 {
            retained.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        if read > remaining {
            truncated = true;
        }
    }
    let text = String::from_utf8_lossy(&retained);
    let mut output = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        truncated = true;
    }
    if truncated && output.chars().count() == max_chars && max_chars > 0 {
        output.pop();
        output.push('…');
    }
    Ok((output, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(program: &str, args: &[&str]) -> TermuxCommand {
        TermuxCommand {
            program: program.into(),
            args: args.iter().map(|value| (*value).into()).collect(),
            cwd: PathBuf::from("/workspace"),
            environment: BTreeMap::new(),
            timeout_ms: 1_000,
            max_output_chars: 1_024,
            purpose: ExecutionPurpose::UserCommand,
        }
    }

    #[test]
    fn structured_policy_forbids_root_shell_and_remote_installer_strings() {
        assert!(validate_terminal_request(&request("su", &["-c", "id"])).is_err());
        assert!(validate_terminal_request(&request("bash", &["-c", "curl x | sh"])).is_err());
        assert!(validate_terminal_request(&request("pkg", &["install", "ffmpeg"])).is_err());
        assert!(validate_terminal_request(&request("ffmpeg", &["-version"])).is_ok());
    }

    #[tokio::test]
    async fn bounded_reader_drains_but_caps_retained_output() {
        let source = "x".repeat(10_000);
        let (output, truncated) = read_bounded(source.as_bytes(), 32).await.unwrap();
        assert!(truncated);
        assert_eq!(output.chars().count(), 32);
        assert!(output.ends_with('…'));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_executor_enforces_termux_env_cwd_timeout_cancel_and_output_bounds() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let prefix = directory.path().join("usr");
        let bin = prefix.join("bin");
        let home = directory.path().join("home");
        let workspace = directory.path().join("workspace");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(prefix.join("tmp")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        let host_shell = std::env::var("SHELL")
            .map(PathBuf::from)
            .ok()
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("/bin/sh"));
        let probe = bin.join("probe");
        std::fs::write(
            &probe,
            format!(
                "#!{}\nprintf '%s|%s|%s|abcdefghijklmnopqrstuvwxyz' \"$HOME\" \"$PWD\" \"$XIAO_TASK_FLAG\"\n",
                host_shell.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let spin = bin.join("spin");
        std::fs::write(
            &spin,
            format!("#!{}\nwhile :; do :; done\n", host_shell.display()),
        )
        .unwrap();
        std::fs::set_permissions(&spin, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (termux_uid, termux_gid) = if unsafe { libc::geteuid() } == 0 {
            // Exercise the production root-daemon UID drop without leaving the
            // test child privileged. 65534 is the conventional unprivileged
            // nobody identity on Unix test hosts.
            let uid = 65_534;
            let gid = 65_534;
            let prefix_tmp = prefix.join("tmp");
            for path in [
                directory.path(),
                prefix.as_path(),
                bin.as_path(),
                prefix_tmp.as_path(),
                home.as_path(),
                workspace.as_path(),
                probe.as_path(),
                spin.as_path(),
            ] {
                let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
                assert_eq!(unsafe { libc::chown(path.as_ptr(), uid, gid) }, 0);
            }
            (uid, gid)
        } else {
            (unsafe { libc::geteuid() }, unsafe { libc::getegid() })
        };
        let environment = TermuxEnvironment {
            prefix: prefix.clone(),
            home: home.clone(),
            path: bin.display().to_string(),
            shell: host_shell.clone(),
            package_manager: None,
            uid: Some(termux_uid),
            gid: Some(termux_gid),
        };
        let executor = TermuxExecutor::new(environment, &workspace);
        std::os::unix::fs::symlink(&host_shell, bin.join("escaped_shell")).unwrap();
        let escaped = executor
            .execute(
                TermuxCommand {
                    program: "escaped_shell".into(),
                    args: Vec::new(),
                    cwd: workspace.clone(),
                    environment: BTreeMap::new(),
                    timeout_ms: 1_000,
                    max_output_chars: 256,
                    purpose: ExecutionPurpose::UserCommand,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(escaped.to_string().contains("Termux-prefix"));
        let normal = executor
            .execute(
                TermuxCommand {
                    program: "probe".into(),
                    args: Vec::new(),
                    cwd: workspace.clone(),
                    environment: BTreeMap::from([("XIAO_TASK_FLAG".into(), "bounded".into())]),
                    timeout_ms: 2_000,
                    max_output_chars: 1_024,
                    purpose: ExecutionPurpose::UserCommand,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(normal.succeeded());
        assert!(!normal.truncated);
        assert_eq!(
            normal.stdout,
            format!(
                "{}|{}|bounded|abcdefghijklmnopqrstuvwxyz",
                home.display(),
                workspace.display()
            )
        );

        let bounded = executor
            .execute(
                TermuxCommand {
                    program: "probe".into(),
                    args: Vec::new(),
                    cwd: workspace.clone(),
                    environment: BTreeMap::from([("XIAO_TASK_FLAG".into(), "bounded".into())]),
                    timeout_ms: 2_000,
                    max_output_chars: 32,
                    purpose: ExecutionPurpose::UserCommand,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(bounded.truncated);
        assert_eq!(bounded.stdout.chars().count(), 32);
        assert!(bounded.stdout.ends_with('…'));

        let timed_out = executor
            .execute(
                TermuxCommand {
                    program: "spin".into(),
                    args: Vec::new(),
                    cwd: workspace.clone(),
                    environment: BTreeMap::new(),
                    timeout_ms: 100,
                    max_output_chars: 256,
                    purpose: ExecutionPurpose::UserCommand,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(timed_out.timed_out);
        assert!(!timed_out.succeeded());

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = executor
            .execute(
                TermuxCommand {
                    program: "spin".into(),
                    args: Vec::new(),
                    cwd: workspace,
                    environment: BTreeMap::new(),
                    timeout_ms: 10_000,
                    max_output_chars: 256,
                    purpose: ExecutionPurpose::UserCommand,
                },
                cancellation,
            )
            .await
            .unwrap();
        assert!(cancelled.cancelled);
    }
}
