use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

const MAX_DOCUMENT_CHARS: usize = 256_000;

const SOUL_TEMPLATE: &str = include_str!("templates/SOUL.md");
const USER_TEMPLATE: &str = include_str!("templates/USER.md");
const MEMORY_TEMPLATE: &str = include_str!("templates/MEMORY.md");
const AGENTS_TEMPLATE: &str = include_str!("templates/AGENTS.md");
const ENVIRONMENT_TEMPLATE: &str = include_str!("templates/ENVIRONMENT.md");

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceDocument {
    Soul,
    User,
    Memory,
    Environment,
    Agents,
}

impl WorkspaceDocument {
    pub fn filename(self) -> &'static str {
        match self {
            Self::Soul => "SOUL.md",
            Self::User => "USER.md",
            Self::Memory => "MEMORY.md",
            Self::Environment => "ENVIRONMENT.md",
            Self::Agents => "AGENTS.md",
        }
    }

    fn template(self) -> &'static str {
        match self {
            Self::Soul => SOUL_TEMPLATE,
            Self::User => USER_TEMPLATE,
            Self::Memory => MEMORY_TEMPLATE,
            Self::Environment => ENVIRONMENT_TEMPLATE,
            Self::Agents => AGENTS_TEMPLATE,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedEntry {
    pub document: WorkspaceDocument,
    pub section: String,
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub soul: String,
    pub user: String,
    pub memory: String,
    pub environment: String,
    pub agents: String,
}

/// Owner-inspectable durable identity and knowledge files. Bootstrap is
/// create-only: an existing owner-edited file is never replaced by defaults.
#[derive(Debug, Clone)]
pub struct IdentityWorkspace {
    root: PathBuf,
}

impl IdentityWorkspace {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    pub fn skill_path(&self, name: &str) -> Result<PathBuf> {
        let name = safe_skill_directory(name)?;
        Ok(self.skills_dir().join(name).join("SKILL.md"))
    }

    pub fn discover_skill_files(&self) -> Result<Vec<PathBuf>> {
        create_private_dir(&self.skills_dir())?;
        let mut files = Vec::new();
        for entry in fs::read_dir(self.skills_dir())? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let path = entry.path().join("SKILL.md");
            if path.is_file() && !fs::symlink_metadata(&path)?.file_type().is_symlink() {
                files.push(path);
            }
        }
        files.sort();
        Ok(files)
    }

    pub fn write_skill_atomic(&self, name: &str, content: &str) -> Result<PathBuf> {
        validate_document(content)?;
        let target = self.skill_path(name)?;
        let directory = target
            .parent()
            .ok_or_else(|| anyhow!("skill path has no parent"))?;
        create_private_dir(directory)?;
        write_path_atomic(&target, content)?;
        Ok(target)
    }

    pub fn path(&self, document: WorkspaceDocument) -> PathBuf {
        self.root.join(document.filename())
    }

    pub fn bootstrap(&self) -> Result<()> {
        create_private_dir(&self.root)?;
        create_private_dir(&self.skills_dir())?;
        for document in [
            WorkspaceDocument::Soul,
            WorkspaceDocument::User,
            WorkspaceDocument::Memory,
            WorkspaceDocument::Agents,
            WorkspaceDocument::Environment,
        ] {
            self.create_if_missing(document, document.template())?;
        }
        Ok(())
    }

    pub fn load(&self) -> Result<WorkspaceSnapshot> {
        Ok(WorkspaceSnapshot {
            soul: self.read(WorkspaceDocument::Soul)?,
            user: self.read(WorkspaceDocument::User)?,
            memory: self.read(WorkspaceDocument::Memory)?,
            environment: self.read(WorkspaceDocument::Environment)?,
            agents: self.read(WorkspaceDocument::Agents)?,
        })
    }

    pub fn read(&self, document: WorkspaceDocument) -> Result<String> {
        let path = self.path(document);
        let content = fs::read_to_string(&path)
            .with_context(|| format!("read workspace document {}", path.display()))?;
        validate_document(&content)?;
        Ok(content)
    }

    /// Generated environment state is the only identity document routinely
    /// replaced wholesale by the runtime.
    pub fn write_environment(&self, content: &str) -> Result<()> {
        self.write_atomic(WorkspaceDocument::Environment, content)
    }

    /// SOUL changes require an explicit owner-approved call site. Ordinary
    /// memory/learning paths intentionally have no access to this method.
    pub fn write_soul_owner_approved(&self, content: &str) -> Result<()> {
        self.write_atomic(WorkspaceDocument::Soul, content)
    }

    pub fn managed_entries(&self, document: WorkspaceDocument) -> Result<Vec<ManagedEntry>> {
        if !matches!(
            document,
            WorkspaceDocument::User | WorkspaceDocument::Memory
        ) {
            return Err(anyhow!(
                "managed entries exist only in USER.md or MEMORY.md"
            ));
        }
        Ok(parse_managed_entries(document, &self.read(document)?))
    }

    pub fn upsert_managed(
        &self,
        document: WorkspaceDocument,
        section: &str,
        key: &str,
        value: &str,
    ) -> Result<bool> {
        ensure_memory_document(document)?;
        let section = normalized_section(section)?;
        let key = canonical_key(key)?;
        let value = value.trim();
        if value.is_empty() || value.chars().count() > 8_192 {
            return Err(anyhow!("managed entry value is empty or too long"));
        }

        let original = self.read(document)?;
        let mut lines = original.lines().map(str::to_owned).collect::<Vec<_>>();
        let entry_line = format!("- [{key}] {value}");
        let mut matching = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            if managed_key(line).is_some_and(|candidate| candidate == key) {
                matching.push(index);
            }
        }
        let changed = match matching.first().copied() {
            Some(first) => {
                let changed = lines[first] != entry_line || matching.len() > 1;
                lines[first] = entry_line;
                for index in matching.into_iter().skip(1).rev() {
                    lines.remove(index);
                }
                changed
            }
            None => {
                insert_under_section(&mut lines, &section, entry_line);
                true
            }
        };
        if changed {
            let mut updated = lines.join("\n");
            updated.push('\n');
            self.write_atomic(document, &updated)?;
        }
        Ok(changed)
    }

    pub fn delete_managed(&self, document: WorkspaceDocument, key: &str) -> Result<bool> {
        ensure_memory_document(document)?;
        let key = canonical_key(key)?;
        let original = self.read(document)?;
        let mut deleted = false;
        let lines = original
            .lines()
            .filter(|line| {
                let matches = managed_key(line).is_some_and(|candidate| candidate == key);
                deleted |= matches;
                !matches
            })
            .collect::<Vec<_>>();
        if deleted {
            let mut updated = lines.join("\n");
            updated.push('\n');
            self.write_atomic(document, &updated)?;
        }
        Ok(deleted)
    }

    fn create_if_missing(&self, document: WorkspaceDocument, content: &str) -> Result<()> {
        validate_document(content)?;
        let path = self.path(document);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        match options.open(&path) {
            Ok(mut file) => {
                file.write_all(content.as_bytes())?;
                file.sync_all()?;
                set_private_file(&path)?;
                sync_parent(&path)?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("create workspace document {}", path.display()))
            }
        }
    }

    fn write_atomic(&self, document: WorkspaceDocument, content: &str) -> Result<()> {
        validate_document(content)?;
        create_private_dir(&self.root)?;
        let target = self.path(document);
        let temporary = self.root.join(format!(
            ".{}.{}.tmp",
            document.filename(),
            uuid::Uuid::new_v4()
        ));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            set_private_file(&temporary)?;
            fs::rename(&temporary, &target)?;
            set_private_file(&target)?;
            sync_parent(&target)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.with_context(|| format!("atomically write {}", target.display()))
    }
}

fn parse_managed_entries(document: WorkspaceDocument, content: &str) -> Vec<ManagedEntry> {
    let mut section = String::from("Other");
    content
        .lines()
        .filter_map(|line| {
            if let Some(title) = line.strip_prefix("## ") {
                section = title.trim().to_owned();
                return None;
            }
            let key = managed_key(line)?;
            let closing = line.find(']')?;
            let value = line[closing + 1..].trim().to_owned();
            (!value.is_empty()).then(|| ManagedEntry {
                document,
                section: section.clone(),
                key,
                value,
            })
        })
        .collect()
}

fn managed_key(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("- [")?;
    let (key, _) = rest.split_once(']')?;
    let key = canonical_key(key).ok()?;
    Some(key)
}

fn insert_under_section(lines: &mut Vec<String>, section: &str, entry: String) {
    let heading = format!("## {section}");
    if let Some(start) = lines.iter().position(|line| line.trim() == heading) {
        let end = lines[start + 1..]
            .iter()
            .position(|line| line.starts_with("## "))
            .map(|relative| start + 1 + relative)
            .unwrap_or(lines.len());
        let insert = lines[start + 1..end]
            .iter()
            .rposition(|line| managed_key(line).is_some())
            .map(|relative| start + 2 + relative)
            .unwrap_or(start + 1);
        lines.insert(insert, entry);
    } else {
        if lines.last().is_some_and(|line| !line.is_empty()) {
            lines.push(String::new());
        }
        lines.push(heading);
        lines.push(entry);
    }
}

fn normalized_section(value: &str) -> Result<String> {
    let value = value.trim().trim_start_matches('#').trim();
    if value.is_empty() || value.chars().count() > 120 || value.contains(['\r', '\n']) {
        return Err(anyhow!("invalid managed entry section"));
    }
    Ok(value.to_owned())
}

pub fn canonical_key(value: &str) -> Result<String> {
    let mut output = String::new();
    let mut separator = false;
    for character in value.trim().to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !output.is_empty() {
                output.push('_');
            }
            output.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    let output = output.trim_matches('_').to_owned();
    if output.is_empty() || output.chars().count() > 120 {
        return Err(anyhow!("invalid managed entry key"));
    }
    Ok(output)
}

fn ensure_memory_document(document: WorkspaceDocument) -> Result<()> {
    if matches!(
        document,
        WorkspaceDocument::User | WorkspaceDocument::Memory
    ) {
        Ok(())
    } else {
        Err(anyhow!(
            "only USER.md and MEMORY.md support managed entries"
        ))
    }
}

fn validate_document(content: &str) -> Result<()> {
    if content.contains('\0') || content.chars().count() > MAX_DOCUMENT_CHARS {
        return Err(anyhow!("workspace document is invalid or too large"));
    }
    Ok(())
}

fn sync_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn write_path_atomic(target: &Path, content: &str) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("atomic target has no parent"))?;
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("atomic target filename is invalid"))?;
    let temporary = parent.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        set_private_file(&temporary)?;
        fs::rename(&temporary, target)?;
        set_private_file(target)?;
        sync_parent(target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.with_context(|| format!("atomically write {}", target.display()))
}

fn safe_skill_directory(value: &str) -> Result<String> {
    let value = value.trim();
    let valid = !value.is_empty()
        && value.len() <= 120
        && !value.starts_with(['-', '.'])
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        });
    if valid {
        Ok(value.to_owned())
    } else {
        Err(anyhow!("invalid skill directory name"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_bootstrap_survives_restart_and_never_overwrites_owner_files() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = IdentityWorkspace::new(directory.path());
        workspace.bootstrap().unwrap();
        let first = workspace.load().unwrap();
        assert!(first.soul.contains("You are Xiao"));
        assert!(first.user.contains("single owner"));

        let owner_soul = first.soul.replace("Be direct", "Be calm and direct");
        workspace.write_soul_owner_approved(&owner_soul).unwrap();
        fs::write(
            workspace.path(WorkspaceDocument::User),
            "# USER\n\n## Identity\n- [preferred_name] Tsaqib\n",
        )
        .unwrap();

        let restarted = IdentityWorkspace::new(directory.path());
        restarted.bootstrap().unwrap();
        let after = restarted.load().unwrap();
        assert_eq!(after.soul, owner_soul);
        assert!(after.user.contains("Tsaqib"));
        assert!(restarted.skills_dir().is_dir());
    }

    #[test]
    fn managed_entries_replace_duplicates_and_manual_edits_are_reindexed() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = IdentityWorkspace::new(directory.path());
        workspace.bootstrap().unwrap();
        workspace
            .upsert_managed(
                WorkspaceDocument::User,
                "Communication Preferences",
                "response-style",
                "Concise answers",
            )
            .unwrap();
        workspace
            .upsert_managed(
                WorkspaceDocument::User,
                "Communication Preferences",
                "response_style",
                "Detailed for technical topics",
            )
            .unwrap();
        let entries = workspace.managed_entries(WorkspaceDocument::User).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "response_style");
        assert!(entries[0].value.contains("Detailed"));
        assert!(workspace
            .delete_managed(WorkspaceDocument::User, "response style")
            .unwrap());
        assert!(workspace
            .managed_entries(WorkspaceDocument::User)
            .unwrap()
            .is_empty());
    }
}
