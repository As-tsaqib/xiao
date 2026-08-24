use anyhow::{bail, Result};
use xiao::{app::AppState, config::AppConfig, standalone::CliPaths};

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if matches!(args.first().map(String::as_str), Some("-h" | "--help")) {
        println!(
            "xiaod {}\nUsage: xiaod\nConfig: XIAO_CONFIG or the xiao user config path",
            xiao::VERSION
        );
        return Ok(());
    }
    if matches!(args.first().map(String::as_str), Some("-V" | "--version")) {
        println!("xiaod {}", xiao::VERSION);
        return Ok(());
    }
    if !args.is_empty() {
        bail!("xiaod takes no arguments; use `xiaod --help`");
    }
    let config_path = CliPaths::from_env()?.config;
    if !config_path.is_file() {
        bail!(
            "xiao config is missing at {}; run `xiao quickstart` first",
            config_path.display()
        );
    }
    let cfg = AppConfig::load(&config_path)?;

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            cfg.daemon.log_level.clone(),
        ))
        .init();

    let app = AppState::build_from_path(cfg.clone(), config_path.clone()).await?;

    let ipc_app = app.clone();
    let ipc_path = config_path.clone();
    let mut ipc_task = tokio::spawn(async move { xiao::ipc::serve(ipc_app, ipc_path).await });

    let mut telegram_task = spawn_telegram(&app).await;
    let mut events = app.events.subscribe();

    tracing::info!(version = xiao::VERSION, "xiao daemon started");

    loop {
        if let Some(mut tg) = telegram_task.take() {
            tokio::select! {
                _ = shutdown_signal() => {
                    tg.abort();
                    tracing::info!("shutdown signal received");
                    break;
                },
                r = &mut ipc_task => {
                    tg.abort();
                    log_task_exit("IPC", r);
                    break;
                },
                r = &mut tg => {
                    log_task_exit("Telegram", r);
                },
                event = events.recv() => {
                    if matches!(event, Ok(xiao::event::AppEvent::ConfigReloaded)) {
                        tg.abort();
                        telegram_task = spawn_telegram(&app).await;
                    } else {
                        telegram_task = Some(tg);
                    }
                }
            }
        } else {
            tokio::select! {
                _ = shutdown_signal() => {
                    tracing::info!("shutdown signal received");
                    break;
                },
                r = &mut ipc_task => {
                    log_task_exit("IPC", r);
                    break;
                },
                event = events.recv() => {
                    if matches!(event, Ok(xiao::event::AppEvent::ConfigReloaded)) {
                        telegram_task = spawn_telegram(&app).await;
                    }
                }
            }
        }
    }

    ipc_task.abort();
    if let Some(tg) = telegram_task {
        tg.abort();
    }
    if let Err(error) = app.storage.checkpoint() {
        tracing::warn!(%error, "SQLite WAL checkpoint failed during shutdown");
    }
    Ok(())
}

async fn spawn_telegram(app: &AppState) -> Option<tokio::task::JoinHandle<Result<()>>> {
    let cfg = app.config.read().await.clone();
    if !cfg.gateway.enabled || !cfg.telegram.enabled {
        app.health.set_telegram_polling(false).await;
        return None;
    }
    match xiao::telegram::TelegramAdapter::from_app(app.clone()).await {
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
