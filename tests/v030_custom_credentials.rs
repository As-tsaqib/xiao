use std::sync::Arc;

use xiao::{
    auth::AuthManager,
    config::CustomProviderConfig,
    providers::profiles::ProviderProfileStore,
    security::secrets::SecretStore,
    storage::{ProviderProfileInput, Storage},
};

#[test]
fn schema_migration_26_adds_api_key_ref_to_provider_profiles() {
    let db = Storage::open_memory().unwrap();
    assert!(db.schema_version().unwrap() >= 26);
    let columns = db
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("PRAGMA table_info(provider_profiles)")
                .unwrap();
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            Ok(rows)
        })
        .unwrap();
    assert!(columns.contains(&"api_key_ref".to_string()));
}

#[test]
fn direct_custom_api_key_persists_in_secret_store_and_resolves() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(SecretStore::new(dir.path().to_path_buf()));
    let storage = Arc::new(Storage::open_memory().unwrap());
    let store = ProviderProfileStore::new(storage.clone());

    let input = ProviderProfileInput {
        profile_id: Some("custom:direct".into()),
        owner_id: "owner:test".into(),
        alias: "Direct Custom".into(),
        endpoint: "http://127.0.0.1:8317/v1".into(),
        protocol: "openai_chat_completions".into(),
        credential_ref: None,
        api_key_ref: None,
        safe_headers_json: "{}".into(),
        secret_headers_ref: None,
    };
    let profile = store.create(input).unwrap();
    store
        .set_direct_api_key(
            &secrets,
            "owner:test",
            &profile.profile_id,
            "sk-test-direct-key",
        )
        .unwrap();

    let updated = store
        .get("owner:test", &profile.profile_id)
        .unwrap()
        .unwrap();
    assert!(updated.api_key_ref.is_some());
    assert!(updated
        .api_key_ref
        .as_ref()
        .unwrap()
        .starts_with("custom-api-key"));

    let resolved = store.resolve_api_key(&secrets, &updated).unwrap();
    assert_eq!(resolved.as_deref(), Some("sk-test-direct-key"));
}

#[test]
fn legacy_custom_credentials_migrate_to_direct_api_key_refs_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(SecretStore::new(dir.path().to_path_buf()));
    let storage = Arc::new(Storage::open_memory().unwrap());
    let auth = Arc::new(AuthManager::new(
        storage.clone(),
        secrets.clone(),
        CustomProviderConfig::default(),
    ));
    let store = ProviderProfileStore::new(storage.clone());

    // Legacy credential in provider_accounts
    let account = auth
        .configure_api_key("owner:legacy", "custom", "sk-legacy-custom-token")
        .unwrap();
    let input = ProviderProfileInput {
        profile_id: Some("custom:migrated".into()),
        owner_id: "owner:legacy".into(),
        alias: "Legacy Profile".into(),
        endpoint: "http://127.0.0.1:8317/v1".into(),
        protocol: "openai_chat_completions".into(),
        credential_ref: Some(account.id),
        api_key_ref: None,
        safe_headers_json: "{}".into(),
        secret_headers_ref: None,
    };
    let profile = store.create(input).unwrap();
    assert!(profile.api_key_ref.is_none());

    // Run migration
    store.migrate_legacy_credentials(&secrets, &auth).unwrap();

    let migrated = store
        .get("owner:legacy", &profile.profile_id)
        .unwrap()
        .unwrap();
    assert!(migrated.api_key_ref.is_some());
    let resolved = store.resolve_api_key(&secrets, &migrated).unwrap();
    assert_eq!(resolved.as_deref(), Some("sk-legacy-custom-token"));

    // Idempotent second run
    store.migrate_legacy_credentials(&secrets, &auth).unwrap();
    let migrated2 = store
        .get("owner:legacy", &profile.profile_id)
        .unwrap()
        .unwrap();
    assert_eq!(migrated.api_key_ref, migrated2.api_key_ref);
}
