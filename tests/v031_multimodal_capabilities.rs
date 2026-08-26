use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::tempdir;
use xiao::{
    auth::AuthManager,
    providers::{
        profile_model_from_probe, CapabilityState, CustomCapabilityProbe, CustomProfileEdit,
        CustomProfileService, ProviderCapabilities, ProviderProfileStore, ToolProtocol,
    },
    security::secrets::SecretStore,
    storage::Storage,
};

#[test]
fn matrix_a1_probe_success_yields_supported() {
    let probe = CustomCapabilityProbe {
        capabilities: ProviderCapabilities {
            text: true,
            vision: true,
            file_input: false,
            native_tools: true,
            tool_protocol: ToolProtocol::Native,
            model_discovery: false,
            structured_output: true,
            continuation: true,
            evidence: "probe success fixture".into(),
        },
        native_tools: CapabilityState::Supported,
        structured_output: CapabilityState::Supported,
        continuation: CapabilityState::Supported,
        vision: CapabilityState::Supported,
        file_input: CapabilityState::Unknown,
    };
    let record = profile_model_from_probe("prof-1", "model-vision", &probe, "2026-08-26T00:00:00Z");
    assert!(record.vision_capable);
    assert_eq!(record.vision_state, "supported");
}

#[test]
fn matrix_a2_probe_content_mismatch_yields_unknown_not_unsupported() {
    let probe = CustomCapabilityProbe {
        capabilities: ProviderCapabilities::chat_only("inconclusive probe"),
        native_tools: CapabilityState::Supported,
        structured_output: CapabilityState::Supported,
        continuation: CapabilityState::Supported,
        vision: CapabilityState::Unknown,
        file_input: CapabilityState::Unknown,
    };
    assert_eq!(probe.vision, CapabilityState::Unknown);
    let record =
        profile_model_from_probe("prof-1", "model-mismatch", &probe, "2026-08-26T00:00:00Z");
    assert!(!record.vision_capable);
    assert_eq!(record.vision_state, "unknown");
    assert!(!record.file_input_capable);
    assert_eq!(record.file_input_state, "unknown");
}

#[test]
fn matrix_a3_probe_timeout_or_500_yields_unknown() {
    // When probe encounters a timeout or 500 status, vision is marked Unknown rather than Unsupported
    let probe = CustomCapabilityProbe {
        capabilities: ProviderCapabilities::chat_only("probe timed out / 500"),
        native_tools: CapabilityState::Unknown,
        structured_output: CapabilityState::Unknown,
        continuation: CapabilityState::Unknown,
        vision: CapabilityState::Unknown,
        file_input: CapabilityState::Unknown,
    };
    let record =
        profile_model_from_probe("prof-1", "model-timeout", &probe, "2026-08-26T00:00:00Z");
    assert_eq!(record.vision_state, "unknown");
    assert!(!record.vision_capable);
}

#[test]
fn matrix_a4_exact_provider_image_schema_unsupported_error_records_unsupported() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let store = ProviderProfileStore::new(storage.clone());

    // Record runtime explicit image unsupported error
    store
        .record_runtime_capability(
            "prof-1",
            "model-no-image",
            "openai_chat_completions",
            "vision",
            "unsupported",
            "provider_explicit_unsupported",
        )
        .unwrap();

    let state = store
        .capability_state(
            "prof-1",
            "model-no-image",
            "openai_chat_completions",
            "vision",
        )
        .unwrap();
    assert_eq!(state, "unsupported");

    let evidence = storage
        .get_capability_evidence(
            "prof-1",
            "model-no-image",
            "openai_chat_completions",
            "vision",
        )
        .unwrap()
        .unwrap();
    assert_eq!(evidence.state, "unsupported");
    assert_eq!(evidence.source, "provider_explicit_unsupported");
}

#[test]
fn matrix_a6_runtime_image_success_upgrades_to_supported_and_reads_back() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let store = ProviderProfileStore::new(storage.clone());

    // Initially unrecorded or unknown
    let initial_state = store
        .capability_state("prof-1", "model-a", "openai_chat_completions", "vision")
        .unwrap();
    assert_eq!(initial_state, "unknown");

    // Upgrade via runtime success
    store
        .record_runtime_capability(
            "prof-1",
            "model-a",
            "openai_chat_completions",
            "vision",
            "supported",
            "runtime_success",
        )
        .unwrap();

    let upgraded = store
        .capability_state("prof-1", "model-a", "openai_chat_completions", "vision")
        .unwrap();
    assert_eq!(upgraded, "supported");

    // Readback from storage confirms persistence and source
    let evidence = storage
        .get_capability_evidence("prof-1", "model-a", "openai_chat_completions", "vision")
        .unwrap()
        .unwrap();
    assert_eq!(evidence.state, "supported");
    assert_eq!(evidence.source, "runtime_success");
}

#[test]
fn matrix_a7_runtime_transient_failure_leaves_unknown_intact() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let store = ProviderProfileStore::new(storage.clone());

    // Set unknown initial evidence
    storage
        .set_capability_evidence(
            "prof-1",
            "model-transient",
            "openai_chat_completions",
            "vision",
            "unknown",
            "probe_inconclusive",
            None,
        )
        .unwrap();

    // Transient failure (e.g. 500 / network drop) does NOT record explicit unsupported
    // So capability state remains Unknown
    let state = store
        .capability_state(
            "prof-1",
            "model-transient",
            "openai_chat_completions",
            "vision",
        )
        .unwrap();
    assert_eq!(state, "unknown");
}

#[test]
fn matrix_a8_force_supported_overrides_auto_state_and_admits_request() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let store = ProviderProfileStore::new(storage.clone());

    // Probed state is unknown
    storage
        .set_capability_evidence(
            "prof-1",
            "model-forced",
            "openai_chat_completions",
            "vision",
            "unknown",
            "probe_inconclusive",
            None,
        )
        .unwrap();

    // Owner sets ForceSupported override
    store
        .set_capability_override(
            "owner-1",
            "prof-1",
            "model-forced",
            "vision",
            "force_supported",
        )
        .unwrap();

    let effective_override = store
        .capability_override(
            "prof-1",
            "model-forced",
            "openai_chat_completions",
            "vision",
        )
        .unwrap();
    assert_eq!(effective_override, "force_supported");
}

#[test]
fn matrix_a9_force_unsupported_prevents_image_request() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let store = ProviderProfileStore::new(storage.clone());

    // Probed state was supported
    storage
        .set_capability_evidence(
            "prof-1",
            "model-blocked",
            "openai_chat_completions",
            "vision",
            "supported",
            "probe_success",
            None,
        )
        .unwrap();

    // Owner sets ForceUnsupported override
    store
        .set_capability_override(
            "owner-1",
            "prof-1",
            "model-blocked",
            "vision",
            "force_unsupported",
        )
        .unwrap();

    let effective_override = store
        .capability_override(
            "prof-1",
            "model-blocked",
            "openai_chat_completions",
            "vision",
        )
        .unwrap();
    assert_eq!(effective_override, "force_unsupported");
}

#[test]
fn matrix_a10_endpoint_edit_invalidates_automatic_evidence_preserving_override() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());

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
fn matrix_a11_exact_profile_model_isolation() {
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
fn custom_profile_service_edit_preserves_owner_override_and_invalidates_automatic() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let secrets = SecretStore::new(dir.path().join("secrets"));
    let auth = Arc::new(AuthManager::new(
        storage.clone(),
        dir.path().join("auth_secrets"),
    ));
    let service = CustomProfileService::with_auth(storage.clone(), secrets, auth);

    let profile = service
        .create_profile(
            "owner:test",
            "test-prof",
            "https://initial.example/v1",
            "openai_chat_completions",
            BTreeMap::new(),
            BTreeMap::new(),
            None,
        )
        .unwrap()
        .profile;

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

    storage
        .set_capability_override(
            &profile.profile_id,
            "model-B",
            "openai_chat_completions",
            "vision",
            "force_supported",
        )
        .unwrap();

    service
        .edit_with_warnings(
            "owner:test",
            &profile.profile_id,
            CustomProfileEdit {
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
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let secrets = SecretStore::new(dir.path().join("secrets"));
    let auth = Arc::new(AuthManager::new(
        storage.clone(),
        dir.path().join("auth_secrets"),
    ));
    let service = CustomProfileService::with_auth(storage.clone(), secrets, auth);

    let profile = service
        .create_profile(
            "owner:test",
            "proto-prof",
            "https://proto.example/v1",
            "openai_chat_completions",
            BTreeMap::new(),
            BTreeMap::new(),
            None,
        )
        .unwrap()
        .profile;

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

    service
        .edit_with_warnings(
            "owner:test",
            &profile.profile_id,
            CustomProfileEdit {
                protocol: Some("openai_responses".into()),
                ..Default::default()
            },
        )
        .unwrap();

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
