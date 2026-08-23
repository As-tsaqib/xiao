use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(test)]
use std::{collections::BTreeSet, sync::Mutex};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::Utc;
use tokio_util::sync::CancellationToken;

use crate::{
    runtime::{
        trusted_package_for_binary, Capability, CapabilityRegistry, CapabilityStatus,
        CommandOutcome, ExecutionPurpose, ProcessExecutor, TermuxCommand, TermuxEnvironment,
    },
    security::redact::redact_text,
    storage::{DependencyInstallStart, Storage},
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DependencyResolution {
    pub binary: String,
    pub package: Option<String>,
    pub installed: bool,
    pub verified: bool,
    pub evidence: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PackageCandidate {
    pub package: String,
    pub source: String,
    #[serde(default)]
    pub provided_binaries: Vec<String>,
}

#[async_trait]
pub trait TrustedPackageRepository: Send + Sync {
    async fn search(&self, binary: &str) -> Result<Vec<PackageCandidate>>;
}

#[async_trait]
pub trait PackageBackend: Send + Sync {
    fn package_manager_name(&self) -> &str;
    async fn binary_available(&self, binary: &str) -> Result<bool>;
    async fn install(
        &self,
        package: &str,
        cancellation: CancellationToken,
    ) -> Result<CommandOutcome>;
}

/// Trusted Termux repository installer. It accepts only normalized package
/// names selected by DependencyResolver; there is no remote-script path.
#[derive(Clone)]
pub struct TermuxPackageBackend {
    executor: Arc<dyn ProcessExecutor>,
    environment: TermuxEnvironment,
    cwd: PathBuf,
    manager_name: String,
}

impl TermuxPackageBackend {
    pub fn new(
        executor: Arc<dyn ProcessExecutor>,
        environment: TermuxEnvironment,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        let manager_name = environment
            .package_manager
            .as_ref()
            .and_then(|manager| manager.file_name())
            .and_then(|name| name.to_str())
            .filter(|name| matches!(*name, "pkg" | "apt" | "apt-get"))
            .unwrap_or("unavailable")
            .to_owned();
        Self {
            executor,
            environment,
            cwd: cwd.into(),
            manager_name,
        }
    }
}

#[async_trait]
impl PackageBackend for TermuxPackageBackend {
    fn package_manager_name(&self) -> &str {
        &self.manager_name
    }

    async fn binary_available(&self, binary: &str) -> Result<bool> {
        validate_binary(binary)?;
        Ok(self
            .environment
            .path
            .split(':')
            .filter(|entry| !entry.is_empty())
            .map(|entry| Path::new(entry).join(binary))
            .any(|candidate| candidate.is_file()))
    }

    async fn install(
        &self,
        package: &str,
        cancellation: CancellationToken,
    ) -> Result<CommandOutcome> {
        validate_package(package)?;
        if self.manager_name == "unavailable" {
            return Err(anyhow!("Termux package manager is unavailable"));
        }
        self.executor
            .execute(
                TermuxCommand {
                    program: self.manager_name.clone(),
                    args: vec!["install".into(), "-y".into(), package.into()],
                    cwd: self.cwd.clone(),
                    environment: Default::default(),
                    timeout_ms: 600_000,
                    max_output_chars: 16_384,
                    purpose: ExecutionPurpose::PackageInstall,
                },
                cancellation,
            )
            .await
    }
}

/// Read-only trusted repository discovery. Results remain candidates until
/// DependencyResolver validates their normalized package/source/provided
/// binary relationship; this never invokes an ecosystem installer or remote
/// script.
#[derive(Clone)]
pub struct TermuxRepositoryBackend {
    executor: Arc<dyn ProcessExecutor>,
    manager_name: String,
    cwd: PathBuf,
}

impl TermuxRepositoryBackend {
    pub fn new(
        executor: Arc<dyn ProcessExecutor>,
        environment: &TermuxEnvironment,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        let manager_name = environment
            .package_manager
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("pkg")
            .to_owned();
        Self {
            executor,
            manager_name,
            cwd: cwd.into(),
        }
    }
}

#[async_trait]
impl TrustedPackageRepository for TermuxRepositoryBackend {
    async fn search(&self, binary: &str) -> Result<Vec<PackageCandidate>> {
        validate_binary(binary)?;
        let (program, args) = if self.manager_name == "pkg" {
            ("pkg".to_owned(), vec!["search".into(), binary.into()])
        } else {
            (
                "apt-cache".to_owned(),
                vec!["search".into(), "--names-only".into(), binary.into()],
            )
        };
        let outcome = self
            .executor
            .execute(
                TermuxCommand {
                    program,
                    args,
                    cwd: self.cwd.clone(),
                    environment: Default::default(),
                    timeout_ms: 30_000,
                    max_output_chars: 64_000,
                    purpose: ExecutionPurpose::Verification,
                },
                CancellationToken::new(),
            )
            .await?;
        if !outcome.succeeded() {
            return Err(anyhow!(
                "trusted Termux repository search failed: {}",
                outcome.observable_summary()
            ));
        }
        let mut candidates = outcome
            .stdout
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(|token| token.split('/').next().unwrap_or(token))
            .filter(|package| validate_package(package).is_ok())
            // Repository search alone proves exact package-name candidates.
            // Non-exact mappings require a stronger trusted index/fake source.
            .filter(|package| *package == binary)
            .map(|package| PackageCandidate {
                package: package.into(),
                source: "termux_repository_search".into(),
                provided_binaries: vec![binary.into()],
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.package.cmp(&right.package));
        candidates.dedup_by(|left, right| left.package == right.package);
        Ok(candidates)
    }
}

#[derive(Clone)]
pub struct DependencyResolver {
    capabilities: Arc<CapabilityRegistry>,
    backend: Arc<dyn PackageBackend>,
    storage: Option<Arc<Storage>>,
    repository: Option<Arc<dyn TrustedPackageRepository>>,
}

impl DependencyResolver {
    pub fn new(
        capabilities: Arc<CapabilityRegistry>,
        backend: Arc<dyn PackageBackend>,
        storage: Option<Arc<Storage>>,
    ) -> Self {
        Self {
            capabilities,
            backend,
            storage,
            repository: None,
        }
    }

    pub fn with_trusted_repository(
        capabilities: Arc<CapabilityRegistry>,
        backend: Arc<dyn PackageBackend>,
        storage: Option<Arc<Storage>>,
        repository: Arc<dyn TrustedPackageRepository>,
    ) -> Self {
        Self {
            capabilities,
            backend,
            storage,
            repository: Some(repository),
        }
    }

    pub async fn ensure_binary(
        &self,
        binary: &str,
        agent_run_id: Option<&str>,
        cancellation: CancellationToken,
        progress: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<DependencyResolution> {
        validate_binary(binary)?;
        if self.backend.binary_available(binary).await? {
            self.mark_available(binary, "executable verified in Termux PATH");
            return Ok(DependencyResolution {
                binary: binary.into(),
                package: None,
                installed: false,
                verified: true,
                evidence: "executable verified in Termux PATH".into(),
                source: None,
            });
        }

        let (package, source) = if let Some(package) = trusted_package_for_binary(binary) {
            (package.to_owned(), "known_mapping".to_owned())
        } else {
            let repository = self.repository.as_ref().ok_or_else(|| {
                anyhow!(
                    "binary {binary} is missing and no trusted Termux repository discovery source is configured"
                )
            })?;
            let candidates = repository.search(binary).await?;
            let candidate = validated_candidate(binary, candidates)?;
            (candidate.package, candidate.source)
        };
        validate_package(&package)?;
        let resolution = self.capabilities.resolve(&format!("binary.{binary}"));
        if !matches!(
            resolution.status,
            CapabilityStatus::Available
                | CapabilityStatus::MissingInstallable
                | CapabilityStatus::Unknown
        ) {
            return Err(anyhow!(
                "{}",
                resolution.concrete_blocker.unwrap_or_else(|| format!(
                    "binary {binary} is not installable in the current runtime"
                ))
            ));
        }

        // A previous environment snapshot can say available even though the
        // immediate backend re-probe above found the executable missing.
        // Continue through the trusted install path instead of producing a
        // stale false-capability refusal.
        self.capabilities.set(Capability {
            name: format!("binary.{binary}"),
            status: CapabilityStatus::MissingInstallable,
            backend: Some("termux".into()),
            requirements: Vec::new(),
            risk: "safe_side_effect".into(),
            install_hint: Some(format!("pkg:{package}")),
            last_probe: Utc::now().to_rfc3339(),
            evidence: "immediate Termux re-probe found the executable missing".into(),
        });

        if let Some(tx) = progress {
            let _ = tx.send(format!(
                "{binary} is missing; installing trusted Termux package {package}…"
            ));
        }
        let install_id = self
            .storage
            .as_ref()
            .map(|storage| {
                let requested_capability = format!("binary.{binary}");
                storage.begin_dependency_install(DependencyInstallStart {
                    agent_run_id,
                    binary,
                    package: &package,
                    package_manager: self.backend.package_manager_name(),
                    source: &source,
                    validated: true,
                    requested_capability: &requested_capability,
                })
            })
            .transpose()?;
        let outcome = self.backend.install(&package, cancellation.clone()).await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                if let (Some(storage), Some(id)) = (&self.storage, install_id.as_deref()) {
                    let _ = storage.finish_dependency_install(
                        id,
                        if cancellation.is_cancelled() {
                            "interrupted"
                        } else {
                            "failed"
                        },
                        &redact_text(&error.to_string()),
                    );
                }
                return Err(error);
            }
        };
        if !outcome.succeeded() {
            let evidence = format!(
                "package install {}; {}",
                outcome.observable_summary(),
                outcome.stderr
            );
            if let (Some(storage), Some(id)) = (&self.storage, install_id.as_deref()) {
                storage.finish_dependency_install(
                    id,
                    if outcome.cancelled {
                        "interrupted"
                    } else {
                        "failed"
                    },
                    &evidence,
                )?;
            }
            return Err(anyhow!(
                "failed to install trusted package {package}: {evidence}"
            ));
        }

        let verified = self.backend.binary_available(binary).await?;
        let evidence = if verified {
            format!("installed {package}; executable {binary} re-probed successfully")
        } else {
            format!("package {package} reported success but {binary} is still missing")
        };
        if let (Some(storage), Some(id)) = (&self.storage, install_id.as_deref()) {
            storage.finish_dependency_install(
                id,
                if verified { "succeeded" } else { "failed" },
                &evidence,
            )?;
        }
        if !verified {
            return Err(anyhow!("{evidence}"));
        }
        self.mark_available(binary, &evidence);
        if let Some(tx) = progress {
            let _ = tx.send(format!("Installed {package}; resuming the original task…"));
        }
        Ok(DependencyResolution {
            binary: binary.into(),
            package: Some(package),
            installed: true,
            verified: true,
            evidence,
            source: Some(source),
        })
    }

    fn mark_available(&self, binary: &str, evidence: &str) {
        self.capabilities.set(Capability {
            name: format!("binary.{binary}"),
            status: CapabilityStatus::Available,
            backend: Some("termux".into()),
            requirements: Vec::new(),
            risk: "safe_side_effect".into(),
            install_hint: trusted_package_for_binary(binary)
                .map(|package| format!("pkg:{package}")),
            last_probe: Utc::now().to_rfc3339(),
            evidence: evidence.into(),
        });
    }
}

fn validated_candidate(
    binary: &str,
    candidates: Vec<PackageCandidate>,
) -> Result<PackageCandidate> {
    let mut valid = candidates
        .into_iter()
        .filter(|candidate| {
            validate_package(&candidate.package).is_ok()
                && candidate.source.starts_with("termux_repository")
                && (candidate.package == binary
                    || candidate
                        .provided_binaries
                        .iter()
                        .any(|provided| provided == binary))
        })
        .collect::<Vec<_>>();
    valid.sort_by(|left, right| {
        (left.package != binary, &left.package).cmp(&(right.package != binary, &right.package))
    });
    valid.into_iter().next().ok_or_else(|| {
        anyhow!("trusted Termux repository returned no validated package providing {binary}")
    })
}

pub fn validate_package(package: &str) -> Result<()> {
    let valid = !package.is_empty()
        && package.len() <= 128
        && package.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '+' | '-' | '.' | '_')
        });
    if !valid || package.starts_with(['-', '.']) || package.contains("..") {
        return Err(anyhow!("invalid or untrusted package name"));
    }
    Ok(())
}

pub fn validate_binary(binary: &str) -> Result<()> {
    let valid = !binary.is_empty()
        && binary.len() <= 128
        && binary.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.' | '_')
        });
    if !valid || binary.starts_with(['-', '.']) || binary.contains("..") {
        return Err(anyhow!("invalid binary name"));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) struct FakePackageBackend {
    pub available: Mutex<BTreeSet<String>>,
    pub installed: Mutex<Vec<String>>,
}

#[cfg(test)]
#[async_trait]
impl PackageBackend for FakePackageBackend {
    fn package_manager_name(&self) -> &str {
        "pkg"
    }

    async fn binary_available(&self, binary: &str) -> Result<bool> {
        Ok(self.available.lock().unwrap().contains(binary))
    }

    async fn install(&self, package: &str, _: CancellationToken) -> Result<CommandOutcome> {
        self.installed.lock().unwrap().push(package.into());
        if package == "ffmpeg" {
            self.available.lock().unwrap().insert("ffmpeg".into());
            self.available.lock().unwrap().insert("ffprobe".into());
        }
        Ok(CommandOutcome {
            program: "pkg".into(),
            args: vec!["install".into(), "-y".into(), package.into()],
            cwd: PathBuf::from("/termux/home"),
            exit_code: Some(0),
            stdout: "installed".into(),
            stderr: String::new(),
            timed_out: false,
            cancelled: false,
            truncated: false,
            duration_ms: 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{ExecutionBackend, RuntimeEnvironment, SelinuxState, TermuxEnvironment};
    use std::collections::BTreeMap;

    fn capabilities() -> Arc<CapabilityRegistry> {
        Arc::new(CapabilityRegistry::from_environment(&RuntimeEnvironment {
            platform: "android".into(),
            os_version: Some("14".into()),
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
                shell: "/termux/usr/bin/bash".into(),
                package_manager: Some("/termux/usr/bin/pkg".into()),
                uid: Some(10234),
                gid: Some(10234),
            }),
            data_root: "/xiao".into(),
            workspace_writable: true,
            binaries: BTreeMap::from([("ffmpeg".into(), None)]),
            execution_backends: vec![ExecutionBackend::Termux],
            probed_at: "now".into(),
        }))
    }

    #[tokio::test]
    async fn trusted_missing_dependency_is_installed_reprobed_and_audited() {
        let storage = Arc::new(Storage::open_memory().unwrap());
        let backend = Arc::new(FakePackageBackend {
            available: Mutex::new(BTreeSet::new()),
            installed: Mutex::new(Vec::new()),
        });
        let resolver = DependencyResolver::new(capabilities(), backend.clone(), Some(storage));
        let storage = resolver.storage.as_ref().unwrap();
        let session = storage
            .create_session("owner", "test", "custom", None, "m", false, None)
            .unwrap();
        let run = storage
            .create_agent_run("owner", &session.id, "custom", "m", Some("extract audio"))
            .unwrap();
        let result = resolver
            .ensure_binary("ffmpeg", Some(&run), CancellationToken::new(), None)
            .await
            .unwrap();
        assert!(result.installed && result.verified);
        assert_eq!(&*backend.installed.lock().unwrap(), &["ffmpeg"]);
        let audit = storage.dependency_installs(&run).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].status, "succeeded");
        assert!(audit[0].evidence.as_deref().unwrap().contains("re-probed"));
    }

    #[test]
    fn package_names_and_unknown_remote_installers_are_rejected() {
        assert!(validate_package("ffmpeg").is_ok());
        for package in ["../../evil", "-hook", "curl | sh", "https://example"] {
            assert!(validate_package(package).is_err());
        }
        assert!(trusted_package_for_binary("unknown-root-binary").is_none());
    }
}
