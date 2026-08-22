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

    let app = AppState::build(cfg.clone()).await?;

    let ipc_app = app.clone();
    let ipc_path = config_path.clone();
    let mut ipc_task = tokio::spawn(async move { xiao::ipc::serve(ipc_app, ipc_path).await });

    let mut telegram_task = if cfg.gateway.enabled && cfg.telegram.enabled {
        let tg = xiao::telegram::TelegramAdapter::from_app(app.clone()).await?;
        Some(tokio::spawn(async move { tg.run().await }))
    } else {
        None
    };

    tracing::info!(version = xiao::VERSION, "xiao daemon started");

    if let Some(tg) = telegram_task.as_mut() {
        tokio::select! {
            _ = shutdown_signal() => tracing::info!("shutdown signal received"),
            r = &mut ipc_task => log_task_exit("IPC", r),
            r = tg => log_task_exit("Telegram", r),
        }
    } else {
        tokio::select! {
            _ = shutdown_signal() => tracing::info!("shutdown signal received"),
            r = &mut ipc_task => log_task_exit("IPC", r),
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
