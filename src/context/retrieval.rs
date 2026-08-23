use std::sync::Arc;

use anyhow::Result;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::{memory::fts_query, security::redact::redact_text, storage::Storage};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSearchResult {
    pub message_id: i64,
    pub session_id: String,
    pub session_name: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Clone)]
pub struct SessionHistoryStore {
    storage: Arc<Storage>,
}

impl SessionHistoryStore {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn search(
        &self,
        owner: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SessionSearchResult>> {
        let Some(query) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let limit = limit.clamp(1, 20) as i64;
        self.storage.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT m.id,m.session_id,s.name,m.role,m.content,m.created_at FROM messages_fts JOIN messages m ON m.id=messages_fts.rowid JOIN sessions s ON s.id=m.session_id WHERE messages_fts MATCH ? AND s.owner_principal=? ORDER BY bm25(messages_fts),m.id DESC LIMIT ?",
            )?;
            let rows = statement.query_map(params![query, owner, limit], |row| {
                let content: String = row.get(4)?;
                Ok(SessionSearchResult {
                    message_id: row.get(0)?,
                    session_id: row.get(1)?,
                    session_name: row.get(2)?,
                    role: row.get(3)?,
                    content: bound(&redact_text(&content), 800),
                    created_at: row.get(5)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

fn bound(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_owned()
    } else {
        value.chars().take(max_chars).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_search_is_relevant_bounded_and_principal_scoped() {
        let storage = Arc::new(Storage::open_memory().unwrap());
        let alice = storage
            .create_session("alice", "Alice project", "custom", None, "m", false, None)
            .unwrap();
        let bob = storage
            .create_session("bob", "Bob project", "custom", None, "m", false, None)
            .unwrap();
        storage
            .append_message(
                "alice",
                &alice.id,
                "user",
                &format!("release-candidate {}", "x".repeat(2_000)),
            )
            .unwrap();
        storage
            .append_message("bob", &bob.id, "user", "private release-candidate")
            .unwrap();
        let search = SessionHistoryStore::new(storage);
        let rows = search.search("alice", "release candidate", 100).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, alice.id);
        assert!(rows[0].content.chars().count() <= 801);
        assert!(search
            .search("nobody", "release candidate", 10)
            .unwrap()
            .is_empty());
    }
}
