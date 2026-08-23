use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::{
    identity::IdentityWorkspace,
    runtime::{CapabilityRegistry, CapabilityStatus, DependencyResolver},
    security::redact::contains_secret_material,
    skills::{canonical_skill_name, SkillCandidate, SkillMutation, SkillRecord, SkillStore},
};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillRequirements {
    pub binaries: Vec<String>,
    pub capabilities: Vec<String>,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillDocument {
    pub path: PathBuf,
    pub name: String,
    pub description: String,
    pub body: String,
    pub requirements: SkillRequirements,
    pub optional_metadata: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SkillEligibility {
    Eligible,
    Installable { binaries: Vec<String> },
    ApprovalRequired { capabilities: Vec<String> },
    Unavailable { blockers: Vec<String> },
}

#[derive(Debug, Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
    #[serde(default, flatten)]
    optional: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Clone)]
pub struct FilesystemSkills {
    workspace: Arc<IdentityWorkspace>,
    store: Arc<SkillStore>,
    capabilities: Option<Arc<CapabilityRegistry>>,
    dependencies: Option<Arc<DependencyResolver>>,
}

impl FilesystemSkills {
    pub fn new(workspace: Arc<IdentityWorkspace>, store: Arc<SkillStore>) -> Self {
        Self {
            workspace,
            store,
            capabilities: None,
            dependencies: None,
        }
    }

    pub fn with_runtime(
        workspace: Arc<IdentityWorkspace>,
        store: Arc<SkillStore>,
        capabilities: Arc<CapabilityRegistry>,
        dependencies: Option<Arc<DependencyResolver>>,
    ) -> Self {
        Self {
            workspace,
            store,
            capabilities: Some(capabilities),
            dependencies,
        }
    }

    pub fn discover(&self) -> Result<Vec<SkillDocument>> {
        self.workspace
            .discover_skill_files()?
            .into_iter()
            .map(|path| {
                let raw = fs::read_to_string(&path)
                    .with_context(|| format!("read community skill {}", path.display()))?;
                parse_skill(path, &raw)
            })
            .collect()
    }

    pub fn reconcile(&self, owner: &str) -> Result<usize> {
        let storage = self.store.storage();
        let mut documents = self.discover()?;
        if documents.is_empty() {
            let legacy = self.store.list(owner, 500)?;
            for record in legacy {
                self.write_record(&record)?;
            }
            documents = self.discover()?;
        }

        let mut changed = 0usize;
        for document in documents {
            let raw = fs::read_to_string(&document.path)?;
            let hash = hash(&raw);
            let path = document.path.display().to_string();
            if storage.skill_file_hash(&path)?.as_deref() == Some(&hash)
                && self.store.view(owner, &document.name)?.is_some()
            {
                continue;
            }
            let (mutation, record) =
                self.store
                    .create_or_update(owner, document.candidate(), None)?;
            changed += usize::from(mutation != SkillMutation::Unchanged);
            storage.set_skill_file_hash(&path, &record.name, &hash)?;
        }
        Ok(changed)
    }

    pub fn learn(
        &self,
        owner: &str,
        candidate: SkillCandidate,
        source_session_id: Option<&str>,
    ) -> Result<(SkillMutation, SkillRecord)> {
        self.reconcile(owner)?;
        let result = self
            .store
            .create_or_update(owner, candidate, source_session_id)?;
        let path = self.write_record(&result.1)?;
        let raw = fs::read_to_string(&path)?;
        self.store.storage().set_skill_file_hash(
            &path.display().to_string(),
            &result.1.name,
            &hash(&raw),
        )?;
        Ok(result)
    }

    pub fn document(&self, name: &str) -> Result<Option<SkillDocument>> {
        let canonical = canonical_skill_name(name);
        Ok(self
            .discover()?
            .into_iter()
            .find(|document| document.name == canonical))
    }

    pub fn eligibility(&self, document: &SkillDocument) -> SkillEligibility {
        let Some(capabilities) = &self.capabilities else {
            return SkillEligibility::Eligible;
        };
        let mut installable = Vec::new();
        let mut approvals = Vec::new();
        let mut blockers = Vec::new();
        for requirement in document
            .requirements
            .binaries
            .iter()
            .map(|binary| format!("binary.{binary}"))
            .chain(document.requirements.capabilities.iter().cloned())
            .chain(
                document
                    .requirements
                    .tools
                    .iter()
                    .map(|tool| format!("tool.{tool}")),
            )
        {
            let resolution = capabilities.resolve(&requirement);
            match resolution.status {
                CapabilityStatus::Available => {}
                CapabilityStatus::MissingInstallable => {
                    installable.push(resolution.canonical.trim_start_matches("binary.").into());
                }
                CapabilityStatus::ApprovalRequired => approvals.push(resolution.canonical),
                _ => blockers.push(
                    resolution
                        .concrete_blocker
                        .unwrap_or_else(|| format!("{} is unavailable", resolution.canonical)),
                ),
            }
        }
        if !blockers.is_empty() {
            SkillEligibility::Unavailable { blockers }
        } else if !approvals.is_empty() {
            SkillEligibility::ApprovalRequired {
                capabilities: approvals,
            }
        } else if !installable.is_empty() {
            installable.sort();
            installable.dedup();
            SkillEligibility::Installable {
                binaries: installable,
            }
        } else {
            SkillEligibility::Eligible
        }
    }

    pub async fn resolve_dependencies(
        &self,
        document: &SkillDocument,
        agent_run_id: Option<&str>,
        cancellation: CancellationToken,
        progress: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<SkillEligibility> {
        match self.eligibility(document) {
            SkillEligibility::Installable { binaries } => {
                let resolver = self
                    .dependencies
                    .as_ref()
                    .ok_or_else(|| anyhow!("skill dependency resolver is unavailable"))?;
                for binary in binaries {
                    resolver
                        .ensure_binary(&binary, agent_run_id, cancellation.clone(), progress)
                        .await?;
                }
                Ok(self.eligibility(document))
            }
            status => Ok(status),
        }
    }

    fn write_record(&self, record: &SkillRecord) -> Result<PathBuf> {
        if contains_secret_material(&record.procedure) || contains_secret_material(&record.pitfalls)
        {
            return Err(anyhow!("refusing to write secret material to SKILL.md"));
        }
        self.workspace
            .write_skill_atomic(&record.name, &render_skill(record))
    }
}

impl SkillDocument {
    pub fn candidate(&self) -> SkillCandidate {
        SkillCandidate {
            name: self.name.clone(),
            summary: self.description.clone(),
            when_to_use: section(&self.body, "When to Use")
                .unwrap_or_else(|| self.description.clone()),
            prerequisites: section(&self.body, "Prerequisites").unwrap_or_default(),
            procedure: section(&self.body, "Procedure")
                .unwrap_or_else(|| body_fallback(&self.body, &self.description)),
            pitfalls: section(&self.body, "Pitfalls").unwrap_or_default(),
            verification: section(&self.body, "Verification").unwrap_or_default(),
        }
    }
}

pub fn parse_skill(path: PathBuf, raw: &str) -> Result<SkillDocument> {
    if raw.contains('\0') || raw.chars().count() > 512_000 {
        return Err(anyhow!("SKILL.md is invalid or too large"));
    }
    let normalized = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut lines = normalized.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Err(anyhow!("SKILL.md must start with YAML frontmatter"));
    }
    let mut yaml = Vec::new();
    let mut found_end = false;
    for line in &mut lines {
        if line.trim() == "---" {
            found_end = true;
            break;
        }
        yaml.push(line);
    }
    if !found_end {
        return Err(anyhow!("SKILL.md frontmatter is not closed"));
    }
    let frontmatter: Frontmatter = serde_yaml::from_str(&yaml.join("\n"))?;
    let name = canonical_skill_name(&frontmatter.name);
    if name.is_empty() || name.chars().count() > 120 {
        return Err(anyhow!("SKILL.md name is invalid"));
    }
    let description = frontmatter.description.trim().to_owned();
    if description.is_empty() || description.chars().count() > 2_000 {
        return Err(anyhow!("SKILL.md description is empty or too long"));
    }
    let requirements = requirements(&frontmatter.optional);
    Ok(SkillDocument {
        path,
        name,
        description,
        body: lines.collect::<Vec<_>>().join("\n").trim().to_owned(),
        requirements,
        optional_metadata: frontmatter.optional,
    })
}

fn requirements(optional: &BTreeMap<String, serde_yaml::Value>) -> SkillRequirements {
    let root = optional
        .get("metadata")
        .and_then(mapping_value)
        .and_then(|mapping| mapping.get(serde_yaml::Value::String("xiao".into())))
        .or_else(|| optional.get("xiao"));
    let requires = root
        .and_then(mapping_value)
        .and_then(|mapping| mapping.get(serde_yaml::Value::String("requires".into())));
    let Some(mapping) = requires.and_then(mapping_value) else {
        return SkillRequirements::default();
    };
    SkillRequirements {
        binaries: string_list(mapping.get(serde_yaml::Value::String("bins".into())))
            .into_iter()
            .chain(string_list(
                mapping.get(serde_yaml::Value::String("binaries".into())),
            ))
            .collect(),
        capabilities: string_list(mapping.get(serde_yaml::Value::String("capabilities".into()))),
        tools: string_list(mapping.get(serde_yaml::Value::String("tools".into()))),
    }
}

fn mapping_value(value: &serde_yaml::Value) -> Option<&serde_yaml::Mapping> {
    value.as_mapping()
}

fn string_list(value: Option<&serde_yaml::Value>) -> Vec<String> {
    match value {
        Some(serde_yaml::Value::Sequence(values)) => values
            .iter()
            .filter_map(serde_yaml::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(serde_yaml::Value::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn section(body: &str, title: &str) -> Option<String> {
    let heading = format!("## {}", title.to_ascii_lowercase());
    let lines = body.lines().collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| line.trim().to_ascii_lowercase() == heading)?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.trim_start().starts_with("## "))
        .map(|relative| start + 1 + relative)
        .unwrap_or(lines.len());
    let content = lines[start + 1..end].join("\n").trim().to_owned();
    (!content.is_empty()).then_some(content)
}

fn body_fallback(body: &str, description: &str) -> String {
    let body = body
        .lines()
        .filter(|line| !line.trim_start().starts_with("# "))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    if body.is_empty() {
        description.to_owned()
    } else {
        body
    }
}

fn render_skill(record: &SkillRecord) -> String {
    let description = serde_json::to_string(&record.summary).unwrap_or_else(|_| "\"Skill\"".into());
    format!(
        "---\nname: {}\ndescription: {}\nmetadata:\n  xiao:\n    requires:\n      bins: []\n      capabilities: []\n---\n\n# {}\n\n## When to Use\n\n{}\n\n## Prerequisites\n\n{}\n\n## Procedure\n\n{}\n\n## Pitfalls\n\n{}\n\n## Verification\n\n{}\n",
        record.name,
        description,
        title(&record.name),
        record.when_to_use,
        if record.prerequisites.trim().is_empty() { "- Use only runtime capabilities allowed by Xiao ToolPolicy." } else { &record.prerequisites },
        record.procedure,
        if record.pitfalls.trim().is_empty() { "- Observe failures and change strategy; do not repeat an identical failed action." } else { &record.pitfalls },
        if record.verification.trim().is_empty() { "Confirm an observable result appropriate to the task before reporting success." } else { &record.verification },
    )
}

fn title(name: &str) -> String {
    name.split('-')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn hash(raw: &str) -> String {
    format!("{:x}", Sha256::digest(raw.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        runtime::{
            CapabilityRegistry, CapabilityStatus, CommandOutcome, DependencyResolver,
            ExecutionBackend, PackageBackend, RuntimeEnvironment, SelinuxState, TermuxEnvironment,
        },
        storage::Storage,
    };
    use async_trait::async_trait;
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::Mutex,
    };

    #[test]
    fn community_minimum_and_optional_metadata_are_tolerated() {
        let raw = r#"---
name: media-audio-extract
description: Extract and verify audio.
version: 1.2.3
author: community
metadata:
  xiao:
    requires:
      bins: [ffmpeg, ffprobe]
      capabilities: [execution.termux]
---
# Media

## Procedure

1. Extract audio.
"#;
        let parsed = parse_skill("/skills/media/SKILL.md".into(), raw).unwrap();
        assert_eq!(parsed.name, "media-audio-extract");
        assert_eq!(parsed.requirements.binaries, ["ffmpeg", "ffprobe"]);
        assert!(parsed.optional_metadata.contains_key("author"));
        assert!(parsed.candidate().procedure.contains("Extract audio"));
    }

    #[test]
    fn discovery_reconciles_manual_skill_and_learning_writes_atomic_skill_file() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = Arc::new(IdentityWorkspace::new(directory.path()));
        workspace.bootstrap().unwrap();
        workspace
            .write_skill_atomic(
                "community-check",
                "---\nname: community-check\ndescription: Check a community artifact.\n---\n\n# Check\n\n## Procedure\n\nInspect then verify.\n",
            )
            .unwrap();
        let store = Arc::new(SkillStore::new(Arc::new(Storage::open_memory().unwrap())));
        let files = FilesystemSkills::new(workspace.clone(), store.clone());
        assert_eq!(files.reconcile("p").unwrap(), 1);
        assert!(store.view("p", "community-check").unwrap().is_some());
        let learned = files
            .learn(
                "p",
                SkillCandidate {
                    name: "repair-widget".into(),
                    summary: "Repair a widget safely".into(),
                    when_to_use: "When a widget fails".into(),
                    prerequisites: "Widget inspection access.".into(),
                    procedure: "1. Inspect.\n2. Repair.".into(),
                    pitfalls: "Do not guess.".into(),
                    verification: "Widget check passes.".into(),
                },
                Some("s"),
            )
            .unwrap();
        assert_eq!(learned.0, SkillMutation::Created);
        let raw =
            fs::read_to_string(directory.path().join("skills/repair-widget/SKILL.md")).unwrap();
        assert!(raw.contains("## Verification"));

        store.set_enabled("p", &learned.1.id, false).unwrap();
        assert_eq!(files.reconcile("p").unwrap(), 0);
        assert!(
            !store.view("p", &learned.1.id).unwrap().unwrap().enabled,
            "filesystem reconciliation must preserve an owner disable"
        );

        // A stale file-hash row must not hide an on-disk skill when its
        // active database row is absent (for example after recovery/import).
        store.delete_learned("p", &learned.1.id).unwrap();
        assert_eq!(files.reconcile("p").unwrap(), 1);
        assert!(store.view("p", "repair-widget").unwrap().is_some());
    }

    #[test]
    fn missing_installable_skill_dependency_is_gated() {
        let environment = skill_environment();
        let capabilities = Arc::new(CapabilityRegistry::from_environment(&environment));
        let directory = tempfile::tempdir().unwrap();
        let workspace = Arc::new(IdentityWorkspace::new(directory.path()));
        workspace.bootstrap().unwrap();
        let files = FilesystemSkills::with_runtime(
            workspace,
            Arc::new(SkillStore::new(Arc::new(Storage::open_memory().unwrap()))),
            capabilities,
            None,
        );
        let document = installable_skill();
        assert_eq!(
            files.eligibility(&document),
            SkillEligibility::Installable {
                binaries: vec!["ffmpeg".into()]
            }
        );
    }

    struct FakeSkillPackages {
        available: Mutex<BTreeSet<String>>,
        installed: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl PackageBackend for FakeSkillPackages {
        fn package_manager_name(&self) -> &str {
            "pkg"
        }

        async fn binary_available(&self, binary: &str) -> Result<bool> {
            Ok(self.available.lock().unwrap().contains(binary))
        }

        async fn install(&self, package: &str, _: CancellationToken) -> Result<CommandOutcome> {
            self.installed.lock().unwrap().push(package.into());
            self.available.lock().unwrap().insert(package.into());
            Ok(CommandOutcome {
                program: "pkg".into(),
                args: vec!["install".into(), package.into()],
                cwd: "/termux/home".into(),
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

    #[tokio::test]
    async fn installable_skill_dependency_is_resolved_before_full_use() {
        let capabilities = Arc::new(CapabilityRegistry::from_environment(&skill_environment()));
        let packages = Arc::new(FakeSkillPackages {
            available: Mutex::new(BTreeSet::new()),
            installed: Mutex::new(Vec::new()),
        });
        let resolver = Arc::new(DependencyResolver::new(
            capabilities.clone(),
            packages.clone(),
            None,
        ));
        let directory = tempfile::tempdir().unwrap();
        let workspace = Arc::new(IdentityWorkspace::new(directory.path()));
        workspace.bootstrap().unwrap();
        let files = FilesystemSkills::with_runtime(
            workspace,
            Arc::new(SkillStore::new(Arc::new(Storage::open_memory().unwrap()))),
            capabilities.clone(),
            Some(resolver),
        );
        let status = files
            .resolve_dependencies(&installable_skill(), None, CancellationToken::new(), None)
            .await
            .unwrap();
        assert_eq!(status, SkillEligibility::Eligible);
        assert_eq!(&*packages.installed.lock().unwrap(), &["ffmpeg"]);
        assert_eq!(
            capabilities.resolve("ffmpeg").status,
            CapabilityStatus::Available
        );
    }

    fn skill_environment() -> RuntimeEnvironment {
        RuntimeEnvironment {
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
            data_root: "/xiao".into(),
            workspace_writable: true,
            binaries: BTreeMap::from([("ffmpeg".into(), None)]),
            execution_backends: vec![ExecutionBackend::Termux],
            probed_at: "now".into(),
        }
    }

    fn installable_skill() -> SkillDocument {
        parse_skill(
            "/skills/media/SKILL.md".into(),
            "---\nname: media\ndescription: Media\nmetadata:\n  xiao:\n    requires:\n      bins: [ffmpeg]\n---\nBody",
        )
        .unwrap()
    }
}
