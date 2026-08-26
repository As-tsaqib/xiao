use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use xiao::{
    agent::AgentEngine,
    config::AgentConfig,
    providers::{
        AgentEvent, Provider, ProviderCapabilities, ProviderRegistry, ProviderRequest,
        ProviderStep, ProviderTurn, ToolCall, ToolProtocol,
    },
    session::SessionManager,
    storage::Storage,
    tools::{Tool, ToolContext, ToolOrigin, ToolRisk, ToolSpec},
};

struct PingPongLoopProvider {
    turns: AtomicUsize,
}

#[async_trait]
impl Provider for PingPongLoopProvider {
    fn id(&self) -> &'static str {
        "custom"
    }
    fn models(&self) -> Vec<String> {
        vec!["pingpong-model".into()]
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
            evidence: "pingpong test".into(),
        }
    }
    async fn generate_agent_step(
        &self,
        _req: ProviderRequest,
        _progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> anyhow::Result<ProviderTurn> {
        let t = self.turns.fetch_add(1, Ordering::SeqCst);
        let tool_name = if t % 2 == 0 { "tool_ping" } else { "tool_pong" };
        Ok(ProviderTurn {
            step: ProviderStep::ToolCalls(vec![ToolCall {
                call_id: format!("call-{t}"),
                name: tool_name.into(),
                arguments: json!({ "turn": t }),
            }]),
            continuation: Some(json!({ "turn": t })),
            events: vec![],
        })
    }
}

struct PingTool;

#[async_trait]
impl Tool for PingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "tool_ping".into(),
            description: "Ping".into(),
            parameters: json!({ "type": "object" }),
            risk: ToolRisk::ReadOnly,
            origin: ToolOrigin::Builtin,
            effect: xiao::tools::ToolEffect::Idempotent,
            required_capabilities: vec![],
            timeout_ms: 5000,
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        _args: serde_json::Value,
    ) -> anyhow::Result<String> {
        Ok(json!({ "status": "ping_ok", "verification_evidence": true }).to_string())
    }
}

struct PongTool;

#[async_trait]
impl Tool for PongTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "tool_pong".into(),
            description: "Pong".into(),
            parameters: json!({ "type": "object" }),
            risk: ToolRisk::ReadOnly,
            origin: ToolOrigin::Builtin,
            effect: xiao::tools::ToolEffect::Idempotent,
            required_capabilities: vec![],
            timeout_ms: 5000,
        }
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        _args: serde_json::Value,
    ) -> anyhow::Result<String> {
        Ok(json!({ "status": "pong_ok", "verification_evidence": true }).to_string())
    }
}

#[tokio::test]
async fn ping_pong_repeating_tool_sequence_is_detected_and_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(PingPongLoopProvider {
        turns: AtomicUsize::new(0),
    });
    let auth = Arc::new(xiao::auth::AuthManager::new(
        storage.clone(),
        xiao::security::secrets::SecretStore::new(dir.path().join("secrets")),
    ));
    let providers = Arc::new(ProviderRegistry::from_single(
        "custom",
        provider.clone(),
        auth,
    ));

    let tools = Arc::new(xiao::tools::ToolRegistry::new(
        xiao::tools::ToolPolicy::default(),
        16384,
    ));
    tools.register(Arc::new(PingTool)).unwrap();
    tools.register(Arc::new(PongTool)).unwrap();

    let engine = AgentEngine::with_registry(
        sessions.clone(),
        storage.clone(),
        providers,
        AgentConfig::default(),
        tools,
    );

    let session = sessions
        .create_session(
            "owner-1",
            "PingPong Test",
            "custom",
            None,
            "pingpong-model",
            false,
            None,
        )
        .unwrap();

    let answer = engine
        .submit_to_session_with_progress("owner-1", &session.id, "run infinite ping pong", None)
        .await
        .unwrap();

    assert!(answer.answer.to_lowercase().contains("blocked"));
    assert!(answer.answer.contains("ping-pong"));
    assert!(provider.turns.load(Ordering::SeqCst) <= 10);
}
