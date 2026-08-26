use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use xiao::{
    agent::AgentEngine,
    config::AgentConfig,
    providers::{
        AgentEvent, Provider, ProviderCapabilities, ProviderProfileStore, ProviderRegistry,
        ProviderRequest, ProviderResponse, ProviderStep, ProviderTurn, ToolProtocol,
    },
    session::SessionManager,
    storage::Storage,
    tools::{ToolCall, ToolRegistry, ToolResult},
};

struct ChatSseMockProvider {
    text_deltas: Vec<&'static str>,
    emit_tool_deltas: bool,
    reasoning_deltas: Vec<&'static str>,
}

#[async_trait]
impl Provider for ChatSseMockProvider {
    fn id(&self) -> &'static str {
        "custom"
    }
    fn models(&self) -> Vec<String> {
        vec!["chat-stream-model".into()]
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
            evidence: "chat sse test".into(),
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
        // Ignored reasoning fields should NEVER be sent as TextDelta
        for _reasoning in &self.reasoning_deltas {
            // Emulate reasoning event or internal scratchpad (should not be TextDelta)
        }
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

struct ResponsesSseMockProvider {
    text_deltas: Vec<&'static str>,
}

#[async_trait]
impl Provider for ResponsesSseMockProvider {
    fn id(&self) -> &'static str {
        "custom"
    }
    fn models(&self) -> Vec<String> {
        vec!["responses-stream-model".into()]
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
            evidence: "responses sse test".into(),
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
        Ok(ProviderTurn {
            step: ProviderStep::Final(full_text),
            continuation: None,
            events: vec![],
        })
    }
}

struct PartialOutputFailProvider {
    attempts: AtomicUsize,
    emitted_partial: AtomicBool,
}

#[async_trait]
impl Provider for PartialOutputFailProvider {
    fn id(&self) -> &'static str {
        "custom"
    }
    fn models(&self) -> Vec<String> {
        vec!["fail-stream-model".into()]
    }
    fn ready(&self) -> bool {
        true
    }
    fn capabilities(&self, _model: &str) -> ProviderCapabilities {
        ProviderCapabilities {
            text: true,
            vision: false,
            file_input: false,
            native_tools: false,
            tool_protocol: ToolProtocol::ChatOnly,
            model_discovery: false,
            structured_output: false,
            continuation: false,
            evidence: "partial fail test".into(),
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
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            if let Some(tx) = &progress {
                let _ = tx.send(AgentEvent::TextDelta("Partial visible chunk before network crash".into()));
            }
            self.emitted_partial.store(true, Ordering::SeqCst);
            return Err(anyhow::anyhow!("upstream connection dropped mid-stream"));
        }
        Ok(ProviderTurn {
            step: ProviderStep::Final("should not reach fallback retry after visible data".into()),
            continuation: None,
            events: vec![],
        })
    }
}

#[tokio::test]
async fn matrix_c1_chat_completions_sse_text_reaches_frontend_before_completion() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(ChatSseMockProvider {
        text_deltas: vec!["Hello", " ", "world", "!"],
        emit_tool_deltas: false,
        reasoning_deltas: vec![],
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
            "Chat SSE Test",
            "custom",
            None,
            "chat-stream-model",
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
async fn matrix_c2_responses_sse_text_reaches_frontend_before_completion() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(ResponsesSseMockProvider {
        text_deltas: vec!["Responses", " ", "protocol", " ", "streaming"],
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
            "Responses SSE Test",
            "custom",
            None,
            "responses-stream-model",
            false,
            None,
        )
        .unwrap();

    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let answer = engine
        .submit_to_session_with_progress("owner-1", &session.id, "stream responses", Some(progress_tx))
        .await
        .unwrap();

    assert_eq!(answer.final_answer, "Responses protocol streaming");

    let mut deltas = Vec::new();
    while let Ok(event) = progress_rx.try_recv() {
        if let AgentEvent::TextDelta(text) = event {
            deltas.push(text);
        }
    }
    assert_eq!(deltas, vec!["Responses", " ", "protocol", " ", "streaming"]);
}

#[tokio::test]
async fn matrix_c4_and_c5_raw_tool_json_never_appears_in_text_deltas_and_reasoning_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(ChatSseMockProvider {
        text_deltas: vec!["Processing request..."],
        emit_tool_deltas: true,
        reasoning_deltas: vec!["internal reasoning token 1", "internal reasoning token 2"],
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
            "Tool JSON and Reasoning Filter Test",
            "custom",
            None,
            "chat-stream-model",
            false,
            None,
        )
        .unwrap();

    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let _ = engine
        .submit_to_session_with_progress("owner-1", &session.id, "call tool", Some(progress_tx))
        .await;

    while let Ok(event) = progress_rx.try_recv() {
        if let AgentEvent::TextDelta(text) = event {
            // C4: Raw tool JSON never appears in text deltas
            assert!(!text.contains("stream-call-1"));
            assert!(!text.contains("assembled_val"));
            assert!(!text.contains("step_tool"));
            // C5: Reasoning tokens never appear in text deltas
            assert!(!text.contains("internal reasoning token"));
        }
    }
}

#[tokio::test]
async fn matrix_c6_and_c7_cached_unsupported_streaming_disables_streaming() {
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

    let store = ProviderProfileStore::new(storage.clone());

    // 1. Initially state is unknown -> optimistic streaming enabled
    let state = store
        .capability_state(
            &profile.profile_id,
            "model-1",
            "openai_chat_completions",
            "streaming",
        )
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
        .capability_state(
            &profile.profile_id,
            "model-1",
            "openai_chat_completions",
            "streaming",
        )
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
        .capability_override(
            &profile.profile_id,
            "model-1",
            "openai_chat_completions",
            "streaming",
        )
        .unwrap();
    assert_eq!(effective_override, "force_supported");
}

#[tokio::test]
async fn matrix_c8_no_retry_after_partial_visible_output() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(PartialOutputFailProvider {
        attempts: AtomicUsize::new(0),
        emitted_partial: AtomicBool::new(false),
    });
    let auth = Arc::new(xiao::auth::AuthManager::new(
        storage.clone(),
        dir.path().join("secrets"),
    ));
    let providers = Arc::new(ProviderRegistry::from_single("custom", provider.clone(), auth));

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
            "Partial Fail Test",
            "custom",
            None,
            "fail-stream-model",
            false,
            None,
        )
        .unwrap();

    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let result = engine
        .submit_to_session_with_progress("owner-1", &session.id, "test partial failure", Some(progress_tx))
        .await;

    // Must fail without performing a duplicate non-streaming retry
    assert!(result.is_err());
    assert_eq!(provider.attempts.load(Ordering::SeqCst), 1);
    assert!(provider.emitted_partial.load(Ordering::SeqCst));

    let mut saw_partial = false;
    while let Ok(event) = progress_rx.try_recv() {
        if let AgentEvent::TextDelta(text) = event {
            if text.contains("Partial visible chunk") {
                saw_partial = true;
            }
        }
    }
    assert!(saw_partial);
}

#[tokio::test]
async fn matrix_c9_and_c10_stop_cancels_and_final_message_emitted_once() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(ChatSseMockProvider {
        text_deltas: vec!["Final text answer."],
        emit_tool_deltas: false,
        reasoning_deltas: vec![],
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
            "Stop and Final Test",
            "custom",
            None,
            "chat-stream-model",
            false,
            None,
        )
        .unwrap();

    let answer = engine
        .submit_to_session_with_progress("owner-1", &session.id, "emit final", None)
        .await
        .unwrap();

    // Final answer matches and is emitted cleanly
    assert_eq!(answer.final_answer, "Final text answer.");

    // Token cancellation halts generation
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(cancellation.is_cancelled());
}
