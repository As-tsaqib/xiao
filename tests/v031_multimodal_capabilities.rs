use xiao::{
    providers::{CapabilityState, CustomCapabilityProbe, ProviderCapabilities},
    storage::Storage,
};

#[test]
fn probe_content_mismatch_yields_unknown_not_unsupported() {
    let probe = CustomCapabilityProbe {
        capabilities: ProviderCapabilities::chat_only("inconclusive probe"),
        native_tools: CapabilityState::Supported,
        structured_output: CapabilityState::Supported,
        continuation: CapabilityState::Supported,
        vision: CapabilityState::Unknown,
        file_input: CapabilityState::Unknown,
    };
    assert_eq!(probe.vision, CapabilityState::Unknown);
    let record = xiao::providers::profile_model_from_probe("prof-1", "model-1", &probe, "2026-08-26T00:00:00Z");
    assert!(!record.vision_capable);
    assert_eq!(record.vision_state, "unknown");
    assert!(!record.file_input_capable);
    assert_eq!(record.file_input_state, "unknown");
}

#[test]
fn force_supported_and_force_unsupported_precedence() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Storage::open(&db_path).unwrap();

    // Store unknown evidence initially
    storage
        .set_capability_evidence(
            "prof-1",
            "model-1",
            "openai_chat_completions",
            "vision",
            "unknown",
            "probe_inconclusive",
            Some("force_supported"),
        )
        .unwrap();

    let evidence = storage
        .get_capability_evidence("prof-1", "model-1", "openai_chat_completions", "vision")
        .unwrap()
        .unwrap();
    assert_eq!(evidence.owner_override, "force_supported");

    // Change to force_unsupported
    storage
        .set_capability_override(
            "prof-1",
            "model-1",
            "openai_chat_completions",
            "vision",
            "force_unsupported",
        )
        .unwrap();

    let evidence = storage
        .get_capability_evidence("prof-1", "model-1", "openai_chat_completions", "vision")
        .unwrap()
        .unwrap();
    assert_eq!(evidence.owner_override, "force_unsupported");
}

#[test]
fn exact_profile_model_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Storage::open(&db_path).unwrap();

    // Record runtime success on model-A
    storage
        .set_capability_evidence(
            "prof-1",
            "model-A",
            "openai_chat_completions",
            "vision",
            "supported",
            "runtime_success",
            None,
        )
        .unwrap();

    let evidence_a = storage
        .get_capability_evidence("prof-1", "model-A", "openai_chat_completions", "vision")
        .unwrap()
        .unwrap();
    assert_eq!(evidence_a.state, "supported");

    // model-B must remain isolated (None or unknown)
    let evidence_b = storage
        .get_capability_evidence("prof-1", "model-B", "openai_chat_completions", "vision")
        .unwrap();
    assert!(evidence_b.is_none());
}

#[test]
fn endpoint_edit_invalidates_automatic_evidence_preserving_override() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Storage::open(&db_path).unwrap();

    // Model A: automatic runtime success
    storage
        .set_capability_evidence(
            "prof-1",
            "model-A",
            "openai_chat_completions",
            "vision",
            "supported",
            "runtime_success",
            None,
        )
        .unwrap();

    // Model B: explicit owner override
    storage
        .set_capability_override(
            "prof-1",
            "model-B",
            "openai_chat_completions",
            "vision",
            "force_supported",
        )
        .unwrap();

    // Endpoint edit invalidates automatic evidence
    storage
        .invalidate_automatic_capability_evidence("prof-1")
        .unwrap();

    let ev_a = storage
        .get_capability_evidence("prof-1", "model-A", "openai_chat_completions", "vision")
        .unwrap()
        .unwrap();
    assert_eq!(ev_a.state, "unknown");

    let ev_b = storage
        .get_capability_evidence("prof-1", "model-B", "openai_chat_completions", "vision")
        .unwrap()
        .unwrap();
    assert_eq!(ev_b.owner_override, "force_supported");
}

#[test]
fn custom_profile_service_edit_preserves_owner_override_and_invalidates_automatic() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = std::sync::Arc::new(Storage::open(&db_path).unwrap());
    let secrets = xiao::security::secrets::SecretStore::new(dir.path().join("secrets"));
    let auth = std::sync::Arc::new(xiao::auth::AuthManager::new(
        storage.clone(),
        dir.path().join("auth_secrets"),
    ));
    let service = xiao::providers::CustomProfileService::with_auth(storage.clone(), secrets, auth);

    let profile = service
        .create_profile(
            "owner:test",
            "test-prof",
            "https://initial.example/v1",
            "openai_chat_completions",
            std::collections::BTreeMap::new(),
            std::collections::BTreeMap::new(),
            None,
        )
        .unwrap()
        .profile;

    // Automatic evidence on model-A
    storage
        .set_capability_evidence(
            &profile.profile_id,
            "model-A",
            "openai_chat_completions",
            "vision",
            "supported",
            "runtime_success",
            None,
        )
        .unwrap();

    // Owner override on model-B
    storage
        .set_capability_override(
            &profile.profile_id,
            "model-B",
            "openai_chat_completions",
            "vision",
            "force_supported",
        )
        .unwrap();

    // Edit endpoint using CustomProfileService::edit_with_warnings
    service
        .edit_with_warnings(
            "owner:test",
            &profile.profile_id,
            xiao::providers::CustomProfileEdit {
                endpoint: Some("https://updated.example/v1".into()),
                ..Default::default()
            },
        )
        .unwrap();

    let ev_a = storage
        .get_capability_evidence(
            &profile.profile_id,
            "model-A",
            "openai_chat_completions",
            "vision",
        )
        .unwrap()
        .unwrap();
    assert_eq!(ev_a.state, "unknown");
    assert_eq!(ev_a.source, "invalidated_on_endpoint_change");

    let ev_b = storage
        .get_capability_evidence(
            &profile.profile_id,
            "model-B",
            "openai_chat_completions",
            "vision",
        )
        .unwrap()
        .unwrap();
    assert_eq!(ev_b.owner_override, "force_supported");
}

#[test]
fn protocol_edit_migrates_owner_override_to_new_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = std::sync::Arc::new(Storage::open(&db_path).unwrap());
    let secrets = xiao::security::secrets::SecretStore::new(dir.path().join("secrets"));
    let auth = std::sync::Arc::new(xiao::auth::AuthManager::new(
        storage.clone(),
        dir.path().join("auth_secrets"),
    ));
    let service = xiao::providers::CustomProfileService::with_auth(storage.clone(), secrets, auth);

    let profile = service
        .create_profile(
            "owner:test",
            "proto-prof",
            "https://proto.example/v1",
            "openai_chat_completions",
            std::collections::BTreeMap::new(),
            std::collections::BTreeMap::new(),
            None,
        )
        .unwrap()
        .profile;

    // Set owner override on streaming and vision
    storage
        .set_capability_override(
            &profile.profile_id,
            "m-override",
            "openai_chat_completions",
            "streaming",
            "force_unsupported",
        )
        .unwrap();
    storage
        .set_capability_override(
            &profile.profile_id,
            "m-override",
            "openai_chat_completions",
            "vision",
            "force_supported",
        )
        .unwrap();

    // Automatic evidence
    storage
        .set_capability_evidence(
            &profile.profile_id,
            "m-auto",
            "openai_chat_completions",
            "vision",
            "supported",
            "runtime_success",
            None,
        )
        .unwrap();

    // Edit protocol to openai_responses
    service
        .edit_with_warnings(
            "owner:test",
            &profile.profile_id,
            xiao::providers::CustomProfileEdit {
                protocol: Some("openai_responses".into()),
                ..Default::default()
            },
        )
        .unwrap();

    // Verify overrides migrated to openai_responses
    let stream_override = storage
        .get_capability_evidence(
            &profile.profile_id,
            "m-override",
            "openai_responses",
            "streaming",
        )
        .unwrap()
        .unwrap();
    assert_eq!(stream_override.owner_override, "force_unsupported");

    let vision_override = storage
        .get_capability_evidence(
            &profile.profile_id,
            "m-override",
            "openai_responses",
            "vision",
        )
        .unwrap()
        .unwrap();
    assert_eq!(vision_override.owner_override, "force_supported");

    // Automatic evidence invalidated
    let auto_evidence = storage
        .get_capability_evidence(
            &profile.profile_id,
            "m-auto",
            "openai_chat_completions",
            "vision",
        )
        .unwrap()
        .unwrap();
    assert_eq!(auto_evidence.state, "unknown");
}
