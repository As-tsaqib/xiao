use xiao::{
    app::AppState,
    command::{parse, Command, CommandResult},
    config::AppConfig,
    session::ChatMode,
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
fn required_commands_parse_to_semantic_variants() {
    assert!(matches!(parse("/new").unwrap(), Some(Command::NewSession)));
    assert!(matches!(
        parse("/btw").unwrap(),
        Some(Command::ToggleSideChat)
    ));
    assert!(matches!(
        parse("/session 2").unwrap(),
        Some(Command::Session { page: 2 })
    ));
    assert!(matches!(
        parse("/provider codex").unwrap(),
        Some(Command::SetProvider { .. })
    ));
    assert!(matches!(
        parse("/model gpt-5.6-sol").unwrap(),
        Some(Command::SetModel { .. })
    ));
    assert!(matches!(
        parse("/login").unwrap(),
        Some(Command::Login { provider: None })
    ));
    assert!(parse("hello, agent").unwrap().is_none());
}

#[tokio::test]
async fn command_core_keeps_session_state_shared_and_durable() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = test_config(dir.path());
    let app = AppState::build(cfg).await.unwrap();
    let principal = "integration:test";

    let initial = app.sessions.context_for(principal).unwrap().main;
    let created = app.commands.execute_text(principal, "/new").await.unwrap();
    assert!(matches!(created, CommandResult::Confirmation(_)));
    let current = app.sessions.context_for(principal).unwrap().main;
    assert_ne!(initial.id, current.id);
    assert!(app
        .storage
        .session(principal, &initial.id)
        .unwrap()
        .is_some());
    assert_eq!(app.storage.count_main_sessions(principal).unwrap(), 2);

    let manager = app
        .commands
        .execute_text(principal, "/session")
        .await
        .unwrap();
    assert!(matches!(manager, CommandResult::ManagerView(_)));

    app.commands.execute_text(principal, "/btw").await.unwrap();
    assert_eq!(
        app.sessions.context_for(principal).unwrap().mode,
        ChatMode::Side
    );
    app.commands.execute_text(principal, "/btw").await.unwrap();
    assert_eq!(
        app.sessions.context_for(principal).unwrap().mode,
        ChatMode::Main
    );
}

#[tokio::test]
async fn provider_and_model_selection_use_command_core() {
    let dir = tempfile::tempdir().unwrap();
    let app = AppState::build(test_config(dir.path())).await.unwrap();
    let principal = "integration:provider";
    app.commands
        .execute_text(principal, "/provider codex")
        .await
        .unwrap();
    app.commands
        .execute_text(principal, "/model gpt-5.6-sol")
        .await
        .unwrap();
    let context = app.sessions.context_for(principal).unwrap();
    assert_eq!(context.active.provider, "codex");
    assert_eq!(context.active.model, "gpt-5.6-sol");
}
