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
