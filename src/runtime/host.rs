use anyhow::Result;

use crate::{
    app::AppState,
    config::AppConfig,
    standalone::{CliPaths, RuntimeLayout, RuntimeLock},
};

pub struct RuntimeHost {
    app: AppState,
    config_path: std::path::PathBuf,
    _runtime_lock: RuntimeLock,
}

impl RuntimeHost {
    pub async fn bootstrap() -> Result<Self> {
        let paths = CliPaths::from_env()?;
        let config_path = paths.config.clone();
        if !config_path.is_file() {
            anyhow::bail!(
                "xiao config is missing at {}; run `xiao quickstart` first",
                config_path.display()
            );
        }
        let config = AppConfig::load(&config_path)?;
        let layout = RuntimeLayout::from_config(&paths, &config);
        let runtime_lock = RuntimeLock::acquire(&layout)?;

        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(
                config.daemon.log_level.clone(),
            ))
            .init();
        let app = AppState::build_from_path(config, config_path.clone()).await?;
        Ok(Self {
            app,
            config_path,
            _runtime_lock: runtime_lock,
        })
    }

    pub async fn run(self) -> Result<()> {
        let ipc_app = self.app.clone();
        let ipc_path = self.config_path.clone();
        let mut ipc_task = tokio::spawn(async move { crate::ipc::serve(ipc_app, ipc_path).await });
        let mut telegram_task = spawn_telegram(&self.app).await;
        let mut events = self.app.events.subscribe();

        tracing::info!(version = crate::VERSION, "xiao daemon started");
        loop {
            if let Some(mut telegram) = telegram_task.take() {
                tokio::select! {
                    _ = shutdown_signal() => {
                        telegram.abort();
                        tracing::info!("shutdown signal received");
                        break;
                    },
                    result = &mut ipc_task => {
                        telegram.abort();
                        log_task_exit("IPC", result);
                        break;
                    },
                    result = &mut telegram => {
                        log_task_exit("Telegram", result);
                    },
                    event = events.recv() => {
                        if matches!(event, Ok(crate::event::AppEvent::ConfigReloaded)) {
                            telegram.abort();
                            telegram_task = spawn_telegram(&self.app).await;
                        } else {
                            telegram_task = Some(telegram);
                        }
                    }
                }
            } else {
                tokio::select! {
                    _ = shutdown_signal() => {
                        tracing::info!("shutdown signal received");
                        break;
                    },
                    result = &mut ipc_task => {
                        log_task_exit("IPC", result);
                        break;
                    },
                    event = events.recv() => {
                        if matches!(event, Ok(crate::event::AppEvent::ConfigReloaded)) {
                            telegram_task = spawn_telegram(&self.app).await;
                        }
                    }
                }
            }
        }

        ipc_task.abort();
        if let Some(telegram) = telegram_task {
            telegram.abort();
        }
        if let Err(error) = self.app.storage.checkpoint() {
            tracing::warn!(%error, "SQLite WAL checkpoint failed during shutdown");
        }
        Ok(())
    }
}

async fn spawn_telegram(app: &AppState) -> Option<tokio::task::JoinHandle<Result<()>>> {
    let config = app.config.read().await.clone();
    let telegram_enabled = app
        .storage
        .telegram_control_state()
        .ok()
        .flatten()
        .is_some_and(|state| state.enabled);
    if !config.gateway.enabled || !telegram_enabled {
        app.health.set_telegram_polling(false).await;
        return None;
    }
    match crate::telegram::TelegramAdapter::from_app(app.clone()).await {
        Ok(adapter) => Some(tokio::spawn(async move { adapter.run().await })),
        Err(error) => {
            app.health.set_telegram_polling(false).await;
            tracing::error!(%error, "Telegram adapter apply failed; daemon remains available for setup");
            None
        }
    }
}

fn log_task_exit(name: &str, result: Result<Result<()>, tokio::task::JoinError>) {
    match result {
        Ok(Ok(())) => tracing::warn!(task = name, "task exited"),
        Ok(Err(error)) => tracing::error!(task = name, %error, "task failed"),
        Err(error) => tracing::error!(task = name, %error, "task panicked"),
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
