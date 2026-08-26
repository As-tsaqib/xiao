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
