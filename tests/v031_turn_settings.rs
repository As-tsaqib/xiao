use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use xiao::{
    agent::AgentEngine,
    config::{AgentConfig, AppConfig},
    providers::{
        AgentEvent, Provider, ProviderCapabilities, ProviderRegistry, ProviderRequest,
        ProviderResponse, ProviderStep, ProviderTurn, ToolProtocol,
    },
    session::SessionManager,
    storage::Storage,
    tools::{Tool, ToolCall, ToolContext, ToolOrigin, ToolResult, ToolRisk, ToolSpec},
};

struct MultiTurnScriptedProvider {
    turns_requested: AtomicUsize,
    target_turns: usize,
}

impl MultiTurnScriptedProvider {
    fn new(target_turns: usize) -> Self {
        Self {
            turns_requested: AtomicUsize::new(0),
            target_turns,
        }
    }
}

#[async_trait]
impl Provider for MultiTurnScriptedProvider {
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
            evidence: "test".into(),
        }
    }
    async fn run(
        &self,
        _: ProviderRequest,
        _: Option<mpsc::UnboundedSender<AgentEvent>>,
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
        let current = self.turns_requested.fetch_add(1, Ordering::SeqCst);
        if current < self.target_turns {
            Ok(ProviderTurn {
                step: ProviderStep::ToolCalls(vec![ToolCall {
                    call_id: format!("call-{current}"),
                    name: "step_tool".into(),
                    arguments: json!({ "step": current }),
                }]),
                continuation: Some(json!({ "turn": current })),
                events: vec![],
            })
        } else {
            Ok(ProviderTurn {
                step: ProviderStep::Final(format!("completed after {current} turns")),
                continuation: None,
                events: vec![],
            })
        }
    }
}

struct StepTool;

#[async_trait]
impl Tool for StepTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "step_tool".into(),
            description: "A test tool for multi-turn execution".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "step": { "type": "integer" }
                },
                "required": ["step"]
            }),
            risk: ToolRisk::ReadOnly,
            origin: ToolOrigin::Builtin,
            effect: xiao::tools::ToolEffect::Idempotent,
            required_capabilities: vec![],
            timeout_ms: 5000,
        }
    }

    async fn execute(
        &self,
        _context: &ToolContext,
        arguments: serde_json::Value,
    ) -> anyhow::Result<String> {
        let step = arguments.get("step").and_then(|v| v.as_u64()).unwrap_or(0);
        Ok(json!({ "status": "ok", "step": step, "verification_evidence": true }).to_string())
    }
}

struct FailingLoopProvider {
    turns: AtomicUsize,
}

#[async_trait]
impl Provider for FailingLoopProvider {
    fn id(&self) -> &'static str {
        "custom"
    }
    fn models(&self) -> Vec<String> {
        vec!["fail-model".into()]
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
            evidence: "test".into(),
        }
    }
    async fn run(
        &self,
        _: ProviderRequest,
        _: Option<mpsc::UnboundedSender<AgentEvent>>,
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
        let turn = self.turns.fetch_add(1, Ordering::SeqCst);
        Ok(ProviderTurn {
            step: ProviderStep::ToolCalls(vec![ToolCall {
                call_id: format!("fail-{turn}"),
                name: "failing_action".into(),
                arguments: json!({ "attempt": "identical" }),
            }]),
            continuation: Some(json!({ "turn": turn })),
            events: vec![],
        })
    }
}

struct FailingActionTool;

#[async_trait]
impl Tool for FailingActionTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "failing_action".into(),
            description: "A tool that intentionally fails".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "attempt": { "type": "string" }
                }
            }),
            risk: ToolRisk::SideEffect,
            origin: ToolOrigin::Builtin,
            effect: xiao::tools::ToolEffect::NonIdempotent,
            required_capabilities: vec![],
            timeout_ms: 5000,
        }
    }

    async fn execute(
        &self,
        _context: &ToolContext,
        _arguments: serde_json::Value,
    ) -> anyhow::Result<String> {
        Err(anyhow::anyhow!("device network interface down"))
    }
}

#[test]
fn default_max_turns_is_150() {
    let config = AgentConfig::default();
    assert_eq!(config.max_turns, 150);
}

#[test]
fn config_validator_accepts_150_and_rejects_bounds() {
    let mut config = AppConfig::default();
    config.agent.max_turns = 150;
    assert!(config.validate().is_ok());

    config.agent.max_turns = 2;
    assert!(config.validate().is_ok());

    config.agent.max_turns = 500;
    assert!(config.validate().is_ok());

    config.agent.max_turns = 1;
    assert!(config.validate().is_err());

    config.agent.max_turns = 501;
    assert!(config.validate().is_err());
}

#[tokio::test]
async fn scripted_provider_exceeds_eight_turns_without_premature_failure() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(MultiTurnScriptedProvider::new(12));
    let auth = Arc::new(xiao::auth::AuthManager::new(
        storage.clone(),
        dir.path().join("secrets"),
    ));
    let providers = Arc::new(ProviderRegistry::from_single(
        "custom",
        provider.clone(),
        auth,
    ));

    let mut config = AgentConfig::default();
    config.max_turns = 150;

    let tools = Arc::new(xiao::tools::ToolRegistry::new(
        xiao::tools::ToolPolicy::default(),
        16384,
    ));
    tools.register(StepTool).unwrap();

    let engine =
        AgentEngine::with_registry(sessions.clone(), storage.clone(), providers, config, tools);

    let session = storage
        .create_session("owner-1", "Test", "custom", None, "test-model", false, None)
        .unwrap();
    let answer = engine
        .submit_to_session_with_progress("owner-1", &session.id, "run 12 turns", None)
        .await
        .unwrap();

    assert!(answer.final_answer.contains("completed after 12 turns"));
    assert_eq!(provider.turns_requested.load(Ordering::SeqCst), 13);
}

#[tokio::test]
async fn no_progress_guard_stops_repeated_loop_at_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(FailingLoopProvider {
        turns: AtomicUsize::new(0),
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

    let mut config = AgentConfig::default();
    config.max_no_progress_repeats = 3;
    config.max_turns = 150;

    let tools = Arc::new(xiao::tools::ToolRegistry::new(
        xiao::tools::ToolPolicy::default(),
        16384,
    ));
    tools.register(FailingActionTool).unwrap();

    let engine =
        AgentEngine::with_registry(sessions.clone(), storage.clone(), providers, config, tools);

    let session = storage
        .create_session("owner-1", "Test", "custom", None, "fail-model", false, None)
        .unwrap();
    let answer = engine
        .submit_to_session_with_progress("owner-1", &session.id, "trigger fail loop", None)
        .await
        .unwrap();

    assert!(answer.final_answer.to_lowercase().contains("blocked"));
    assert!(answer.final_answer.contains("no-progress limit reached"));
    assert!(provider.turns.load(Ordering::SeqCst) <= 5);
}
