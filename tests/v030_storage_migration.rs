use xiao::storage::Storage;

#[test]
fn storage_migration_v030_is_additive_and_idempotent() {
    let db = Storage::open_memory().unwrap();
    assert!(db.schema_version().unwrap() >= 26);

    let session = db
        .create_session(
            "owner:test",
            "v030-session",
            "custom",
            None,
            "m",
            false,
            None,
        )
        .unwrap();
    db.append_message("owner:test", &session.id, "user", "test prompt")
        .unwrap();

    // Idempotent migration
    db.migrate().unwrap();

    let fetched = db.session("owner:test", &session.id).unwrap().unwrap();
    assert_eq!(fetched.name, "v030-session");

    let messages = db.stored_messages("owner:test", &session.id).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "test prompt");
}

#[test]
fn database_contains_all_core_tables_in_v030() {
    let db = Storage::open_memory().unwrap();
    let tables: Vec<String> = db
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table'")
                .unwrap();
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            Ok(rows)
        })
        .unwrap();

    for expected in [
        "sessions",
        "messages",
        "session_summaries",
        "memories",
        "skills",
        "provider_profiles",
        "provider_profile_models",
        "attachments",
        "agent_runs",
        "tool_runs",
        "audit_events",
        "installation_owner",
        "schema_migrations",
    ] {
        assert!(
            tables.contains(&expected.to_string()),
            "missing table: {expected}"
        );
    }
}
