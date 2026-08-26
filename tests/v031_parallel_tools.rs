use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

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
    tools::{Tool, ToolCall, ToolContext, ToolOrigin, ToolResult, ToolRisk, ToolSpec},
};

struct ParallelTwoReadToolsProvider {
    turn: AtomicUsize,
}

#[async_trait]
impl Provider for ParallelTwoReadToolsProvider {
    fn id(&self) -> &'static str {
        "custom"
    }
    fn models(&self) -> Vec<String> {
        vec!["parallel-model".into()]
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
            evidence: "parallel test".into(),
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
        let t = self.turn.fetch_add(1, Ordering::SeqCst);
        if t == 0 {
            Ok(ProviderTurn {
                step: ProviderStep::ToolCalls(vec![
                    ToolCall {
                        call_id: "call-read-1".into(),
                        name: "slow_read_a".into(),
                        arguments: json!({}),
                    },
                    ToolCall {
                        call_id: "call-read-2".into(),
                        name: "slow_read_b".into(),
                        arguments: json!({}),
                    },
                ]),
                continuation: Some(json!({ "turn": 0 })),
                events: vec![],
            })
        } else {
            Ok(ProviderTurn {
                step: ProviderStep::Final("both reads completed".into()),
                continuation: None,
                events: vec![],
            })
        }
    }
}

struct SlowReadToolA {
    active_concurrent: Arc<AtomicUsize>,
    max_concurrent_seen: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for SlowReadToolA {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "slow_read_a".into(),
            description: "Read A".into(),
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
        let current = self.active_concurrent.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_concurrent_seen
            .fetch_max(current, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.active_concurrent.fetch_sub(1, Ordering::SeqCst);
        Ok(json!({ "result": "A", "verification_evidence": true }).to_string())
    }
}

struct SlowReadToolB {
    active_concurrent: Arc<AtomicUsize>,
    max_concurrent_seen: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for SlowReadToolB {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "slow_read_b".into(),
            description: "Read B".into(),
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
        let current = self.active_concurrent.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_concurrent_seen
            .fetch_max(current, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.active_concurrent.fetch_sub(1, Ordering::SeqCst);
        Ok(json!({ "result": "B", "verification_evidence": true }).to_string())
    }
}

#[tokio::test]
async fn read_only_tools_execute_concurrently_and_preserve_stable_order() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());
    let sessions = Arc::new(SessionManager::new(storage.clone()));
    let provider = Arc::new(ParallelTwoReadToolsProvider {
        turn: AtomicUsize::new(0),
    });
    let auth = Arc::new(xiao::auth::AuthManager::new(
        storage.clone(),
        dir.path().join("secrets"),
    ));
    let providers = Arc::new(ProviderRegistry::from_single("custom", provider, auth));

    let active_concurrent = Arc::new(AtomicUsize::new(0));
    let max_concurrent_seen = Arc::new(AtomicUsize::new(0));

    let tools = Arc::new(xiao::tools::ToolRegistry::new(
        xiao::tools::ToolPolicy::default(),
        16384,
    ));
    tools
        .register(SlowReadToolA {
            active_concurrent: active_concurrent.clone(),
            max_concurrent_seen: max_concurrent_seen.clone(),
        })
        .unwrap();
    tools
        .register(SlowReadToolB {
            active_concurrent: active_concurrent.clone(),
            max_concurrent_seen: max_concurrent_seen.clone(),
        })
        .unwrap();

    let config = AgentConfig {
        parallel_readonly_tools: true,
        max_parallel_readonly_tools: 8,
        ..Default::default()
    };

    let engine =
        AgentEngine::with_registry(sessions.clone(), storage.clone(), providers, config, tools);

    let session = storage
        .create_session(
            "owner-1",
            "Parallel Test",
            "custom",
            None,
            "parallel-model",
            false,
            None,
        )
        .unwrap();

    let answer = engine
        .submit_to_session_with_progress(
            "owner-1",
            &session.id,
            "inspect both readings in parallel",
            None,
        )
        .await
        .unwrap();

    assert_eq!(answer.final_answer, "both reads completed");
    assert!(max_concurrent_seen.load(Ordering::SeqCst) >= 2);
}
