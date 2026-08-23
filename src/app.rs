use std::{sync::Arc, time::Instant};

use anyhow::Result;
use tokio::sync::RwLock;

use crate::{
    auth::{AuthEvent, AuthManager},
    command::CommandCore,
    config::AppConfig,
    event::EventBus,
    identity::IdentityWorkspace,
    providers::{ProviderRegistry, ProviderState},
    runtime::{EnvironmentProbe, RuntimeState},
    session::SessionManager,
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
    pub storage: Arc<Storage>,
    pub sessions: Arc<SessionManager>,
    pub auth: Arc<AuthManager>,
    pub providers: Arc<ProviderRegistry>,
    pub commands: Arc<CommandCore>,
    pub health: Arc<HealthState>,
    pub events: Arc<EventBus>,
    pub identity: Arc<IdentityWorkspace>,
    pub runtime: Arc<RuntimeState>,
}

impl AppState {
    pub async fn build(config: AppConfig) -> Result<Self> {
        config.validate()?;
        let config = Arc::new(RwLock::new(config));
        let cfg = config.read().await.clone();
        let storage = Arc::new(Storage::open(&cfg.storage.database)?);
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
        let auth = Arc::new(AuthManager::with_config(
            storage.clone(),
            cfg.paths.secrets_dir.clone(),
            config.clone(),
        ));
        let providers = Arc::new(ProviderRegistry::new(cfg.clone(), auth.clone()));
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
        ));
        Ok(Self {
            config,
            storage,
            sessions,
            auth,
            providers,
            commands,
            health,
            events,
            identity,
            runtime,
        })
    }
}
