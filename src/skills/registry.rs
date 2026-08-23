use std::sync::Arc;

use anyhow::Result;

use crate::skills::{SkillCandidate, SkillMutation, SkillRecord, SkillStore};

#[derive(Clone)]
pub struct SkillRegistry {
    store: Arc<SkillStore>,
}

impl SkillRegistry {
    pub fn new(store: Arc<SkillStore>) -> Self {
        Self { store }
    }

    pub fn search(&self, owner: &str, query: &str, limit: usize) -> Result<Vec<SkillRecord>> {
        self.store.search(owner, query, limit)
    }

    pub fn view(&self, owner: &str, name_or_id: &str) -> Result<Option<SkillRecord>> {
        self.store.view(owner, name_or_id)
    }

    pub fn learn(
        &self,
        owner: &str,
        candidate: SkillCandidate,
        source_session_id: Option<&str>,
    ) -> Result<(SkillMutation, SkillRecord)> {
        self.store
            .create_or_update(owner, candidate, source_session_id)
    }
}
