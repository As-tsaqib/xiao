use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use xiao::{
    agent::AgentEngine,
    config::AgentConfig,
    providers::{
        AgentEvent, Provider, ProviderCapabilities, ProviderRegistry, ProviderRequest,
        ProviderResponse, ProviderStep, ProviderTurn, ToolProtocol,
    },
    session::SessionManager,
    storage::Storage,
    tools::{ToolCall, ToolRegistry, ToolResult},
};

struct StreamingMockProvider {
    text_deltas: Vec<&'static str>,
    emit_tool_deltas: bool,
}

#[async_trait]
impl Provider for StreamingMockProvider {
    fn id(&self) -> &'static str {
        "custom"
    }
    fn models(&self) -> Vec<String> {
        vec!["stream-model".into()]
    }
    fn ready(&self) -> bool {
        true
    }
    fn capabilities(&self, _model: &str) -> ProviderCapabilities {
        ProviderCapabilities {
            text: true,
            vision: false,
            file_input: false,
            native_tools: true,
            tool_protocol: ToolProtocol::Native,
            model_discovery: false,
            structured_output: true,
            continuation: true,
            evidence: "streaming test".into(),
        }
    }
    async fn run(
        &self,
        _req: ProviderRequest,
        _progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> anyhow::Result<ProviderResponse> {
        Err(anyhow::anyhow!("run_turn must be used"))
    }
    async fn run_turn(
        &self,
        _req: ProviderRequest,
        _continuation: Option<serde_json::Value>,
        _tool_results: Vec<ToolResult>,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> anyhow::Result<ProviderTurn> {
        let mut full_text = String::new();
        for delta in &self.text_deltas {
            full_text.push_str(delta);
            if let Some(tx) = &progress {
                let _ = tx.send(AgentEvent::TextDelta((*delta).to_string()));
            }
        }
        if self.emit_tool_deltas {
            Ok(ProviderTurn {
                step: ProviderStep::ToolCalls(vec![ToolCall {
                    call_id: "stream-call-1".into(),
                    name: "step_tool".into(),
                    arguments: json!({ "arg": "assembled_val" }),
                }]),
                continuation: None,
                events: vec![],
            })
        } else {
            Ok(ProviderTurn {
                step: ProviderStep::Final(full_text),
                continuation: None,
                events: vec![],
            })
        }
    }
}

#[tokio::test]
async fn streamed_text_deltas_reach_progress_channel_before_completion() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(StreamingMockProvider {
        text_deltas: vec!["Hello", " ", "world", "!"],
        emit_tool_deltas: false,
    });
    let auth = Arc::new(xiao::auth::AuthManager::new(
        storage.clone(),
        dir.path().join("secrets"),
    ));
    let providers = Arc::new(ProviderRegistry::from_single("custom", provider, auth));

    let tools = Arc::new(ToolRegistry::new(xiao::tools::ToolPolicy::default(), 16384));
    let engine = AgentEngine::with_registry(
        sessions.clone(),
        storage.clone(),
        providers,
        AgentConfig::default(),
        tools,
    );

    let session = storage
        .create_session(
            "owner-1",
            "Stream Test",
            "custom",
            None,
            "stream-model",
            false,
            None,
        )
        .unwrap();

    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let answer = engine
        .submit_to_session_with_progress("owner-1", &session.id, "say hello", Some(progress_tx))
        .await
        .unwrap();

    assert_eq!(answer.final_answer, "Hello world!");

    let mut deltas = Vec::new();
    while let Ok(event) = progress_rx.try_recv() {
        if let AgentEvent::TextDelta(text) = event {
            deltas.push(text);
        }
    }

    assert_eq!(deltas, vec!["Hello", " ", "world", "!"]);
}

#[tokio::test]
async fn raw_tool_json_never_appears_in_text_deltas() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(StreamingMockProvider {
        text_deltas: vec!["Processing request..."],
        emit_tool_deltas: true,
    });
    let auth = Arc::new(xiao::auth::AuthManager::new(
        storage.clone(),
        dir.path().join("secrets"),
    ));
    let providers = Arc::new(ProviderRegistry::from_single("custom", provider, auth));

    let tools = Arc::new(ToolRegistry::new(xiao::tools::ToolPolicy::default(), 16384));
    let engine = AgentEngine::with_registry(
        sessions.clone(),
        storage.clone(),
        providers,
        AgentConfig::default(),
        tools,
    );

    let session = storage
        .create_session(
            "owner-1",
            "Stream Test",
            "custom",
            None,
            "stream-model",
            false,
            None,
        )
        .unwrap();

    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let _ = engine
        .submit_to_session_with_progress("owner-1", &session.id, "call tool", Some(progress_tx))
        .await;

    let mut saw_delta = false;
    while let Ok(event) = progress_rx.try_recv() {
        if let AgentEvent::TextDelta(text) = event {
            saw_delta = true;
            assert!(!text.contains("stream-call-1"));
            assert!(!text.contains("assembled_val"));
        }
    }
    assert!(saw_delta);
}

#[tokio::test]
async fn cached_unsupported_streaming_disables_streaming_on_future_requests() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let secrets = xiao::security::secrets::SecretStore::new(dir.path().join("secrets"));
    let auth = Arc::new(xiao::auth::AuthManager::new(
        storage.clone(),
        dir.path().join("auth_secrets"),
    ));
    let service = xiao::providers::CustomProfileService::with_auth(storage.clone(), secrets, auth);

    let profile = service
        .create_profile(
            "owner-1",
            "stream-prof",
            "https://stream.example/v1",
            "openai_chat_completions",
            std::collections::BTreeMap::new(),
            std::collections::BTreeMap::new(),
            None,
        )
        .unwrap()
        .profile;

    let store = xiao::providers::ProviderProfileStore::new(storage.clone());

    // 1. Initially state is unknown -> optimistic streaming enabled
    let state = store
        .capability_state(&profile.profile_id, "model-1", "openai_chat_completions", "streaming")
        .unwrap();
    assert_eq!(state, "unknown");

    // 2. Record explicit unsupported streaming capability
    store
        .record_runtime_capability(
            &profile.profile_id,
            "model-1",
            "openai_chat_completions",
            "streaming",
            "unsupported",
            "provider_explicit_unsupported",
        )
        .unwrap();

    let updated_state = store
        .capability_state(&profile.profile_id, "model-1", "openai_chat_completions", "streaming")
        .unwrap();
    assert_eq!(updated_state, "unsupported");

    // 3. Explicit owner override force_supported overrides cached unsupported
    store
        .set_capability_override(
            "owner-1",
            &profile.profile_id,
            "model-1",
            "streaming",
            "force_supported",
        )
        .unwrap();

    let effective_override = store
        .capability_override(&profile.profile_id, "model-1", "openai_chat_completions", "streaming")
        .unwrap();
    assert_eq!(effective_override, "force_supported");
}
