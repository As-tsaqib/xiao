use rusqlite::Connection;
use serde_json::json;
use std::sync::Arc;
use tempfile::tempdir;
use xiao::storage::Storage;

#[test]
fn matrix_i1_migration_26_to_27_preserves_sessions_memory_skills_profiles() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("v030_legacy.db");

    // 1. Populate database with pre-migration v0.3.0 data, then reset schema state to 26
    {
        let storage = Arc::new(Storage::open(&db_path).unwrap());
        let session = storage
            .create_session(
                "owner-1",
                "Legacy Session",
                "custom",
                None,
                "model-v26",
                false,
                None,
            )
            .unwrap();
        storage
            .append_message("owner-1", &session.id, "user", "legacy question")
            .unwrap();

        let memory_store = xiao::memory::MemoryStore::new(storage.clone());
        memory_store
            .upsert(
                "owner-1",
                xiao::memory::MemoryScope::User,
                "preference",
                "user_lang",
                "en",
                1.0,
                "user_statement",
                Some(&session.id),
            )
            .unwrap();

        let skill_store = xiao::skills::SkillStore::new(storage.clone());
        skill_store
            .create_or_update(
                "owner-1",
                xiao::skills::SkillCandidate {
                    name: "custom-tool".into(),
                    summary: "skill desc".into(),
                    when_to_use: "when to use".into(),
                    prerequisites: String::new(),
                    procedure: "main.sh".into(),
                    pitfalls: String::new(),
                    verification: String::new(),
                },
                None,
            )
            .unwrap();

        let profile_store = xiao::providers::ProviderProfileStore::new(storage.clone());
        profile_store
            .create(xiao::storage::ProviderProfileInput {
                profile_id: Some("prof-1".into()),
                owner_id: "owner-1".into(),
                alias: "Custom Provider".into(),
                endpoint: "https://api.example.com/v1".into(),
                protocol: "openai_chat_completions".into(),
                safe_headers_json: "{}".into(),
                api_key_ref: None,
                credential_ref: None,
                secret_headers_ref: None,
            })
            .unwrap();

        drop(profile_store);
        drop(skill_store);
        drop(memory_store);
        drop(storage);

        // Roll back schema state to 26 (drop migration 27 tables & migration 27 record)
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            DELETE FROM schema_migrations WHERE version >= 27;
            DROP TABLE IF EXISTS provider_capability_evidence;
            DROP TABLE IF EXISTS learning_jobs;
            DROP TABLE IF EXISTS tool_run_steps;
            DROP TABLE IF EXISTS agent_run_events;
            "#,
        )
        .unwrap();
    }

    // 2. Open via Xiao Storage (runs Migration 26 -> 27)
    let storage = std::sync::Arc::new(Storage::open(&db_path).unwrap());
    assert_eq!(storage.schema_version().unwrap(), 27);

    // 3. Verify all legacy data is preserved intact
    let sess_list = storage.list_main_sessions("owner-1", 10, 0, true).unwrap();
    assert_eq!(sess_list.len(), 1);
    let sess = &sess_list[0];
    assert_eq!(sess.name, "Legacy Session");
    assert_eq!(sess.model, "model-v26");

    let msgs = storage.stored_messages("owner-1", &sess.id).unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "legacy question");

    let mems = xiao::memory::MemoryStore::new(storage.clone())
        .list("owner-1", None, 10)
        .unwrap();
    assert_eq!(mems.len(), 1);
    assert_eq!(mems[0].key, "user_lang");
    assert_eq!(mems[0].value, "en");

    let skills = xiao::skills::SkillStore::new(storage.clone())
        .list("owner-1", 10)
        .unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "custom-tool");

    let profiles = xiao::providers::ProviderProfileStore::new(storage.clone())
        .list("owner-1")
        .unwrap();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].alias, "Custom Provider");
}

#[test]
fn matrix_i2_and_i3_production_learning_payload_survives_restart_and_stale_lease_recovery() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Arc::new(Storage::open(&db_path).unwrap());

    let session = storage
        .create_session("owner-1", "Test", "custom", None, "m", false, None)
        .unwrap();
    let run_id = storage
        .create_agent_run("owner-1", &session.id, "custom", "m", Some("learning goal"))
        .unwrap();

    let payload = json!({
        "trace": {
            "goal": "learning goal",
            "steps": [{"tool": "uname", "output": "Linux"}]
        },
        "explicit_prompt": "learning goal"
    });

    // Enqueue production learning payload
    storage
        .enqueue_learning_payload("owner-1", &run_id, &payload)
        .unwrap();

    // Release after frontend delivery
    storage
        .release_learning_job_after_delivery(&run_id)
        .unwrap();

    // Claim job by background worker
    let (job_id, owner, run, claimed_payload) = storage.claim_learning_job().unwrap().unwrap();
    assert_eq!(owner, "owner-1");
    assert_eq!(run, run_id);
    assert_eq!(claimed_payload, payload);

    // Simulate daemon restart while job is in 'running' state
    drop(storage);
    let reopened = Storage::open(&db_path).unwrap();

    // After restart, stale running jobs are reset to pending and reclaimed safely
    let reclaimed = reopened.claim_learning_job().unwrap().unwrap();
    assert_eq!(reclaimed.0, job_id);
    assert_eq!(reclaimed.1, "owner-1");
    assert_eq!(reclaimed.2, run_id);
    assert_eq!(reclaimed.3, payload);
}

#[test]
fn matrix_i4_capability_overrides_and_evidence_persist() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Storage::open(&db_path).unwrap();

    // 1. Set evidence as Unknown initially
    storage
        .set_capability_evidence(
            "prof-1",
            "model-a",
            "openai_chat_completions",
            "vision",
            "unknown",
            "probe_inconclusive",
            None,
        )
        .unwrap();

    let evidence = storage
        .get_capability_evidence("prof-1", "model-a", "openai_chat_completions", "vision")
        .unwrap()
        .unwrap();
    assert_eq!(evidence.state, "unknown");
    assert_eq!(evidence.owner_override, "auto");
    assert_eq!(evidence.source, "probe_inconclusive");

    // 2. Upgrade to Supported via RuntimeSuccess
    storage
        .set_capability_evidence(
            "prof-1",
            "model-a",
            "openai_chat_completions",
            "vision",
            "supported",
            "runtime_success",
            None,
        )
        .unwrap();

    let evidence = storage
        .get_capability_evidence("prof-1", "model-a", "openai_chat_completions", "vision")
        .unwrap()
        .unwrap();
    assert_eq!(evidence.state, "supported");
    assert_eq!(evidence.source, "runtime_success");

    // 3. Set explicit owner override ForceUnsupported
    storage
        .set_capability_override(
            "prof-1",
            "model-a",
            "openai_chat_completions",
            "vision",
            "force_unsupported",
        )
        .unwrap();

    let evidence = storage
        .get_capability_evidence("prof-1", "model-a", "openai_chat_completions", "vision")
        .unwrap()
        .unwrap();
    assert_eq!(evidence.owner_override, "force_unsupported");

    // 4. Invalidation clears automatic evidence but preserves explicit owner override
    storage
        .invalidate_automatic_capability_evidence("prof-1")
        .unwrap();

    let evidence = storage
        .get_capability_evidence("prof-1", "model-a", "openai_chat_completions", "vision")
        .unwrap()
        .unwrap();
    assert_eq!(evidence.state, "unknown");
    assert_eq!(evidence.owner_override, "force_unsupported");

    // 5. Reopen database and verify persistence across restarts
    drop(storage);
    let reopened = Storage::open(&db_path).unwrap();
    let evidence = reopened
        .get_capability_evidence("prof-1", "model-a", "openai_chat_completions", "vision")
        .unwrap()
        .unwrap();
    assert_eq!(evidence.owner_override, "force_unsupported");
}

#[test]
fn matrix_i5_final_frontend_delivery_and_background_learning_timing_nonzero_and_ordered() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Storage::open(&db_path).unwrap();

    let session = storage
        .create_session("owner-1", "Timing Test", "custom", None, "m", false, None)
        .unwrap();
    let run_id = storage
        .create_agent_run("owner-1", &session.id, "custom", "m", Some("timing goal"))
        .unwrap();

    let elapsed = storage.agent_run_elapsed_ms(&run_id);
    assert!(elapsed >= 1);

    storage
        .record_agent_run_event(
            &run_id,
            "final_frontend_delivery",
            elapsed,
            &json!({"frontend":"telegram"}),
        )
        .unwrap();

    let later_elapsed = storage.agent_run_elapsed_ms(&run_id).max(elapsed);
    storage
        .record_agent_run_event(
            &run_id,
            "background_learning",
            later_elapsed,
            &json!({"status":"succeeded"}),
        )
        .unwrap();

    let events = storage.agent_run_events(&run_id).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_kind, "final_frontend_delivery");
    assert!(events[0].elapsed_ms >= 1);
    assert_eq!(events[1].event_kind, "background_learning");
    assert!(events[1].elapsed_ms >= events[0].elapsed_ms);
}
