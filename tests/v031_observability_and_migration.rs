use rusqlite::Connection;
use serde_json::json;
use std::sync::Arc;
use tempfile::tempdir;
use xiao::storage::Storage;

#[test]
fn matrix_i1_migration_26_to_27_preserves_sessions_memory_skills_profiles() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("v030_legacy.db");

    // 1. Manually instantiate a pre-migration schema 26 SQLite database
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY);
            INSERT INTO schema_migrations(version) VALUES(1),(26);

            CREATE TABLE installation_owner(
              id TEXT PRIMARY KEY,
              telegram_user_id INTEGER UNIQUE,
              created_at TEXT NOT NULL
            );
            INSERT INTO installation_owner VALUES('owner-1', 12345678, '2026-08-20T00:00:00Z');

            CREATE TABLE sessions(
              id TEXT PRIMARY KEY,
              owner_principal TEXT NOT NULL,
              name TEXT NOT NULL,
              provider TEXT NOT NULL,
              account_id TEXT,
              model TEXT NOT NULL,
              yolo_mode INTEGER NOT NULL DEFAULT 0,
              agent_profile TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              context_summary TEXT,
              summary_updated_at TEXT
            );
            INSERT INTO sessions VALUES('sess-1', 'owner-1', 'Legacy Session', 'custom', NULL, 'model-v26', 0, NULL, '2026-08-20T00:00:00Z', '2026-08-20T00:00:00Z', NULL, NULL);

            CREATE TABLE messages(
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              owner_principal TEXT NOT NULL,
              session_id TEXT NOT NULL,
              role TEXT NOT NULL,
              content TEXT NOT NULL,
              created_at TEXT NOT NULL
            );
            INSERT INTO messages(owner_principal, session_id, role, content, created_at)
            VALUES('owner-1', 'sess-1', 'user', 'legacy question', '2026-08-20T00:00:00Z');

            CREATE TABLE memories(
              id TEXT PRIMARY KEY,
              owner_principal TEXT NOT NULL,
              kind TEXT NOT NULL,
              key TEXT NOT NULL,
              content TEXT NOT NULL,
              tags_csv TEXT NOT NULL DEFAULT '',
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              UNIQUE(owner_principal, kind, key)
            );
            INSERT INTO memories VALUES('mem-1', 'owner-1', 'fact', 'user_lang', 'en', 'pref,lang', '2026-08-20T00:00:00Z', '2026-08-20T00:00:00Z');

            CREATE TABLE skills(
              id TEXT PRIMARY KEY,
              owner_id TEXT NOT NULL,
              name TEXT NOT NULL,
              version TEXT NOT NULL,
              description TEXT NOT NULL,
              definition_yaml TEXT NOT NULL,
              entrypoint TEXT NOT NULL,
              status TEXT NOT NULL,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            INSERT INTO skills VALUES('sk-1', 'owner-1', 'custom_tool', '1.0.0', 'skill desc', 'steps: []', 'main.sh', 'enabled', '2026-08-20T00:00:00Z', '2026-08-20T00:00:00Z');

            CREATE TABLE provider_profiles(
              profile_id TEXT PRIMARY KEY,
              owner_id TEXT NOT NULL,
              alias TEXT NOT NULL,
              endpoint TEXT NOT NULL,
              protocol TEXT NOT NULL,
              credential_ref TEXT,
              api_key_ref TEXT,
              safe_headers_json TEXT NOT NULL DEFAULT '{}',
              secret_headers_ref TEXT,
              enabled INTEGER NOT NULL DEFAULT 1,
              reachability TEXT NOT NULL DEFAULT 'unknown',
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              last_probe_at TEXT
            );
            INSERT INTO provider_profiles VALUES('prof-1', 'owner-1', 'Custom Provider', 'https://api.example.com/v1', 'openai_chat_completions', NULL, NULL, '{}', NULL, 1, 'reachable', '2026-08-20T00:00:00Z', '2026-08-20T00:00:00Z', NULL);

            CREATE TABLE provider_profile_models(
              profile_id TEXT NOT NULL,
              model_id TEXT NOT NULL,
              text_capable INTEGER NOT NULL DEFAULT 1,
              vision_capable INTEGER NOT NULL DEFAULT 0,
              file_input_capable INTEGER NOT NULL DEFAULT 0,
              native_tools INTEGER NOT NULL DEFAULT 1,
              structured_output INTEGER NOT NULL DEFAULT 1,
              continuation INTEGER NOT NULL DEFAULT 1,
              native_tools_state TEXT NOT NULL DEFAULT 'supported',
              structured_output_state TEXT NOT NULL DEFAULT 'supported',
              continuation_state TEXT NOT NULL DEFAULT 'supported',
              vision_state TEXT NOT NULL DEFAULT 'unknown',
              file_input_state TEXT NOT NULL DEFAULT 'unknown',
              model_discovery INTEGER NOT NULL DEFAULT 0,
              tool_protocol TEXT NOT NULL DEFAULT 'native',
              evidence TEXT NOT NULL DEFAULT '',
              probe_status TEXT NOT NULL DEFAULT 'completed',
              probe_version INTEGER NOT NULL DEFAULT 1,
              probed_at TEXT NOT NULL DEFAULT '',
              PRIMARY KEY(profile_id, model_id)
            );
            INSERT INTO provider_profile_models VALUES('prof-1', 'model-v26', 1, 0, 0, 1, 1, 1, 'supported', 'supported', 'supported', 'unknown', 'unknown', 0, 'native', 'probed v26', 'completed', 1, '2026-08-20T00:00:00Z');

            CREATE TABLE agent_runs(
              id TEXT PRIMARY KEY,
              owner_principal TEXT NOT NULL,
              session_id TEXT NOT NULL,
              provider TEXT NOT NULL,
              model TEXT NOT NULL,
              goal TEXT,
              status TEXT NOT NULL,
              created_at TEXT NOT NULL,
              completed_at TEXT,
              error TEXT
            );

            CREATE TABLE tool_runs(
              id TEXT PRIMARY KEY,
              agent_run_id TEXT NOT NULL,
              call_id TEXT NOT NULL,
              tool_name TEXT NOT NULL,
              arguments_json TEXT NOT NULL,
              risk TEXT NOT NULL,
              status TEXT NOT NULL,
              output TEXT,
              error TEXT,
              started_at TEXT NOT NULL,
              completed_at TEXT
            );
            "#,
        )
        .unwrap();
    }

    // 2. Open via Xiao Storage (runs Migration 26 -> 27)
    let storage = Storage::open(&db_path).unwrap();
    assert_eq!(storage.schema_version().unwrap(), 27);

    // 3. Verify all legacy data is preserved intact
    let sess = storage.session("owner-1", "sess-1").unwrap().unwrap();
    assert_eq!(sess.name, "Legacy Session");
    assert_eq!(sess.model, "model-v26");

    let msgs = storage.stored_messages("owner-1", "sess-1").unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "legacy question");

    let mems = storage.list_memories("owner-1", None, 10).unwrap();
    assert_eq!(mems.len(), 1);
    assert_eq!(mems[0].key, "user_lang");
    assert_eq!(mems[0].content, "en");

    let skills = storage.list_skills("owner-1", 10).unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "custom_tool");

    let profiles = storage.provider_profiles("owner-1").unwrap();
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
    storage.release_learning_job_after_delivery(&run_id).unwrap();

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
