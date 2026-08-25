use xiao::{
    app::AppState,
    command::{parse, Command, CommandResult},
    config::AppConfig,
    session::ChatMode,
    telegram::commands::TelegramCommandRegistry,
};

fn test_config(root: &std::path::Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.database = root.join("data/xiao.db");
    cfg.paths.data_dir = root.to_path_buf();
    cfg.paths.logs_dir = root.join("logs");
    cfg.paths.secrets_dir = root.join("secrets");
    cfg.ipc.bind = "127.0.0.1:37921".into();
    cfg
}

#[test]
fn v028_public_telegram_parser_matches_the_exact_registry() {
    let primary = TelegramCommandRegistry::public()
        .iter()
        .map(|command| command.name)
        .collect::<Vec<_>>();
    assert_eq!(
        primary,
        [
            "start", "help", "login", "model", "new", "sessions", "btw", "status",
            "context", "retry", "yolo", "stop", "skills", "tools",
        ]
    );
    assert_eq!(TelegramCommandRegistry::bot_commands().len(), 14);
    assert!(matches!(parse("/new").unwrap(), Some(Command::NewSession)));
    assert!(matches!(parse("/n").unwrap(), Some(Command::NewSession)));
    assert!(matches!(
        parse("/sessions").unwrap(),
        Some(Command::Session { page: 1 })
    ));
    assert!(matches!(parse("/s").unwrap(), Some(Command::Session { page: 1 })));
    assert!(matches!(parse("/retry").unwrap(), Some(Command::Retry)));
    assert!(matches!(parse("/r").unwrap(), Some(Command::Retry)));
    assert!(matches!(
        parse("/yolo").unwrap(),
        Some(Command::Yolo { enabled: None })
    ));
    assert!(matches!(
        parse("/y").unwrap(),
        Some(Command::Yolo { enabled: None })
    ));
    assert!(matches!(parse("/stop").unwrap(), Some(Command::Stop)));
    assert!(matches!(parse("/login").unwrap(), Some(Command::Login)));
    assert!(matches!(parse("/model").unwrap(), Some(Command::Model)));
    assert!(parse("hello, agent").unwrap().is_none());

    for removed in [
        "/cancel",
        "/memory",
        "/doctor",
        "/approvals",
        "/approve id",
        "/deny id",
        "/session",
        "/account",
        "/provider custom",
        "/settings",
        "/usage",
        "/env",
        "/about",
        "/logout",
    ] {
        assert!(parse(removed).is_err(), "{removed} must remain unknown");
    }
    for unsupported_tree in [
        "/login codex",
        "/login antigravity",
        "/model gpt-5",
        "/s 2",
    ] {
        assert!(
            parse(unsupported_tree).is_err(),
            "{unsupported_tree} must use scoped controls"
        );
    }
    assert_eq!(TelegramCommandRegistry::canonical("s"), Some("sessions"));
    assert_ne!(TelegramCommandRegistry::canonical("s"), Some("stop"));
}

#[tokio::test]
async fn command_core_keeps_new_alias_sessions_and_yolo_session_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    let app = AppState::build(cfg).await.unwrap();
    let principal = "integration:test";

    let initial = app.sessions.context_for(principal).unwrap().main;
    let created = app.commands.execute_text(principal, "/new").await.unwrap();
    assert!(matches!(created, CommandResult::Confirmation(_)));
    let from_alias = app.commands.execute_text(principal, "/n").await.unwrap();
    assert!(matches!(from_alias, CommandResult::Confirmation(_)));
    let current = app.sessions.context_for(principal).unwrap().main;
    assert_ne!(initial.id, current.id);
    assert!(app.storage.session(principal, &initial.id).unwrap().is_some());
    assert_eq!(app.storage.count_main_sessions(principal).unwrap(), 3);

    let manager = app.commands.execute_text(principal, "/s").await.unwrap();
    assert!(matches!(manager, CommandResult::ManagerView(_)));
    let enabled = app.commands.execute_text(principal, "/y").await.unwrap();
    assert!(matches!(enabled, CommandResult::ManagerView(_)));
    assert!(app.sessions.context_for(principal).unwrap().active.yolo_mode);
    app.commands.execute_text(principal, "/yolo").await.unwrap();
    assert!(!app.sessions.context_for(principal).unwrap().active.yolo_mode);

    app.commands.execute_text(principal, "/btw").await.unwrap();
    assert_eq!(app.sessions.context_for(principal).unwrap().mode, ChatMode::Side);
    assert!(!app.sessions.context_for(principal).unwrap().active.yolo_mode);
    app.commands.execute_text(principal, "/btw").await.unwrap();
    assert_eq!(app.sessions.context_for(principal).unwrap().mode, ChatMode::Main);
}

#[tokio::test]
async fn direct_custom_login_and_model_surface_replace_provider_manager_routes() {
    let dir = tempfile::tempdir().unwrap();
    let app = AppState::build(test_config(dir.path())).await.unwrap();
    let principal = "integration:provider";

    assert!(matches!(
        app.commands.execute_text(principal, "/login").await.unwrap(),
        CommandResult::StartCustomLogin
    ));
    assert!(matches!(
        app.commands.execute_text(principal, "/model").await.unwrap(),
        CommandResult::ManagerView(_)
    ));
    for removed in [
        "/login codex",
        "/login antigravity",
        "/provider codex",
        "/model gpt-5",
    ] {
        assert!(app.commands.execute_text(principal, removed).await.is_err());
    }
}
