use std::{collections::BTreeSet, sync::Arc};

use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    identity::{IdentityWorkspace, WorkspaceDocument},
    security::redact::contains_secret_material,
    storage::Storage,
};

const MAX_CATEGORY_CHARS: usize = 80;
const MAX_KEY_CHARS: usize = 120;
const MAX_VALUE_CHARS: usize = 8_192;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    User,
    Agent,
}

impl MemoryScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
        }
    }
}

impl TryFrom<&str> for MemoryScope {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "user" => Ok(Self::User),
            "agent" => Ok(Self::Agent),
            _ => Err(anyhow!("memory scope must be user or agent")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryRecord {
    pub id: String,
    pub owner_principal: String,
    pub scope: MemoryScope,
    pub category: String,
    pub key: String,
    pub value: String,
    pub confidence: f64,
    pub source_kind: String,
    pub source_session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryHistoryRecord {
    pub memory_id: Option<String>,
    pub owner_principal: String,
    pub scope: String,
    pub category: String,
    pub key: String,
    pub action: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub source_session_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryUpsert {
    Created,
    Updated,
    Unchanged,
}

#[derive(Clone)]
pub struct MemoryStore {
    storage: Arc<Storage>,
    workspace: Option<Arc<IdentityWorkspace>>,
}

impl MemoryStore {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self {
            storage,
            workspace: None,
        }
    }

    pub fn with_workspace(storage: Arc<Storage>, workspace: Arc<IdentityWorkspace>) -> Self {
        Self {
            storage,
            workspace: Some(workspace),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upsert(
        &self,
        owner: &str,
        scope: MemoryScope,
        category: &str,
        key: &str,
        value: &str,
        confidence: f64,
        source_kind: &str,
        source_session_id: Option<&str>,
    ) -> Result<(MemoryUpsert, MemoryRecord)> {
        validate_owner(owner)?;
        let category = canonical_category(category);
        let key = canonical_key(&category, key);
        let value = value.trim();
        validate_component("memory category", &category, MAX_CATEGORY_CHARS)?;
        validate_component("memory key", &key, MAX_KEY_CHARS)?;
        validate_component("memory value", value, MAX_VALUE_CHARS)?;
        validate_component("memory source kind", source_kind, 80)?;
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(anyhow!("memory confidence must be between 0 and 1"));
        }
        if contains_secret_material(value) || sensitive_identity(&category, &key) {
            return Err(anyhow!("refusing to persist secret material as memory"));
        }

        if let Some(workspace) = &self.workspace {
            workspace.upsert_managed(
                document_for_scope(scope),
                &section_for(scope, &category),
                &key,
                value,
            )?;
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let outcome = self.storage.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let existing = transaction
                .query_row(
                    "SELECT id,value,confidence,source_kind,source_session_id,created_at,updated_at FROM memories WHERE owner_principal=? AND scope=? AND category=? AND key=?",
                    params![owner, scope.as_str(), category, key],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, f64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                        ))
                    },
                )
                .optional()?;

            let (outcome, memory_id, created_at, updated_at) = match existing {
                Some((memory_id, old_value, old_confidence, old_source, old_session, created_at, old_updated_at))
                    if old_value == value
                        && old_confidence == confidence
                        && old_source == source_kind
                        && old_session.as_deref() == source_session_id =>
                {
                    (MemoryUpsert::Unchanged, memory_id, created_at, old_updated_at)
                }
                Some((memory_id, old_value, _, _, _, created_at, _)) => {
                    transaction.execute(
                        "UPDATE memories SET value=?,confidence=?,source_kind=?,source_session_id=?,updated_at=? WHERE id=?",
                        params![value, confidence, source_kind, source_session_id, now, memory_id],
                    )?;
                    transaction.execute(
                        "INSERT INTO memory_history(memory_id,owner_principal,scope,category,key,action,old_value,new_value,source_session_id,created_at) VALUES(?,?,?,?,?,'update',?,?,?,?)",
                        params![memory_id, owner, scope.as_str(), category, key, old_value, value, source_session_id, now],
                    )?;
                    (MemoryUpsert::Updated, memory_id, created_at, now.clone())
                }
                None => {
                    transaction.execute(
                        "INSERT INTO memories(id,owner_principal,scope,category,key,value,confidence,source_kind,source_session_id,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                        params![id, owner, scope.as_str(), category, key, value, confidence, source_kind, source_session_id, now, now],
                    )?;
                    transaction.execute(
                        "INSERT INTO memory_history(memory_id,owner_principal,scope,category,key,action,old_value,new_value,source_session_id,created_at) VALUES(?,?,?,?,?,'create',NULL,?,?,?)",
                        params![id, owner, scope.as_str(), category, key, value, source_session_id, now],
                    )?;
                    (MemoryUpsert::Created, id, now.clone(), now.clone())
                }
            };
            transaction.commit()?;
            Ok((outcome, memory_id, created_at, updated_at))
        })?;

        let record = self
            .get(owner, scope, &category, &key)?
            .ok_or_else(|| anyhow!("upserted memory is missing"))?;
        debug_assert_eq!(record.id, outcome.1);
        debug_assert_eq!(record.created_at, outcome.2);
        debug_assert_eq!(record.updated_at, outcome.3);
        Ok((outcome.0, record))
    }

    pub fn get(
        &self,
        owner: &str,
        scope: MemoryScope,
        category: &str,
        key: &str,
    ) -> Result<Option<MemoryRecord>> {
        let category = canonical_category(category);
        let key = canonical_key(&category, key);
        self.storage.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT id,owner_principal,scope,category,key,value,confidence,source_kind,source_session_id,created_at,updated_at FROM memories WHERE owner_principal=? AND scope=? AND category=? AND key=?",
                    params![owner, scope.as_str(), category, key],
                    row_memory,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn delete(
        &self,
        owner: &str,
        scope: MemoryScope,
        category: &str,
        key: &str,
        source_session_id: Option<&str>,
    ) -> Result<bool> {
        let category = canonical_category(category);
        let key = canonical_key(&category, key);
        if let Some(workspace) = &self.workspace {
            workspace.delete_managed(document_for_scope(scope), &key)?;
        }
        self.storage.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let existing = transaction
                .query_row(
                    "SELECT id,value FROM memories WHERE owner_principal=? AND scope=? AND category=? AND key=?",
                    params![owner, scope.as_str(), category, key],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            let Some((memory_id, old_value)) = existing else {
                transaction.commit()?;
                return Ok(false);
            };
            transaction.execute(
                "DELETE FROM memories WHERE id=? AND owner_principal=?",
                params![memory_id, owner],
            )?;
            transaction.execute(
                "INSERT INTO memory_history(memory_id,owner_principal,scope,category,key,action,old_value,new_value,source_session_id,created_at) VALUES(?,?,?,?,?,'delete',?,NULL,?,?)",
                params![memory_id, owner, scope.as_str(), category, key, old_value, source_session_id, Utc::now().to_rfc3339()],
            )?;
            transaction.commit()?;
            Ok(true)
        })
    }

    pub fn list(
        &self,
        owner: &str,
        scope: Option<MemoryScope>,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        let limit = limit.clamp(1, 200) as i64;
        self.storage.with_conn(|connection| {
            let sql = if scope.is_some() {
                "SELECT id,owner_principal,scope,category,key,value,confidence,source_kind,source_session_id,created_at,updated_at FROM memories WHERE owner_principal=? AND scope=? ORDER BY category,key LIMIT ?"
            } else {
                "SELECT id,owner_principal,scope,category,key,value,confidence,source_kind,source_session_id,created_at,updated_at FROM memories WHERE owner_principal=? ORDER BY scope,category,key LIMIT ?"
            };
            let mut statement = connection.prepare(sql)?;
            let records = if let Some(scope) = scope {
                statement
                    .query_map(params![owner, scope.as_str(), limit], row_memory)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                statement
                    .query_map(params![owner, limit], row_memory)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            Ok(records)
        })
    }

    pub fn search(&self, owner: &str, query: &str, limit: usize) -> Result<Vec<MemoryRecord>> {
        let Some(query) = fts_query(query) else {
            return self.list(owner, None, limit);
        };
        let limit = limit.clamp(1, 50) as i64;
        self.storage.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT m.id,m.owner_principal,m.scope,m.category,m.key,m.value,m.confidence,m.source_kind,m.source_session_id,m.created_at,m.updated_at FROM memories_fts JOIN memories m ON m.rowid=memories_fts.rowid WHERE memories_fts MATCH ? AND m.owner_principal=? ORDER BY bm25(memories_fts),m.updated_at DESC LIMIT ?",
            )?;
            let rows = statement.query_map(params![query, owner, limit], row_memory)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn history(&self, owner: &str, limit: usize) -> Result<Vec<MemoryHistoryRecord>> {
        self.storage.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT memory_id,owner_principal,scope,category,key,action,old_value,new_value,source_session_id,created_at FROM memory_history WHERE owner_principal=? ORDER BY id DESC LIMIT ?",
            )?;
            let rows = statement.query_map(params![owner, limit.clamp(1, 500) as i64], |row| {
                Ok(MemoryHistoryRecord {
                    memory_id: row.get(0)?,
                    owner_principal: row.get(1)?,
                    scope: row.get(2)?,
                    category: row.get(3)?,
                    key: row.get(4)?,
                    action: row.get(5)?,
                    old_value: row.get(6)?,
                    new_value: row.get(7)?,
                    source_session_id: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Rebuild the derived SQLite active index from owner-editable files.
    /// History remains append-only and changes caused by manual edits are
    /// labeled `manual_file_reconcile`.
    pub fn reconcile(&self, owner: &str) -> Result<usize> {
        let Some(workspace) = &self.workspace else {
            return Ok(0);
        };
        validate_owner(owner)?;
        let mut changed = 0usize;
        for (scope, document) in [
            (MemoryScope::User, WorkspaceDocument::User),
            (MemoryScope::Agent, WorkspaceDocument::Memory),
        ] {
            let content = workspace.read(document)?;
            let hash = content_hash(&content);
            let path = workspace.path(document).display().to_string();
            let indexed_hash = self.storage.workspace_file_hash(&path)?;
            if indexed_hash.as_deref() == Some(&hash) {
                continue;
            }

            let existing = self.list(owner, Some(scope), 200)?;
            let mut entries = workspace.managed_entries(document)?;
            // One-time upgrade bridge for the earlier SQLite-authoritative
            // partial v0.2 implementation. Once indexed, an empty owner file
            // is authoritative and correctly deletes active state.
            if indexed_hash.is_none() && entries.is_empty() && !existing.is_empty() {
                for record in &existing {
                    workspace.upsert_managed(
                        document,
                        &section_for(scope, &record.category),
                        &record.key,
                        &record.value,
                    )?;
                }
                entries = workspace.managed_entries(document)?;
            }
            let active_keys = entries
                .iter()
                .map(|entry| entry.key.clone())
                .collect::<BTreeSet<_>>();
            for entry in entries {
                let matching = existing.iter().find(|record| record.key == entry.key);
                if matching.is_some_and(|record| record.value == entry.value) {
                    continue;
                }
                let category = matching
                    .map(|record| record.category.clone())
                    .unwrap_or_else(|| category_for_section(scope, &entry.section));
                let (outcome, _) = self.upsert(
                    owner,
                    scope,
                    &category,
                    &entry.key,
                    &entry.value,
                    1.0,
                    "manual_file_reconcile",
                    None,
                )?;
                changed += usize::from(outcome != MemoryUpsert::Unchanged);
            }
            for record in existing {
                if !active_keys.contains(&record.key)
                    && self.delete(owner, scope, &record.category, &record.key, None)?
                {
                    changed += 1;
                }
            }
            let final_hash = content_hash(&workspace.read(document)?);
            self.storage
                .set_workspace_file_hash(&path, document.filename(), &final_hash)?;
        }
        Ok(changed)
    }
}

fn document_for_scope(scope: MemoryScope) -> WorkspaceDocument {
    match scope {
        MemoryScope::User => WorkspaceDocument::User,
        MemoryScope::Agent => WorkspaceDocument::Memory,
    }
}

fn section_for(scope: MemoryScope, category: &str) -> String {
    match (scope, category) {
        (MemoryScope::User, "preference") => "Communication Preferences".into(),
        (MemoryScope::User, "profile") => "Identity".into(),
        (MemoryScope::User, "constraint") => "Constraints".into(),
        (MemoryScope::Agent, "fact") => "Durable Facts".into(),
        (MemoryScope::Agent, "lesson") => "Lessons".into(),
        _ => title(category),
    }
}

fn category_for_section(scope: MemoryScope, section: &str) -> String {
    let normalized = canonical_category(section);
    match scope {
        MemoryScope::User if normalized.contains("preference") => "preference".into(),
        MemoryScope::User if normalized.contains("identity") => "profile".into(),
        MemoryScope::User if normalized.contains("constraint") => "constraint".into(),
        MemoryScope::Agent if normalized.contains("fact") => "fact".into(),
        MemoryScope::Agent if normalized.contains("lesson") => "lesson".into(),
        _ => normalized,
    }
}

fn title(value: &str) -> String {
    value
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut characters = word.chars();
            characters
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + characters.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn content_hash(content: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

fn row_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRecord> {
    let scope = match row.get::<_, String>(2)?.as_str() {
        "user" => MemoryScope::User,
        "agent" => MemoryScope::Agent,
        invalid => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid memory scope {invalid}"),
                )
                .into(),
            ))
        }
    };
    Ok(MemoryRecord {
        id: row.get(0)?,
        owner_principal: row.get(1)?,
        scope,
        category: row.get(3)?,
        key: row.get(4)?,
        value: row.get(5)?,
        confidence: row.get(6)?,
        source_kind: row.get(7)?,
        source_session_id: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

pub fn canonical_category(value: &str) -> String {
    canonical_component(value)
}

pub fn canonical_key(category: &str, value: &str) -> String {
    let component = canonical_component(value);
    match (canonical_component(category).as_str(), component.as_str()) {
        ("preference", "answer_style" | "response_length" | "verbosity" | "detail_level") => {
            "response_style".into()
        }
        ("preference", "coding_language" | "language_programming" | "preferred_language") => {
            "programming_language".into()
        }
        _ => component,
    }
}

fn canonical_component(value: &str) -> String {
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
    output.trim_matches('_').to_owned()
}

fn validate_owner(owner: &str) -> Result<()> {
    validate_component("memory owner", owner, 512)
}

fn sensitive_identity(category: &str, key: &str) -> bool {
    let identity = format!("{category}_{key}");
    [
        "credential",
        "password",
        "passcode",
        "api_key",
        "access_token",
        "refresh_token",
        "private_key",
        "client_secret",
        "bot_token",
    ]
    .iter()
    .any(|marker| identity.contains(marker))
}

fn validate_component(label: &str, value: &str, max_chars: usize) -> Result<()> {
    let count = value.chars().count();
    if value.trim().is_empty() || count > max_chars {
        return Err(anyhow!(
            "{label} is empty or exceeds {max_chars} characters"
        ));
    }
    Ok(())
}

pub(crate) fn fts_query(value: &str) -> Option<String> {
    let tokens = value
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| token.chars().count() >= 2)
        .take(12)
        .map(|token| format!("\"{}\"*", token.replace('"', "")))
        .collect::<Vec<_>>();
    (!tokens.is_empty()).then(|| tokens.join(" OR "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> MemoryStore {
        MemoryStore::new(Arc::new(Storage::open_memory().unwrap()))
    }

    #[test]
    fn create_upsert_alias_and_delete_keep_one_active_state_with_history() {
        let store = store();
        let first = store
            .upsert(
                "p",
                MemoryScope::User,
                "preference",
                "answer-style",
                "concise",
                1.0,
                "explicit_user",
                Some("s"),
            )
            .unwrap();
        assert_eq!(first.0, MemoryUpsert::Created);
        let second = store
            .upsert(
                "p",
                MemoryScope::User,
                "preference",
                "verbosity",
                "detailed",
                1.0,
                "explicit_user",
                Some("s"),
            )
            .unwrap();
        assert_eq!(second.0, MemoryUpsert::Updated);
        assert_eq!(first.1.id, second.1.id);
        assert_eq!(store.list("p", None, 10).unwrap().len(), 1);
        assert_eq!(store.history("p", 10).unwrap().len(), 2);
        assert!(store
            .delete(
                "p",
                MemoryScope::User,
                "preference",
                "response_length",
                Some("s")
            )
            .unwrap());
        assert!(store.list("p", None, 10).unwrap().is_empty());
        assert_eq!(store.history("p", 10).unwrap()[0].action, "delete");
    }

    #[test]
    fn search_and_mutation_are_principal_isolated() {
        let store = store();
        store
            .upsert(
                "alice",
                MemoryScope::Agent,
                "project_xiao",
                "language",
                "Rust",
                0.9,
                "implicit_evaluator",
                None,
            )
            .unwrap();
        assert_eq!(store.search("alice", "Rust", 10).unwrap().len(), 1);
        assert!(store.search("bob", "Rust", 10).unwrap().is_empty());
        assert!(!store
            .delete("bob", MemoryScope::Agent, "project_xiao", "language", None)
            .unwrap());
        assert_eq!(store.list("alice", None, 10).unwrap().len(), 1);
    }

    #[test]
    fn secrets_are_never_persisted_as_memory() {
        let store = store();
        let error = store
            .upsert(
                "p",
                MemoryScope::User,
                "credential",
                "service",
                "API key is do-not-save",
                1.0,
                "explicit_user",
                None,
            )
            .unwrap_err();
        assert!(error.to_string().contains("secret"));
        assert!(store.list("p", None, 10).unwrap().is_empty());
        assert!(store
            .upsert(
                "p",
                MemoryScope::User,
                "credential",
                "service_token",
                "opaque-value-without-a-label",
                1.0,
                "explicit_user",
                None,
            )
            .is_err());
    }
}
