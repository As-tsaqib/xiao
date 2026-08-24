use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::{
    app::AppState,
    event::AppEvent,
    memory::MemoryStore,
    providers::{ProviderProfileStore, ProviderRegistry},
    security::secrets::SecretStore,
    skills::{FilesystemSkills, SkillStore},
    storage::{SessionRecord, Storage},
    telegram::{client::TelegramClient, types::BotIdentity},
};

const TELEGRAM_TOKEN_KEY: &str = "telegram-bot-token";
const TELEGRAM_IDENTITY_KEY: &str = "telegram_bot_identity";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OwnerSetupState {
    Configured,
    SetupRequired,
    ResolutionRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramSetupStatus {
    pub enabled: bool,
    pub owner_user_id: Option<i64>,
    pub owner_state: OwnerSetupState,
    pub legacy_candidate_count: usize,
    pub allowed_chat_ids: Vec<i64>,
    pub token_configured: bool,
    pub bot: Option<BotIdentity>,
}

#[derive(Debug, Clone, Default)]
pub struct TelegramConfigureInput {
    pub enabled: Option<bool>,
    pub bot_token: Option<String>,
    pub owner_user_id: Option<i64>,
    pub confirm_owner_change: bool,
    pub allowed_chat_ids: Option<Vec<i64>>,
    pub test_connection: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TelegramConfigureResult {
    pub applied: bool,
    pub tested: bool,
    pub status: TelegramSetupStatus,
}

#[derive(Clone)]
pub struct TelegramSetupService {
    app: AppState,
    config_path: PathBuf,
}

impl TelegramSetupService {
    pub fn new(app: AppState, config_path: impl Into<PathBuf>) -> Self {
        Self {
            app,
            config_path: config_path.into(),
        }
    }

    pub async fn status(&self) -> Result<TelegramSetupStatus> {
        let cfg = self.app.config.read().await.clone();
        let access = &cfg.telegram.access;
        let owner_state = if access.owner_user_id.is_some() {
            OwnerSetupState::Configured
        } else if access.owner_resolution_required() {
            OwnerSetupState::ResolutionRequired
        } else {
            OwnerSetupState::SetupRequired
        };
        let secrets = SecretStore::new(cfg.paths.secrets_dir.clone());
        let token_configured = secrets.get(TELEGRAM_TOKEN_KEY)?.is_some();
        let bot = self
            .app
            .storage
            .setting(TELEGRAM_IDENTITY_KEY)?
            .and_then(|raw| serde_json::from_str::<BotIdentity>(&raw).ok());
        Ok(TelegramSetupStatus {
            enabled: cfg.telegram.enabled,
            owner_user_id: access.owner_user_id,
            owner_state,
            legacy_candidate_count: access.allowed_user_ids.len(),
            allowed_chat_ids: access.allowed_chat_ids.clone(),
            token_configured,
            bot,
        })
    }

    pub async fn test_connection(&self, token_override: Option<&str>) -> Result<BotIdentity> {
        let cfg = self.app.config.read().await.clone();
        let token = match token_override
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(value) => value.to_owned(),
            None => SecretStore::new(cfg.paths.secrets_dir)
                .get(TELEGRAM_TOKEN_KEY)?
                .ok_or_else(|| anyhow!("Telegram bot token is not configured"))?,
        };
        let client = TelegramClient::new(token)?;
        tokio::time::timeout(Duration::from_secs(15), client.get_me())
            .await
            .map_err(|_| anyhow!("Telegram getMe probe timed out"))?
    }

    pub async fn configure(
        &self,
        input: TelegramConfigureInput,
    ) -> Result<TelegramConfigureResult> {
        let old = self.app.config.read().await.clone();
        let mut next = old.clone();

        if let Some(enabled) = input.enabled {
            next.telegram.enabled = enabled;
        }
        if let Some(owner_user_id) = input.owner_user_id {
            if owner_user_id <= 0 {
                return Err(anyhow!(
                    "Telegram owner user id must be positive (got {owner_user_id})"
                ));
            }
            if old
                .telegram
                .access
                .owner_user_id
                .is_some_and(|old_id| old_id != owner_user_id)
                && !input.confirm_owner_change
            {
                return Err(anyhow!(
                    "changing Telegram owner requires explicit confirmation"
                ));
            }
            next.telegram.access.owner_user_id = Some(owner_user_id);
            next.telegram.access.allowed_user_ids.clear();
        }
        if let Some(mut allowed_chat_ids) = input.allowed_chat_ids {
            if allowed_chat_ids.contains(&0) {
                return Err(anyhow!("allowed chat ids cannot contain zero"));
            }
            allowed_chat_ids.sort_unstable();
            allowed_chat_ids.dedup();
            next.telegram.access.allowed_chat_ids = allowed_chat_ids;
        }
        next.telegram.access.migrate_legacy_owner();
        next.validate()?;
        // P0-5: validate all inputs before durable mutation.
        if input.owner_user_id.is_some() && next.telegram.access.owner_user_id.is_none() {
            return Err(anyhow!("owner_user_id validation failed after migration"));
        }

        let supplied_token = input
            .bot_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let bot = if input.test_connection {
            Some(self.test_connection(supplied_token).await?)
        } else {
            None
        };

        // P0-5: staging/compensation across config file, SecretStore and SQLite.
        // No partial commit is published; ConfigReloaded only after coherent commit.
        // Snapshot old config for rollback if later step fails.
        let staged_secret = supplied_token.is_some();
        // Commit durable non-secret config first; if this fails nothing else ran.
        if let Err(e) = next.save_atomic(&self.config_path) {
            return Err(anyhow!("persist config: {e}"));
        }
        let mut committed_config = true;
        // Stage write-only secret.
        if let Some(token) = supplied_token {
            if let Err(e) =
                SecretStore::new(next.paths.secrets_dir.clone()).put(TELEGRAM_TOKEN_KEY, token)
            {
                // Roll back config file to old state; staged secret was not fully written yet.
                let _ = old.save_atomic(&self.config_path);
                let _ = SecretStore::new(old.paths.secrets_dir.clone()).remove(TELEGRAM_TOKEN_KEY);
                let _ = committed_config;
                return Err(anyhow!("persist telegram token: {e}"));
            }
        }
        // Persist bot identity if probed.
        if let Some(bot) = bot.as_ref() {
            if let Err(e) = self.app.storage.put_setting(
                TELEGRAM_IDENTITY_KEY,
                &serde_json::to_string(bot).unwrap_or_default(),
            ) {
                if staged_secret {
                    let _ =
                        SecretStore::new(next.paths.secrets_dir.clone()).remove(TELEGRAM_TOKEN_KEY);
                }
                let _ = old.save_atomic(&self.config_path);
                return Err(anyhow!("persist bot identity: {e}"));
            }
        }

        // Reconcile owner migration transactionally.
        if let Some(owner_user_id) = next.telegram.access.owner_user_id {
            let migration = match self.app.storage.ensure_telegram_owner(owner_user_id) {
                Ok(m) => m,
                Err(e) => {
                    if staged_secret {
                        let _ = SecretStore::new(next.paths.secrets_dir.clone())
                            .remove(TELEGRAM_TOKEN_KEY);
                    }
                    let _ = old.save_atomic(&self.config_path);
                    return Err(anyhow!("ensure telegram owner: {e}"));
                }
            };
            if migration.requires_file_reconcile {
                if let Err(e) = (|| -> Result<()> {
                    MemoryStore::with_workspace(
                        self.app.storage.clone(),
                        self.app.identity.clone(),
                    )
                    .reconcile(&migration.owner_id)?;
                    FilesystemSkills::new(
                        self.app.identity.clone(),
                        std::sync::Arc::new(SkillStore::new(self.app.storage.clone())),
                    )
                    .reconcile(&migration.owner_id)?;
                    Ok(())
                })() {
                    // DB owner already created; file reconcile failure is not rolled back fully
                    // but we avoid publishing ConfigReloaded and surface error for operator retry.
                    return Err(anyhow!("reconcile owner files: {e}"));
                }
            }
        }
        *self.app.config.write().await = next;
        self.app.events.publish(AppEvent::ConfigReloaded);

        Ok(TelegramConfigureResult {
            applied: true,
            tested: input.test_connection,
            status: self.status().await?,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionAiConfigInput {
    pub session_id: String,
    pub provider: String,
    pub account_or_profile_id: Option<String>,
    pub model: String,
}

#[derive(Clone)]
pub struct SessionAiService {
    storage: Arc<Storage>,
    providers: Arc<ProviderRegistry>,
}

impl SessionAiService {
    pub fn new(storage: Arc<Storage>, providers: Arc<ProviderRegistry>) -> Self {
        Self { storage, providers }
    }

    pub fn from_app(app: &AppState) -> Self {
        Self::new(app.storage.clone(), app.providers.clone())
    }

    /// Apply AI selection to exactly one owner session. No frontend active
    /// pointer is consulted or changed; CLI, WebUI and Telegram can therefore
    /// reuse this operation without inheriting each other's session state.
    pub fn apply(&self, owner: &str, input: SessionAiConfigInput) -> Result<SessionRecord> {
        let session = self
            .storage
            .session(owner, &input.session_id)?
            .ok_or_else(|| anyhow!("session not found for owner"))?;
        if session.archived {
            return Err(anyhow!(
                "cannot change AI configuration for an archived session"
            ));
        }
        let provider = match input.provider.trim().to_ascii_lowercase().as_str() {
            "codex" => "codex",
            "antigravity" | "agy" => "antigravity",
            "custom" => "custom",
            _ => return Err(anyhow!("unknown provider")),
        };
        let model = input.model.trim();
        if model.is_empty() || model.chars().count() > 200 {
            return Err(anyhow!("model is required"));
        }
        let binding = input
            .account_or_profile_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("account/profile id is required"))?;

        if provider == "custom" {
            let profiles = ProviderProfileStore::new(self.storage.clone());
            let profile = profiles
                .get(owner, binding)?
                .ok_or_else(|| anyhow!("Custom profile not found for owner"))?;
            let record = profiles
                .model(&profile.profile_id, model)?
                .ok_or_else(|| anyhow!("model has not been discovered for this Custom profile"))?;
            // P0-4: never silently activate an unprobed model as ChatOnly.
            // Unknown means probe budget was not spent; require exact probe first.
            if record.native_tools_state == "unknown"
                && record.structured_output_state == "unknown"
                && record.vision_state == "unknown"
                && record.file_input_state == "unknown"
                && record.evidence.contains("probe budget not spent")
                || record.probed_at.is_empty()
            {
                return Err(anyhow!(
                    "capability_probe_required: model '{model}' has not been probed for this Custom profile; run Test for this exact model first"
                ));
            }
            if record.tool_protocol == "chat_only"
                && record.native_tools_state == "unknown"
                && record.evidence.contains("not been probed")
            {
                return Err(anyhow!(
                    "capability_probe_required: model '{model}' capability is Unknown, probe exact model before activation"
                ));
            }
            self.storage.set_session_provider(
                owner,
                &input.session_id,
                provider,
                Some(&profile.profile_id),
                model,
            )?;
        } else {
            let account = self
                .storage
                .account_for_owner(owner, binding)?
                .ok_or_else(|| anyhow!("provider account not found for owner"))?;
            if account.provider != provider {
                return Err(anyhow!("account does not belong to selected provider"));
            }
            let models = self.providers.models(provider)?;
            if !models.iter().any(|candidate| candidate == model) {
                return Err(anyhow!("model is not available for selected provider"));
            }
            self.storage.activate_account(
                owner,
                &input.session_id,
                &account.id,
                provider,
                model,
            )?;
        }
        self.storage
            .session(owner, &input.session_id)?
            .ok_or_else(|| anyhow!("updated session disappeared"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    async fn test_service() -> (TelegramSetupService, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let mut config = AppConfig::default();
        config.storage.database = directory.path().join("xiao.db");
        config.paths.data_dir = directory.path().join("data");
        config.paths.logs_dir = directory.path().join("logs");
        config.paths.secrets_dir = directory.path().join("secrets");
        let config_path = directory.path().join("config.toml");
        config.save_atomic(&config_path).unwrap();
        let app = AppState::build_from_path(config, &config_path)
            .await
            .unwrap();
        (TelegramSetupService::new(app, &config_path), directory)
    }

    #[tokio::test]
    async fn telegram_token_is_write_only_and_owner_change_requires_confirmation() {
        let (service, _directory) = test_service().await;
        let first = service
            .configure(TelegramConfigureInput {
                enabled: Some(true),
                bot_token: Some("123456:WRITE_ONLY_TOKEN_SENTINEL".into()),
                owner_user_id: Some(42),
                allowed_chat_ids: Some(vec![-200, -100, -100]),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(first.status.owner_user_id, Some(42));
        assert_eq!(first.status.allowed_chat_ids, vec![-200, -100]);
        assert!(first.status.token_configured);
        let serialized = serde_json::to_string(&first.status).unwrap();
        assert!(!serialized.contains("WRITE_ONLY_TOKEN_SENTINEL"));

        let rejected = service
            .configure(TelegramConfigureInput {
                owner_user_id: Some(43),
                ..Default::default()
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(rejected.contains("explicit confirmation"));
        assert_eq!(service.status().await.unwrap().owner_user_id, Some(42));

        service
            .configure(TelegramConfigureInput {
                owner_user_id: Some(43),
                confirm_owner_change: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(service.status().await.unwrap().owner_user_id, Some(43));
    }
}
