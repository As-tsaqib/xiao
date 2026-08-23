use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    config::AppConfig,
    security::{redact::redact_text, secrets::SecretStore},
};

const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(15);
const DAEMON_STOP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliPaths {
    pub config: PathBuf,
    pub client_config: PathBuf,
    pub default_data_dir: PathBuf,
}

impl CliPaths {
    pub fn from_env() -> Result<Self> {
        let home = nonempty_env("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("HOME is not set; set HOME or the explicit XIAO_* paths"))?;
        let cwd = env::current_dir().context("resolve current directory")?;
        Ok(resolve_cli_paths(
            &home,
            nonempty_env("XDG_CONFIG_HOME").as_deref(),
            nonempty_env("XDG_DATA_HOME").as_deref(),
            nonempty_env("XIAO_CONFIG").as_deref(),
            nonempty_env("XIAO_CLIENT_CONFIG").as_deref(),
            nonempty_env("XIAO_HOME").as_deref(),
            &cwd,
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeLayout {
    pub data_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub secrets_dir: PathBuf,
    pub database: PathBuf,
    pub daemon_log: PathBuf,
    pub managed_state: PathBuf,
    lifecycle_lock: PathBuf,
}

impl RuntimeLayout {
    pub fn from_config(paths: &CliPaths, config: &AppConfig) -> Self {
        let base = paths
            .config
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let data_dir = resolve_from(&base, &config.paths.data_dir);
        let logs_dir = resolve_from(&base, &config.paths.logs_dir);
        let secrets_dir = resolve_from(&base, &config.paths.secrets_dir);
        let database = resolve_from(&base, &config.storage.database);
        Self {
            daemon_log: logs_dir.join("daemon.log"),
            managed_state: data_dir.join("xiaod-managed.toml"),
            lifecycle_lock: data_dir.join("xiaod-lifecycle.lock"),
            data_dir,
            logs_dir,
            secrets_dir,
            database,
        }
    }
}

#[derive(Clone)]
pub struct ClientConfig {
    pub endpoint: String,
    pub token: String,
    pub principal: String,
}

#[derive(Serialize, Deserialize)]
struct ClientConfigFile {
    endpoint: String,
    token: String,
    #[serde(default = "default_principal")]
    principal: String,
}

impl ClientConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read {} (run `xiao quickstart` first)", path.display()))?;
        let file: ClientConfigFile =
            toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
        let config = Self {
            endpoint: file.endpoint,
            token: file.token,
            principal: file.principal,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        let url = url::Url::parse(&self.endpoint).context("invalid client endpoint")?;
        if url.scheme() != "http" {
            bail!("client endpoint must use http over loopback");
        }
        if !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1")) {
            bail!("client refuses non-loopback endpoints");
        }
        if self.token.trim().is_empty() {
            bail!("client token is empty");
        }
        if self.principal.trim().is_empty() {
            bail!("client principal is empty");
        }
        Ok(())
    }

    fn to_toml(&self) -> Result<String> {
        self.validate()?;
        Ok(toml::to_string_pretty(&ClientConfigFile {
            endpoint: self.endpoint.clone(),
            token: self.token.clone(),
            principal: self.principal.clone(),
        })?)
    }
}

#[derive(Debug)]
pub struct InitResult {
    pub config: AppConfig,
    pub runtime: RuntimeLayout,
    pub config_created: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartResult {
    pub pid: Option<u32>,
    pub already_running: bool,
    pub client_config_created: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopResult {
    Stopped { pid: u32, forced: bool },
    NotRunning,
    UnmanagedRunning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonStatus {
    pub managed_pid: Option<u32>,
    pub reachable: bool,
    pub endpoint: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManagedProcess {
    pid: u32,
    executable: PathBuf,
    config: PathBuf,
}

pub fn initialize(paths: &CliPaths) -> Result<InitResult> {
    ensure_parent(&paths.config)?;
    ensure_parent(&paths.client_config)?;

    let (config, config_created) = if paths.config.exists() {
        (AppConfig::load(&paths.config)?, false)
    } else {
        ensure_private_dir(&paths.default_data_dir, false)?;
        let config = AppConfig::standalone(paths.default_data_dir.clone());
        config.validate()?;
        let raw = toml::to_string_pretty(&config)?;
        let created = write_new_private(&paths.config, raw.as_bytes())?;
        if created {
            (config, true)
        } else {
            (AppConfig::load(&paths.config)?, false)
        }
    };
    config.validate()?;
    set_private_file(&paths.config)?;

    let runtime = RuntimeLayout::from_config(paths, &config);
    ensure_private_dir(&runtime.data_dir, false)?;
    ensure_private_dir(&runtime.logs_dir, false)?;
    ensure_private_dir(&runtime.secrets_dir, true)?;
    if let Some(parent) = runtime.database.parent() {
        ensure_private_dir(parent, false)?;
    }
    if paths.client_config.exists() {
        ClientConfig::load(&paths.client_config)?;
        set_private_file(&paths.client_config)?;
    }

    Ok(InitResult {
        config,
        runtime,
        config_created,
    })
}

pub fn load_existing(paths: &CliPaths) -> Result<InitResult> {
    if !paths.config.is_file() {
        bail!(
            "xiao config is missing at {}; run `xiao quickstart` first",
            paths.config.display()
        );
    }
    initialize(paths)
}

pub fn provision_client_config(
    paths: &CliPaths,
    config: &AppConfig,
    runtime: &RuntimeLayout,
) -> Result<bool> {
    if paths.client_config.exists() {
        ClientConfig::load(&paths.client_config)?;
        set_private_file(&paths.client_config)?;
        return Ok(false);
    }
    let token = SecretStore::new(runtime.secrets_dir.clone())
        .get("ipc-client-token")?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("daemon has not provisioned the client credential yet"))?;
    let client = ClientConfig {
        endpoint: format!("http://{}", config.ipc.bind),
        token,
        principal: default_principal(),
    };
    ensure_parent(&paths.client_config)?;
    write_new_private(&paths.client_config, client.to_toml()?.as_bytes())
}

pub async fn start_daemon(paths: &CliPaths, init: &InitResult) -> Result<StartResult> {
    let _lock = LifecycleLock::acquire(&init.runtime)?;
    if let Some(pid) = valid_managed_pid(paths, &init.runtime)? {
        if probe_daemon(&init.config, &init.runtime).await {
            let client_config_created =
                provision_client_config(paths, &init.config, &init.runtime)?;
            return Ok(StartResult {
                pid: Some(pid),
                already_running: true,
                client_config_created,
            });
        }
        bail!("managed xiaod PID {pid} exists but its IPC endpoint is not ready");
    }
    if probe_daemon(&init.config, &init.runtime).await {
        let client_config_created = provision_client_config(paths, &init.config, &init.runtime)?;
        return Ok(StartResult {
            pid: None,
            already_running: true,
            client_config_created,
        });
    }

    let executable = find_daemon()?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&init.runtime.daemon_log)
        .with_context(|| format!("open {}", init.runtime.daemon_log.display()))?;
    set_private_file(&init.runtime.daemon_log)?;
    let stderr = log.try_clone()?;
    let working_dir = paths
        .config
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| init.runtime.data_dir.clone());
    let mut command = Command::new(&executable);
    command
        .env("XIAO_CONFIG", &paths.config)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));
    configure_detached(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("start {}", executable.display()))?;
    let record = ManagedProcess {
        pid: child.id(),
        executable,
        config: paths.config.clone(),
    };
    write_managed_process(&init.runtime, &record)?;

    if let Err(error) = wait_until_ready(&mut child, &init.config, &init.runtime).await {
        let _ = terminate_record(&record, libc::SIGTERM);
        if !wait_for_exit(&record, Duration::from_secs(2)).await {
            let _ = terminate_record(&record, libc::SIGKILL);
            let _ = wait_for_exit(&record, Duration::from_secs(1)).await;
        }
        let _ = remove_managed_process(&init.runtime, Some(record.pid));
        return Err(error);
    }
    let client_config_created = provision_client_config(paths, &init.config, &init.runtime)?;
    Ok(StartResult {
        pid: Some(record.pid),
        already_running: false,
        client_config_created,
    })
}

pub async fn stop_daemon(paths: &CliPaths, init: &InitResult) -> Result<StopResult> {
    let _lock = LifecycleLock::acquire(&init.runtime)?;
    let Some(record) = read_managed_process(&init.runtime)? else {
        return if probe_daemon(&init.config, &init.runtime).await {
            Ok(StopResult::UnmanagedRunning)
        } else {
            Ok(StopResult::NotRunning)
        };
    };
    if !record_belongs_to(&record, paths) || !process_matches(&record) {
        remove_managed_process(&init.runtime, Some(record.pid))?;
        return if probe_daemon(&init.config, &init.runtime).await {
            Ok(StopResult::UnmanagedRunning)
        } else {
            Ok(StopResult::NotRunning)
        };
    }

    terminate_record(&record, libc::SIGTERM)?;
    let stopped = wait_for_exit(&record, DAEMON_STOP_TIMEOUT).await;
    let forced = if stopped {
        false
    } else {
        terminate_record(&record, libc::SIGKILL)?;
        if !wait_for_exit(&record, Duration::from_secs(2)).await {
            bail!("xiaod PID {} did not exit after SIGKILL", record.pid);
        }
        true
    };
    remove_managed_process(&init.runtime, Some(record.pid))?;
    Ok(StopResult::Stopped {
        pid: record.pid,
        forced,
    })
}

pub async fn daemon_status(paths: &CliPaths, init: &InitResult) -> Result<DaemonStatus> {
    let managed_pid = valid_managed_pid(paths, &init.runtime)?;
    let reachable = probe_daemon(&init.config, &init.runtime).await;
    Ok(DaemonStatus {
        managed_pid,
        reachable,
        endpoint: format!("http://{}", init.config.ipc.bind),
    })
}

pub fn run_daemon_foreground(paths: &CliPaths, init: &InitResult) -> Result<ExitStatus> {
    let _lock = LifecycleLock::acquire(&init.runtime)?;
    if valid_managed_pid(paths, &init.runtime)?.is_some() {
        bail!("xiaod is already running for this config");
    }
    let executable = find_daemon()?;
    let mut child = Command::new(&executable)
        .env("XIAO_CONFIG", &paths.config)
        .current_dir(
            paths
                .config
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| init.runtime.data_dir.clone()),
        )
        .spawn()
        .with_context(|| format!("start {}", executable.display()))?;
    let record = ManagedProcess {
        pid: child.id(),
        executable,
        config: paths.config.clone(),
    };
    write_managed_process(&init.runtime, &record)?;
    drop(_lock);
    let status = child.wait()?;
    remove_managed_process(&init.runtime, Some(record.pid))?;
    Ok(status)
}

pub fn tail_daemon_log(runtime: &RuntimeLayout, lines: usize) -> Result<Vec<String>> {
    if !runtime.daemon_log.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&runtime.daemon_log)
        .with_context(|| format!("read {}", runtime.daemon_log.display()))?;
    let limit = lines.clamp(1, 500);
    Ok(content
        .lines()
        .rev()
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(redact_text)
        .collect())
}

pub fn find_daemon() -> Result<PathBuf> {
    let current = env::current_exe().context("locate the xiao CLI executable")?;
    discover_daemon(
        nonempty_env("XIAOD_BIN").as_deref(),
        &current,
        env::var_os("PATH").as_deref(),
    )
    .ok_or_else(|| {
        anyhow!("cannot find xiaod; install it next to xiao, put it on PATH, or set XIAOD_BIN")
    })
}

async fn wait_until_ready(
    child: &mut Child,
    config: &AppConfig,
    runtime: &RuntimeLayout,
) -> Result<()> {
    let deadline = Instant::now() + DAEMON_START_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            bail!(
                "xiaod exited during startup with {status}; inspect {}",
                runtime.daemon_log.display()
            );
        }
        if probe_daemon(config, runtime).await {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "xiaod did not become ready within {} seconds; inspect {}",
                DAEMON_START_TIMEOUT.as_secs(),
                runtime.daemon_log.display()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn probe_daemon(config: &AppConfig, runtime: &RuntimeLayout) -> bool {
    let Ok(Some(token)) = SecretStore::new(runtime.secrets_dir.clone()).get("ipc-client-token")
    else {
        return false;
    };
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_millis(750))
        .build()
    else {
        return false;
    };
    client
        .get(format!("http://{}/v1/status", config.ipc.bind))
        .bearer_auth(token)
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

fn valid_managed_pid(paths: &CliPaths, runtime: &RuntimeLayout) -> Result<Option<u32>> {
    let Some(record) = read_managed_process(runtime)? else {
        return Ok(None);
    };
    if record_belongs_to(&record, paths) && process_matches(&record) {
        return Ok(Some(record.pid));
    }
    remove_managed_process(runtime, Some(record.pid))?;
    Ok(None)
}

fn record_belongs_to(record: &ManagedProcess, paths: &CliPaths) -> bool {
    normalized(&record.config) == normalized(&paths.config)
}

fn process_matches(record: &ManagedProcess) -> bool {
    if !process_alive(record.pid) {
        return false;
    }
    let proc_exe = fs::read_link(format!("/proc/{}/exe", record.pid));
    match proc_exe {
        Ok(actual) => {
            normalized(&without_deleted_suffix(&actual)) == normalized(&record.executable)
        }
        Err(_) => false,
    }
}

fn without_deleted_suffix(path: &Path) -> PathBuf {
    let rendered = path.to_string_lossy();
    rendered
        .strip_suffix(" (deleted)")
        .map(PathBuf::from)
        .unwrap_or_else(|| path.to_path_buf())
}

fn read_managed_process(runtime: &RuntimeLayout) -> Result<Option<ManagedProcess>> {
    if !runtime.managed_state.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&runtime.managed_state)
        .with_context(|| format!("read {}", runtime.managed_state.display()))?;
    toml::from_str(&raw)
        .with_context(|| format!("parse {}", runtime.managed_state.display()))
        .map(Some)
}

fn write_managed_process(runtime: &RuntimeLayout, record: &ManagedProcess) -> Result<()> {
    let raw = toml::to_string(record)?;
    let tmp = runtime.managed_state.with_extension("toml.tmp");
    fs::write(&tmp, raw)?;
    set_private_file(&tmp)?;
    fs::rename(&tmp, &runtime.managed_state)?;
    set_private_file(&runtime.managed_state)
}

fn remove_managed_process(runtime: &RuntimeLayout, expected_pid: Option<u32>) -> Result<()> {
    if !runtime.managed_state.exists() {
        return Ok(());
    }
    if let Some(expected) = expected_pid {
        if read_managed_process(runtime)?.is_some_and(|record| record.pid != expected) {
            return Ok(());
        }
    }
    fs::remove_file(&runtime.managed_state)
        .with_context(|| format!("remove {}", runtime.managed_state.display()))
}

fn terminate_record(record: &ManagedProcess, signal: i32) -> Result<()> {
    if !process_matches(record) {
        return Ok(());
    }
    #[cfg(unix)]
    {
        // The PID is loaded from our private managed-state file and its /proc
        // executable identity is checked immediately before signaling.
        let result = unsafe { libc::kill(record.pid as i32, signal) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        Err(error).with_context(|| format!("signal xiaod PID {}", record.pid))
    }
    #[cfg(not(unix))]
    {
        let _ = signal;
        bail!("xiao daemon lifecycle is supported only on Unix-like systems")
    }
}

async fn wait_for_exit(record: &ManagedProcess, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_alive(record.pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    !process_alive(record.pid)
}

fn process_alive(pid: u32) -> bool {
    if fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(") ")
                .map(|(_, tail)| tail.starts_with('Z'))
        })
        .unwrap_or(false)
    {
        return false;
    }
    #[cfg(unix)]
    {
        // Signal zero performs a liveness/permission check without changing
        // the target process.
        let result = unsafe { libc::kill(pid as i32, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(unix)]
fn configure_detached(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: this closure runs after fork and before exec. `setsid` has no
    // Rust-managed state dependencies and is async-signal-safe on Android/Linux.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

#[cfg(not(unix))]
fn configure_detached(_command: &mut Command) {}

struct LifecycleLock {
    path: PathBuf,
}

impl LifecycleLock {
    fn acquire(runtime: &RuntimeLayout) -> Result<Self> {
        for _ in 0..2 {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&runtime.lifecycle_lock)
            {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    file.sync_all()?;
                    set_private_file(&runtime.lifecycle_lock)?;
                    return Ok(Self {
                        path: runtime.lifecycle_lock.clone(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let owner = fs::read_to_string(&runtime.lifecycle_lock)
                        .ok()
                        .and_then(|raw| raw.trim().parse::<u32>().ok());
                    if owner.is_some_and(process_alive) {
                        bail!("another xiao daemon lifecycle command is still running");
                    }
                    fs::remove_file(&runtime.lifecycle_lock).with_context(|| {
                        format!("remove stale {}", runtime.lifecycle_lock.display())
                    })?;
                }
                Err(error) => return Err(error).context("create daemon lifecycle lock"),
            }
        }
        bail!("could not acquire daemon lifecycle lock")
    }
}

impl Drop for LifecycleLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn resolve_cli_paths(
    home: &Path,
    xdg_config_home: Option<&OsStr>,
    xdg_data_home: Option<&OsStr>,
    config_override: Option<&OsStr>,
    client_override: Option<&OsStr>,
    data_override: Option<&OsStr>,
    cwd: &Path,
) -> CliPaths {
    let config_home = xdg_config_home
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let data_home = xdg_data_home
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    CliPaths {
        config: absolute_from(
            cwd,
            &config_override
                .map(PathBuf::from)
                .unwrap_or_else(|| config_home.join("xiao/config.toml")),
        ),
        client_config: absolute_from(
            cwd,
            &client_override
                .map(PathBuf::from)
                .unwrap_or_else(|| config_home.join("xiao/client.toml")),
        ),
        default_data_dir: absolute_from(
            cwd,
            &data_override
                .map(PathBuf::from)
                .unwrap_or_else(|| data_home.join("xiao")),
        ),
    }
}

fn discover_daemon(
    override_path: Option<&OsStr>,
    current_exe: &Path,
    path: Option<&OsStr>,
) -> Option<PathBuf> {
    if let Some(value) = override_path {
        let candidate = PathBuf::from(value);
        if is_executable(&candidate) {
            return Some(normalized(&candidate));
        }
        return None;
    }
    if let Some(parent) = current_exe.parent() {
        let sibling = parent.join("xiaod");
        if is_executable(&sibling) {
            return Some(normalized(&sibling));
        }
    }
    path.and_then(|value| {
        env::split_paths(value)
            .map(|part| part.join("xiaod"))
            .find(|candidate| is_executable(candidate))
            .map(|candidate| normalized(&candidate))
    })
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn nonempty_env(name: &str) -> Option<OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}

fn default_principal() -> String {
    "termux:default".into()
}

fn absolute_from(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn resolve_from(base: &Path, path: &Path) -> PathBuf {
    absolute_from(base, path)
}

fn normalized(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent, false)?;
    }
    Ok(())
}

fn ensure_private_dir(path: &Path, tighten_existing: bool) -> Result<()> {
    let existed = path.exists();
    fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
    if !existed || tighten_existing {
        set_private_dir(path)?;
    }
    Ok(())
}

fn write_new_private(path: &Path, bytes: &[u8]) -> Result<bool> {
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("create {}", path.display())),
    };
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error).with_context(|| format!("write {}", path.display()));
    }
    set_private_file(path)?;
    Ok(true)
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_dir(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_resolution_uses_xdg_and_explicit_overrides() {
        let paths = resolve_cli_paths(
            Path::new("/home/test"),
            Some(OsStr::new("/cfg")),
            Some(OsStr::new("/state")),
            None,
            None,
            None,
            Path::new("/work"),
        );
        assert_eq!(paths.config, PathBuf::from("/cfg/xiao/config.toml"));
        assert_eq!(paths.client_config, PathBuf::from("/cfg/xiao/client.toml"));
        assert_eq!(paths.default_data_dir, PathBuf::from("/state/xiao"));

        let overridden = resolve_cli_paths(
            Path::new("/home/test"),
            None,
            None,
            Some(OsStr::new("relative/config.toml")),
            Some(OsStr::new("relative/client.toml")),
            Some(OsStr::new("relative/data")),
            Path::new("/work"),
        );
        assert_eq!(
            overridden.config,
            PathBuf::from("/work/relative/config.toml")
        );
        assert_eq!(
            overridden.client_config,
            PathBuf::from("/work/relative/client.toml")
        );
        assert_eq!(
            overridden.default_data_dir,
            PathBuf::from("/work/relative/data")
        );
    }

    #[test]
    fn quickstart_initialization_is_idempotent_and_does_not_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CliPaths {
            config: temp.path().join("config/config.toml"),
            client_config: temp.path().join("config/client.toml"),
            default_data_dir: temp.path().join("data"),
        };
        let first = initialize(&paths).unwrap();
        assert!(first.config_created);
        let original = fs::read(&paths.config).unwrap();
        let second = initialize(&paths).unwrap();
        assert!(!second.config_created);
        assert_eq!(fs::read(&paths.config).unwrap(), original);
        assert_eq!(first.runtime, second.runtime);
    }

    #[cfg(unix)]
    #[test]
    fn generated_config_and_client_credentials_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let paths = CliPaths {
            config: temp.path().join("config/config.toml"),
            client_config: temp.path().join("config/client.toml"),
            default_data_dir: temp.path().join("data"),
        };
        let init = initialize(&paths).unwrap();
        SecretStore::new(init.runtime.secrets_dir.clone())
            .put("ipc-client-token", "private-token")
            .unwrap();
        assert!(provision_client_config(&paths, &init.config, &init.runtime).unwrap());
        assert_eq!(
            fs::metadata(&paths.config).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&paths.client_config)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&init.runtime.secrets_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn existing_client_config_is_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CliPaths {
            config: temp.path().join("config/config.toml"),
            client_config: temp.path().join("config/client.toml"),
            default_data_dir: temp.path().join("data"),
        };
        let init = initialize(&paths).unwrap();
        let existing = ClientConfig {
            endpoint: "http://127.0.0.1:49999".into(),
            token: "existing-token".into(),
            principal: "termux:kept".into(),
        }
        .to_toml()
        .unwrap();
        fs::write(&paths.client_config, &existing).unwrap();
        SecretStore::new(init.runtime.secrets_dir.clone())
            .put("ipc-client-token", "new-token")
            .unwrap();
        assert!(!provision_client_config(&paths, &init.config, &init.runtime).unwrap());
        assert_eq!(fs::read_to_string(&paths.client_config).unwrap(), existing);
    }

    #[test]
    fn client_config_accepts_only_http_loopback_with_nonempty_identity() {
        let valid = ClientConfig {
            endpoint: "http://127.0.0.1:37921".into(),
            token: "token".into(),
            principal: "termux:test".into(),
        };
        assert!(valid.validate().is_ok());

        let mut invalid = valid.clone();
        invalid.endpoint = "https://127.0.0.1:37921".into();
        assert!(invalid.validate().is_err());
        invalid.endpoint = "http://192.0.2.1:37921".into();
        assert!(invalid.validate().is_err());
        invalid.endpoint = "http://127.0.0.1:37921".into();
        invalid.token.clear();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn stale_or_identity_mismatched_pid_state_is_removed_without_signaling() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CliPaths {
            config: temp.path().join("config/config.toml"),
            client_config: temp.path().join("config/client.toml"),
            default_data_dir: temp.path().join("data"),
        };
        let init = initialize(&paths).unwrap();
        let record = ManagedProcess {
            pid: std::process::id(),
            executable: temp.path().join("definitely-not-this-test"),
            config: paths.config.clone(),
        };
        write_managed_process(&init.runtime, &record).unwrap();
        assert_eq!(valid_managed_pid(&paths, &init.runtime).unwrap(), None);
        assert!(!init.runtime.managed_state.exists());
    }

    #[cfg(unix)]
    #[test]
    fn daemon_discovery_prefers_explicit_then_sibling_then_path() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let explicit = temp.path().join("explicit-xiaod");
        let sibling_dir = temp.path().join("sibling");
        let path_dir = temp.path().join("path");
        fs::create_dir_all(&sibling_dir).unwrap();
        fs::create_dir_all(&path_dir).unwrap();
        for candidate in [
            explicit.clone(),
            sibling_dir.join("xiaod"),
            path_dir.join("xiaod"),
        ] {
            fs::write(&candidate, b"binary").unwrap();
            fs::set_permissions(&candidate, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let current = sibling_dir.join("xiao");
        let path = env::join_paths([path_dir.clone()]).unwrap();
        assert_eq!(
            discover_daemon(Some(explicit.as_os_str()), &current, Some(&path)),
            Some(normalized(&explicit))
        );
        assert_eq!(
            discover_daemon(None, &current, Some(&path)),
            Some(normalized(&sibling_dir.join("xiaod")))
        );
        fs::remove_file(sibling_dir.join("xiaod")).unwrap();
        assert_eq!(
            discover_daemon(None, &current, Some(&path)),
            Some(normalized(&path_dir.join("xiaod")))
        );
    }
}
