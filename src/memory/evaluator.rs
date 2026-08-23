use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{
    memory::{MemoryScope, MemoryStore, MemoryUpsert},
    security::redact::contains_secret_material,
};

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

    /// Deterministic handling for authoritative, explicit durable preferences.
    /// Ambiguous prose is left to the typed memory tools instead of being
    /// over-learned heuristically.
    pub fn apply_explicit(
        &self,
        owner: &str,
        session_id: &str,
        prompt: &str,
    ) -> Result<Vec<AppliedMemoryMutation>> {
        if contains_secret_material(prompt) {
            return Ok(Vec::new());
        }
        let normalized = prompt
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        let mut mutations = Vec::new();

        let response_topic = contains_any(
            &normalized,
            &[
                "answer",
                "response",
                "explain",
                "concise",
                "brief",
                "detail",
                "jawaban",
                "penjelasan",
                "singkat",
                "ringkas",
                "rinci",
            ],
        );
        let forget = contains_any(
            &normalized,
            &[
                "forget",
                "lupakan",
                "hapus ingatan",
                "don't remember",
                "do not remember",
            ],
        );
        let programming_topic = contains_any(
            &normalized,
            &[
                "programming language",
                "programming-language",
                "coding language",
                "prefer rust",
                "prefer go",
                "prefer python",
                "pakai rust",
                "pakai go",
                "gunakan rust",
                "gunakan go",
            ],
        );
        if forget && programming_topic {
            let deleted = self.store.delete(
                owner,
                MemoryScope::User,
                "preference",
                "programming_language",
                Some(session_id),
            )?;
            mutations.push(AppliedMemoryMutation::Delete {
                scope: "user".into(),
                category: "preference".into(),
                key: "programming_language".into(),
                deleted,
            });
            return Ok(mutations);
        }
        if forget && response_topic {
            let deleted = self.store.delete(
                owner,
                MemoryScope::User,
                "preference",
                "response_style",
                Some(session_id),
            )?;
            mutations.push(AppliedMemoryMutation::Delete {
                scope: "user".into(),
                category: "preference".into(),
                key: "response_style".into(),
                deleted,
            });
            return Ok(mutations);
        }

        if forget {
            let topic = forget_topic(&normalized);
            if !topic.is_empty() {
                let active = self.store.list(owner, Some(MemoryScope::User), 200)?;
                for memory in active.into_iter().filter(|memory| {
                    memory.key == topic
                        || memory.key.contains(&topic)
                        || topic.contains(&memory.key)
                }) {
                    let deleted = self.store.delete(
                        owner,
                        MemoryScope::User,
                        &memory.category,
                        &memory.key,
                        Some(session_id),
                    )?;
                    mutations.push(AppliedMemoryMutation::Delete {
                        scope: "user".into(),
                        category: memory.category,
                        key: memory.key,
                        deleted,
                    });
                }
            }
            return Ok(mutations);
        }

        let authoritative = contains_any(
            &normalized,
            &[
                "remember",
                "ingat",
                "prefer",
                "from now on",
                "mulai sekarang",
                "actually",
                "sekarang",
                "please answer",
                "tolong jawab",
                "give me",
            ],
        );
        if authoritative && response_topic {
            let detailed = contains_any(
                &normalized,
                &[
                    "more detail",
                    "detailed",
                    "in detail",
                    "detail from now",
                    "lebih detail",
                    "lebih rinci",
                    "mendalam",
                    "lengkap",
                ],
            );
            let concise = contains_any(
                &normalized,
                &["concise", "brief", "short answer", "singkat", "ringkas"],
            );
            if detailed || concise {
                let value = if detailed { "detailed" } else { "concise" };
                let (outcome, _) = self.store.upsert(
                    owner,
                    MemoryScope::User,
                    "preference",
                    "response_style",
                    value,
                    1.0,
                    "explicit_user",
                    Some(session_id),
                )?;
                mutations.push(AppliedMemoryMutation::Set {
                    scope: "user".into(),
                    category: "preference".into(),
                    key: "response_style".into(),
                    value: value.into(),
                    outcome,
                });
            }
        }

        if authoritative && programming_topic {
            let language = [
                ("rust", "Rust"),
                ("python", "Python"),
                ("typescript", "TypeScript"),
                ("javascript", "JavaScript"),
                ("kotlin", "Kotlin"),
                ("swift", "Swift"),
                (" go", "Go"),
            ]
            .into_iter()
            .find_map(|(needle, language)| normalized.contains(needle).then_some(language));
            if let Some(language) = language {
                let (outcome, _) = self.store.upsert(
                    owner,
                    MemoryScope::User,
                    "preference",
                    "programming_language",
                    language,
                    1.0,
                    "explicit_user",
                    Some(session_id),
                )?;
                mutations.push(AppliedMemoryMutation::Set {
                    scope: "user".into(),
                    category: "preference".into(),
                    key: "programming_language".into(),
                    value: language.into(),
                    outcome,
                });
            }
        }

        if authoritative
            && contains_any(&normalized, &["language", "respond in", "speak", "bahasa"])
        {
            let language = if contains_any(&normalized, &["indonesian", "bahasa indonesia"]) {
                Some("Indonesian")
            } else if contains_any(&normalized, &["english", "bahasa inggris"]) {
                Some("English")
            } else {
                None
            };
            if let Some(language) = language {
                let (outcome, _) = self.store.upsert(
                    owner,
                    MemoryScope::User,
                    "preference",
                    "language",
                    language,
                    1.0,
                    "explicit_user",
                    Some(session_id),
                )?;
                mutations.push(AppliedMemoryMutation::Set {
                    scope: "user".into(),
                    category: "preference".into(),
                    key: "language".into(),
                    value: language.into(),
                    outcome,
                });
            }
        }

        if mutations.is_empty() {
            if let Some((scope, category, key, value)) = generic_explicit_fact(&normalized) {
                let (outcome, _) = self.store.upsert(
                    owner,
                    scope,
                    &category,
                    &key,
                    &value,
                    1.0,
                    "explicit_user",
                    Some(session_id),
                )?;
                mutations.push(AppliedMemoryMutation::Set {
                    scope: scope.as_str().into(),
                    category,
                    key,
                    value,
                    outcome,
                });
            }
        }

        Ok(mutations)
    }

    /// Conservative post-completion learning. Only project facts with a clear
    /// durable subject and value are accepted; casual conversation is ignored.
    pub fn apply_implicit(
        &self,
        owner: &str,
        session_id: &str,
        prompt: &str,
    ) -> Result<Vec<AppliedMemoryMutation>> {
        if contains_secret_material(prompt) {
            return Ok(Vec::new());
        }
        let normalized = prompt
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        if !(normalized.contains("xiao project") || normalized.contains("project xiao")) {
            return Ok(Vec::new());
        }
        let language = if normalized.contains("rust") {
            Some("Rust")
        } else if normalized.contains("kotlin") {
            Some("Kotlin")
        } else {
            None
        };
        let Some(language) = language else {
            return Ok(Vec::new());
        };
        let (outcome, _) = self.store.upsert(
            owner,
            MemoryScope::Agent,
            "project_xiao",
            "implementation_language",
            language,
            0.85,
            "implicit_evaluator",
            Some(session_id),
        )?;
        Ok(vec![AppliedMemoryMutation::Set {
            scope: "agent".into(),
            category: "project_xiao".into(),
            key: "implementation_language".into(),
            value: language.into(),
            outcome,
        }])
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn generic_explicit_fact(normalized: &str) -> Option<(MemoryScope, String, String, String)> {
    let statement = [
        "please remember that ",
        "remember that ",
        "remember ",
        "tolong ingat bahwa ",
        "ingat bahwa ",
        "ingat ",
        "actually, ",
        "actually ",
        "sebenarnya ",
    ]
    .iter()
    .find_map(|prefix| normalized.strip_prefix(prefix))?
    .trim();
    let (left, value) = [" is ", " adalah ", " = ", " uses ", " menggunakan "]
        .iter()
        .find_map(|separator| statement.split_once(separator))?;
    let left = left
        .trim()
        .trim_start_matches("my ")
        .trim_start_matches("our ")
        .trim_start_matches("the ")
        .trim_start_matches("saya ");
    let value = value
        .trim()
        .trim_end_matches(['.', '!', '?'])
        .trim_end_matches(" from now on")
        .trim();
    if left.is_empty() || value.is_empty() {
        return None;
    }

    let project_words = left.split_whitespace().collect::<Vec<_>>();
    if let Some(project_index) = project_words.iter().position(|word| *word == "project") {
        let project_name = project_words
            .get(project_index + 1)
            .or_else(|| {
                project_index
                    .checked_sub(1)
                    .and_then(|index| project_words.get(index))
            })
            .copied()
            .unwrap_or("general");
        let fact_words = project_words
            .iter()
            .enumerate()
            .filter(|(index, word)| {
                *index != project_index && **word != project_name && **word != "uses"
            })
            .map(|(_, word)| *word)
            .collect::<Vec<_>>();
        let key = if project_name == "xiao" && matches!(value, "rust" | "kotlin" | "go" | "python")
        {
            "implementation_language".into()
        } else if fact_words.is_empty() {
            "technology".into()
        } else {
            crate::memory::canonical_key("project", &fact_words.join(" "))
        };
        return Some((
            MemoryScope::Agent,
            format!(
                "project_{}",
                crate::memory::canonical_category(project_name)
            ),
            key,
            value.to_owned(),
        ));
    }

    let key_source = left
        .trim_start_matches("favorite ")
        .trim_start_matches("preferred ")
        .trim_end_matches(" preference");
    let category =
        if left.contains("favorite") || left.contains("preferred") || left.contains("preference") {
            "preference"
        } else {
            "profile"
        };
    let key = crate::memory::canonical_key(category, key_source);
    (!key.is_empty()).then(|| (MemoryScope::User, category.into(), key, value.to_owned()))
}

fn forget_topic(normalized: &str) -> String {
    let remainder = ["forget ", "lupakan ", "hapus ingatan tentang "]
        .iter()
        .find_map(|marker| {
            normalized
                .find(marker)
                .map(|index| &normalized[index + marker.len()..])
        })
        .unwrap_or_default();
    let words = remainder
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| {
            !matches!(
                *word,
                "my" | "the"
                    | "our"
                    | "about"
                    | "memory"
                    | "preference"
                    | "preferensi"
                    | "saya"
                    | "ingatan"
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    crate::memory::canonical_key("profile", &words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;

    #[test]
    fn synonymous_explicit_preference_change_updates_one_canonical_memory() {
        let store = Arc::new(MemoryStore::new(Arc::new(Storage::open_memory().unwrap())));
        let evaluator = MemoryEvaluator::new(store.clone());
        evaluator
            .apply_explicit("p", "s", "Remember that I prefer concise answers.")
            .unwrap();
        evaluator
            .apply_explicit(
                "p",
                "s",
                "Actually, explain things in more detail from now on.",
            )
            .unwrap();
        let active = store.list("p", Some(MemoryScope::User), 10).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].key, "response_style");
        assert_eq!(active[0].value, "detailed");
        assert_eq!(store.history("p", 10).unwrap().len(), 2);
    }

    #[test]
    fn explicit_forget_removes_active_memory() {
        let store = Arc::new(MemoryStore::new(Arc::new(Storage::open_memory().unwrap())));
        let evaluator = MemoryEvaluator::new(store.clone());
        evaluator
            .apply_explicit("p", "s", "Please answer briefly from now on")
            .unwrap();
        evaluator
            .apply_explicit("p", "s", "Forget my answer-style preference")
            .unwrap();
        assert!(store.list("p", None, 10).unwrap().is_empty());
        assert_eq!(store.history("p", 10).unwrap()[0].action, "delete");
    }

    #[test]
    fn explicit_programming_language_change_updates_canonical_key() {
        let store = Arc::new(MemoryStore::new(Arc::new(Storage::open_memory().unwrap())));
        let evaluator = MemoryEvaluator::new(store.clone());
        evaluator
            .apply_explicit("p", "s", "Remember that I prefer Rust")
            .unwrap();
        evaluator
            .apply_explicit("p", "s", "I prefer Go now")
            .unwrap();
        let active = store.list("p", None, 10).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].key, "programming_language");
        assert_eq!(active[0].value, "Go");
    }

    #[test]
    fn generic_explicit_fact_changes_and_forgets_same_canonical_state() {
        let store = Arc::new(MemoryStore::new(Arc::new(Storage::open_memory().unwrap())));
        let evaluator = MemoryEvaluator::new(store.clone());
        evaluator
            .apply_explicit("p", "s", "Remember that my editor is Neovim")
            .unwrap();
        evaluator
            .apply_explicit("p", "s", "Actually, my editor is VS Code")
            .unwrap();
        let active = store.list("p", None, 10).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].key, "editor");
        assert_eq!(active[0].value, "vs code");
        evaluator
            .apply_explicit("p", "s", "Forget my editor preference")
            .unwrap();
        assert!(store.list("p", None, 10).unwrap().is_empty());
    }
}
