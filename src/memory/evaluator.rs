use std::{collections::BTreeSet, sync::Arc};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{
    memory::{MemoryRecord, MemoryScope, MemoryStore, MemoryUpsert},
    security::redact::contains_secret_material,
    semantic::{SemanticEvaluator, SemanticResult},
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
    semantic: Arc<SemanticEvaluator>,
}

impl MemoryEvaluator {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self {
            store,
            semantic: Arc::new(SemanticEvaluator::deterministic()),
        }
    }

    pub fn with_semantic(store: Arc<MemoryStore>, semantic: Arc<SemanticEvaluator>) -> Self {
        Self { store, semantic }
    }

    /// Evaluate explicit owner intent against related current entries. The
    /// extractor is conservative, but general subjects are supported rather
    /// than limiting memory to a small fixed field list.
    pub fn evaluate_explicit(&self, owner: &str, prompt: &str) -> Result<Vec<MemoryDecision>> {
        self.evaluate_explicit_with_cancellation(owner, prompt, CancellationToken::new())
    }

    fn evaluate_explicit_with_cancellation(
        &self,
        owner: &str,
        prompt: &str,
        cancellation: CancellationToken,
    ) -> Result<Vec<MemoryDecision>> {
        if contains_secret_material(prompt) {
            return Ok(Vec::new());
        }
        self.store.reconcile(owner)?;
        let existing = self.store.list(owner, None, 200)?;
        match self
            .semantic
            .evaluate_with_cancellation::<SemanticMemoryOutput>(
            "memory_decision",
            memory_decision_schema(),
            serde_json::json!({
                "owner_statement": prompt,
                "current_memory": existing.iter().take(100).map(memory_input).collect::<Vec<_>>(),
                "rules": [
                    "Choose NONE, CREATE, UPDATE, DELETE, MERGE, or REKEY.",
                    "Explicit owner changes replace related prior active state.",
                    "Do not create near-duplicates or store secrets/transient task details."
                ]
            }),
            cancellation,
        ) {
            SemanticResult::Valid(output) => {
                return Ok(validate_semantic_decisions(output.decisions, &existing));
            }
            SemanticResult::Malformed => return Ok(Vec::new()),
            SemanticResult::Unavailable => {}
        }
        if let Some(mut candidate) = preferred_address_candidate(prompt) {
            let related = existing
                .iter()
                .filter(|memory| {
                    memory.scope == MemoryScope::User
                        && memory.category == "preference"
                        && memory.key == "preferred_address"
                })
                .collect::<Vec<_>>();
            if let Some(current) = related.first() {
                candidate.kind = if semantically_equal(
                    candidate.value.as_deref().unwrap_or_default(),
                    &current.value,
                ) {
                    MemoryDecisionKind::None
                } else {
                    MemoryDecisionKind::Update
                };
            }
            return Ok(vec![candidate]);
        }
        let normalized = normalize(prompt);
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

    /// Agent-loop entry point that keeps provider-backed semantic evaluation
    /// off Tokio's async worker threads.
    pub async fn apply_explicit_async(
        &self,
        owner: &str,
        session_id: &str,
        prompt: &str,
        cancellation: CancellationToken,
    ) -> Result<Vec<AppliedMemoryMutation>> {
        let evaluator = self.clone();
        let owner = owner.to_owned();
        let session_id = session_id.to_owned();
        let prompt = prompt.to_owned();
        tokio::task::spawn_blocking(move || {
            if cancellation.is_cancelled() {
                return Err(anyhow::anyhow!("memory evaluation cancelled"));
            }
            let decisions = evaluator.evaluate_explicit_with_cancellation(
                &owner,
                &prompt,
                cancellation.clone(),
            )?;
            if cancellation.is_cancelled() {
                return Err(anyhow::anyhow!("memory evaluation cancelled"));
            }
            evaluator.apply_decisions(&owner, Some(&session_id), "explicit_user", decisions)
        })
        .await
        .map_err(|error| anyhow::anyhow!("memory evaluator worker failed: {error}"))?
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

    /// Evaluate the complete sanitized successful task trace. Only observable
    /// data is accepted; there is no hidden reasoning field. A malformed
    /// semantic response makes no memory mutation.
    pub fn apply_implicit_trace(
        &self,
        owner: &str,
        session_id: &str,
        trace: &serde_json::Value,
    ) -> Result<Vec<AppliedMemoryMutation>> {
        if contains_secret_material(&trace.to_string()) {
            return Ok(Vec::new());
        }
        self.store.reconcile(owner)?;
        let existing = self.store.list(owner, None, 200)?;
        match self.semantic.evaluate::<SemanticMemoryOutput>(
            "durable_trace_memory",
            memory_decision_schema(),
            serde_json::json!({
                "successful_sanitized_trace": trace,
                "current_memory": existing.iter().take(100).map(memory_input).collect::<Vec<_>>(),
                "rules": [
                    "Only durable owner/project/workspace/device facts from successful observations may be remembered.",
                    "Ignore transient errors, failed attempts, secrets, and one-off outputs.",
                    "Update or merge related current state instead of duplicating it."
                ]
            }),
        ) {
            SemanticResult::Valid(output) => self.apply_decisions(
                owner,
                Some(session_id),
                "implicit_trace_evaluator",
                validate_semantic_decisions(output.decisions, &existing),
            ),
            SemanticResult::Malformed => Ok(Vec::new()),
            SemanticResult::Unavailable => {
                let mut mutations = Vec::new();
                for statement in successful_trace_strings(trace).into_iter().take(32) {
                    mutations.extend(self.apply_implicit(owner, session_id, &statement)?);
                }
                Ok(mutations)
            }
        }
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

fn preferred_address_candidate(prompt: &str) -> Option<MemoryDecision> {
    let lower = prompt.to_ascii_lowercase();
    let markers = [
        "call me ",
        "address me as ",
        "refer to me as ",
        "panggil saya ",
        "panggil aku ",
    ];
    let (index, marker) = markers
        .iter()
        .filter_map(|marker| lower.find(marker).map(|index| (index, *marker)))
        .min_by_key(|(index, _)| *index)?;
    let start = index + marker.len();
    let value = prompt
        .get(start..)?
        .trim()
        .trim_end_matches(['.', '!', '?']);
    let value = [" from now on", " instead", " mulai sekarang"]
        .iter()
        .find_map(|suffix| {
            value
                .to_ascii_lowercase()
                .strip_suffix(suffix)
                .map(|stripped| value[..stripped.len()].trim())
        })
        .unwrap_or(value)
        .trim_matches(['\'', '"'])
        .trim();
    (!value.is_empty() && value.chars().count() <= 120).then(|| MemoryDecision {
        kind: MemoryDecisionKind::Create,
        scope: MemoryScope::User,
        category: "preference".into(),
        key: "preferred_address".into(),
        value: Some(value.to_owned()),
        related_keys: Vec::new(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SemanticMemoryOutput {
    decisions: Vec<MemoryDecision>,
}

fn memory_decision_schema() -> serde_json::Value {
    serde_json::json!({
        "type":"object",
        "additionalProperties":false,
        "required":["decisions"],
        "properties":{
            "decisions":{
                "type":"array","maxItems":8,
                "items":{
                    "type":"object","additionalProperties":false,
                    "required":["kind","scope","category","key","value","related_keys"],
                    "properties":{
                        "kind":{"enum":["none","create","update","delete","merge","rekey"]},
                        "scope":{"enum":["user","agent"]},
                        "category":{"type":"string","maxLength":120},
                        "key":{"type":"string","maxLength":160},
                        "value":{"type":["string","null"],"maxLength":8192},
                        "related_keys":{"type":"array","maxItems":8,"items":{"type":"string","maxLength":160}}
                    }
                }
            }
        }
    })
}

fn memory_input(memory: &MemoryRecord) -> serde_json::Value {
    serde_json::json!({
        "scope":memory.scope,
        "category":memory.category,
        "key":memory.key,
        "value":memory.value,
    })
}

fn validate_semantic_decisions(
    decisions: Vec<MemoryDecision>,
    existing: &[MemoryRecord],
) -> Vec<MemoryDecision> {
    decisions
        .into_iter()
        .take(8)
        .filter_map(|mut decision| {
            decision.category = crate::memory::canonical_category(&decision.category);
            decision.key = crate::memory::canonical_key(&decision.category, &decision.key);
            decision.related_keys = decision
                .related_keys
                .into_iter()
                .filter(|key| {
                    existing
                        .iter()
                        .any(|memory| memory.scope == decision.scope && memory.key == *key)
                })
                .take(8)
                .collect();
            let target_exists = existing.iter().any(|memory| {
                memory.scope == decision.scope
                    && memory.category == decision.category
                    && memory.key == decision.key
            });
            let value_ok = decision.value.as_ref().is_none_or(|value| {
                !value.trim().is_empty()
                    && value.chars().count() <= 8_192
                    && !contains_secret_material(value)
            });
            let shape_ok = match decision.kind {
                MemoryDecisionKind::None => true,
                MemoryDecisionKind::Delete => target_exists && decision.value.is_none(),
                MemoryDecisionKind::Create => decision.value.is_some(),
                MemoryDecisionKind::Update => target_exists && decision.value.is_some(),
                MemoryDecisionKind::Merge | MemoryDecisionKind::Rekey => {
                    decision.value.is_some() && !decision.related_keys.is_empty()
                }
            };
            (shape_ok && value_ok).then_some(decision)
        })
        .collect()
}

fn successful_trace_strings(value: &serde_json::Value) -> Vec<String> {
    fn visit(value: &serde_json::Value, output: &mut Vec<String>) {
        match value {
            serde_json::Value::String(value) if value.chars().count() <= 8_192 => {
                output.push(value.clone());
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    visit(value, output);
                }
            }
            serde_json::Value::Object(values) => {
                for (key, value) in values {
                    if !matches!(key.as_str(), "failed_actions" | "errors") {
                        visit(value, output);
                    }
                }
            }
            _ => {}
        }
    }
    let mut output = Vec::new();
    visit(value, &mut output);
    output
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
    use crate::{
        identity::IdentityWorkspace,
        semantic::{SemanticBackend, SemanticRequest},
        storage::Storage,
    };
    use std::sync::Mutex;

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

    #[test]
    fn preferred_address_replacement_keeps_one_active_entry_and_history() {
        let (evaluator, store, _directory) = evaluator();
        evaluator
            .apply_explicit("p", "s", "Please call me Bos from now on.")
            .unwrap();
        evaluator
            .apply_explicit("p", "s", "Call me Tuan instead.")
            .unwrap();
        let active = store.list("p", Some(MemoryScope::User), 10).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].key, "preferred_address");
        assert_eq!(active[0].value, "Tuan");
        assert_eq!(store.history("p", 10).unwrap().len(), 2);
    }

    #[test]
    fn successful_full_trace_can_learn_durable_fact_but_ignores_failed_branch() {
        let (evaluator, store, _directory) = evaluator();
        evaluator
            .apply_implicit_trace(
                "p",
                "s",
                &serde_json::json!({
                    "successful_actions":[{
                        "tool":"termux_terminal",
                        "observation":"project orion is located at /data/projects/orion"
                    }],
                    "failed_actions":[{
                        "observation":"project orion is temporarily unavailable at /wrong/path"
                    }],
                    "final_observable_result":"project orion uses the workspace /data/projects/orion"
                }),
            )
            .unwrap();
        let active = store.list("p", Some(MemoryScope::Agent), 10).unwrap();
        assert!(!active.is_empty());
        assert!(active
            .iter()
            .any(|memory| memory.value.contains("/data/projects/orion")));
        assert!(active
            .iter()
            .all(|memory| !memory.value.contains("/wrong/path")));
    }

    struct QueueSemantic(Mutex<Vec<String>>);
    impl SemanticBackend for QueueSemantic {
        fn evaluate(&self, _: &SemanticRequest) -> Result<String> {
            Ok(self.0.lock().unwrap().remove(0))
        }
    }

    #[test]
    fn semantic_memory_handles_arbitrary_domain_and_malformed_output_is_conservative() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = Arc::new(IdentityWorkspace::new(directory.path()));
        workspace.bootstrap().unwrap();
        let store = Arc::new(MemoryStore::with_workspace(
            Arc::new(Storage::open_memory().unwrap()),
            workspace,
        ));
        let semantic = Arc::new(SemanticEvaluator::with_backend(Arc::new(QueueSemantic(
            Mutex::new(vec![
                r#"{"decisions":[{"kind":"create","scope":"user","category":"preference","key":"diagram_notation","value":"PlantUML","related_keys":[]}]}"#.into(),
                "malformed".into(),
                "still malformed".into(),
            ]),
        ))));
        let evaluator = MemoryEvaluator::with_semantic(store.clone(), semantic);
        evaluator
            .apply_explicit("p", "s", "Use PlantUML whenever a diagram would help.")
            .unwrap();
        assert_eq!(
            store.list("p", None, 10).unwrap()[0].key,
            "diagram_notation"
        );
        evaluator
            .apply_explicit("p", "s", "Remember that my editor is Neovim")
            .unwrap();
        assert_eq!(store.list("p", None, 10).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancelled_async_evaluation_cannot_mutate_memory_after_cancel() {
        let (evaluator, store, _directory) = evaluator();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(evaluator
            .apply_explicit_async("p", "s", "Please call me Bos from now on", cancellation,)
            .await
            .is_err());
        assert!(store.list("p", None, 10).unwrap().is_empty());
    }
}
