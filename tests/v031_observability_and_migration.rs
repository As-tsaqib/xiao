use std::sync::Arc;
use xiao::storage::Storage;

#[test]
fn capability_overrides_and_evidence_persist() {
    let dir = tempfile::tempdir().unwrap();
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
fn learning_job_queue_durable_recovery_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Storage::open(&db_path).unwrap();

    // Create session and agent run
    let session = storage
        .create_session("owner-1", "Test", "custom", None, "m", false, None)
        .unwrap();
    let run_id = storage
        .create_agent_run("owner-1", &session.id, "custom", "m", Some("learning goal"))
        .unwrap();

    // Enqueue learning job
    let job_id = storage.enqueue_learning_job("owner-1", &run_id, None).unwrap();
    let job = storage.learning_job(&job_id).unwrap().unwrap();
    assert_eq!(job.status, "pending");
    assert_eq!(job.run_id, run_id);

    // Claim pending job
    let claimed = storage.claim_pending_learning_job(3).unwrap().unwrap();
    assert_eq!(claimed.id, job_id);
    assert_eq!(claimed.status, "running");
    assert_eq!(claimed.attempts, 1);

    // Reopen database to simulate daemon restart during execution
    drop(storage);
    let reopened = Storage::open(&db_path).unwrap();

    // The stale running job should be safely recovered to pending
    let recovered_job = reopened.learning_job(&job_id).unwrap().unwrap();
    assert_eq!(recovered_job.status, "pending");

    // Can be claimed again after restart
    let reclaimed = reopened.claim_pending_learning_job(3).unwrap().unwrap();
    assert_eq!(reclaimed.id, job_id);
    assert_eq!(reclaimed.status, "running");
    assert_eq!(reclaimed.attempts, 2);

    // Finish successfully
    reopened.finish_learning_job(&job_id, "succeeded", None).unwrap();
    let finished = reopened.learning_job(&job_id).unwrap().unwrap();
    assert_eq!(finished.status, "succeeded");
}

#[test]
fn tool_run_steps_and_agent_run_events_stored_and_retrieved() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("xiao.db");
    let storage = Storage::open(&db_path).unwrap();

    let session = storage
        .create_session("owner-1", "Test", "custom", None, "m", false, None)
        .unwrap();
    let run_id = storage
        .create_agent_run("owner-1", &session.id, "custom", "m", Some("goal"))
        .unwrap();
    let tool_run_id = storage
        .create_tool_run(&run_id, "call-1", "termux_job", "{}", "side_effect")
        .unwrap();

    // Record tool substeps
    storage
        .record_tool_run_step(
            &tool_run_id,
            0,
            "step-free",
            "free",
            r#"["-m"]"#,
            "succeeded",
            Some("Mem: 8000"),
            None,
        )
        .unwrap();

    storage
        .record_tool_run_step(
            &tool_run_id,
            1,
            "step-ps",
            "ps",
            r#"["-A"]"#,
            "succeeded",
            Some("PID CMD"),
            None,
        )
        .unwrap();

    let steps = storage.tool_run_steps(&tool_run_id).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].step_id, "step-free");
    assert_eq!(steps[1].step_id, "step-ps");

    // Record agent run events
    storage
        .record_run_event(&run_id, "run_started", 0, None)
        .unwrap();
    storage
        .record_run_event(&run_id, "provider_first_text_delta", 320, Some(r#"{"tokens":1}"#))
        .unwrap();
    storage
        .record_run_event(&run_id, "final_answer_ready", 1200, None)
        .unwrap();

    let events = storage.agent_run_events(&run_id).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].event_kind, "run_started");
    assert_eq!(events[1].event_kind, "provider_first_text_delta");
    assert_eq!(events[1].elapsed_ms, 320);
    assert_eq!(events[2].event_kind, "final_answer_ready");
}
