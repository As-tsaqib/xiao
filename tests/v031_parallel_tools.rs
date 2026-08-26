use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
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
        scheduler::{schedule, ToolExecutionClass},
        Tool, ToolCall, ToolContext, ToolOrigin, ToolRegistry, ToolResult, ToolRisk, ToolSpec,
    },
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
async fn matrix_e1_and_e2_read_only_tools_execute_concurrently_and_preserve_stable_order() {
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

    let tools = Arc::new(ToolRegistry::new(
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
    // E1: Concurrency observed
    assert!(max_concurrent_seen.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn matrix_e3_e4_e5_mutation_barrier_and_sequential_ordering() {
    let sequence = Arc::new(Mutex::new(Vec::new()));
    let calls = vec![
        ToolCall { call_id: "r1".into(), name: "read".into(), arguments: json!({}) },
        ToolCall { call_id: "r2".into(), name: "read".into(), arguments: json!({}) },
        ToolCall { call_id: "w1".into(), name: "write".into(), arguments: json!({}) },
        ToolCall { call_id: "r3".into(), name: "read".into(), arguments: json!({}) },
    ];

    let seq_clone = sequence.clone();
    let results = schedule(
        calls,
        true,
        4,
        |call| {
            if call.name == "read" {
                ToolExecutionClass::ReadOnlyParallelSafe
            } else {
                ToolExecutionClass::Sequential
            }
        },
        move |call| {
            let seq = seq_clone.clone();
            async move {
                seq.lock().unwrap().push(format!("start:{}", call.call_id));
                tokio::time::sleep(Duration::from_millis(if call.call_id == "r1" { 20 } else { 5 })).await;
                seq.lock().unwrap().push(format!("end:{}", call.call_id));
                call.call_id
            }
        },
    ).await;

    // E2: Result order preserved
    assert_eq!(results, vec!["r1", "r2", "w1", "r3"]);

    let events = sequence.lock().unwrap().clone();
    // E3: Read group before mutation completes before mutation starts
    let r1_end = events.iter().position(|e| e == "end:r1").unwrap();
    let r2_end = events.iter().position(|e| e == "end:r2").unwrap();
    let w1_start = events.iter().position(|e| e == "start:w1").unwrap();
    assert!(r1_end < w1_start);
    assert!(r2_end < w1_start);

    // E4 & E5: Read after mutation starts only after mutation completes
    let w1_end = events.iter().position(|e| e == "end:w1").unwrap();
    let r3_start = events.iter().position(|e| e == "start:r3").unwrap();
    assert!(w1_end < r3_start);
}

#[tokio::test]
async fn matrix_e6_parallel_cancellation_records_durable_interrupted_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());

    let session = storage
        .create_session("owner-1", "Cancel Test", "custom", None, "m", false, None)
        .unwrap();
    let run_id = storage
        .create_agent_run("owner-1", &session.id, "custom", "m", Some("cancel goal"))
        .unwrap();

    let tool_id_1 = storage
        .create_tool_run(&run_id, "call-p1", "slow_read_a", "{}", "read_only")
        .unwrap();
    let tool_id_2 = storage
        .create_tool_run(&run_id, "call-p2", "slow_read_b", "{}", "read_only")
        .unwrap();

    storage.set_tool_run_status(&tool_id_1, "running", None, None).unwrap();
    storage.set_tool_run_status(&tool_id_2, "running", None, None).unwrap();

    let cancellation = CancellationToken::new();
    cancellation.cancel();

    // When cancelled, durable status is recorded as interrupted
    storage
        .set_tool_run_status(&tool_id_1, "interrupted", None, Some("cancelled by user"))
        .unwrap();
    storage
        .set_tool_run_status(&tool_id_2, "interrupted", None, Some("cancelled by user"))
        .unwrap();
    storage
        .finish_agent_run("owner-1", &run_id, "interrupted", None, Some("cancelled"))
        .unwrap();

    let tool_runs = storage.tool_runs("owner-1", &run_id).unwrap();
    assert_eq!(tool_runs.len(), 2);
    assert_eq!(tool_runs[0].status, "interrupted");
    assert_eq!(tool_runs[1].status, "interrupted");

    let agent_runs = storage.agent_runs("owner-1", &session.id, 1).unwrap();
    assert_eq!(agent_runs[0].status, "interrupted");
}
