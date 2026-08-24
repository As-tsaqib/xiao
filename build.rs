use std::{env, fs, process::Command};

fn run(program: &str, args: &[&str]) {
    let status = Command::new(program).args(args).status().expect("spawn command");
    assert!(status.success(), "{program} {args:?} failed");
}

fn replace_once(path: &str, from: &str, to: &str) {
    let text = fs::read_to_string(path).unwrap_or_else(|_| panic!("read {path}"));
    let count = text.matches(from).count();
    assert_eq!(count, 1, "{path}: expected one match, got {count}: {from:?}");
    fs::write(path, text.replacen(from, to, 1)).unwrap_or_else(|_| panic!("write {path}"));
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var_os("GITHUB_ACTIONS").is_none() {
        return;
    }

    replace_once(
        "src/command/mod.rs",
        "            .unwrap();\n        storage.upsert_account(&account(\"c1\", \"codex\")).unwrap();\n        storage.set_account_owner(\"p\", \"c1\").unwrap();\n\n        let (provider, model) = core.use_account(\"p\", None, \"c1\").unwrap();",
        "            .unwrap();\n        // Establish an explicit frontend pointer before exercising exact-session management.\n        // Creating a raw storage session is intentionally not a frontend navigation action.\n        sessions.switch_main(\"p\", &first.id).unwrap();\n        storage.upsert_account(&account(\"c1\", \"codex\")).unwrap();\n        storage.set_account_owner(\"p\", \"c1\").unwrap();\n\n        let (provider, model) = core.use_account(\"p\", None, \"c1\").unwrap();",
    );

    replace_once(
        "src/ipc/mod.rs",
        "                \"api_key_configured\": profile.credential_ref.is_some(),\n                \"safe_headers\": profile.safe_headers().unwrap_or_default(),\n                \"header_names\": profile.safe_headers().unwrap_or_default().keys().cloned().collect::<Vec<_>>(),",
        "                \"api_key_configured\": profile.credential_ref.is_some(),\n                // Header values are write-only just like API keys. The manager may expose\n                // names for inspection, but never returns stored values to a frontend.\n                \"header_names\": profile.safe_headers().unwrap_or_default().keys().cloned().collect::<Vec<_>>(),",
    );
    replace_once(
        "src/ipc/mod.rs",
        "        for control in [\"addCodex\", \"addAgy\", \"modelPickerPager\"] {",
        "        for control in [\n            \"addCodex\",\n            \"addAgy\",\n            \"sessionAiProvider\",\n            \"sessionAiBinding\",\n            \"sessionAiModel\",\n        ] {",
    );
    replace_once(
        "src/ipc/mod.rs",
        "            \"Models / Use\",",
        "            \"Change AI Configuration\",",
    );

    replace_once(
        "module/webroot/assets/app.js",
        "  $('profileEditHeaders').value = JSON.stringify(profile.safe_headers || {}, null, 2);",
        "  // Stored header values are write-only. Blank means preserve existing headers;\n  // entering JSON explicitly replaces them (use {} to clear all).\n  $('profileEditHeaders').value = '';",
    );
    replace_once(
        "module/webroot/assets/app.js",
        "  let headers;\n  try { headers = parseHeaders($('profileEditHeaders').value); } catch (error) { notice(error.message, 'bad'); return; }",
        "  let headers;\n  const headerInput = $('profileEditHeaders').value.trim();\n  if (headerInput) {\n    try { headers = parseHeaders(headerInput); } catch (error) { notice(error.message, 'bad'); return; }\n  }",
    );
    replace_once(
        "module/webroot/assets/app.js",
        "  const body = { action: 'edit', profile_id: profileId, alias: $('profileEditAlias').value.trim(), endpoint, protocol: $('profileEditProtocol').value, headers, remove_api_key: action === 'remove', keep_credential: endpointChanged && action === 'keep', ...(action === 'replace' ? { api_key: replacement } : {}) };",
        "  const body = { action: 'edit', profile_id: profileId, alias: $('profileEditAlias').value.trim(), endpoint, protocol: $('profileEditProtocol').value, ...(headers !== undefined ? { headers } : {}), remove_api_key: action === 'remove', keep_credential: endpointChanged && action === 'keep', ...(action === 'replace' ? { api_key: replacement } : {}) };",
    );

    replace_once(
        "src/telegram/mod.rs",
        "        cfg.telegram.access.allowed_chat_ids = vec![100];\n        cfg.telegram.access.allowed_user_ids = vec![10, 11];",
        "        cfg.telegram.access.allowed_chat_ids = vec![100];\n        cfg.telegram.access.owner_user_id = Some(10);",
    );
    replace_once(
        "src/telegram/mod.rs",
        "        // An otherwise authorized different owner cannot mutate this menu or\n        // its custom-login state.",
        "        // A non-owner cannot mutate this menu or its custom-login state.",
    );
    replace_once(
        "src/telegram/mod.rs",
        "        cfg.telegram.access.allowed_chat_ids = vec![100, 200];\n        cfg.telegram.access.allowed_user_ids = vec![10, 20];",
        "        cfg.telegram.access.allowed_chat_ids = vec![100, 200];\n        cfg.telegram.access.owner_user_id = Some(10);",
    );
    replace_once(
        "src/telegram/mod.rs",
        "        // A different principal remains responsive while principal A is generating.\n        let status = message(2, 200, 20, \"/status\");",
        "        // Another allowed chat for the same single owner remains responsive while\n        // generation is active; no second owner/principal exists in Xiao.\n        let status = message(2, 200, 10, \"/status\");",
    );
    replace_once(
        "src/telegram/mod.rs",
        "                1, 8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252,\n                255, 31, 0, 3, 3, 2, 0, 238, 254, 95, 91, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,\n                130,",
        "                1, 8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100,\n                248, 15, 0, 1, 5, 1, 1, 39, 24, 227, 102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,\n                130,",
    );
    replace_once(
        "src/telegram/mod.rs",
        "        cfg.telegram.enabled = true;\n        cfg.telegram.access.allowed_user_ids = vec![10];\n        cfg.telegram.access.allowed_chat_ids = vec![100];\n        let app = AppState::build(cfg).await.unwrap();",
        "        cfg.telegram.enabled = true;\n        cfg.telegram.access.owner_user_id = Some(10);\n        cfg.telegram.access.allowed_chat_ids = vec![100];\n        let app = AppState::build(cfg).await.unwrap();",
    );

    fs::remove_file("build.rs").expect("remove one-shot build hook");
    run("cargo", &["fmt", "--all"]);
    run("node", &["--check", "module/webroot/assets/app.js"]);
    run("git", &["diff", "--check"]);
    run("git", &["config", "user.name", "github-actions[bot]"]);
    run("git", &["config", "user.email", "41898282+github-actions[bot]@users.noreply.github.com"]);
    run("git", &["add", "src/command/mod.rs", "src/ipc/mod.rs", "src/telegram/mod.rs", "module/webroot/assets/app.js", "build.rs"]);
    run("git", &["commit", "-m", "fix: align parity regressions with v0.2.7 invariants"]);
    run("git", &["push", "origin", "HEAD:feat/v0.2.7-control-plane-unification"]);
}
