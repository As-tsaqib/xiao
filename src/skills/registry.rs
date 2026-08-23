use std::sync::Arc;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use crate::skills::{
    FilesystemSkills, SkillCandidate, SkillEligibility, SkillMutation, SkillRecord, SkillStore,
};

#[derive(Clone)]
pub struct SkillRegistry {
    store: Arc<SkillStore>,
    filesystem: Option<Arc<FilesystemSkills>>,
}

impl SkillRegistry {
    pub fn new(store: Arc<SkillStore>) -> Self {
        Self {
            store,
            filesystem: None,
        }
    }

    pub fn with_filesystem(store: Arc<SkillStore>, filesystem: Arc<FilesystemSkills>) -> Self {
        Self {
            store,
            filesystem: Some(filesystem),
        }
    }

    pub fn search(&self, owner: &str, query: &str, limit: usize) -> Result<Vec<SkillRecord>> {
        self.sync(owner)?;
        Ok(self
            .store
            .search(owner, query, limit)?
            .into_iter()
            .filter(|record| {
                matches!(
                    self.eligibility(&record.name),
                    Ok(SkillEligibility::Eligible)
                )
            })
            .collect())
    }

    pub fn search_with_eligibility(
        &self,
        owner: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(SkillRecord, SkillEligibility)>> {
        self.sync(owner)?;
        self.store
            .search(owner, query, limit)?
            .into_iter()
            .map(|record| {
                let eligibility = self.eligibility(&record.name)?;
                Ok((record, eligibility))
            })
            .collect()
    }

    pub fn view(&self, owner: &str, name_or_id: &str) -> Result<Option<SkillRecord>> {
        self.sync(owner)?;
        let record = self.store.view(owner, name_or_id)?;
        Ok(record.filter(|record| {
            record.enabled
                && matches!(
                    self.eligibility(&record.name),
                    Ok(SkillEligibility::Eligible)
                )
        }))
    }

    pub async fn view_ready(
        &self,
        owner: &str,
        name_or_id: &str,
        agent_run_id: Option<&str>,
        cancellation: CancellationToken,
        progress: Option<&tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<Option<SkillRecord>> {
        self.sync(owner)?;
        let Some(record) = self.store.view(owner, name_or_id)? else {
            return Ok(None);
        };
        if !record.enabled {
            return Ok(None);
        }
        let Some(filesystem) = &self.filesystem else {
            return Ok(Some(record));
        };
        let Some(document) = filesystem.document(&record.name)? else {
            return Ok(Some(record));
        };
        let status = filesystem
            .resolve_dependencies(&document, agent_run_id, cancellation, progress)
            .await?;
        if matches!(status, SkillEligibility::Eligible) {
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }

    pub fn learn(
        &self,
        owner: &str,
        candidate: SkillCandidate,
        source_session_id: Option<&str>,
    ) -> Result<(SkillMutation, SkillRecord)> {
        if let Some(filesystem) = &self.filesystem {
            filesystem.learn(owner, candidate, source_session_id)
        } else {
            self.store
                .create_or_update(owner, candidate, source_session_id)
        }
    }

    pub fn sync(&self, owner: &str) -> Result<usize> {
        self.filesystem
            .as_ref()
            .map(|filesystem| filesystem.reconcile(owner))
            .unwrap_or(Ok(0))
    }

    pub fn eligibility(&self, name: &str) -> Result<SkillEligibility> {
        let Some(filesystem) = &self.filesystem else {
            return Ok(SkillEligibility::Eligible);
        };
        Ok(filesystem
            .document(name)?
            .map(|document| filesystem.eligibility(&document))
            .unwrap_or(SkillEligibility::Eligible))
    }
}
