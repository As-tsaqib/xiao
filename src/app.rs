use std::{sync::Arc, time::Instant};

use anyhow::Result;
use tokio::sync::RwLock;

use crate::{
    attachments::AttachmentManager,
    auth::{AuthEvent, AuthManager},
    command::CommandCore,
    config::AppConfig,
    event::EventBus,
    identity::IdentityWorkspace,
    memory::MemoryStore,
    owner::OwnerIdentity,
    providers::{ProviderProfileStore, ProviderRegistry, ProviderState},
    runtime::{EnvironmentProbe, RuntimeState},
    security::secrets::SecretStore,
    session::SessionManager,
    skills::{FilesystemSkills, SkillStore},
    storage::Storage,
};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayStatus {
    Running,
    Starting,
    Degraded,
    Error,
    Stopped,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthSnapshot {
    pub gateway: GatewayStatus,
    pub daemon_running: bool,
    pub db_healthy: bool,
    pub telegram_enabled: bool,
    pub telegram_polling: bool,
    pub telegram_last_update_at: Option<String>,
    pub providers_ready: usize,
    pub provider_states: std::collections::BTreeMap<String, ProviderState>,
    pub uptime_seconds: u64,
    pub memory_bytes: Option<u64>,
    pub version: String,
}

#[derive(Debug)]
pub struct HealthState {
    started: Instant,
    telegram_polling: RwLock<bool>,
    telegram_last_update_at: RwLock<Option<String>>,
}

impl HealthState {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            telegram_polling: RwLock::new(false),
            telegram_last_update_at: RwLock::new(None),
        }
    }

    pub async fn set_telegram_polling(&self, value: bool) {
        *self.telegram_polling.write().await = value;
    }

    pub async fn mark_telegram_update(&self) {
        *self.telegram_last_update_at.write().await = Some(chrono::Utc::now().to_rfc3339());
    }

    pub async fn snapshot(
        &self,
        config: &AppConfig,
        db_healthy: bool,
        provider_states: std::collections::BTreeMap<String, ProviderState>,
    ) -> HealthSnapshot {
        let providers_ready = provider_states
            .values()
            .filter(|s| matches!(s, ProviderState::Ready))
            .count();
        let telegram_polling = *self.telegram_polling.read().await;
        let gateway = if !config.gateway.enabled {
            GatewayStatus::Stopped
        } else if !db_healthy {
            GatewayStatus::Error
        } else if (config.telegram.enabled && !telegram_polling) || providers_ready == 0 {
            GatewayStatus::Degraded
        } else {
            GatewayStatus::Running
        };
        HealthSnapshot {
            gateway,
            daemon_running: true,
            db_healthy,
            telegram_enabled: config.telegram.enabled,
            telegram_polling,
            telegram_last_update_at: self.telegram_last_update_at.read().await.clone(),
            providers_ready,
            provider_states,
            uptime_seconds: self.started.elapsed().as_secs(),
            memory_bytes: process_memory_bytes(),
            version: crate::VERSION.to_owned(),
        }
    }
}

impl Default for HealthState {
    fn default() -> Self {
        Self::new()
    }
}

fn process_memory_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kib = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some(kib * 1024)
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    /// Source config path when the daemon was started from a durable file.
    /// Tests/embedded callers may intentionally leave this unset.
    pub config_path: Option<Arc<std::path::PathBuf>>,
    pub storage: Arc<Storage>,
    pub sessions: Arc<SessionManager>,
    pub auth: Arc<AuthManager>,
    pub providers: Arc<ProviderRegistry>,
    pub commands: Arc<CommandCore>,
    pub health: Arc<HealthState>,
    pub events: Arc<EventBus>,
    pub identity: Arc<IdentityWorkspace>,
    pub runtime: Arc<RuntimeState>,
    pub attachments: Arc<AttachmentManager>,
}

impl AppState {
    pub async fn build(config: AppConfig) -> Result<Self> {
        Self::build_inner(config, None).await
    }

    pub async fn build_from_path(
        config: AppConfig,
        config_path: impl Into<std::path::PathBuf>,
    ) -> Result<Self> {
        Self::build_inner(config, Some(Arc::new(config_path.into()))).await
    }

    async fn build_inner(
        mut config: AppConfig,
        config_path: Option<Arc<std::path::PathBuf>>,
    ) -> Result<Self> {
        config.telegram.access.migrate_legacy_owner();
        config.validate()?;
        let config = Arc::new(RwLock::new(config));
        let mut cfg = config.read().await.clone();
        let storage = Arc::new(Storage::open(&cfg.storage.database)?);
        // Import legacy file/config Telegram state once into the authoritative
        // control-plane row. Subsequent setup changes use only SQLite plus
        // immutable secret references; the TOML remains a compatibility
        // snapshot rather than a second mutable authority.
        if storage.telegram_control_needs_legacy_import()? {
            let secrets = SecretStore::new(cfg.paths.secrets_dir.clone());
            let reference = secrets
                .get("telegram-bot-token")?
                .map(|token| secrets.put_versioned("telegram-bot-token", &token))
                .transpose()?;
            let identity = storage.setting("telegram_bot_identity")?;
            storage.import_legacy_telegram_state(
                cfg.telegram.enabled,
                cfg.telegram.access.owner_user_id,
                &cfg.telegram.access.allowed_chat_ids,
                reference.as_deref(),
                identity.as_deref(),
            )?;
        }
        // SQLite is the authoritative control plane after the one-time legacy
        // import above. Refresh the in-memory compatibility config from that
        // row so a failed/stale TOML snapshot cannot disable Telegram or
        // reintroduce an old owner binding after restart. This does not make
        // the TOML a second authority; it is only a runtime projection.
        if let Some(control) = storage.telegram_control_state()? {
            cfg.telegram.enabled = control.enabled;
            cfg.telegram.access.owner_user_id = control.owner_user_id;
            cfg.telegram.access.allowed_chat_ids = control.allowed_chat_ids;
            cfg.telegram.access.allowed_user_ids.clear();
            *config.write().await = cfg.clone();
        }
        let identity = Arc::new(IdentityWorkspace::new(cfg.paths.data_dir.clone()));
        let runtime = Arc::new(RuntimeState::initialize(
            identity.clone(),
            EnvironmentProbe::real(),
        )?);
        let environment = runtime.environment();
        storage.record_environment_probe(
            &serde_json::to_string(&environment)?,
            &environment.probed_at,
        )?;
        let sessions = Arc::new(SessionManager::new(storage.clone()));
        let attachments = Arc::new(AttachmentManager::new(
            storage.clone(),
            cfg.paths.data_dir.clone(),
            cfg.attachments.clone(),
        )?);
        // Startup reconciliation is safe after Storage::open has quarantined any
        // interrupted runs. Retention/orphan cleanup never crosses the private
        // attachment root and protects sessions with live runs.
        if let Err(error) = attachments.cleanup_retention(None) {
            tracing::warn!(error = %error, "attachment retention cleanup failed");
        }
        if let Err(error) = attachments.cleanup_orphans() {
            tracing::warn!(error = %error, "attachment orphan cleanup failed");
        }
        if let Err(error) = storage.cleanup_attachment_reservations() {
            tracing::warn!(error = %error, "attachment reservation cleanup failed");
        }
        if let Err(error) = storage.cleanup_orphan_attachment_reservations() {
            tracing::warn!(error = %error, "orphan attachment reservation cleanup failed");
        }
        let auth = Arc::new(AuthManager::with_config(
            storage.clone(),
            cfg.paths.secrets_dir.clone(),
            config.clone(),
        ));
        let profiles = ProviderProfileStore::new(storage.clone());
        // Migrate the singleton Custom compatibility profile only for the
        // already-authoritative Telegram binding. Do not consult the TOML
        // owner field here: changing that file must never rekey or rebind an
        // installation. Ambiguous legacy data remains fail-closed until the
        // setup service receives an explicit resolution decision.
        if storage.owner_resolution_candidates()?.is_empty()
            && storage
                .telegram_control_state()?
                .is_some_and(|control| control.owner_user_id.is_some())
        {
            let owner_id = storage.management_owner_id()?;
            let legacy_credential = auth
                .accounts(Some("custom"))?
                .into_iter()
                .find(|account| account.status == "connected")
                .map(|account| account.id);
            let _ =
                profiles.migrate_singleton(&owner_id, &cfg.providers.custom, legacy_credential)?;
        }
        let _ = profiles.migrate_legacy_credentials(auth.secrets(), &auth);
        let providers = Arc::new(ProviderRegistry::new(cfg.clone(), auth.clone()));
        for provider_id in providers.list() {
            // Custom capabilities are endpoint/model-specific and are stored
            // only after an actual wizard/model probe. Never stamp one global
            // config assumption across every discovered Custom model.
            if provider_id == "custom" {
                continue;
            }
            for model in providers.models(&provider_id).unwrap_or_default() {
                if let Ok(capabilities) = providers.capabilities(&provider_id, &model) {
                    let _ = storage.upsert_provider_capability(
                        &crate::storage::ProviderCapabilityRecord {
                            provider: provider_id.clone(),
                            model,
                            tool_protocol: capabilities.tool_protocol.as_str().into(),
                            native_tool_calls: capabilities.tool_protocol
                                == crate::providers::ToolProtocol::Native,
                            structured_output: capabilities.structured_output,
                            continuation: capabilities.continuation,
                            probe_status: "completed".into(),
                            probe_version: 1,
                            probed_at: chrono::Utc::now().to_rfc3339(),
                            evidence: capabilities.evidence,
                        },
                    );
                }
            }
        }
        let health = Arc::new(HealthState::new());
        let events = Arc::new(EventBus::new(128));
        {
            let mut auth_events = auth.subscribe();
            let bus = events.clone();
            tokio::spawn(async move {
                while let Ok(event) = auth_events.recv().await {
                    match event {
                        AuthEvent::Completed {
                            transaction_id,
                            account,
                        } => bus.publish(crate::event::AppEvent::AuthCompleted {
                            provider: account.provider,
                            transaction_id,
                            account_id: account.id,
                        }),
                        AuthEvent::Failed {
                            transaction_id,
                            provider,
                            error,
                        } => bus.publish(crate::event::AppEvent::AuthFailed {
                            provider,
                            transaction_id,
                            message: error,
                        }),
                    }
                }
            });
        }
        let commands = Arc::new(CommandCore::with_runtime(
            config.clone(),
            storage.clone(),
            sessions.clone(),
            providers.clone(),
            auth.clone(),
            health.clone(),
            events.clone(),
            runtime.clone(),
            attachments.clone(),
        ));
        spawn_learning_worker(storage.clone(), identity.clone());
        Ok(Self {
            config,
            config_path,
            storage,
            sessions,
            auth,
            providers,
            commands,
            health,
            events,
            identity,
            runtime,
            attachments,
        })
    }

    /// Resolve Telegram authorization identity separately from conversation
    /// scope and reconcile owner-global living state after any legacy rekey.
    pub fn resolve_telegram_owner(&self, telegram_user_id: i64) -> Result<OwnerIdentity> {
        let migration = self.storage.ensure_telegram_owner(telegram_user_id)?;
        let owner = OwnerIdentity::from_owner_id(migration.owner_id);
        MemoryStore::with_workspace(self.storage.clone(), self.identity.clone())
            .reconcile(owner.as_str())?;
        FilesystemSkills::new(
            self.identity.clone(),
            Arc::new(SkillStore::new(self.storage.clone())),
        )
        .reconcile(owner.as_str())?;
        Ok(owner)
    }
}

fn spawn_learning_worker(storage: Arc<Storage>, identity: Arc<IdentityWorkspace>) {
    tokio::spawn(async move {
        let memory = Arc::new(crate::memory::MemoryStore::with_workspace(
            storage.clone(),
            identity,
        ));
        let evaluator = crate::learning::LearningEvaluator::new(
            Arc::new(crate::skills::SkillRegistry::new(Arc::new(
                crate::skills::SkillStore::new(storage.clone()),
            ))),
            Arc::new(crate::memory::MemoryEvaluator::new(memory)),
        );
        loop {
            match storage.claim_learning_job() {
                Ok(Some((id, owner, run, payload))) => {
                    let result = payload
                        .get("trace")
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("learning trace missing"))
                        .and_then(|value| serde_json::from_value(value).map_err(Into::into))
                        .and_then(|trace| evaluator.evaluate(&owner, &trace).map(|_| ()));
                    let _ = match &result {
                        Err(error) => {
                            storage.finish_learning_job(&id, "failed", Some(&error.to_string()))
                        }
                        Ok(()) => storage.finish_learning_job(&id, "succeeded", None),
                    };
                    let _ = storage.record_agent_run_event(
                        &run,
                        "background_learning",
                        0,
                        &serde_json::json!({"status":if result.is_ok(){"succeeded"}else{"failed"}}),
                    );
                }
                Ok(None) => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
                Err(error) => {
                    tracing::warn!(%error,"learning worker poll failed");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        memory::{MemoryScope, MemoryStore},
        providers::ProviderProfileStore,
        skills::{SkillCandidate, SkillStore},
        storage::{AttachmentChunkRecord, NewAttachmentRecord, ProviderProfileInput},
        telegram::TelegramScope,
    };

    #[tokio::test]
    async fn stable_owner_state_is_global_while_dm_group_and_topics_stay_isolated() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = AppConfig::default();
        config.storage.database = directory.path().join("xiao.db");
        config.paths.data_dir = directory.path().join("data");
        config.paths.logs_dir = directory.path().join("logs");
        config.paths.secrets_dir = directory.path().join("secrets");
        config.telegram.access.allowed_user_ids = vec![42];
        let app = AppState::build(config).await.unwrap();
        let owner = app.resolve_telegram_owner(42).unwrap();

        let dm = app
            .sessions
            .context_for_telegram(owner.as_str(), TelegramScope::new(42, None))
            .unwrap()
            .main;
        let group = app
            .sessions
            .context_for_telegram(owner.as_str(), TelegramScope::new(-1007, None))
            .unwrap()
            .main;
        let topic_a = app
            .sessions
            .context_for_telegram(owner.as_str(), TelegramScope::new(-1007, Some(10)))
            .unwrap()
            .main;
        let topic_b = app
            .sessions
            .context_for_telegram(owner.as_str(), TelegramScope::new(-1007, Some(20)))
            .unwrap()
            .main;
        let ids = [dm.id, group.id, topic_a.id, topic_b.id]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            ids.len(),
            4,
            "every TelegramScope needs its own conversation"
        );

        let memory = MemoryStore::with_workspace(app.storage.clone(), app.identity.clone());
        memory
            .upsert(
                owner.as_str(),
                MemoryScope::User,
                "preferences",
                "response.detail",
                "Prefer concise evidence-backed replies",
                1.0,
                "owner_explicit",
                Some(&ids.iter().next().unwrap().clone()),
            )
            .unwrap();
        for _scope in [
            TelegramScope::new(42, None),
            TelegramScope::new(-1007, None),
            TelegramScope::new(-1007, Some(10)),
            TelegramScope::new(-1007, Some(20)),
        ] {
            assert_eq!(memory.list(owner.as_str(), None, 10).unwrap().len(), 1);
        }

        SkillStore::new(app.storage.clone())
            .create_or_update(
                owner.as_str(),
                SkillCandidate {
                    name: "verify-release".into(),
                    summary: "Verify a Xiao release safely".into(),
                    when_to_use: "Before publishing a release".into(),
                    prerequisites: "Rust toolchain".into(),
                    procedure: "Run bounded validation commands".into(),
                    pitfalls: "Never claim commands that were not run".into(),
                    verification: "All required commands exit successfully".into(),
                },
                Some(&ids.iter().next().unwrap().clone()),
            )
            .unwrap();
        assert_eq!(
            SkillStore::new(app.storage.clone())
                .search(owner.as_str(), "verify release", 5)
                .unwrap()
                .len(),
            1
        );

        let profile = ProviderProfileStore::new(app.storage.clone())
            .create(ProviderProfileInput {
                profile_id: None,
                owner_id: owner.owner_id.clone(),
                alias: "owner-global".into(),
                endpoint: "https://example.invalid/v1".into(),
                protocol: "openai_chat_completions".into(),
                credential_ref: None,
                api_key_ref: None,
                safe_headers_json: "{}".into(),
                secret_headers_ref: None,
            })
            .unwrap();
        assert_eq!(profile.owner_id, owner.owner_id);
        assert_eq!(
            ProviderProfileStore::new(app.storage.clone())
                .list(owner.as_str())
                .unwrap()
                .len(),
            1
        );

        // Telegram authentication is a replaceable binding. Replacing it
        // must leave the durable installation owner and every owner-scoped
        // record under the same key.
        let replacement = app.resolve_telegram_owner(43).unwrap();
        assert_eq!(replacement, owner);
        assert_eq!(app.storage.management_owner_id().unwrap(), owner.owner_id);
        assert_eq!(
            memory.list(replacement.as_str(), None, 10).unwrap().len(),
            1
        );
        assert_eq!(
            SkillStore::new(app.storage.clone())
                .list_all(replacement.as_str(), 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            ProviderProfileStore::new(app.storage.clone())
                .list(replacement.as_str())
                .unwrap()
                .len(),
            1
        );
        assert!(app
            .storage
            .with_conn(|connection| {
                let binding: String = connection.query_row(
                    "SELECT external_id FROM owner_bindings WHERE owner_id=? AND binding_kind='telegram_user'",
                    rusqlite::params![replacement.owner_id],
                    |row| row.get(0),
                )?;
                assert_eq!(binding, "43");
                Ok(())
            })
            .is_ok());
    }

    #[tokio::test]
    async fn webui_first_local_owner_is_transactionally_claimed_by_telegram_owner() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = AppConfig::default();
        config.storage.database = directory.path().join("local-first.db");
        config.paths.data_dir = directory.path().join("data");
        config.paths.logs_dir = directory.path().join("logs");
        config.paths.secrets_dir = directory.path().join("secrets");
        let app = AppState::build(config).await.unwrap();
        let local = app.storage.management_owner_id().unwrap();
        assert!(local.starts_with("owner:installation:"));

        let credential = app
            .auth
            .configure_api_key("custom", "local-first", "LOCAL_FIRST_SECRET")
            .unwrap();
        app.storage
            .set_account_owner(&local, &credential.id)
            .unwrap();
        let profile = ProviderProfileStore::new(app.storage.clone())
            .create(ProviderProfileInput {
                profile_id: None,
                owner_id: local.clone(),
                alias: "local-first".into(),
                endpoint: "https://local-first.example/v1".into(),
                protocol: "openai_chat_completions".into(),
                credential_ref: Some(credential.id.clone()),
                api_key_ref: None,
                safe_headers_json: r#"{"x-client":"local-first"}"#.into(),
                secret_headers_ref: None,
            })
            .unwrap();
        let session = app
            .storage
            .create_session(
                &local,
                "WebUI first",
                "custom",
                Some(&profile.profile_id),
                "local-model",
                false,
                None,
            )
            .unwrap();
        app.storage
            .append_message(&local, &session.id, "user", "preserve local raw history")
            .unwrap();
        let run = app
            .storage
            .create_agent_run(
                &local,
                &session.id,
                "custom",
                "local-model",
                Some("preserve local audit"),
            )
            .unwrap();
        app.storage
            .set_agent_run_status(&local, &run, "completed", None)
            .unwrap();
        app.storage
            .audit(&local, "local_first", "preserve this event")
            .unwrap();

        MemoryStore::with_workspace(app.storage.clone(), app.identity.clone())
            .upsert(
                &local,
                MemoryScope::User,
                "preferences",
                "local.preference",
                "Keep owner-global local state",
                1.0,
                "owner_explicit",
                Some(&session.id),
            )
            .unwrap();
        FilesystemSkills::new(
            app.identity.clone(),
            Arc::new(SkillStore::new(app.storage.clone())),
        )
        .learn(
            &local,
            SkillCandidate {
                name: "local-owner-recovery".into(),
                summary: "Recover owner-global installation state".into(),
                when_to_use: "When Telegram is configured after WebUI use".into(),
                prerequisites: "A local Xiao owner".into(),
                procedure: "Rekey durable state in one transaction".into(),
                pitfalls: "Do not split one owner into tenants".into(),
                verification: "History remains visible under the Telegram owner".into(),
            },
            Some(&session.id),
        )
        .unwrap();
        app.storage
            .insert_attachment(NewAttachmentRecord {
                attachment_id: "local-attachment",
                owner_id: &local,
                session_id: &session.id,
                telegram_file_id: None,
                telegram_unique_id: None,
                original_name: "local.txt",
                declared_mime: Some("text/plain"),
                detected_mime: "text/plain",
                kind: "document",
                size_bytes: 24,
                sha256: &"a".repeat(64),
                local_path: "/private/local-attachment/local.txt",
            })
            .unwrap();
        app.storage
            .replace_attachment_chunks(
                &local,
                "local-attachment",
                &[AttachmentChunkRecord {
                    attachment_id: "local-attachment".into(),
                    chunk_no: 0,
                    page_no: None,
                    start_offset: Some(0),
                    end_offset: Some(24),
                    text: "copper migration sentinel".into(),
                }],
            )
            .unwrap();

        let owner = app.resolve_telegram_owner(42).unwrap();
        assert_eq!(owner.as_str(), local);
        assert_eq!(app.storage.management_owner_id().unwrap(), owner.as_str());
        assert!(app
            .storage
            .account_for_owner(owner.as_str(), &credential.id)
            .unwrap()
            .is_some());
        let migrated_profile = ProviderProfileStore::new(app.storage.clone())
            .get(owner.as_str(), &profile.profile_id)
            .unwrap()
            .unwrap();
        assert_eq!(migrated_profile.credential_ref, Some(credential.id));
        assert_eq!(
            app.storage
                .session(owner.as_str(), &session.id)
                .unwrap()
                .unwrap()
                .message_count,
            1
        );
        assert_eq!(
            app.storage
                .agent_run(owner.as_str(), &run)
                .unwrap()
                .unwrap()
                .status,
            "completed"
        );
        assert_eq!(
            app.storage.audit_events(owner.as_str(), 10).unwrap().len(),
            1
        );
        assert_eq!(
            MemoryStore::with_workspace(app.storage.clone(), app.identity.clone())
                .list(owner.as_str(), None, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            SkillStore::new(app.storage.clone())
                .list_all(owner.as_str(), 10)
                .unwrap()
                .len(),
            1
        );
        assert!(app
            .storage
            .attachment(owner.as_str(), "local-attachment")
            .unwrap()
            .is_some());
        assert_eq!(
            app.storage
                .search_attachment_chunks(owner.as_str(), &session.id, "copper sentinel", 5)
                .unwrap()
                .len(),
            1
        );
        let second = app.storage.ensure_telegram_owner(42).unwrap();
        assert_eq!(second.migrated_legacy_principals, 0);
        assert!(!second.requires_file_reconcile);
    }

    #[tokio::test]
    async fn representative_v025_state_migrates_transactionally_and_idempotently() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = AppConfig::default();
        config.storage.database = directory.path().join("legacy-v025.db");
        config.paths.data_dir = directory.path().join("data");
        config.paths.logs_dir = directory.path().join("logs");
        config.paths.secrets_dir = directory.path().join("secrets");
        config.providers.custom.enabled = true;
        config.providers.custom.name = Some("legacy-custom".into());
        config.providers.custom.base_url = Some("https://legacy.example/v1".into());
        config.providers.custom.models = vec!["legacy-model".into()];
        config.providers.custom.default_model = Some("legacy-model".into());

        let legacy_owner = "telegram:-1007:42";
        let (legacy_session_id, legacy_run_id, credential_id) = {
            let app = AppState::build(config.clone()).await.unwrap();
            let credential = app
                .auth
                .configure_api_key("custom", "legacy", "LEGACY_SECRET_SENTINEL")
                .unwrap();
            let session = app
                .storage
                .create_session(
                    legacy_owner,
                    "Legacy topic",
                    "custom",
                    None,
                    "legacy-model",
                    false,
                    None,
                )
                .unwrap();
            app.storage
                .append_message(
                    legacy_owner,
                    &session.id,
                    "user",
                    "raw v0.2.5 history survives",
                )
                .unwrap();
            let run = app
                .storage
                .create_agent_run(
                    legacy_owner,
                    &session.id,
                    "custom",
                    "legacy-model",
                    Some("legacy audited task"),
                )
                .unwrap();
            app.storage
                .set_agent_run_status(legacy_owner, &run, "completed", None)
                .unwrap();
            MemoryStore::with_workspace(app.storage.clone(), app.identity.clone())
                .upsert(
                    legacy_owner,
                    MemoryScope::User,
                    "preferences",
                    "migration.preference",
                    "Preserve this canonical preference",
                    1.0,
                    "owner_explicit",
                    Some(&session.id),
                )
                .unwrap();
            FilesystemSkills::new(
                app.identity.clone(),
                Arc::new(SkillStore::new(app.storage.clone())),
            )
            .learn(
                legacy_owner,
                SkillCandidate {
                    name: "legacy-release-check".into(),
                    summary: "Check a migrated legacy release".into(),
                    when_to_use: "When validating migrated state".into(),
                    prerequisites: "Legacy database".into(),
                    procedure: "Inspect preserved history and verify current state".into(),
                    pitfalls: "Do not duplicate canonical skills".into(),
                    verification: "One canonical skill remains".into(),
                },
                Some(&session.id),
            )
            .unwrap();
            (session.id, run, credential.id)
        };

        config.telegram.access.allowed_user_ids = vec![42];
        let migrated = AppState::build(config.clone()).await.unwrap();
        let owner = migrated.resolve_telegram_owner(42).unwrap();
        let session = migrated
            .storage
            .session(owner.as_str(), &legacy_session_id)
            .unwrap()
            .unwrap();
        assert_eq!(session.message_count, 1);
        assert_eq!(
            migrated
                .storage
                .agent_run(owner.as_str(), &legacy_run_id)
                .unwrap()
                .unwrap()
                .status,
            "completed"
        );
        assert_eq!(
            MemoryStore::with_workspace(migrated.storage.clone(), migrated.identity.clone())
                .list(owner.as_str(), None, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            SkillStore::new(migrated.storage.clone())
                .list_all(owner.as_str(), 10)
                .unwrap()
                .len(),
            1
        );
        let profiles = ProviderProfileStore::new(migrated.storage.clone())
            .list(owner.as_str())
            .unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(
            profiles[0].credential_ref.as_deref(),
            Some(credential_id.as_str())
        );
        assert_eq!(
            migrated
                .auth
                .credential(&credential_id)
                .unwrap()
                .unwrap()
                .api_key
                .as_deref(),
            Some("LEGACY_SECRET_SENTINEL")
        );
        assert_eq!(
            migrated
                .storage
                .telegram_scope_for_session(owner.as_str(), &legacy_session_id)
                .unwrap(),
            Some((-1007, None))
        );
        drop(migrated);

        let reopened = AppState::build(config).await.unwrap();
        let owner = reopened.resolve_telegram_owner(42).unwrap();
        assert_eq!(
            ProviderProfileStore::new(reopened.storage.clone())
                .list(owner.as_str())
                .unwrap()
                .len(),
            1
        );
        assert!(reopened
            .storage
            .session(owner.as_str(), &legacy_session_id)
            .unwrap()
            .is_some());
    }
}
