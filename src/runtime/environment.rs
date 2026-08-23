use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::{identity::IdentityWorkspace, runtime::CapabilityRegistry};

const PROBED_BINARIES: &[&str] = &[
    "bash", "cargo", "curl", "ffmpeg", "ffprobe", "file", "git", "jq", "node", "npm", "pkg",
    "python", "python3", "rg", "sh", "su", "tar", "unzip", "zip",
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelinuxState {
    Enforcing,
    Permissive,
    Disabled,
    Unknown,
}

impl SelinuxState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Enforcing => "enforcing",
            Self::Permissive => "permissive",
            Self::Disabled => "disabled",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackend {
    Termux,
    AndroidPrivileged,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TermuxEnvironment {
    pub prefix: PathBuf,
    pub home: PathBuf,
    pub path: String,
    pub shell: PathBuf,
    pub package_manager: Option<PathBuf>,
    /// Owner of the Termux home. A root xiaod process drops to these IDs for
    /// every general-purpose Termux command.
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeEnvironment {
    pub platform: String,
    pub os_version: Option<String>,
    pub android_version: Option<String>,
    pub device_model: Option<String>,
    pub architecture: String,
    pub xiao_version: String,
    pub effective_uid: u32,
    pub root_available: bool,
    pub root_evidence: String,
    pub selinux: SelinuxState,
    pub termux: Option<TermuxEnvironment>,
    pub data_root: PathBuf,
    pub workspace_writable: bool,
    pub binaries: BTreeMap<String, Option<PathBuf>>,
    pub execution_backends: Vec<ExecutionBackend>,
    pub probed_at: String,
}

pub trait HostProbe: Send + Sync {
    fn env(&self, key: &str) -> Option<String>;
    fn is_file(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    fn read(&self, path: &Path) -> Option<String>;
    fn effective_uid(&self) -> u32;
    fn architecture(&self) -> String;
    fn platform(&self) -> String;
    fn owner_ids(&self, path: &Path) -> Option<(u32, u32)>;
}

#[derive(Debug, Default)]
pub struct RealHostProbe;

impl HostProbe for RealHostProbe {
    fn env(&self, key: &str) -> Option<String> {
        env::var(key).ok()
    }
    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }
    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }
    fn read(&self, path: &Path) -> Option<String> {
        fs::read_to_string(path).ok()
    }
    fn effective_uid(&self) -> u32 {
        #[cfg(unix)]
        {
            // SAFETY: geteuid has no preconditions and does not mutate memory.
            unsafe { libc::geteuid() }
        }
        #[cfg(not(unix))]
        {
            0
        }
    }
    fn architecture(&self) -> String {
        env::consts::ARCH.to_owned()
    }
    fn platform(&self) -> String {
        env::consts::OS.to_owned()
    }
    fn owner_ids(&self, path: &Path) -> Option<(u32, u32)> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(path).ok()?;
            Some((metadata.uid(), metadata.gid()))
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            None
        }
    }
}

#[derive(Clone)]
pub struct EnvironmentProbe {
    host: Arc<dyn HostProbe>,
}

impl EnvironmentProbe {
    pub fn new(host: Arc<dyn HostProbe>) -> Self {
        Self { host }
    }

    pub fn real() -> Self {
        Self::new(Arc::new(RealHostProbe))
    }

    pub fn probe(&self, data_root: impl Into<PathBuf>) -> RuntimeEnvironment {
        let data_root = data_root.into();
        let build_properties = [
            Path::new("/system/build.prop"),
            Path::new("/product/build.prop"),
        ]
        .into_iter()
        .filter_map(|path| self.host.read(path))
        .collect::<Vec<_>>()
        .join("\n");
        let android_version = property(&build_properties, "ro.build.version.release")
            .or_else(|| self.host.env("ANDROID_VERSION"));
        let device_model = property(&build_properties, "ro.product.model");
        let platform = if android_version.is_some()
            || self.host.is_dir(Path::new("/system"))
            || self.host.env("ANDROID_ROOT").is_some()
        {
            "android".to_owned()
        } else {
            self.host.platform()
        };
        let uid = self.host.effective_uid();
        let inherited_path = self.host.env("PATH").unwrap_or_default();
        let termux = detect_termux(self.host.as_ref(), &inherited_path);
        let effective_path = termux
            .as_ref()
            .map(|termux| termux.path.as_str())
            .unwrap_or(&inherited_path);
        let binaries = PROBED_BINARIES
            .iter()
            .map(|binary| {
                (
                    (*binary).to_owned(),
                    find_binary(self.host.as_ref(), effective_path, binary),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let root_path = binaries.get("su").and_then(Clone::clone).or_else(|| {
            ["/system/bin/su", "/system/xbin/su", "/sbin/su"]
                .into_iter()
                .map(PathBuf::from)
                .find(|path| self.host.is_file(path))
        });
        let root_available = uid == 0 || root_path.is_some();
        let root_evidence = if uid == 0 {
            "effective UID is 0".into()
        } else if let Some(path) = root_path {
            format!("root entry point detected at {}", path.display())
        } else {
            "no root entry point detected".into()
        };
        let selinux = match self
            .host
            .read(Path::new("/sys/fs/selinux/enforce"))
            .as_deref()
            .map(str::trim)
        {
            Some("1") => SelinuxState::Enforcing,
            Some("0") => SelinuxState::Permissive,
            _ if !self.host.is_dir(Path::new("/sys/fs/selinux")) && platform == "android" => {
                SelinuxState::Disabled
            }
            _ => SelinuxState::Unknown,
        };
        let mut execution_backends = Vec::new();
        if termux.is_some() {
            execution_backends.push(ExecutionBackend::Termux);
        }
        // The typed broker intentionally never invokes a discovered `su`
        // binary with model-controlled data. It is executable only when the
        // daemon itself already has the required privilege.
        if uid == 0 {
            execution_backends.push(ExecutionBackend::AndroidPrivileged);
        }
        let workspace_writable = fs::metadata(&data_root)
            .map(|metadata| !metadata.permissions().readonly())
            .unwrap_or(false);
        RuntimeEnvironment {
            platform,
            os_version: android_version.clone().or_else(|| {
                self.host
                    .read(Path::new("/proc/version"))
                    .map(|value| compact(&value, 160))
            }),
            android_version,
            device_model,
            architecture: self.host.architecture(),
            xiao_version: crate::VERSION.into(),
            effective_uid: uid,
            root_available,
            root_evidence,
            selinux,
            termux,
            data_root,
            workspace_writable,
            binaries,
            execution_backends,
            probed_at: Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Clone)]
pub struct RuntimeState {
    probe: EnvironmentProbe,
    data_root: PathBuf,
    workspace: Arc<IdentityWorkspace>,
    environment: Arc<RwLock<RuntimeEnvironment>>,
    capabilities: Arc<CapabilityRegistry>,
}

impl RuntimeState {
    pub fn initialize(workspace: Arc<IdentityWorkspace>, probe: EnvironmentProbe) -> Result<Self> {
        workspace.bootstrap()?;
        let data_root = workspace.root().to_path_buf();
        let environment = probe.probe(&data_root);
        let capabilities = Arc::new(CapabilityRegistry::from_environment(&environment));
        let state = Self {
            probe,
            data_root,
            workspace,
            environment: Arc::new(RwLock::new(environment)),
            capabilities,
        };
        state.persist_snapshot()?;
        Ok(state)
    }

    pub fn refresh(&self) -> Result<RuntimeEnvironment> {
        let environment = self.probe.probe(&self.data_root);
        self.capabilities.refresh(&environment);
        *self.environment.write().expect("runtime state poisoned") = environment.clone();
        self.persist_snapshot()?;
        Ok(environment)
    }

    pub fn environment(&self) -> RuntimeEnvironment {
        self.environment
            .read()
            .expect("runtime state poisoned")
            .clone()
    }

    pub fn capabilities(&self) -> Arc<CapabilityRegistry> {
        self.capabilities.clone()
    }

    pub fn workspace(&self) -> Arc<IdentityWorkspace> {
        self.workspace.clone()
    }

    pub fn concise_context(&self) -> String {
        let environment = self.environment();
        format!(
            "<VERIFIED_RUNTIME probed_at=\"{}\">\nPlatform: {}\nAndroid: {}\nArchitecture: {}\nXiao: {}\nUID: {}\nRoot: {}\nSELinux: {}\nTermux: {}\nCapabilities:\n{}\n</VERIFIED_RUNTIME>",
            environment.probed_at,
            environment.platform,
            environment.android_version.as_deref().unwrap_or("not detected"),
            environment.architecture,
            environment.xiao_version,
            environment.effective_uid,
            if environment.root_available { "available" } else { "unavailable" },
            environment.selinux.as_str(),
            environment.termux.as_ref().map(|termux| termux.prefix.display().to_string()).unwrap_or_else(|| "not detected".into()),
            self.capabilities.concise_summary(24)
        )
    }

    fn persist_snapshot(&self) -> Result<()> {
        let environment = self.environment();
        self.workspace
            .write_environment(&render_environment(&environment, &self.capabilities))
            .context("refresh ENVIRONMENT.md")
    }
}

pub fn render_environment(
    environment: &RuntimeEnvironment,
    capabilities: &CapabilityRegistry,
) -> String {
    let termux = environment.termux.as_ref();
    format!(
        "# ENVIRONMENT\n\n> GENERATED BY XIAO at {}. Current in-memory probes remain authoritative.\n\n## Runtime\n\n- Platform: {}\n- OS/Android version: {}\n- Device model: {}\n- Architecture: {}\n- Xiao version: {}\n- Effective UID: {}\n- Root: {} ({})\n- SELinux: {}\n- Data root writable: {}\n\n## Termux\n\n- Available: {}\n- Prefix: {}\n- Home: {}\n- Shell: {}\n- Package manager: {}\n\n## Execution Backends\n\n{}\n\n## Selected Capabilities\n\n{}\n",
        environment.probed_at,
        environment.platform,
        environment.android_version.as_deref().or(environment.os_version.as_deref()).unwrap_or("unknown"),
        environment.device_model.as_deref().unwrap_or("unknown"),
        environment.architecture,
        environment.xiao_version,
        environment.effective_uid,
        if environment.root_available { "available" } else { "unavailable" },
        environment.root_evidence,
        environment.selinux.as_str(),
        environment.workspace_writable,
        termux.is_some(),
        termux.map(|value| value.prefix.display().to_string()).unwrap_or_else(|| "not detected".into()),
        termux.map(|value| value.home.display().to_string()).unwrap_or_else(|| "not detected".into()),
        termux.map(|value| value.shell.display().to_string()).unwrap_or_else(|| "not detected".into()),
        termux.and_then(|value| value.package_manager.as_ref()).map(|value| value.display().to_string()).unwrap_or_else(|| "not detected".into()),
        environment.execution_backends.iter().map(|backend| format!("- {:?}: available", backend)).collect::<Vec<_>>().join("\n"),
        capabilities.concise_summary(40),
    )
}

fn detect_termux(host: &dyn HostProbe, _inherited_path: &str) -> Option<TermuxEnvironment> {
    let prefix = host
        .env("PREFIX")
        .map(PathBuf::from)
        .filter(|prefix| host.is_dir(&prefix.join("bin")))
        .or_else(|| {
            let prefix = PathBuf::from("/data/data/com.termux/files/usr");
            host.is_dir(&prefix.join("bin")).then_some(prefix)
        })?;
    let home = host
        .env("TERMUX_HOME")
        .map(PathBuf::from)
        .filter(|home| host.is_dir(home))
        .or_else(|| {
            prefix
                .parent()
                .map(|parent| parent.join("home"))
                .filter(|home| host.is_dir(home))
        })
        .unwrap_or_else(|| prefix.join("home"));
    let prefix_bin = prefix.join("bin");
    // xiaod commonly starts as root with a system/root PATH. Never carry that
    // PATH into the unprivileged general executor: it could resolve `su` or
    // other Android administration binaries from a child process. Use the
    // canonical Termux bin directory as the complete child PATH.
    let path = prefix_bin.display().to_string();
    let shell = ["bash", "sh"]
        .into_iter()
        .map(|name| prefix_bin.join(name))
        .find(|path| host.is_file(path))
        .unwrap_or_else(|| prefix_bin.join("sh"));
    let package_manager = ["pkg", "apt"]
        .into_iter()
        .map(|name| prefix_bin.join(name))
        .find(|path| host.is_file(path));
    let (uid, gid) = host
        .owner_ids(&home)
        .map(|(uid, gid)| (Some(uid), Some(gid)))
        .unwrap_or((None, None));
    Some(TermuxEnvironment {
        prefix,
        home,
        path,
        shell,
        package_manager,
        uid,
        gid,
    })
}

fn find_binary(host: &dyn HostProbe, path: &str, binary: &str) -> Option<PathBuf> {
    if binary.contains('/') {
        let path = PathBuf::from(binary);
        return host.is_file(&path).then_some(path);
    }
    path.split(':')
        .filter(|entry| !entry.is_empty())
        .map(|entry| Path::new(entry).join(binary))
        .find(|candidate| host.is_file(candidate))
}

fn property(content: &str, name: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let (key, value) = line.trim().split_once('=')?;
        (key == name && !value.trim().is_empty()).then(|| value.trim().to_owned())
    })
}

fn compact(value: &str, max: usize) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::CapabilityStatus;
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Default)]
    struct FakeHost {
        env: BTreeMap<String, String>,
        files: BTreeMap<PathBuf, String>,
        dirs: BTreeSet<PathBuf>,
        uid: u32,
    }

    impl HostProbe for FakeHost {
        fn env(&self, key: &str) -> Option<String> {
            self.env.get(key).cloned()
        }
        fn is_file(&self, path: &Path) -> bool {
            self.files.contains_key(path)
        }
        fn is_dir(&self, path: &Path) -> bool {
            self.dirs.contains(path)
        }
        fn read(&self, path: &Path) -> Option<String> {
            self.files.get(path).cloned()
        }
        fn effective_uid(&self) -> u32 {
            self.uid
        }
        fn architecture(&self) -> String {
            "aarch64".into()
        }
        fn platform(&self) -> String {
            "linux".into()
        }
        fn owner_ids(&self, _path: &Path) -> Option<(u32, u32)> {
            Some((10234, 10234))
        }
    }

    #[test]
    fn fake_probe_detects_android_termux_root_and_installability() {
        let prefix = PathBuf::from("/termux/usr");
        let bin = prefix.join("bin");
        let host = FakeHost {
            env: BTreeMap::from([
                ("PREFIX".into(), prefix.display().to_string()),
                ("PATH".into(), bin.display().to_string()),
            ]),
            files: BTreeMap::from([
                (
                    PathBuf::from("/system/build.prop"),
                    "ro.build.version.release=14\nro.product.model=Test Phone".into(),
                ),
                (PathBuf::from("/sys/fs/selinux/enforce"), "1".into()),
                (bin.join("bash"), String::new()),
                (bin.join("pkg"), String::new()),
                (bin.join("git"), String::new()),
                (PathBuf::from("/system/bin/su"), String::new()),
            ]),
            dirs: BTreeSet::from([
                PathBuf::from("/system"),
                PathBuf::from("/sys/fs/selinux"),
                bin.clone(),
                PathBuf::from("/termux/home"),
            ]),
            uid: 10234,
        };
        let environment = EnvironmentProbe::new(Arc::new(host)).probe("/xiao");
        assert_eq!(environment.platform, "android");
        assert_eq!(environment.android_version.as_deref(), Some("14"));
        assert!(environment.root_available);
        assert_eq!(environment.selinux, SelinuxState::Enforcing);
        assert!(environment.termux.is_some());
        assert!(environment.binaries["git"].is_some());
        assert!(environment.binaries["ffmpeg"].is_none());
        let capabilities = CapabilityRegistry::from_environment(&environment);
        assert_eq!(
            capabilities.resolve("ffmpeg").status,
            CapabilityStatus::MissingInstallable
        );
    }

    #[test]
    fn runtime_state_refreshes_environment_file_without_touching_soul() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = Arc::new(IdentityWorkspace::new(directory.path()));
        let state = RuntimeState::initialize(workspace.clone(), EnvironmentProbe::real()).unwrap();
        let soul = workspace
            .read(crate::identity::WorkspaceDocument::Soul)
            .unwrap();
        let environment = workspace
            .read(crate::identity::WorkspaceDocument::Environment)
            .unwrap();
        assert!(environment.contains("Xiao version: 0.2.0"));
        state.refresh().unwrap();
        assert_eq!(
            workspace
                .read(crate::identity::WorkspaceDocument::Soul)
                .unwrap(),
            soul
        );
    }
}
