use serde_json::json;
use tempfile::tempdir;
use xiao::{
    security::redact::redact_text,
    tools::cache::{
        dynamic_observation_is_cacheable, script_hash, CachedPlan, CachedScript, PlanCache,
    },
};

#[test]
fn secret_bearing_content_is_detected_and_redacted() {
    let secret_prompt = "api_key=sk-1234567890abcdef1234567890abcdef";
    let redacted = redact_text(secret_prompt);
    assert!(!redacted.contains("sk-1234567890abcdef1234567890abcdef"));
}

#[test]
fn matrix_g1_same_safe_plan_gets_stable_cache_key() {
    let plan = CachedPlan {
        steps: json!({
            "steps": [
                { "id": "1", "program": "free", "args": ["-m"] },
                { "id": "2", "program": "ps", "args": ["-A"] }
            ]
        }),
        schema_version: 1,
        environment_fingerprint: "termux-env".into(),
    };
    let key1 = plan.key().unwrap();
    let key2 = plan.key().unwrap();
    assert_eq!(key1, key2);

    let cache = PlanCache::new();
    let inserted_key = cache.insert(plan.clone()).unwrap();
    assert_eq!(inserted_key, key1);
    let retrieved = cache.get(&key1).unwrap();
    assert_eq!(retrieved, plan);
}

#[test]
fn matrix_g2_secret_bearing_content_rejected_from_cache() {
    let secret_plan = CachedPlan {
        steps: json!({
            "steps": [
                {
                    "id": "1",
                    "program": "echo",
                    "args": ["Authorization: Bearer sk-1234567890abcdef"]
                }
            ]
        }),
        schema_version: 1,
        environment_fingerprint: "termux-env".into(),
    };
    assert!(secret_plan.key().is_err());
    let cache = PlanCache::new();
    assert!(cache.insert(secret_plan).is_err());
}

#[test]
fn matrix_g3_environment_or_schema_version_change_invalidates_plan() {
    let plan = CachedPlan {
        steps: json!([{"program": "ps", "args": ["-A"]}]),
        schema_version: 1,
        environment_fingerprint: "termux-v1".into(),
    };
    let key_original = plan.key().unwrap();

    let mut schema_changed = plan.clone();
    schema_changed.schema_version = 2;
    assert_ne!(key_original, schema_changed.key().unwrap());

    let mut env_changed = plan.clone();
    env_changed.environment_fingerprint = "termux-v2".into();
    assert_ne!(key_original, env_changed.key().unwrap());
}

#[test]
fn matrix_g4_dynamic_read_result_tools_not_cached_by_default() {
    assert!(!dynamic_observation_is_cacheable("termux_terminal"));
    assert!(!dynamic_observation_is_cacheable("termux_job"));
    assert!(!dynamic_observation_is_cacheable("context_stats"));
    assert!(!dynamic_observation_is_cacheable("session_search"));
    assert!(!dynamic_observation_is_cacheable("memory_search"));
    assert!(dynamic_observation_is_cacheable("pdf_create"));
}

#[test]
fn matrix_g5_cached_file_backed_script_hash_verified_before_execution() {
    let dir = tempdir().unwrap();
    let script_path = dir.path().join("inspect.sh");
    std::fs::write(&script_path, "echo 'safe inspection'").unwrap();
    let hash = script_hash(&script_path).unwrap();

    let cached = CachedScript {
        path: script_path.clone(),
        interpreter: "/bin/sh".into(),
        sha256: hash,
        source: "builtin:test".into(),
    };
    assert!(cached.verify().is_ok());

    // Modifying the script must invalidate hash verification
    std::fs::write(&script_path, "echo 'modified inspection'").unwrap();
    assert!(cached.verify().is_err());
}

#[test]
fn matrix_g6_script_cannot_become_root_escalation_path() {
    let dir = tempdir().unwrap();

    // 1. su command in script rejected
    let su_path = dir.path().join("su_script.sh");
    std::fs::write(&su_path, "su -c 'id'").unwrap();
    let cached_su = CachedScript {
        path: su_path.clone(),
        interpreter: "/bin/sh".into(),
        sha256: script_hash(&su_path).unwrap(),
        source: "builtin:test".into(),
    };
    assert!(cached_su.verify().is_err());

    // 2. tsu command in script rejected
    let tsu_path = dir.path().join("tsu_script.sh");
    std::fs::write(&tsu_path, "tsu -c 'whoami'").unwrap();
    let cached_tsu = CachedScript {
        path: tsu_path.clone(),
        interpreter: "/bin/sh".into(),
        sha256: script_hash(&tsu_path).unwrap(),
        source: "builtin:test".into(),
    };
    assert!(cached_tsu.verify().is_err());

    // 3. Untrusted interpreter path rejected
    let safe_path = dir.path().join("safe.sh");
    std::fs::write(&safe_path, "echo hello").unwrap();
    let untrusted_interp = CachedScript {
        path: safe_path.clone(),
        interpreter: "/tmp/evil_sh".into(),
        sha256: script_hash(&safe_path).unwrap(),
        source: "builtin:test".into(),
    };
    assert!(untrusted_interp.verify().is_err());
}

#[tokio::test]
async fn matrix_g7_termux_job_execution_registers_and_hits_cache() {
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;
    use xiao::storage::Storage;

    let dir = tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());

    struct SimpleExecutor;
    #[async_trait::async_trait]
    impl xiao::runtime::ProcessExecutor for SimpleExecutor {
        async fn execute(
            &self,
            command: xiao::runtime::TermuxCommand,
            _cancellation: CancellationToken,
        ) -> anyhow::Result<xiao::runtime::CommandOutcome> {
            Ok(xiao::runtime::CommandOutcome {
                program: command.program,
                args: command.args,
                cwd: command.cwd,
                exit_code: Some(0),
                stdout: "ok\n".into(),
                stderr: String::new(),
                duration_ms: 5,
                truncated: false,
                timed_out: false,
                cancelled: false,
            })
        }
    }

    struct SimpleBackend;
    #[async_trait::async_trait]
    impl xiao::runtime::PackageBackend for SimpleBackend {
        async fn is_available(&self, _pkg: &str) -> anyhow::Result<bool> {
            Ok(true)
        }
        async fn is_installed(&self, _pkg: &str) -> anyhow::Result<bool> {
            Ok(true)
        }
        async fn install(&self, _pkg: &str, _c: CancellationToken) -> anyhow::Result<()> {
            Ok(())
        }
    }

    let caps = Arc::new(xiao::runtime::CapabilityRegistry::empty());
    caps.register_runtime(
        "execution.termux",
        xiao::runtime::CapabilityStatus::Available {
            backend: xiao::runtime::ExecutionBackend::Termux,
            path: PathBuf::from("/data/data/com.termux/files/usr/bin"),
        },
    );

    let resolver = Arc::new(xiao::runtime::DependencyResolver::new(
        caps,
        Arc::new(SimpleBackend),
        Arc::new(xiao::runtime::TrustedPackageRepository::default()),
        storage.clone(),
    ));

    let terminal = xiao::tools::builtin::TermuxTerminalTool::new(
        Arc::new(SimpleExecutor),
        resolver,
        dir.path(),
    );
    let plan_cache = Arc::new(PlanCache::new());
    let job = xiao::tools::builtin::TermuxJobTool::with_cache(
        terminal,
        16,
        Some(storage.clone()),
        plan_cache.clone(),
    );

    let session = storage
        .create_session("owner-1", "Cache Test", "custom", None, "m", false, None)
        .unwrap();
    let run_id = storage
        .create_agent_run("owner-1", &session.id, "custom", "m", Some("cache test"))
        .unwrap();

    let ctx = xiao::tools::ToolContext {
        principal: "owner-1".into(),
        session_id: session.id.clone(),
        agent_run_id: run_id.clone(),
        yolo_mode: false,
        messages: vec![],
        cancellation: CancellationToken::new(),
        progress: None,
    };

    let args = json!({
        "steps": [
            { "id": "1", "program": "ps", "args": ["-A"] }
        ]
    });

    // First execution: cache miss, then inserts
    let res1 = job.execute(&ctx, args.clone()).await.unwrap();
    assert!(res1.contains("succeeded"));

    let events1 = storage.agent_run_events(&run_id).unwrap();
    assert!(events1.iter().any(|e| e.event_kind == "plan_cache_miss"));

    // Second execution: cache hit
    let res2 = job.execute(&ctx, args).await.unwrap();
    assert!(res2.contains("succeeded"));

    let events2 = storage.agent_run_events(&run_id).unwrap();
    assert!(events2.iter().any(|e| e.event_kind == "plan_cache_hit"));
}
