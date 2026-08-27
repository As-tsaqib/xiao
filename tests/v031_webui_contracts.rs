use std::sync::Arc;
use tempfile::tempdir;
use xiao::{
    config::AppConfig,
    ipc::{ApiState, AppState},
    storage::Storage,
};

#[tokio::test]
async fn webui_all_manager_get_and_post_contracts_execute_successfully() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());

    let mut config = AppConfig::default();
    config.storage.database = db_path.clone();
    config.paths.data_dir = dir.path().join("data");
    config.paths.logs_dir = dir.path().join("logs");
    config.paths.secrets_dir = dir.path().join("secrets");
    let config_path = dir.path().join("config.toml");
    config.save_atomic(&config_path).unwrap();

    let app = AppState::build_from_path(config, &config_path)
        .await
        .unwrap();

    let state = ApiState {
        app,
        config_path: config_path.clone(),
        client_token: Arc::new("client-token".into()),
        admin_token: Arc::new("admin-token".into()),
    };

    let owner = storage.ensure_local_owner().unwrap();
    let session = storage
        .create_session(&owner, "WebUI Test Session", "custom", None, "m", false, None)
        .unwrap();

    // 1. Session new
    let res = state
        .app
        .sessions
        .create_and_switch(&owner)
        .unwrap();
    assert!(!res.id.is_empty());

    // 2. Session ai_config
    let profile_store = xiao::providers::ProviderProfileStore::new(storage.clone());
    let profile = profile_store
        .create(xiao::storage::ProviderProfileInput {
            profile_id: Some("prof-webui".into()),
            owner_id: owner.clone(),
            alias: "webui-custom".into(),
            endpoint: "https://api.example.com/v1".into(),
            protocol: "openai_chat_completions".into(),
            safe_headers_json: "{}".into(),
            api_key_ref: None,
            credential_ref: None,
            secret_headers_ref: None,
        })
        .unwrap();

    storage
        .set_session_provider(&owner, &session.id, "custom", Some(&profile.profile_id), "model-1")
        .unwrap();

    // 3. Capability override
    profile_store
        .set_capability_override(&profile.profile_id, "model-1", "vision", "force_supported")
        .unwrap();
    let ovr = profile_store
        .capability_override(&profile.profile_id, "model-1", "openai_chat_completions", "vision")
        .unwrap();
    assert_eq!(ovr, Some("force_supported".into()));

    // 4. Memory delete with scope, category, key
    let mem_store = xiao::memory::MemoryStore::new(storage.clone());
    mem_store
        .upsert(
            &owner,
            xiao::memory::MemoryScope::User,
            "pref",
            "lang",
            "en",
            1.0,
            "statement",
            Some(&session.id),
        )
        .unwrap();
    assert_eq!(mem_store.list(&owner, None, 10).unwrap().len(), 1);
    mem_store
        .delete(&owner, xiao::memory::MemoryScope::User, "pref", "lang")
        .unwrap();
    assert_eq!(mem_store.list(&owner, None, 10).unwrap().len(), 0);

    // 5. Agent settings update
    let mut current_cfg = state.app.config.read().await.clone();
    current_cfg.agent.plan_cache_enabled = true;
    current_cfg.agent.background_learning = true;
    assert!(current_cfg.agent.plan_cache_enabled);
    assert!(current_cfg.agent.background_learning);
}
