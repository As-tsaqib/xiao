use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::runtime::RuntimeEnvironment;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Available,
    MissingInstallable,
    ApprovalRequired,
    TemporarilyUnavailable,
    Unsupported,
    Forbidden,
    Unknown,
}

impl CapabilityStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::MissingInstallable => "missing_installable",
            Self::ApprovalRequired => "approval_required",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::Unsupported => "unsupported",
            Self::Forbidden => "forbidden",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Capability {
    pub name: String,
    pub status: CapabilityStatus,
    pub backend: Option<String>,
    pub requirements: Vec<String>,
    pub risk: String,
    pub install_hint: Option<String>,
    pub last_probe: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityResolution {
    pub requested: String,
    pub canonical: String,
    pub status: CapabilityStatus,
    pub capability: Option<Capability>,
    pub concrete_blocker: Option<String>,
}

/// Current runtime truth. A status is deliberately richer than a boolean so
/// the agent can install, request approval, try another backend, or report a
/// concrete blocker instead of emitting a generic capability refusal.
#[derive(Debug, Clone)]
pub struct CapabilityRegistry {
    entries: Arc<RwLock<BTreeMap<String, Capability>>>,
}

impl CapabilityRegistry {
    pub fn from_environment(environment: &RuntimeEnvironment) -> Self {
        let registry = Self {
            entries: Arc::new(RwLock::new(BTreeMap::new())),
        };
        registry.refresh(environment);
        registry
    }

    pub fn refresh(&self, environment: &RuntimeEnvironment) {
        let now = Utc::now().to_rfc3339();
        let mut entries = BTreeMap::new();
        let mut add = |name: &str,
                       status: CapabilityStatus,
                       backend: Option<&str>,
                       risk: &str,
                       install_hint: Option<String>,
                       evidence: String| {
            entries.insert(
                name.to_owned(),
                Capability {
                    name: name.to_owned(),
                    status,
                    backend: backend.map(str::to_owned),
                    requirements: Vec::new(),
                    risk: risk.to_owned(),
                    install_hint,
                    last_probe: now.clone(),
                    evidence,
                },
            );
        };

        add(
            "execution.termux",
            if environment.termux.is_some() {
                CapabilityStatus::Available
            } else {
                CapabilityStatus::Unsupported
            },
            environment.termux.as_ref().map(|_| "termux"),
            "safe_side_effect",
            None,
            environment
                .termux
                .as_ref()
                .map(|termux| format!("detected prefix {}", termux.prefix.display()))
                .unwrap_or_else(|| "Termux prefix was not detected".into()),
        );
        add(
            "android.root",
            if environment.effective_uid == 0 {
                CapabilityStatus::Available
            } else {
                CapabilityStatus::TemporarilyUnavailable
            },
            Some("android_privileged"),
            "privileged",
            None,
            environment.root_evidence.clone(),
        );
        add(
            "android.service.inspect",
            if environment.effective_uid == 0 {
                CapabilityStatus::Available
            } else {
                CapabilityStatus::TemporarilyUnavailable
            },
            Some("android_privileged"),
            "read_only",
            None,
            "typed Xiao service inspection; no raw root command".into(),
        );
        add(
            "android.service.restart",
            if environment.effective_uid == 0 {
                CapabilityStatus::ApprovalRequired
            } else {
                CapabilityStatus::TemporarilyUnavailable
            },
            Some("android_privileged"),
            "privileged",
            None,
            "typed Android broker operation; never a raw root command".into(),
        );
        for name in [
            "xiao.memory",
            "xiao.session_search",
            "xiao.skills",
            "xiao.tool_registry",
        ] {
            add(
                name,
                CapabilityStatus::Available,
                Some("builtin"),
                "read_only",
                None,
                "registered Xiao subsystem".into(),
            );
        }

        for (binary, path) in &environment.binaries {
            let package = trusted_package_for_binary(binary);
            let status = if path.is_some() {
                CapabilityStatus::Available
            } else if environment.termux.is_some() && package.is_some() {
                CapabilityStatus::MissingInstallable
            } else if environment.termux.is_some() {
                CapabilityStatus::Unknown
            } else {
                CapabilityStatus::Unsupported
            };
            add(
                &format!("binary.{binary}"),
                status,
                environment.termux.as_ref().map(|_| "termux"),
                "safe_side_effect",
                package.map(|package| format!("pkg:{package}")),
                path.as_ref()
                    .map(|path| format!("found at {}", path.display()))
                    .unwrap_or_else(|| "binary not found in probed Termux PATH".into()),
            );
        }
        *self.entries.write().expect("capability registry poisoned") = entries;
    }

    pub fn get(&self, name: &str) -> Option<Capability> {
        let canonical = canonical_capability(name);
        self.entries.read().ok()?.get(&canonical).cloned()
    }

    pub fn resolve(&self, requested: &str) -> CapabilityResolution {
        let canonical = canonical_capability(requested);
        let capability = self.get(&canonical);
        let status = capability
            .as_ref()
            .map(|capability| capability.status)
            .unwrap_or(CapabilityStatus::Unknown);
        let concrete_blocker = match status {
            CapabilityStatus::Available | CapabilityStatus::MissingInstallable => None,
            CapabilityStatus::ApprovalRequired => {
                Some(format!("{canonical} requires explicit owner approval"))
            }
            CapabilityStatus::TemporarilyUnavailable => Some(format!(
                "{canonical} was probed but is temporarily unavailable"
            )),
            CapabilityStatus::Unsupported => {
                Some(format!("no supported runtime backend provides {canonical}"))
            }
            CapabilityStatus::Forbidden => {
                Some(format!("{canonical} is forbidden by Xiao runtime policy"))
            }
            CapabilityStatus::Unknown => Some(format!(
                "{canonical} is unknown after checking registered runtime capabilities"
            )),
        };
        CapabilityResolution {
            requested: requested.to_owned(),
            canonical,
            status,
            capability,
            concrete_blocker,
        }
    }

    pub fn set(&self, capability: Capability) {
        self.entries
            .write()
            .expect("capability registry poisoned")
            .insert(capability.name.clone(), capability);
    }

    pub fn list(&self) -> Vec<Capability> {
        self.entries
            .read()
            .map(|entries| entries.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn concise_summary(&self, limit: usize) -> String {
        self.list()
            .into_iter()
            .take(limit.clamp(1, 100))
            .map(|capability| {
                let backend = capability
                    .backend
                    .as_deref()
                    .map(|backend| format!(" via {backend}"))
                    .unwrap_or_default();
                format!(
                    "- {}: {}{}",
                    capability.name,
                    capability.status.as_str(),
                    backend
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn trusted_package_for_binary(binary: &str) -> Option<&'static str> {
    match binary {
        "bash" => Some("bash"),
        "cargo" => Some("rust"),
        "curl" => Some("curl"),
        "ffmpeg" | "ffprobe" => Some("ffmpeg"),
        "file" => Some("file"),
        "git" => Some("git"),
        "jq" => Some("jq"),
        "node" | "npm" => Some("nodejs"),
        "python" | "python3" => Some("python"),
        "rg" => Some("ripgrep"),
        "tar" => Some("tar"),
        "unzip" => Some("unzip"),
        "zip" => Some("zip"),
        _ => None,
    }
}

pub fn canonical_capability(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "exec"
        | "terminal"
        | "shell"
        | "termux"
        | "execution.terminal"
        | "tool.exec"
        | "tool.terminal"
        | "tool.termux_terminal" => "execution.termux".into(),
        "tool.memory_search" | "tool.memory_set" | "tool.memory_delete" => "xiao.memory".into(),
        "tool.session_search" => "xiao.session_search".into(),
        "tool.skill_search" | "tool.skill_view" => "xiao.skills".into(),
        "root" | "android_root" | "execution.android_root" => "android.root".into(),
        value if value.starts_with("binary.") || value.contains('.') => value.to_owned(),
        value => format!("binary.{value}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{ExecutionBackend, SelinuxState, TermuxEnvironment};
    use std::{collections::BTreeMap, path::PathBuf};

    fn environment() -> RuntimeEnvironment {
        RuntimeEnvironment {
            platform: "android".into(),
            os_version: Some("14".into()),
            android_version: Some("14".into()),
            device_model: Some("fake".into()),
            architecture: "aarch64".into(),
            xiao_version: crate::VERSION.into(),
            effective_uid: 0,
            root_available: true,
            root_evidence: "su detected".into(),
            selinux: SelinuxState::Enforcing,
            termux: Some(TermuxEnvironment {
                prefix: PathBuf::from("/termux/usr"),
                home: PathBuf::from("/termux/home"),
                path: "/termux/usr/bin".into(),
                shell: PathBuf::from("/termux/usr/bin/bash"),
                package_manager: Some(PathBuf::from("/termux/usr/bin/pkg")),
                uid: Some(10234),
                gid: Some(10234),
            }),
            data_root: PathBuf::from("/xiao"),
            workspace_writable: true,
            binaries: BTreeMap::from([
                ("git".into(), Some(PathBuf::from("/termux/usr/bin/git"))),
                ("ffmpeg".into(), None),
            ]),
            execution_backends: vec![
                ExecutionBackend::Termux,
                ExecutionBackend::AndroidPrivileged,
            ],
            probed_at: "now".into(),
        }
    }

    #[test]
    fn capability_resolution_distinguishes_available_installable_and_approval() {
        let registry = CapabilityRegistry::from_environment(&environment());
        assert_eq!(
            registry.resolve("terminal").status,
            CapabilityStatus::Available
        );
        assert_eq!(
            registry.resolve("ffmpeg").status,
            CapabilityStatus::MissingInstallable
        );
        assert_eq!(
            registry.resolve("android.service.restart").status,
            CapabilityStatus::ApprovalRequired
        );
        assert!(registry
            .resolve("android.service.restart")
            .concrete_blocker
            .unwrap()
            .contains("approval"));
    }

    #[test]
    fn capability_resolution_prevents_false_cannot_when_termux_backend_is_usable() {
        let registry = CapabilityRegistry::from_environment(&environment());
        for alias in ["exec", "terminal", "shell", "tool.termux_terminal"] {
            let resolution = registry.resolve(alias);
            assert_eq!(resolution.canonical, "execution.termux");
            assert_eq!(resolution.status, CapabilityStatus::Available);
            assert!(resolution.concrete_blocker.is_none());
        }
    }
}
