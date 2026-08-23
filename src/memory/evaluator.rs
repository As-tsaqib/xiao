use std::{collections::BTreeSet, sync::Arc};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{
    memory::{MemoryRecord, MemoryScope, MemoryStore, MemoryUpsert},
    security::redact::contains_secret_material,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryDecisionKind {
    None,
    Create,
    Update,
    Delete,
    Merge,
    Rekey,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryDecision {
    pub kind: MemoryDecisionKind,
    pub scope: MemoryScope,
    pub category: String,
    pub key: String,
    pub value: Option<String>,
    #[serde(default)]
    pub related_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AppliedMemoryMutation {
    Set {
        scope: String,
        category: String,
        key: String,
        value: String,
        outcome: MemoryUpsert,
    },
    Delete {
        scope: String,
        category: String,
        key: String,
        deleted: bool,
    },
}

#[derive(Clone)]
pub struct MemoryEvaluator {
    store: Arc<MemoryStore>,
}

impl MemoryEvaluator {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }

    /// Evaluate explicit owner intent against related current entries. The
    /// extractor is conservative, but general subjects are supported rather
    /// than limiting memory to a small fixed field list.
    pub fn evaluate_explicit(&self, owner: &str, prompt: &str) -> Result<Vec<MemoryDecision>> {
        if contains_secret_material(prompt) {
            return Ok(Vec::new());
        }
        self.store.reconcile(owner)?;
        let normalized = normalize(prompt);
        let existing = self.store.list(owner, None, 200)?;
        if is_forget(&normalized) {
            return Ok(delete_decisions(&normalized, &existing));
        }
        let Some(mut candidate) = extract_candidate(&normalized, true) else {
            return Ok(Vec::new());
        };
        let related = related_entries(&candidate, &existing);
        if let Some(best) = related.first() {
            candidate.key = best.key.clone();
            candidate.category = best.category.clone();
            candidate.scope = best.scope;
            if candidate
                .value
                .as_deref()
                .is_some_and(|value| semantically_equal(value, &best.value))
            {
                candidate.kind = MemoryDecisionKind::None;
                return Ok(vec![candidate]);
            }
            candidate.kind = if related.len() > 1 {
                MemoryDecisionKind::Merge
            } else {
                MemoryDecisionKind::Update
            };
            candidate.related_keys = related.iter().map(|memory| memory.key.clone()).collect();
        } else {
            candidate.kind = MemoryDecisionKind::Create;
        }
        Ok(vec![candidate])
    }

    pub fn apply_explicit(
        &self,
        owner: &str,
        session_id: &str,
        prompt: &str,
    ) -> Result<Vec<AppliedMemoryMutation>> {
        let decisions = self.evaluate_explicit(owner, prompt)?;
        self.apply_decisions(owner, Some(session_id), "explicit_user", decisions)
    }

    /// Implicit learning is restricted to declarative project/repository/
    /// workspace/device facts. Casual task requests are ignored.
    pub fn apply_implicit(
        &self,
        owner: &str,
        session_id: &str,
        prompt: &str,
    ) -> Result<Vec<AppliedMemoryMutation>> {
        if contains_secret_material(prompt) {
            return Ok(Vec::new());
        }
        self.store.reconcile(owner)?;
        let normalized = normalize(prompt);
        let Some(mut candidate) = extract_candidate(&normalized, false) else {
            return Ok(Vec::new());
        };
        if candidate.scope != MemoryScope::Agent {
            return Ok(Vec::new());
        }
        let existing = self.store.list(owner, Some(MemoryScope::Agent), 200)?;
        let related = related_entries(&candidate, &existing);
        if let Some(best) = related.first() {
            candidate.key = best.key.clone();
            candidate.category = best.category.clone();
            candidate.kind = if candidate
                .value
                .as_deref()
                .is_some_and(|value| semantically_equal(value, &best.value))
            {
                MemoryDecisionKind::None
            } else {
                MemoryDecisionKind::Update
            };
        } else {
            candidate.kind = MemoryDecisionKind::Create;
        }
        self.apply_decisions(
            owner,
            Some(session_id),
            "implicit_evaluator",
            vec![candidate],
        )
    }

    fn apply_decisions(
        &self,
        owner: &str,
        session_id: Option<&str>,
        source: &str,
        decisions: Vec<MemoryDecision>,
    ) -> Result<Vec<AppliedMemoryMutation>> {
        let mut mutations = Vec::new();
        for decision in decisions {
            match decision.kind {
                MemoryDecisionKind::None => {}
                MemoryDecisionKind::Delete => {
                    let deleted = self.store.delete(
                        owner,
                        decision.scope,
                        &decision.category,
                        &decision.key,
                        session_id,
                    )?;
                    mutations.push(AppliedMemoryMutation::Delete {
                        scope: decision.scope.as_str().into(),
                        category: decision.category,
                        key: decision.key,
                        deleted,
                    });
                }
                MemoryDecisionKind::Create
                | MemoryDecisionKind::Update
                | MemoryDecisionKind::Merge
                | MemoryDecisionKind::Rekey => {
                    let Some(value) = decision.value else {
                        continue;
                    };
                    for duplicate in decision
                        .related_keys
                        .iter()
                        .filter(|key| **key != decision.key)
                    {
                        let _ = self.store.delete(
                            owner,
                            decision.scope,
                            &decision.category,
                            duplicate,
                            session_id,
                        )?;
                    }
                    let (outcome, record) = self.store.upsert(
                        owner,
                        decision.scope,
                        &decision.category,
                        &decision.key,
                        &value,
                        if source == "explicit_user" { 1.0 } else { 0.85 },
                        source,
                        session_id,
                    )?;
                    mutations.push(AppliedMemoryMutation::Set {
                        scope: record.scope.as_str().into(),
                        category: record.category,
                        key: record.key,
                        value: record.value,
                        outcome,
                    });
                }
            }
        }
        Ok(mutations)
    }
}

fn extract_candidate(normalized: &str, require_explicit: bool) -> Option<MemoryDecision> {
    if require_explicit && !has_explicit_marker(normalized) {
        return None;
    }
    let declarative = [
        " is ",
        " are ",
        " uses ",
        " use ",
        " adalah ",
        " menggunakan ",
        " = ",
    ]
    .iter()
    .any(|separator| normalized.contains(separator));
    let durable_subject = contains_any(
        normalized,
        &[
            "project",
            "repository",
            "repo ",
            "workspace",
            "device",
            "host",
        ],
    );
    if !require_explicit && !(declarative && durable_subject) {
        return None;
    }

    let statement = strip_intent_prefix(normalized);
    let preference = is_preference(statement);
    let scope = if durable_subject && !preference {
        MemoryScope::Agent
    } else {
        MemoryScope::User
    };
    let category = if preference {
        "preference".to_owned()
    } else if scope == MemoryScope::Agent {
        project_category(statement)
    } else {
        "profile".to_owned()
    };
    let (subject, raw_value) = split_fact(statement).unwrap_or_else(|| {
        if preference {
            ("preference".into(), preference_object(statement))
        } else {
            (compact_subject(statement), statement.trim().to_owned())
        }
    });
    let value = normalize_value(statement, &raw_value, preference)?;
    let key = semantic_key(statement, &subject, &value, preference);
    if key.is_empty() || value.is_empty() {
        return None;
    }
    Some(MemoryDecision {
        kind: MemoryDecisionKind::Create,
        scope,
        category,
        key,
        value: Some(value),
        related_keys: Vec::new(),
    })
}

fn delete_decisions(normalized: &str, existing: &[MemoryRecord]) -> Vec<MemoryDecision> {
    let topic = forget_topic(normalized);
    let topic_key = semantic_key(&topic, &topic, &topic, true);
    let topic_tokens = tokens(&format!("{topic} {topic_key}"));
    let mut ranked = existing
        .iter()
        .filter_map(|memory| {
            let candidate_tokens = tokens(&format!(
                "{} {} {}",
                memory.category, memory.key, memory.value
            ));
            let overlap = topic_tokens.intersection(&candidate_tokens).count();
            let exact = memory.key == topic_key
                || memory.key.contains(&topic_key)
                || topic_key.contains(&memory.key);
            (exact || overlap > 0).then_some((usize::from(exact) * 100 + overlap, memory))
        })
        .collect::<Vec<_>>();
    ranked.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    ranked
        .into_iter()
        .take(4)
        .map(|(_, memory)| MemoryDecision {
            kind: MemoryDecisionKind::Delete,
            scope: memory.scope,
            category: memory.category.clone(),
            key: memory.key.clone(),
            value: None,
            related_keys: Vec::new(),
        })
        .collect()
}

fn related_entries<'a>(
    candidate: &MemoryDecision,
    existing: &'a [MemoryRecord],
) -> Vec<&'a MemoryRecord> {
    let candidate_tokens = tokens(&format!(
        "{} {} {}",
        candidate.category,
        candidate.key,
        candidate.value.as_deref().unwrap_or_default()
    ));
    let mut ranked = existing
        .iter()
        .filter(|memory| memory.scope == candidate.scope)
        .filter_map(|memory| {
            if memory.key == candidate.key && memory.category == candidate.category {
                return Some((1000usize, memory));
            }
            let memory_tokens = tokens(&format!(
                "{} {} {}",
                memory.category, memory.key, memory.value
            ));
            let intersection = candidate_tokens.intersection(&memory_tokens).count();
            let union = candidate_tokens.union(&memory_tokens).count().max(1);
            let key_overlap = tokens(&candidate.key)
                .intersection(&tokens(&memory.key))
                .count();
            let same_category = candidate.category == memory.category;
            let related =
                key_overlap > 0 || same_category && intersection >= 2 && intersection * 3 >= union;
            related.then_some((key_overlap * 100 + intersection, memory))
        })
        .collect::<Vec<_>>();
    ranked.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    ranked.into_iter().map(|(_, memory)| memory).collect()
}

fn semantic_key(statement: &str, subject: &str, value: &str, preference: bool) -> String {
    if contains_any(
        statement,
        &[
            "answer",
            "response",
            "explain",
            "jawab",
            "penjelasan",
            "concise",
            "brief",
            "detail",
            "ringkas",
            "rinci",
        ],
    ) {
        return "response_style".into();
    }
    if preference && is_programming_language(value) {
        return "programming_language".into();
    }
    if contains_any(
        statement,
        &["respond in", "speak in", "bahasa ", "language for replies"],
    ) {
        return "language".into();
    }
    let cleaned_subject = subject
        .trim()
        .trim_start_matches("my ")
        .trim_start_matches("our ")
        .trim_start_matches("the ")
        .trim_start_matches("saya ")
        .trim_start_matches("project ");
    let key_source = if cleaned_subject.is_empty() || cleaned_subject == "preference" {
        domain_word(value).unwrap_or("general")
    } else {
        cleaned_subject
    };
    crate::memory::canonical_key(
        if preference { "preference" } else { "profile" },
        key_source,
    )
}

fn split_fact(statement: &str) -> Option<(String, String)> {
    for separator in [
        " is ",
        " are ",
        " uses ",
        " use ",
        " adalah ",
        " menggunakan ",
        " = ",
    ] {
        if let Some((left, right)) = statement.split_once(separator) {
            let subject = left
                .trim()
                .trim_start_matches("that ")
                .trim_start_matches("actually ")
                .trim_start_matches("sebenarnya ")
                .to_owned();
            let value = right.trim().to_owned();
            if !subject.is_empty() && !value.is_empty() {
                return Some((subject, value));
            }
        }
    }
    None
}

fn normalize_value(statement: &str, raw: &str, preference: bool) -> Option<String> {
    if contains_any(
        statement,
        &[
            "detailed",
            "more detail",
            "in detail",
            "rinci",
            "mendalam",
            "lengkap",
        ],
    ) && contains_any(
        statement,
        &["answer", "response", "explain", "jawab", "penjelasan"],
    ) {
        return Some("detailed".into());
    }
    if contains_any(statement, &["concise", "brief", "ringkas", "singkat"])
        && contains_any(
            statement,
            &["answer", "response", "explain", "jawab", "penjelasan"],
        )
    {
        return Some("concise".into());
    }
    if preference {
        for (needle, canonical) in [
            ("typescript", "TypeScript"),
            ("javascript", "JavaScript"),
            ("python", "Python"),
            ("kotlin", "Kotlin"),
            ("swift", "Swift"),
            ("rust", "Rust"),
            (" golang", "Go"),
            (" go", "Go"),
        ] {
            if format!(" {raw}").contains(needle) {
                return Some(canonical.into());
            }
        }
    }
    let value = raw
        .trim()
        .trim_end_matches(['.', '!', '?'])
        .trim_end_matches(" from now on")
        .trim()
        .to_owned();
    (!value.is_empty() && value.chars().count() <= 8_192).then_some(value)
}

fn project_category(statement: &str) -> String {
    let words = statement.split_whitespace().collect::<Vec<_>>();
    let project = words
        .iter()
        .position(|word| matches!(*word, "project" | "repository" | "repo" | "workspace"))
        .and_then(|index| words.get(index + 1))
        .copied()
        .unwrap_or("general");
    format!("project_{}", crate::memory::canonical_category(project))
}

fn strip_intent_prefix(value: &str) -> &str {
    let value = value.trim();
    [
        "please remember that ",
        "remember that ",
        "please remember ",
        "remember ",
        "tolong ingat bahwa ",
        "ingat bahwa ",
        "ingat ",
    ]
    .iter()
    .find_map(|prefix| value.strip_prefix(prefix))
    .unwrap_or(value)
}

fn preference_object(statement: &str) -> String {
    [
        "i now prefer ",
        "i prefer ",
        "we prefer ",
        "saya lebih suka ",
        "saya suka ",
        "prefer ",
    ]
    .iter()
    .find_map(|marker| {
        statement
            .find(marker)
            .map(|index| statement[index + marker.len()..].trim().to_owned())
    })
    .or_else(|| {
        statement
            .find("want ")
            .map(|index| statement[index + "want ".len()..].trim().to_owned())
    })
    .unwrap_or_else(|| statement.trim().to_owned())
}

fn compact_subject(statement: &str) -> String {
    statement
        .split_whitespace()
        .filter(|word| !stop_word(word))
        .take(5)
        .collect::<Vec<_>>()
        .join(" ")
}

fn domain_word(value: &str) -> Option<&str> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .rfind(|word| word.chars().count() >= 3 && !stop_word(word))
}

fn forget_topic(value: &str) -> String {
    [
        "forget ",
        "lupakan ",
        "hapus ingatan tentang ",
        "do not remember ",
        "don't remember ",
    ]
    .iter()
    .find_map(|marker| {
        value
            .find(marker)
            .map(|index| value[index + marker.len()..].trim().to_owned())
    })
    .unwrap_or_default()
}

fn has_explicit_marker(value: &str) -> bool {
    contains_any(
        value,
        &[
            "remember",
            "ingat",
            "i prefer",
            "we prefer",
            "saya lebih suka",
            "from now on",
            "mulai sekarang",
            "actually",
            "sebenarnya",
            "i now want",
            "i now prefer",
        ],
    )
}

fn is_forget(value: &str) -> bool {
    contains_any(
        value,
        &[
            "forget",
            "lupakan",
            "hapus ingatan",
            "don't remember",
            "do not remember",
        ],
    )
}

fn is_preference(value: &str) -> bool {
    contains_any(
        value,
        &[
            "prefer",
            "preference",
            "from now on",
            "i now want",
            "lebih suka",
            "mulai sekarang",
        ],
    )
}

fn is_programming_language(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "rust" | "go" | "golang" | "python" | "typescript" | "javascript" | "kotlin" | "swift"
    )
}

fn semantically_equal(left: &str, right: &str) -> bool {
    normalize(left) == normalize(right) || {
        let left = tokens(left);
        let right = tokens(right);
        !left.is_empty() && left == right
    }
}

fn tokens(value: &str) -> BTreeSet<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|token| token.chars().count() >= 2 && !stop_word(token))
        .map(|token| match token.as_str() {
            "answers" | "answering" | "jawaban" => "answer".into(),
            "responses" => "response".into(),
            "details" | "detailed" => "detail".into(),
            "preferences" | "preferred" => "preference".into(),
            value => value.trim_end_matches('s').to_owned(),
        })
        .collect()
}

fn stop_word(word: &str) -> bool {
    matches!(
        word,
        "a" | "an"
            | "the"
            | "that"
            | "this"
            | "i"
            | "we"
            | "my"
            | "our"
            | "me"
            | "is"
            | "are"
            | "to"
            | "for"
            | "of"
            | "in"
            | "on"
            | "now"
            | "want"
            | "please"
            | "actually"
            | "from"
            | "saya"
            | "aku"
            | "yang"
            | "dan"
            | "untuk"
            | "dengan"
    )
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{identity::IdentityWorkspace, storage::Storage};

    fn evaluator() -> (MemoryEvaluator, Arc<MemoryStore>, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let workspace = Arc::new(IdentityWorkspace::new(directory.path()));
        workspace.bootstrap().unwrap();
        let store = Arc::new(MemoryStore::with_workspace(
            Arc::new(Storage::open_memory().unwrap()),
            workspace,
        ));
        (MemoryEvaluator::new(store.clone()), store, directory)
    }

    #[test]
    fn synonymous_explicit_preference_change_updates_one_canonical_memory_and_file() {
        let (evaluator, store, directory) = evaluator();
        evaluator
            .apply_explicit("p", "s", "Remember that I prefer concise answers.")
            .unwrap();
        evaluator
            .apply_explicit(
                "p",
                "s",
                "For technical topics, I now want detailed answers.",
            )
            .unwrap();
        let active = store.list("p", Some(MemoryScope::User), 10).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].key, "response_style");
        assert_eq!(active[0].value, "detailed");
        assert_eq!(store.history("p", 10).unwrap().len(), 2);
        let user = std::fs::read_to_string(directory.path().join("USER.md")).unwrap();
        assert_eq!(user.matches("[response_style]").count(), 1, "{user}");
        assert!(user.contains("detailed"));
    }

    #[test]
    fn explicit_forget_removes_active_memory_and_file_entry() {
        let (evaluator, store, directory) = evaluator();
        evaluator
            .apply_explicit("p", "s", "I prefer brief answers from now on")
            .unwrap();
        evaluator
            .apply_explicit("p", "s", "Forget my answer-style preference")
            .unwrap();
        assert!(store.list("p", None, 10).unwrap().is_empty());
        assert!(!std::fs::read_to_string(directory.path().join("USER.md"))
            .unwrap()
            .contains("[response_style]"));
    }

    #[test]
    fn generalized_preferences_facts_and_manual_edits_reconcile() {
        let (evaluator, store, directory) = evaluator();
        evaluator
            .apply_explicit("p", "s", "Remember that my editor is Neovim")
            .unwrap();
        evaluator
            .apply_explicit("p", "s", "Actually, my editor is VS Code")
            .unwrap();
        evaluator
            .apply_explicit("p", "s", "Remember that project orion uses Zig")
            .unwrap();
        let active = store.list("p", None, 10).unwrap();
        assert_eq!(active.len(), 2);
        assert_eq!(
            active.iter().find(|row| row.key == "editor").unwrap().value,
            "vs code"
        );

        let user_path = directory.path().join("USER.md");
        let before_manual = std::fs::read_to_string(&user_path).unwrap();
        let manually_edited = before_manual.replace("[editor] vs code", "[editor] Helix");
        assert_ne!(before_manual, manually_edited, "{before_manual}");
        std::fs::write(&user_path, manually_edited).unwrap();
        store.reconcile("p").unwrap();
        assert_eq!(
            store.list("p", Some(MemoryScope::User), 10).unwrap()[0].value,
            "Helix"
        );
    }

    #[test]
    fn decision_vocabulary_covers_generalized_lifecycle() {
        let variants = [
            MemoryDecisionKind::None,
            MemoryDecisionKind::Create,
            MemoryDecisionKind::Update,
            MemoryDecisionKind::Delete,
            MemoryDecisionKind::Merge,
            MemoryDecisionKind::Rekey,
        ];
        assert_eq!(variants.len(), 6);
    }
}
