use async_trait::async_trait;
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
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
    tools::{
        Tool, ToolCall, ToolContext, ToolOrigin, ToolRegistry, ToolResult, ToolRisk, ToolSpec,
    },
};

struct TrackingMockProvider {
    calls: Arc<Mutex<Vec<String>>>,
    semantic_calls: Arc<AtomicUsize>,
    turn_responses: Vec<ProviderTurn>,
    turn_index: AtomicUsize,
}

impl TrackingMockProvider {
    fn new(turn_responses: Vec<ProviderTurn>) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            semantic_calls: Arc::new(AtomicUsize::new(0)),
            turn_responses,
            turn_index: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Provider for TrackingMockProvider {
    fn id(&self) -> &'static str {
        "custom"
    }
    fn models(&self) -> Vec<String> {
        vec!["latency-model".into()]
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
            evidence: "latency tracking fixture".into(),
        }
    }
    async fn generate_text(&self, _req: ProviderRequest) -> anyhow::Result<String> {
        self.semantic_calls.fetch_add(1, Ordering::SeqCst);
        self.calls
            .lock()
            .unwrap()
            .push("semantic_generate_text".into());
        Ok(json!({
            "verified": true,
            "state": "verified_success",
            "confidence": 0.95,
            "reason": "verified by semantic model"
        })
        .to_string())
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
        _progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> anyhow::Result<ProviderTurn> {
        let idx = self.turn_index.fetch_add(1, Ordering::SeqCst);
        self.calls
            .lock()
            .unwrap()
            .push(format!("main_provider_turn_{idx}"));
        if idx < self.turn_responses.len() {
            Ok(self.turn_responses[idx].clone())
        } else {
            Ok(ProviderTurn {
                step: ProviderStep::Final("fallback final".into()),
                continuation: None,
                events: vec![],
            })
        }
    }
}

struct DeterministicSuccessTool;

#[async_trait]
impl Tool for DeterministicSuccessTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "deterministic_tool".into(),
            description: "Deterministic action tool".into(),
            parameters: json!({ "type": "object" }),
            risk: ToolRisk::SideEffect,
            origin: ToolOrigin::Builtin,
            effect: xiao::tools::ToolEffect::NonIdempotent,
            required_capabilities: vec![],
            timeout_ms: 5000,
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        _args: serde_json::Value,
    ) -> anyhow::Result<String> {
        Ok(json!({
            "status": "succeeded",
            "verification_evidence": true,
            "output": "operation completed successfully"
        })
        .to_string())
    }
}

#[tokio::test]
async fn matrix_d1_and_d2_ordinary_info_no_pre_provider_call_and_no_verifier_call() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(TrackingMockProvider::new(vec![ProviderTurn {
        step: ProviderStep::Final("The speed of light is ~300,000 km/s.".into()),
        continuation: None,
        events: vec![],
    }]));
    let auth = Arc::new(xiao::auth::AuthManager::new(
        storage.clone(),
        dir.path().join("secrets"),
    ));
    let providers = Arc::new(ProviderRegistry::from_single(
        "custom",
        provider.clone(),
        auth,
    ));

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
            "Info Test",
            "custom",
            None,
            "latency-model",
            false,
            None,
        )
        .unwrap();

    let answer = engine
        .submit_to_session_with_progress(
            "owner-1",
            &session.id,
            "What is the speed of light?",
            None,
        )
        .await
        .unwrap();

    assert_eq!(answer.final_answer, "The speed of light is ~300,000 km/s.");

    let call_history = provider.calls.lock().unwrap().clone();
    // D1: First call was main_provider_turn_0 (no semantic calls before)
    assert_eq!(
        call_history.first().map(String::as_str),
        Some("main_provider_turn_0")
    );
    // D2: Total semantic verifier calls is exactly 0 for informational prompt
    assert_eq!(provider.semantic_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn matrix_d3_deterministic_action_makes_no_verifier_call() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(TrackingMockProvider::new(vec![
        ProviderTurn {
            step: ProviderStep::ToolCalls(vec![ToolCall {
                call_id: "call-det-1".into(),
                name: "deterministic_tool".into(),
                arguments: json!({}),
            }]),
            continuation: Some(json!({ "turn": 0 })),
            events: vec![],
        },
        ProviderTurn {
            step: ProviderStep::Final("Action finished with verification evidence.".into()),
            continuation: None,
            events: vec![],
        },
    ]));
    let auth = Arc::new(xiao::auth::AuthManager::new(
        storage.clone(),
        dir.path().join("secrets"),
    ));
    let providers = Arc::new(ProviderRegistry::from_single(
        "custom",
        provider.clone(),
        auth,
    ));

    let tools = Arc::new(ToolRegistry::new(
        xiao::tools::ToolPolicy::default().allow_side_effect("deterministic_tool"),
        16384,
    ));
    tools.register(DeterministicSuccessTool).unwrap();

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
            "Action Test",
            "custom",
            None,
            "latency-model",
            false,
            None,
        )
        .unwrap();

    let answer = engine
        .submit_to_session_with_progress("owner-1", &session.id, "Execute deterministic task", None)
        .await
        .unwrap();

    assert!(answer.final_answer.contains("Action finished"));
    // D3: Verified action required 0 semantic provider verifier calls
    assert_eq!(provider.semantic_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn matrix_d5_and_d6_final_delivery_before_background_learning() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(TrackingMockProvider::new(vec![
        ProviderTurn {
            step: ProviderStep::ToolCalls(vec![ToolCall {
                call_id: "call-det-2".into(),
                name: "deterministic_tool".into(),
                arguments: json!({}),
            }]),
            continuation: Some(json!({ "turn": 0 })),
            events: vec![],
        },
        ProviderTurn {
            step: ProviderStep::Final("Completed successfully.".into()),
            continuation: None,
            events: vec![],
        },
    ]));
    let auth = Arc::new(xiao::auth::AuthManager::new(
        storage.clone(),
        dir.path().join("secrets"),
    ));
    let providers = Arc::new(ProviderRegistry::from_single(
        "custom",
        provider.clone(),
        auth,
    ));

    let tools = Arc::new(ToolRegistry::new(
        xiao::tools::ToolPolicy::default().allow_side_effect("deterministic_tool"),
        16384,
    ));
    tools.register(DeterministicSuccessTool).unwrap();

    let config = AgentConfig {
        background_learning: true,
        ..Default::default()
    };

    let engine =
        AgentEngine::with_registry(sessions.clone(), storage.clone(), providers, config, tools);

    let session = storage
        .create_session(
            "owner-1",
            "Learning Order Test",
            "custom",
            None,
            "latency-model",
            false,
            None,
        )
        .unwrap();

    // D6: AgentAnswer returns immediately without holding for background learning
    let answer = engine
        .submit_to_session_with_progress("owner-1", &session.id, "Perform verified action", None)
        .await
        .unwrap();

    assert_eq!(answer.final_answer, "Completed successfully.");

    // D5: Before frontend delivery acknowledgement, job is NOT claimable
    let claim_before = storage.claim_learning_job().unwrap();
    assert!(claim_before.is_none());

    // Acknowledge delivery from frontend
    let _run_id = storage
        .stored_messages("owner-1", &session.id)
        .unwrap()
        .into_iter()
        .rev()
        .find(|m| m.role == "assistant")
        .map(|_| session.id.clone())
        .unwrap();

    // Find the latest agent run
    let runs = storage.agent_runs("owner-1", 1).unwrap();
    assert!(!runs.is_empty());
    let agent_run_id = &runs[0].id;

    // Release after frontend delivery
    storage
        .release_learning_job_after_delivery(agent_run_id)
        .unwrap();

    // Now background worker claims the job
    let claim_after = storage.claim_learning_job().unwrap();
    assert!(claim_after.is_some());
    let (_job_id, owner, run, payload) = claim_after.unwrap();
    assert_eq!(owner, "owner-1");
    assert_eq!(run, *agent_run_id);
    assert!(payload.get("trace").is_some());
}
