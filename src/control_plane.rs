use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::{
    app::AppState,
    event::AppEvent,
    memory::MemoryStore,
    providers::{ProviderProfileStore, ProviderRegistry},
    security::redact::redact_text,
    security::secrets::SecretStore,
    skills::{FilesystemSkills, SkillStore},
    storage::{SessionRecord, Storage, TelegramControlState},
    telegram::{client::TelegramClient, types::BotIdentity},
};

const TELEGRAM_TOKEN_KEY: &str = "telegram-bot-token";

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
    pub resolve_legacy_owners: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TelegramConfigureResult {
    pub applied: bool,
    pub tested: bool,
    pub status: TelegramSetupStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
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
        let control = self
            .app
            .storage
            .telegram_control_state()?
            .unwrap_or(TelegramControlState {
                enabled: cfg.telegram.enabled,
                owner_user_id: cfg.telegram.access.owner_user_id,
                allowed_chat_ids: cfg.telegram.access.allowed_chat_ids.clone(),
                bot_token_ref: None,
                bot_identity_json: None,
                updated_at: String::new(),
            });
        let candidates = self.app.storage.owner_resolution_candidates()?;
        let owner_state = if !candidates.is_empty() {
            OwnerSetupState::ResolutionRequired
        } else if control.owner_user_id.is_some() {
            OwnerSetupState::Configured
        } else {
            OwnerSetupState::SetupRequired
        };
        let secrets = SecretStore::new(cfg.paths.secrets_dir.clone());
        // Once the schema exists, only the immutable ref recorded in SQLite
        // is authoritative. The legacy unversioned secret is imported during
        // AppState bootstrap and is never consulted as a fallback here.
        let token_configured = control
            .bot_token_ref
            .as_deref()
            .map(|reference| secrets.get(reference).map(|value| value.is_some()))
            .transpose()?
            .unwrap_or(false);
        let bot = control
            .bot_identity_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<BotIdentity>(raw).ok());
        Ok(TelegramSetupStatus {
            enabled: control.enabled,
            owner_user_id: control.owner_user_id,
            owner_state,
            legacy_candidate_count: candidates.len(),
            allowed_chat_ids: control.allowed_chat_ids,
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
            None => {
                let control = self.app.storage.telegram_control_state()?;
                let secrets = SecretStore::new(cfg.paths.secrets_dir);
                match control.and_then(|state| state.bot_token_ref) {
                    Some(reference) => secrets
                        .get(&reference)?
                        .ok_or_else(|| anyhow!("Telegram bot token reference is unavailable"))?,
                    None => {
                        return Err(anyhow!(
                        "Telegram bot token is not configured in the authoritative control plane"
                    ))
                    }
                }
            }
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
        let current = self
            .app
            .storage
            .telegram_control_state()?
            .unwrap_or(TelegramControlState {
                enabled: old.telegram.enabled,
                owner_user_id: old.telegram.access.owner_user_id,
                allowed_chat_ids: old.telegram.access.allowed_chat_ids.clone(),
                bot_token_ref: None,
                bot_identity_json: None,
                updated_at: String::new(),
            });
        let owner_user_id = input.owner_user_id.or(current.owner_user_id);
        if owner_user_id.is_some_and(|id| id <= 0) {
            return Err(anyhow!("Telegram owner user id must be positive"));
        }
        if current
            .owner_user_id
            .is_some_and(|id| Some(id) != input.owner_user_id)
            && input.owner_user_id.is_some()
            && !input.confirm_owner_change
        {
            return Err(anyhow!(
                "changing Telegram owner requires explicit confirmation"
            ));
        }
        let mut allowed_chat_ids = input
            .allowed_chat_ids
            .unwrap_or_else(|| current.allowed_chat_ids.clone());
        if allowed_chat_ids.contains(&0) {
            return Err(anyhow!("allowed chat ids cannot contain zero"));
        }
        allowed_chat_ids.sort_unstable();
        allowed_chat_ids.dedup();
        let enabled = input.enabled.unwrap_or(current.enabled);
        let secrets = SecretStore::new(old.paths.secrets_dir.clone());
        let supplied_token = input
            .bot_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let mut new_ref = current.bot_token_ref.clone();
        let mut created_ref = None;
        if let Some(token) = supplied_token {
            let reference = secrets.put_versioned(TELEGRAM_TOKEN_KEY, token)?;
            new_ref = Some(reference.clone());
            created_ref = Some(reference);
        } else if new_ref.is_none() {
            if let Some(token) = secrets.get(TELEGRAM_TOKEN_KEY)? {
                let reference = secrets.put_versioned(TELEGRAM_TOKEN_KEY, &token)?;
                new_ref = Some(reference.clone());
                created_ref = Some(reference);
            }
        }
        if let Ok(injection) = std::env::var("XIAO_INJECT_TELEGRAM_FAILURE") {
            if injection.contains("secret_stage") || injection == "all" {
                if let Some(reference) = created_ref.as_deref() {
                    let _ = secrets.remove(reference);
                }
                return Err(anyhow!("injected failure: secret_stage"));
            }
        }
        // Probe the staged token exactly once. The result is also the bot
        // identity committed with the control-plane row; a second pre-commit
        // getMe would make a flaky provider look like a split transition.
        let staged_token = if let Some(reference) = created_ref.as_deref() {
            secrets.get(reference)?
        } else {
            None
        };
        let probe_token = supplied_token.map(str::to_owned).or(staged_token);
        if let Ok(injection) = std::env::var("XIAO_INJECT_TELEGRAM_FAILURE") {
            if injection == "probe" {
                if let Some(reference) = created_ref.as_deref() {
                    let _ = secrets.remove(reference);
                }
                return Err(anyhow!("injected failure: probe"));
            }
        }
        let bot = if input.test_connection {
            Some(
                self.test_connection(probe_token.as_deref())
                    .await
                    .inspect_err(|_| {
                        if let Some(reference) = created_ref.as_deref() {
                            let _ = secrets.remove(reference);
                        }
                    })?,
            )
        } else {
            current
                .bot_identity_json
                .as_deref()
                .and_then(|raw| serde_json::from_str::<BotIdentity>(raw).ok())
        };
        let identity_json = bot.as_ref().map(serde_json::to_string).transpose()?;
        if let Ok(injection) = std::env::var("XIAO_INJECT_TELEGRAM_FAILURE") {
            if injection.contains("db") || injection == "all" {
                if let Some(reference) = created_ref.as_deref() {
                    let _ = secrets.remove(reference);
                }
                return Err(anyhow!("injected failure: db"));
            }
        }
        let migration = match self.app.storage.commit_telegram_control_plane(
            enabled,
            owner_user_id,
            &allowed_chat_ids,
            new_ref.as_deref(),
            identity_json.as_deref(),
            input.resolve_legacy_owners,
        ) {
            Ok(migration) => migration,
            Err(error) => {
                if let Some(reference) = created_ref.as_deref() {
                    let _ = secrets.remove(reference);
                }
                return Err(error);
            }
        };
        let mut warnings = Vec::new();
        if migration.requires_file_reconcile {
            if let Err(error) = (|| -> Result<()> {
                MemoryStore::with_workspace(self.app.storage.clone(), self.app.identity.clone())
                    .reconcile(&migration.owner_id)?;
                FilesystemSkills::new(
                    self.app.identity.clone(),
                    Arc::new(SkillStore::new(self.app.storage.clone())),
                )
                .reconcile(&migration.owner_id)?;
                Ok(())
            })() {
                warnings.push(format!(
                    "owner file reconciliation pending: {}",
                    redact_text(&error.to_string())
                ));
            }
        }
        if let (Some(old_ref), Some(new_ref)) =
            (current.bot_token_ref.as_deref(), new_ref.as_deref())
        {
            if old_ref != new_ref {
                let injected_cleanup_failure = std::env::var("XIAO_INJECT_TELEGRAM_FAILURE")
                    .ok()
                    .is_some_and(|value| value == "cleanup");
                if injected_cleanup_failure {
                    warnings
                        .push("obsolete Telegram secret cleanup pending: injected failure".into());
                } else if let Err(error) = secrets.remove(old_ref) {
                    warnings.push(format!(
                        "obsolete Telegram secret cleanup pending: {}",
                        redact_text(&error.to_string())
                    ));
                }
            }
        }
        let mut next = old.clone();
        next.telegram.enabled = enabled;
        next.telegram.access.owner_user_id = owner_user_id;
        next.telegram.access.allowed_chat_ids = allowed_chat_ids;
        next.telegram.access.allowed_user_ids.clear();
        if let Ok(injection) = std::env::var("XIAO_INJECT_TELEGRAM_FAILURE") {
            if injection.contains("config") || injection == "all" {
                warnings.push(
                    "config snapshot persistence pending; SQLite control state is authoritative"
                        .into(),
                );
            } else if let Err(error) = next.save_atomic(&self.config_path) {
                warnings.push(format!(
                    "config snapshot persistence pending: {}",
                    redact_text(&error.to_string())
                ));
            }
        } else if let Err(error) = next.save_atomic(&self.config_path) {
            warnings.push(format!(
                "config snapshot persistence pending: {}",
                redact_text(&error.to_string())
            ));
        }
        if !warnings.is_empty() {
            let _ = self.app.storage.audit(
                &migration.owner_id,
                "telegram_control_plane_warning",
                &redact_text(&warnings.join("; ")),
            );
        }
        *self.app.config.write().await = next;
        self.app.events.publish(AppEvent::ConfigReloaded);
        Ok(TelegramConfigureResult {
            applied: true,
            tested: input.test_connection,
            status: self.status().await?,
            warning: (!warnings.is_empty()).then(|| warnings.join("; ")),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomModelReadiness {
    Unprobed,
    AgentNative,
    AgentStructured,
    ChatOnly,
    Indeterminate,
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

    /// P0-1 helper: separate exact probe completeness from optional capabilities.
    /// Vision and file_input Unknown are NOT blockers for agent activation.
    pub fn model_readiness(
        record: &crate::storage::ProviderProfileModelRecord,
    ) -> CustomModelReadiness {
        match record.probe_status.as_str() {
            "unprobed" => return CustomModelReadiness::Unprobed,
            "indeterminate" => return CustomModelReadiness::Indeterminate,
            "completed" if !record.probed_at.trim().is_empty() => {}
            _ => return CustomModelReadiness::Unprobed,
        }
        if record.native_tools_state == "supported" && record.continuation_state == "supported" {
            return CustomModelReadiness::AgentNative;
        }
        if record.native_tools_state == "unsupported"
            && record.structured_output_state == "supported"
            && record.continuation_state == "supported"
        {
            return CustomModelReadiness::AgentStructured;
        }
        if record.native_tools_state == "unsupported"
            && record.structured_output_state == "unsupported"
            && record.continuation_state == "unsupported"
        {
            return CustomModelReadiness::ChatOnly;
        }
        CustomModelReadiness::Indeterminate
    }

    /// P0-4 helper: whether the stored model record has not been capability-probed.
    pub fn is_unprobed(record: &crate::storage::ProviderProfileModelRecord) -> bool {
        matches!(
            Self::model_readiness(record),
            CustomModelReadiness::Unprobed | CustomModelReadiness::Indeterminate
        )
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
            "custom" => "custom",
            "codex" | "antigravity" | "agy" => {
                return Err(anyhow!(
                    "provider_configuration_required: legacy provider is no longer supported; select a Custom profile and model"
                ))
            }
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
            // P0-1 / P0-4: enforce readiness semantics.
            // Vision/file Unknown is NOT a blocker.
            // Unprobed or Indeterminate requires exact-model probe.
            match Self::model_readiness(&record) {
                CustomModelReadiness::AgentNative
                | CustomModelReadiness::AgentStructured
                | CustomModelReadiness::ChatOnly => {
                    self.storage.set_session_provider(
                        owner,
                        &input.session_id,
                        provider,
                        Some(&profile.profile_id),
                        model,
                    )?;
                }
                CustomModelReadiness::Unprobed => {
                    return Err(anyhow!(
                        "capability_probe_required: model '{model}' has not been probed for this Custom profile; run exact-model probe first"
                    ));
                }
                CustomModelReadiness::Indeterminate => {
                    return Err(anyhow!(
                        "capability_probe_required: model '{model}' agent protocol capability is Indeterminate; probe exact model before activation"
                    ));
                }
            }
        }
        self.storage
            .session(owner, &input.session_id)?
            .ok_or_else(|| anyhow!("updated session disappeared"))
    }

    /// P0-4 / P1-1 bounded exact-model probe: uses selected profile's merged headers and API key.
    pub async fn probe_exact_model(
        &self,
        owner: &str,
        profile_id: &str,
        model: &str,
    ) -> Result<crate::storage::ProviderProfileModelRecord> {
        let profiles = ProviderProfileStore::new(self.storage.clone());
        let profile = profiles
            .get(owner, profile_id)?
            .ok_or_else(|| anyhow!("Custom profile not found for owner"))?;
        let api_key = match profile.credential_ref.as_deref() {
            Some(reference) => self
                .providers
                .auth()
                .credential(reference)?
                .and_then(|credential| credential.api_key)
                .filter(|key| !key.trim().is_empty()),
            None => None,
        };
        let headers = profile.merged_headers(self.providers.auth().secrets())?;
        let probe = crate::providers::probe_custom_capabilities(
            &profile.endpoint,
            &headers,
            api_key.as_deref(),
            &profile.protocol,
            model,
        )
        .await;
        let now = chrono::Utc::now().to_rfc3339();
        let mut models = profiles.models(profile_id)?;
        let pos = models
            .iter()
            .position(|m| m.model_id == model)
            .ok_or_else(|| anyhow!("model has not been discovered for this Custom profile"))?;
        models[pos] = crate::providers::profile_model_from_probe(profile_id, model, &probe, &now);
        profiles.replace_models(owner, profile_id, &models)?;
        Ok(models[pos].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use std::sync::OnceLock;

    static TELEGRAM_ENV_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

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

    #[test]
    fn custom_model_readiness_semantics_handles_optional_vision_and_file_capabilities() {
        use crate::storage::ProviderProfileModelRecord;

        let make_record = |native: &str,
                           structured: &str,
                           cont: &str,
                           vis: &str,
                           file: &str,
                           probed: &str,
                           ev: &str|
         -> ProviderProfileModelRecord {
            ProviderProfileModelRecord {
                profile_id: "p1".into(),
                model_id: "m1".into(),
                text_capable: true,
                vision_capable: vis == "supported",
                file_input_capable: file == "supported",
                native_tools: native == "supported",
                structured_output: structured == "supported",
                continuation: cont == "supported",
                native_tools_state: native.into(),
                structured_output_state: structured.into(),
                continuation_state: cont.into(),
                vision_state: vis.into(),
                file_input_state: file.into(),
                model_discovery: true,
                tool_protocol: if native == "supported" {
                    "native".into()
                } else if structured == "supported" {
                    "structured_json_fallback".into()
                } else {
                    "chat_only".into()
                },
                evidence: ev.into(),
                probe_status: "completed".into(),
                probe_version: 1,
                probed_at: probed.into(),
            }
        };

        // A. native=Supported, structured=Supported, continuation=Supported, vision=Unknown, file=Unknown => AgentNative
        let rec_a = make_record(
            "supported",
            "supported",
            "supported",
            "unknown",
            "unknown",
            "2026-01-01T00:00:00Z",
            "bounded custom probe",
        );
        assert_eq!(
            SessionAiService::model_readiness(&rec_a),
            CustomModelReadiness::AgentNative
        );
        assert!(!SessionAiService::is_unprobed(&rec_a));

        // B. native=Unsupported, structured=Supported, continuation=Supported, vision=Unknown, file=Unknown => AgentStructured
        let rec_b = make_record(
            "unsupported",
            "supported",
            "supported",
            "unknown",
            "unknown",
            "2026-01-01T00:00:00Z",
            "bounded custom probe",
        );
        assert_eq!(
            SessionAiService::model_readiness(&rec_b),
            CustomModelReadiness::AgentStructured
        );
        assert!(!SessionAiService::is_unprobed(&rec_b));

        // C. native=Unsupported, structured=Unsupported, continuation=Unsupported, completed probe => ChatOnly
        let rec_c = make_record(
            "unsupported",
            "unsupported",
            "unsupported",
            "unsupported",
            "unsupported",
            "2026-01-01T00:00:00Z",
            "bounded custom probe",
        );
        assert_eq!(
            SessionAiService::model_readiness(&rec_c),
            CustomModelReadiness::ChatOnly
        );
        assert!(!SessionAiService::is_unprobed(&rec_c));

        // D. native=Unknown, structured=Unknown, continuation=Unknown => Indeterminate
        let rec_d = make_record(
            "unknown",
            "unknown",
            "unknown",
            "unknown",
            "unknown",
            "2026-01-01T00:00:00Z",
            "bounded custom probe",
        );
        assert_eq!(
            SessionAiService::model_readiness(&rec_d),
            CustomModelReadiness::Indeterminate
        );
        assert!(SessionAiService::is_unprobed(&rec_d));

        // E. catalog discovery without exact capability probe => Unprobed
        let rec_e = make_record(
            "unknown",
            "unknown",
            "unknown",
            "unknown",
            "unknown",
            "",
            "model discovered; active capability probe budget not spent",
        );
        assert_eq!(
            SessionAiService::model_readiness(&rec_e),
            CustomModelReadiness::Unprobed
        );
        assert!(SessionAiService::is_unprobed(&rec_e));
    }

    #[tokio::test]
    async fn telegram_setup_config_snapshot_failure_commits_authoritative_state_with_warning() {
        let _guard = TELEGRAM_ENV_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let (service, _directory) = test_service().await;
        service
            .configure(TelegramConfigureInput {
                enabled: Some(true),
                bot_token: Some("123456:INITIAL_VALID_TOKEN".into()),
                owner_user_id: Some(100),
                ..Default::default()
            })
            .await
            .unwrap();

        // Inject failure during config write
        std::env::set_var("XIAO_INJECT_TELEGRAM_FAILURE", "config");
        let result = service
            .configure(TelegramConfigureInput {
                bot_token: Some("123456:NEW_CANDIDATE_TOKEN_THAT_FAILS".into()),
                owner_user_id: Some(101),
                confirm_owner_change: true,
                ..Default::default()
            })
            .await;
        std::env::remove_var("XIAO_INJECT_TELEGRAM_FAILURE");
        let applied = result.unwrap();
        assert!(applied.applied);
        assert!(applied
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("authoritative")));

        // SQLite is authoritative even when the compatibility TOML snapshot
        // cannot be persisted. There is no fake rollback after commit.
        let status = service.status().await.unwrap();
        assert_eq!(status.owner_user_id, Some(101));
        assert!(status.token_configured);
    }

    #[tokio::test]
    async fn telegram_probe_failure_keeps_old_token_binding_and_control_state_active() {
        let _guard = TELEGRAM_ENV_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let (service, _directory) = test_service().await;
        service
            .configure(TelegramConfigureInput {
                enabled: Some(true),
                bot_token: Some("123456:OLD_TOKEN_SENTINEL".into()),
                owner_user_id: Some(100),
                ..Default::default()
            })
            .await
            .unwrap();
        let before = service
            .app
            .storage
            .telegram_control_state()
            .unwrap()
            .unwrap();
        std::env::set_var("XIAO_INJECT_TELEGRAM_FAILURE", "probe");
        let error = service
            .configure(TelegramConfigureInput {
                bot_token: Some("123456:NEW_TOKEN_MUST_NOT_COMMIT".into()),
                owner_user_id: Some(101),
                confirm_owner_change: true,
                test_connection: true,
                ..Default::default()
            })
            .await
            .unwrap_err()
            .to_string();
        std::env::remove_var("XIAO_INJECT_TELEGRAM_FAILURE");
        assert!(error.contains("injected failure: probe"));
        let after = service
            .app
            .storage
            .telegram_control_state()
            .unwrap()
            .unwrap();
        assert_eq!(after.owner_user_id, before.owner_user_id);
        assert_eq!(after.bot_token_ref, before.bot_token_ref);
        assert_eq!(service.status().await.unwrap().owner_user_id, Some(100));
        let binding: String = service
            .app
            .storage
            .with_conn(|connection| {
                connection
                    .query_row(
                        "SELECT external_id FROM owner_bindings WHERE binding_kind='telegram_user'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(binding, "100");
    }

    #[tokio::test]
    async fn telegram_late_db_failure_rolls_back_binding_and_staged_token_as_one_transaction() {
        let (service, _directory) = test_service().await;
        service
            .configure(TelegramConfigureInput {
                enabled: Some(true),
                bot_token: Some("123456:OLD_DB_TOKEN".into()),
                owner_user_id: Some(100),
                ..Default::default()
            })
            .await
            .unwrap();
        let before = service
            .app
            .storage
            .telegram_control_state()
            .unwrap()
            .unwrap();
        service
            .app
            .storage
            .with_conn(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER reject_telegram_control_update
                     BEFORE UPDATE ON telegram_control_state
                     WHEN NEW.owner_user_id=101
                     BEGIN SELECT RAISE(FAIL,'synthetic Telegram DB failure'); END;",
                )?;
                Ok(())
            })
            .unwrap();
        let error = service
            .configure(TelegramConfigureInput {
                bot_token: Some("123456:NEW_DB_TOKEN_MUST_NOT_COMMIT".into()),
                owner_user_id: Some(101),
                confirm_owner_change: true,
                ..Default::default()
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("synthetic Telegram DB failure"));
        let after = service
            .app
            .storage
            .telegram_control_state()
            .unwrap()
            .unwrap();
        assert_eq!(after.owner_user_id, before.owner_user_id);
        assert_eq!(after.bot_token_ref, before.bot_token_ref);
        assert_eq!(service.status().await.unwrap().owner_user_id, Some(100));
        service
            .app
            .storage
            .with_conn(|connection| {
                connection.execute_batch("DROP TRIGGER reject_telegram_control_update;")?;
                Ok(())
            })
            .unwrap();
    }

    #[tokio::test]
    async fn telegram_post_commit_secret_cleanup_failure_is_success_with_warning() {
        let _guard = TELEGRAM_ENV_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let (service, _directory) = test_service().await;
        service
            .configure(TelegramConfigureInput {
                enabled: Some(true),
                bot_token: Some("123456:OLD_GC_TOKEN".into()),
                owner_user_id: Some(100),
                ..Default::default()
            })
            .await
            .unwrap();
        let old_ref = service
            .app
            .storage
            .telegram_control_state()
            .unwrap()
            .unwrap()
            .bot_token_ref
            .unwrap();
        let cfg = service.app.config.read().await.clone();
        let secrets = SecretStore::new(cfg.paths.secrets_dir);
        std::env::set_var("XIAO_INJECT_TELEGRAM_FAILURE", "cleanup");
        let result = service
            .configure(TelegramConfigureInput {
                bot_token: Some("123456:NEW_GC_TOKEN".into()),
                owner_user_id: Some(101),
                confirm_owner_change: true,
                ..Default::default()
            })
            .await
            .unwrap();
        std::env::remove_var("XIAO_INJECT_TELEGRAM_FAILURE");
        assert!(result.applied);
        assert!(result
            .warning
            .as_deref()
            .is_some_and(|warning| warning.contains("cleanup pending")));
        let current_ref = service
            .app
            .storage
            .telegram_control_state()
            .unwrap()
            .unwrap()
            .bot_token_ref
            .unwrap();
        assert_ne!(current_ref, old_ref);
        assert!(secrets.exists(&old_ref).unwrap());
        assert_eq!(service.status().await.unwrap().owner_user_id, Some(101));
    }
}
