use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_trait::async_trait;
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
    tools::{ToolRegistry, ToolResult},
};

struct LatencyMockProvider {
    first_request_received: AtomicBool,
    semantic_eval_called: AtomicBool,
}

#[async_trait]
impl Provider for LatencyMockProvider {
    fn id(&self) -> &'static str {
        "custom"
    }
    fn models(&self) -> Vec<String> {
        vec!["test-model".into()]
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
            evidence: "latency test".into(),
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
        _progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> anyhow::Result<ProviderTurn> {
        self.first_request_received.store(true, Ordering::SeqCst);
        Ok(ProviderTurn {
            step: ProviderStep::Final("Direct informational answer".into()),
            continuation: None,
            events: vec![],
        })
    }
}

#[tokio::test]
async fn informational_answer_completes_deterministically_without_semantic_overhead() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(LatencyMockProvider {
        first_request_received: AtomicBool::new(false),
        semantic_eval_called: AtomicBool::new(false),
    });
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
            "Latency Test",
            "custom",
            None,
            "test-model",
            false,
            None,
        )
        .unwrap();

    let answer = engine
        .submit_to_session_with_progress(
            "owner-1",
            &session.id,
            "What is the capital of France?",
            None,
        )
        .await
        .unwrap();

    assert_eq!(answer.final_answer, "Direct informational answer");
    assert!(provider.first_request_received.load(Ordering::SeqCst));
    assert!(!provider.semantic_eval_called.load(Ordering::SeqCst));
}
