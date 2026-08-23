use std::{collections::BTreeSet, sync::Arc};

use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{memory::fts_query, security::redact::contains_secret_material, storage::Storage};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillCandidate {
    pub name: String,
    pub summary: String,
    pub when_to_use: String,
    #[serde(default)]
    pub prerequisites: String,
    pub procedure: String,
    #[serde(default)]
    pub pitfalls: String,
    #[serde(default)]
    pub verification: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillRecord {
    pub id: String,
    pub owner_principal: String,
    pub name: String,
    pub summary: String,
    pub when_to_use: String,
    pub prerequisites: String,
    pub procedure: String,
    pub pitfalls: String,
    pub verification: String,
    pub source_kind: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillHistoryRecord {
    pub skill_id: Option<String>,
    pub owner_principal: String,
    pub action: String,
    pub old_content_json: Option<String>,
    pub new_content_json: Option<String>,
    pub source_session_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillMutation {
    Created,
    Updated,
    Unchanged,
}

#[derive(Clone)]
pub struct SkillStore {
    storage: Arc<Storage>,
}

impl SkillStore {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub(crate) fn storage(&self) -> Arc<Storage> {
        self.storage.clone()
    }

    pub fn create_or_update(
        &self,
        owner: &str,
        candidate: SkillCandidate,
        source_session_id: Option<&str>,
    ) -> Result<(SkillMutation, SkillRecord)> {
        validate_owner(owner)?;
        let mut candidate = normalize_candidate(candidate)?;
        let related = self.find_related(owner, &candidate)?;
        if let Some(existing) = related {
            candidate.name = existing.name.clone();
            candidate.summary = merge_sentence(&existing.summary, &candidate.summary, 2_000);
            candidate.when_to_use =
                merge_sentence(&existing.when_to_use, &candidate.when_to_use, 3_000);
            candidate.prerequisites =
                merge_lines(&existing.prerequisites, &candidate.prerequisites, 6_000);
            candidate.procedure = merge_lines(&existing.procedure, &candidate.procedure, 12_000);
            candidate.pitfalls = merge_lines(&existing.pitfalls, &candidate.pitfalls, 6_000);
            candidate.verification =
                merge_lines(&existing.verification, &candidate.verification, 6_000);
            if existing.summary == candidate.summary
                && existing.when_to_use == candidate.when_to_use
                && existing.prerequisites == candidate.prerequisites
                && existing.procedure == candidate.procedure
                && existing.pitfalls == candidate.pitfalls
                && existing.verification == candidate.verification
            {
                return Ok((SkillMutation::Unchanged, existing));
            }
            let old_json = serde_json::to_string(&existing)?;
            let now = Utc::now().to_rfc3339();
            self.storage.with_conn(|connection| {
                let transaction = connection.transaction()?;
                let changed = transaction.execute(
                    "UPDATE skills SET summary=?,when_to_use=?,prerequisites=?,procedure=?,pitfalls=?,verification=?,updated_at=? WHERE id=? AND owner_principal=?",
                    params![candidate.summary, candidate.when_to_use, candidate.prerequisites, candidate.procedure, candidate.pitfalls, candidate.verification, now, existing.id, owner],
                )?;
                if changed != 1 {
                    return Err(anyhow!("skill not found for principal"));
                }
                let updated = SkillRecord {
                    id: existing.id.clone(),
                    owner_principal: owner.to_owned(),
                    name: existing.name.clone(),
                    summary: candidate.summary.clone(),
                    when_to_use: candidate.when_to_use.clone(),
                    prerequisites: candidate.prerequisites.clone(),
                    procedure: candidate.procedure.clone(),
                    pitfalls: candidate.pitfalls.clone(),
                    verification: candidate.verification.clone(),
                    source_kind: existing.source_kind.clone(),
                    enabled: existing.enabled,
                    created_at: existing.created_at.clone(),
                    updated_at: now.clone(),
                };
                transaction.execute(
                    "INSERT INTO skill_history(skill_id,owner_principal,action,old_content_json,new_content_json,source_session_id,created_at) VALUES(?,?,'update',?,?,?,?)",
                    params![existing.id, owner, old_json, serde_json::to_string(&updated)?, source_session_id, now],
                )?;
                transaction.commit()?;
                Ok(())
            })?;
            let record = self
                .view(owner, &existing.id)?
                .ok_or_else(|| anyhow!("updated skill is missing"))?;
            return Ok((SkillMutation::Updated, record));
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let record = SkillRecord {
            id: id.clone(),
            owner_principal: owner.to_owned(),
            name: candidate.name,
            summary: candidate.summary,
            when_to_use: candidate.when_to_use,
            prerequisites: candidate.prerequisites,
            procedure: candidate.procedure,
            pitfalls: candidate.pitfalls,
            verification: candidate.verification,
            source_kind: if source_session_id.is_some() {
                "learned".into()
            } else {
                "imported".into()
            },
            enabled: true,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        self.storage.with_conn(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO skills(id,owner_principal,name,summary,when_to_use,prerequisites,procedure,pitfalls,verification,source_kind,enabled,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params![record.id, record.owner_principal, record.name, record.summary, record.when_to_use, record.prerequisites, record.procedure, record.pitfalls, record.verification, record.source_kind, record.enabled as i32, record.created_at, record.updated_at],
            )?;
            transaction.execute(
                "INSERT INTO skill_history(skill_id,owner_principal,action,old_content_json,new_content_json,source_session_id,created_at) VALUES(?,?,'create',NULL,?,?,?)",
                params![record.id, owner, serde_json::to_string(&record)?, source_session_id, now],
            )?;
            transaction.commit()?;
            Ok(())
        })?;
        Ok((SkillMutation::Created, record))
    }

    pub fn view(&self, owner: &str, name_or_id: &str) -> Result<Option<SkillRecord>> {
        let canonical = canonical_skill_name(name_or_id);
        self.storage.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT id,owner_principal,name,summary,when_to_use,prerequisites,procedure,pitfalls,verification,source_kind,enabled,created_at,updated_at FROM skills WHERE owner_principal=? AND (id=? OR name=?)",
                    params![owner, name_or_id, canonical],
                    row_skill,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn list(&self, owner: &str, limit: usize) -> Result<Vec<SkillRecord>> {
        self.storage.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,owner_principal,name,summary,when_to_use,prerequisites,procedure,pitfalls,verification,source_kind,enabled,created_at,updated_at FROM skills WHERE owner_principal=? AND enabled=1 ORDER BY updated_at DESC,name LIMIT ?",
            )?;
            let rows = statement.query_map(params![owner, limit.clamp(1, 500) as i64], row_skill)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn search(&self, owner: &str, query: &str, limit: usize) -> Result<Vec<SkillRecord>> {
        let Some(query) = fts_query(query) else {
            return Ok(Vec::new());
        };
        self.storage.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT s.id,s.owner_principal,s.name,s.summary,s.when_to_use,s.prerequisites,s.procedure,s.pitfalls,s.verification,s.source_kind,s.enabled,s.created_at,s.updated_at FROM skills_fts JOIN skills s ON s.rowid=skills_fts.rowid WHERE skills_fts MATCH ? AND s.owner_principal=? AND s.enabled=1 ORDER BY bm25(skills_fts),s.updated_at DESC LIMIT ?",
            )?;
            let rows = statement.query_map(
                params![query, owner, limit.clamp(1, 20) as i64],
                row_skill,
            )?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn history(&self, owner: &str, limit: usize) -> Result<Vec<SkillHistoryRecord>> {
        self.storage.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT skill_id,owner_principal,action,old_content_json,new_content_json,source_session_id,created_at FROM skill_history WHERE owner_principal=? ORDER BY id DESC LIMIT ?",
            )?;
            let rows = statement.query_map(params![owner, limit.clamp(1, 500) as i64], |row| {
                Ok(SkillHistoryRecord {
                    skill_id: row.get(0)?,
                    owner_principal: row.get(1)?,
                    action: row.get(2)?,
                    old_content_json: row.get(3)?,
                    new_content_json: row.get(4)?,
                    source_session_id: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn list_all(&self, owner: &str, limit: usize) -> Result<Vec<SkillRecord>> {
        self.storage.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,owner_principal,name,summary,when_to_use,prerequisites,procedure,pitfalls,verification,source_kind,enabled,created_at,updated_at FROM skills WHERE owner_principal=? ORDER BY updated_at DESC,name LIMIT ?",
            )?;
            let rows = statement.query_map(params![owner, limit.clamp(1, 500) as i64], row_skill)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn set_enabled(&self, owner: &str, id: &str, enabled: bool) -> Result<SkillRecord> {
        let changed = self.storage.with_conn(|connection| {
            Ok(connection.execute(
                "UPDATE skills SET enabled=?,updated_at=? WHERE id=? AND owner_principal=?",
                params![enabled as i32, Utc::now().to_rfc3339(), id, owner],
            )?)
        })?;
        if changed != 1 {
            return Err(anyhow!("skill not found for principal"));
        }
        self.view(owner, id)?
            .ok_or_else(|| anyhow!("updated skill is missing"))
    }

    pub fn delete_learned(&self, owner: &str, id: &str) -> Result<SkillRecord> {
        let record = self
            .view(owner, id)?
            .ok_or_else(|| anyhow!("skill not found"))?;
        if record.source_kind != "learned" {
            return Err(anyhow!("only learned owner-created skills can be deleted"));
        }
        let now = Utc::now().to_rfc3339();
        self.storage.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let deleted = transaction.execute(
                "DELETE FROM skills WHERE id=? AND owner_principal=?",
                params![id, owner],
            )?;
            if deleted != 1 {
                return Err(anyhow!("skill not found for principal"));
            }
            transaction.execute(
                "INSERT INTO skill_history(skill_id,owner_principal,action,old_content_json,new_content_json,source_session_id,created_at) VALUES(?,?,'delete',?,NULL,NULL,?)",
                params![id, owner, serde_json::to_string(&record)?, now],
            )?;
            transaction.commit()?;
            Ok(())
        })?;
        Ok(record)
    }

    fn find_related(&self, owner: &str, candidate: &SkillCandidate) -> Result<Option<SkillRecord>> {
        let candidate_intent = intent_tokens(&format!("{} {}", candidate.name, candidate.summary));
        let mut best: Option<(f64, SkillRecord)> = None;
        for skill in self.list_all(owner, 500)? {
            if skill.name == candidate.name {
                return Ok(Some(skill));
            }
            let skill_intent = intent_tokens(&format!("{} {}", skill.name, skill.summary));
            let score = similarity(&candidate_intent, &skill_intent);
            if score >= 0.55
                && candidate_intent.intersection(&skill_intent).count() >= 2
                && best.as_ref().is_none_or(|(current, _)| score > *current)
            {
                best = Some((score, skill));
            }
        }
        Ok(best.map(|(_, skill)| skill))
    }
}

fn row_skill(row: &rusqlite::Row<'_>) -> rusqlite::Result<SkillRecord> {
    Ok(SkillRecord {
        id: row.get(0)?,
        owner_principal: row.get(1)?,
        name: row.get(2)?,
        summary: row.get(3)?,
        when_to_use: row.get(4)?,
        prerequisites: row.get(5)?,
        procedure: row.get(6)?,
        pitfalls: row.get(7)?,
        verification: row.get(8)?,
        source_kind: row.get(9)?,
        enabled: row.get::<_, i64>(10)? != 0,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn normalize_candidate(candidate: SkillCandidate) -> Result<SkillCandidate> {
    let candidate = SkillCandidate {
        name: canonical_skill_name(&candidate.name),
        summary: candidate.summary.trim().to_owned(),
        when_to_use: candidate.when_to_use.trim().to_owned(),
        prerequisites: candidate.prerequisites.trim().to_owned(),
        procedure: candidate.procedure.trim().to_owned(),
        pitfalls: candidate.pitfalls.trim().to_owned(),
        verification: candidate.verification.trim().to_owned(),
    };
    for (label, value, max) in [
        ("name", candidate.name.as_str(), 120),
        ("summary", candidate.summary.as_str(), 2_000),
        ("when_to_use", candidate.when_to_use.as_str(), 3_000),
        ("prerequisites", candidate.prerequisites.as_str(), 6_000),
        ("procedure", candidate.procedure.as_str(), 12_000),
        ("pitfalls", candidate.pitfalls.as_str(), 6_000),
        ("verification", candidate.verification.as_str(), 6_000),
    ] {
        let count = value.chars().count();
        let required = matches!(label, "name" | "summary" | "when_to_use" | "procedure");
        if (required && value.is_empty()) || count > max {
            return Err(anyhow!(
                "skill {label} is empty or exceeds {max} characters"
            ));
        }
        if contains_secret_material(value) {
            return Err(anyhow!("refusing to persist secret material in a skill"));
        }
    }
    Ok(candidate)
}

pub fn canonical_skill_name(value: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for character in value.trim().to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !output.is_empty() {
                output.push('-');
            }
            output.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    output.trim_matches('-').to_owned()
}

fn intent_tokens(value: &str) -> BTreeSet<String> {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 2)
        .filter_map(|token| {
            let token = token.to_ascii_lowercase();
            let normalized = match token.as_str() {
                "debug" | "debugging" | "fix" | "repair" | "recover" | "recovery"
                | "troubleshoot" | "troubleshooting" | "diagnosing" => "diagnose",
                "xiaod" => "xiao",
                "crash" | "crashes" | "failure" | "failed" | "daemon" => "service",
                "workflow" | "procedure" | "how" | "the" | "and" | "for" | "with" | "v2" => {
                    return None
                }
                value => value,
            };
            Some(normalized.to_owned())
        })
        .collect()
}

fn similarity(left: &BTreeSet<String>, right: &BTreeSet<String>) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let intersection = left.intersection(right).count() as f64;
    let union = left.union(right).count() as f64;
    intersection / union
}

fn merge_sentence(existing: &str, candidate: &str, max_chars: usize) -> String {
    if candidate.is_empty() || existing.contains(candidate) {
        return existing.to_owned();
    }
    if candidate.contains(existing) {
        return bound(candidate, max_chars);
    }
    bound(
        &format!("{} {}", existing.trim(), candidate.trim()),
        max_chars,
    )
}

fn merge_lines(existing: &str, candidate: &str, max_chars: usize) -> String {
    let mut lines = existing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for line in candidate
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let canonical = canonical_line(line);
        if !lines
            .iter()
            .any(|existing| canonical_line(existing) == canonical)
        {
            lines.push(line.to_owned());
        }
    }
    bound(&lines.join("\n"), max_chars)
}

fn canonical_line(value: &str) -> String {
    value
        .trim_start_matches(|character: char| {
            character.is_ascii_digit()
                || character == '.'
                || character == '-'
                || character == '*'
                || character.is_whitespace()
        })
        .trim()
        .to_ascii_lowercase()
}

fn bound(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_owned()
    } else {
        value.chars().take(max_chars).collect::<String>() + "…"
    }
}

fn validate_owner(owner: &str) -> Result<()> {
    if owner.trim().is_empty() || owner.chars().count() > 512 {
        return Err(anyhow!("skill owner is empty or too long"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(name: &str) -> SkillCandidate {
        SkillCandidate {
            name: name.into(),
            summary: "Diagnose Xiao service failures safely".into(),
            when_to_use: "When xiaod fails to start or becomes unhealthy".into(),
            prerequisites: "Access to service status and bounded logs.".into(),
            procedure: "1. Check process status.\n2. Inspect bounded recent logs.".into(),
            pitfalls: "Do not restart repeatedly without diagnosis.".into(),
            verification: "Service is healthy and the error does not recur.".into(),
        }
    }

    #[test]
    fn related_skill_updates_canonical_row_instead_of_creating_duplicate() {
        let storage = Arc::new(Storage::open_memory().unwrap());
        let store = SkillStore::new(storage);
        let first = store
            .create_or_update("p", candidate("diagnose-xiao-service"), Some("s1"))
            .unwrap();
        assert_eq!(first.0, SkillMutation::Created);
        let mut improved = candidate("fix-xiao-crash-v2");
        improved.procedure.push_str("\n3. Check file ownership.");
        improved.pitfalls.push_str("\nPreserve secrets in logs.");
        let second = store.create_or_update("p", improved, Some("s2")).unwrap();
        assert_eq!(second.0, SkillMutation::Updated);
        assert_eq!(second.1.id, first.1.id);
        assert_eq!(second.1.name, "diagnose-xiao-service");
        assert!(second.1.procedure.contains("ownership"));
        assert_eq!(store.list("p", 10).unwrap().len(), 1);
        assert_eq!(store.history("p", 10).unwrap().len(), 2);
    }

    #[test]
    fn skill_search_and_view_are_principal_scoped() {
        let store = SkillStore::new(Arc::new(Storage::open_memory().unwrap()));
        store
            .create_or_update("alice", candidate("diagnose-xiao-service"), None)
            .unwrap();
        assert_eq!(
            store
                .search("alice", "unhealthy service", 10)
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .search("bob", "unhealthy service", 10)
            .unwrap()
            .is_empty());
        assert!(store
            .view("bob", "diagnose-xiao-service")
            .unwrap()
            .is_none());
    }

    #[test]
    fn secrets_are_rejected_from_skills() {
        let store = SkillStore::new(Arc::new(Storage::open_memory().unwrap()));
        let mut secret = candidate("unsafe");
        secret.procedure = "API key=abc".into();
        assert!(store.create_or_update("p", secret, None).is_err());
        assert!(store.list("p", 10).unwrap().is_empty());
    }

    #[test]
    fn learned_and_imported_sources_support_disable_and_guarded_delete() {
        let store = SkillStore::new(Arc::new(Storage::open_memory().unwrap()));
        let learned = store
            .create_or_update("p", candidate("learned-workflow"), Some("session"))
            .unwrap()
            .1;
        let mut community = candidate("community-image-transform");
        community.summary = "Transform image colors with a community workflow".into();
        community.when_to_use = "When an imported image needs color conversion".into();
        let imported = store.create_or_update("p", community, None).unwrap().1;
        assert_eq!(learned.source_kind, "learned");
        assert_eq!(imported.source_kind, "imported");
        let disabled = store.set_enabled("p", &learned.id, false).unwrap();
        assert!(!disabled.enabled);
        assert!(store
            .list("p", 10)
            .unwrap()
            .iter()
            .all(|skill| skill.id != learned.id));
        assert!(store.delete_learned("p", &imported.id).is_err());
        store.delete_learned("p", &learned.id).unwrap();
        assert!(store.view("p", &learned.id).unwrap().is_none());
        assert!(store
            .history("p", 10)
            .unwrap()
            .iter()
            .any(|history| history.action == "delete"));
    }
}
