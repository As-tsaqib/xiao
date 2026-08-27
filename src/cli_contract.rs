//! P1-9 Stable CLI success JSON contracts and human-readable formatting.
//!
//! The outer envelope `{status:"ok", data: <Dto>}` is stable in `cli::CliPresenter`.
//! This module defines stable, application-facing DTOs / projections for the
//! *success data* payloads, along with concise, human-readable terminal renderers.
//!
//! Invariants enforced here:
//! - No Telegram View/button schema (`blocks`, `actions`, `view`, `buttons`) ever
//!   surfaces on the public CLI, even if the daemon accidentally returns it.
//! - No secrets: tokens, api keys, header values, credential blobs are stripped.
//!   Only booleans such as `token_configured` / `api_key_configured` may appear.
//! - Stable keys: each projection emits exactly the documented keys; unknown
//!   raw keys are dropped.
//! - Human output: consistent, concise, labeled rows and readable bounded tables.

use serde_json::{json, Map, Value};

pub const CONTRACT_VERSION: &str = "1";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn strip_view_schema(value: &mut Value) {
    if let Value::Object(map) = value {
        map.remove("blocks");
        map.remove("actions");
        map.remove("view");
        map.remove("buttons");
        map.remove("actionRows");
        map.remove("action_rows");
        for v in map.values_mut() {
            strip_view_schema(v);
        }
    } else if let Value::Array(arr) = value {
        for v in arr {
            strip_view_schema(v);
        }
    }
}

fn strip_secret_keys(value: &mut Value) {
    const FORBIDDEN_EXACT: &[&str] = &[
        "token",
        "telegram_bot_token",
        "bot_token",
        "api_key",
        "custom_api_key",
        "antigravity_oauth_client_secret",
        "secret",
        "credential",
        "credentials",
        "headers",
        "header_value",
        "client_secret",
        "authorization",
    ];
    const FORBIDDEN_SUBSTR: &[&str] = &["secret", "token", "credential"];
    if let Value::Object(map) = value {
        let keys: Vec<String> = map.keys().cloned().collect();
        for k in keys {
            let lk = k.to_ascii_lowercase();
            let exact = FORBIDDEN_EXACT.iter().any(|e| lk == *e);
            let substr = FORBIDDEN_SUBSTR.iter().any(|s| lk.contains(*s))
                && lk != "token_configured"
                && lk != "api_key_configured"
                && lk != "credential_configured"
                && lk != "credential_metadata";
            if exact || substr {
                if matches!(
                    k.as_str(),
                    "token_configured" | "api_key_configured" | "credential_configured"
                ) {
                    continue;
                }
                map.remove(&k);
                continue;
            }
            if let Some(v) = map.get_mut(&k) {
                strip_secret_keys(v);
            }
        }
        map.remove("blocks");
        map.remove("actions");
        map.remove("view");
        map.remove("buttons");
    } else if let Value::Array(arr) = value {
        for v in arr {
            strip_secret_keys(v);
        }
    }
}

fn sanitize(mut v: Value) -> Value {
    strip_view_schema(&mut v);
    strip_secret_keys(&mut v);
    v
}

fn pick_object(raw: &Value, allowed: &[&str]) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(obj) = raw.as_object() {
        for k in allowed {
            if let Some(v) = obj.get(*k) {
                out.insert((*k).to_string(), v.clone());
            }
        }
    }
    out
}

fn is_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

fn scalar(value: &Value) -> String {
    match value {
        Value::Null => "none".into(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}

fn human_key(value: &str) -> String {
    value.replace('_', " ")
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

// ---------------------------------------------------------------------------
// Projections — each returns a stable Value ready for {status:"ok", data:…}
// ---------------------------------------------------------------------------

pub fn project_status(raw: Value) -> Value {
    let raw = sanitize(raw);
    let allowed = ["owner_id", "health", "counts", "current_ai", "runtime"];
    let out = pick_object(&raw, &allowed);
    Value::Object(out)
}

pub fn project_telegram(raw: Value) -> Value {
    let raw = sanitize(raw);
    let target = if let Some(t) = raw.get("telegram") {
        Some(t)
    } else if let Some(t) = raw.get("result").and_then(|r| r.get("status")) {
        Some(t)
    } else if let Some(t) = raw.get("status").filter(|s| s.is_object()) {
        Some(t)
    } else {
        None
    };
    if let Some(t) = target {
        let t = sanitize(t.clone());
        let allowed = [
            "enabled",
            "owner_user_id",
            "owner_state",
            "legacy_candidate_count",
            "allowed_chat_ids",
            "token_configured",
            "bot",
        ];
        let mut bot_filtered = Map::new();
        if let Some(bot) = t.as_object().and_then(|m| m.get("bot")) {
            if let Some(bobj) = bot.as_object() {
                for k in ["id", "username", "first_name"] {
                    if let Some(v) = bobj.get(k) {
                        bot_filtered.insert(k.to_string(), v.clone());
                    }
                }
            } else if !bot.is_null() {
                bot_filtered.insert("value".to_string(), bot.clone());
            }
        }
        let mut telegram_out = pick_object(&t, &allowed);
        if !bot_filtered.is_empty() {
            if t.get("bot").is_some() {
                telegram_out.insert("bot".to_string(), Value::Object(bot_filtered));
            }
        } else if t.get("bot").is_some() && t.get("bot").unwrap().is_null() {
            telegram_out.insert("bot".to_string(), Value::Null);
        }
        return json!({ "telegram": Value::Object(telegram_out) });
    }
    let allowed = [
        "enabled",
        "owner_user_id",
        "owner_state",
        "allowed_chat_ids",
        "token_configured",
        "bot",
    ];
    let m = pick_object(&raw, &allowed);
    if !m.is_empty() {
        return json!({ "telegram": Value::Object(m) });
    }
    sanitize(raw)
}

pub fn project_sessions(raw: Value) -> Value {
    let raw = sanitize(raw);
    let mut out = pick_object(
        &raw,
        &[
            "items",
            "page",
            "pages",
            "page_size",
            "active_cli_session_id",
        ],
    );
    if let Some(items) = out.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            if let Some(obj) = item.as_object() {
                let allowed = [
                    "id",
                    "name",
                    "provider",
                    "account_or_profile_id",
                    "model",
                    "message_count",
                    "archived",
                    "yolo",
                    "created_at",
                    "last_active_at",
                    "telegram_scope",
                ];
                let filtered = pick_object(&Value::Object(obj.clone()), &allowed);
                *item = Value::Object(filtered);
            }
            strip_view_schema(item);
        }
    }
    Value::Object(out)
}

pub fn project_session_item(raw: Value) -> Value {
    let raw = sanitize(raw);
    let allowed = [
        "id",
        "name",
        "provider",
        "account_or_profile_id",
        "model",
        "message_count",
        "archived",
        "yolo",
        "created_at",
        "last_active_at",
        "telegram_scope",
    ];
    let inner = if let Some(obj) = raw.as_object() {
        if let Some(sess) = obj.get("session") {
            sess
        } else {
            &raw
        }
    } else {
        &raw
    };
    let m = pick_object(inner, &allowed);
    if m.is_empty() {
        sanitize(inner.clone())
    } else {
        Value::Object(m)
    }
}

pub fn project_context(raw: Value) -> Value {
    let raw = sanitize(raw);
    let allowed = [
        "session_id",
        "main_session_id",
        "mode",
        "main_messages",
        "effective_messages",
        "stored_characters",
        "context_budget_characters",
        "summary_available",
        "active_memory_entries",
        "skills_available",
        "provider",
        "account_or_profile_id",
        "model",
    ];
    Value::Object(pick_object(&raw, &allowed))
}

pub fn project_memory(raw: Value) -> Value {
    let raw = sanitize(raw);
    let mut out = pick_object(&raw, &["items", "page", "pages", "page_size", "reconciled"]);
    if let Some(items) = out.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            if let Some(obj) = item.as_object() {
                let allowed = [
                    "id",
                    "owner_principal",
                    "scope",
                    "category",
                    "key",
                    "value",
                    "confidence",
                    "source_kind",
                    "source_session_id",
                    "created_at",
                    "updated_at",
                ];
                let filtered = pick_object(&Value::Object(obj.clone()), &allowed);
                *item = Value::Object(filtered);
            }
        }
    }
    Value::Object(out)
}

pub fn project_memory_item(raw: Value) -> Value {
    let raw = sanitize(raw);
    let allowed = [
        "id",
        "owner_principal",
        "scope",
        "category",
        "key",
        "value",
        "confidence",
        "source_kind",
        "source_session_id",
        "created_at",
        "updated_at",
    ];
    let m = pick_object(&raw, &allowed);
    if m.is_empty() {
        raw
    } else {
        Value::Object(m)
    }
}

pub fn project_skills(raw: Value) -> Value {
    let raw = sanitize(raw);
    let mut out = pick_object(&raw, &["items", "page", "pages", "page_size", "reconciled"]);
    if let Some(items) = out.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            if let Some(obj) = item.as_object() {
                let allowed = [
                    "id",
                    "name",
                    "owner_principal",
                    "source_kind",
                    "enabled",
                    "version",
                    "description",
                    "capabilities",
                    "created_at",
                    "updated_at",
                ];
                let filtered = pick_object(&Value::Object(obj.clone()), &allowed);
                *item = Value::Object(filtered);
            }
        }
    }
    Value::Object(out)
}

pub fn project_skill_item(raw: Value) -> Value {
    let raw = sanitize(raw);
    let allowed = [
        "id",
        "name",
        "owner_principal",
        "source_kind",
        "enabled",
        "version",
        "description",
        "capabilities",
        "created_at",
        "updated_at",
    ];
    let m = pick_object(&raw, &allowed);
    if m.is_empty() {
        raw
    } else {
        Value::Object(m)
    }
}

pub fn project_approvals(raw: Value) -> Value {
    let raw = sanitize(raw);
    let items = if let Some(obj) = raw.as_object() {
        if let Some(arr) = obj.get("items").and_then(|v| v.as_array()) {
            arr.clone()
        } else if let Some(arr) = obj.get("pending_approvals").and_then(|v| v.as_array()) {
            arr.clone()
        } else {
            vec![]
        }
    } else if let Some(arr) = raw.as_array() {
        arr.clone()
    } else {
        vec![]
    };
    let mut sanitized = Vec::new();
    for mut item in items {
        strip_view_schema(&mut item);
        strip_secret_keys(&mut item);
        if let Some(obj) = item.as_object_mut() {
            let allowed = [
                "id",
                "owner_principal",
                "session_id",
                "agent_run_id",
                "tool_call_id",
                "capability",
                "tool_name",
                "arguments_hash",
                "risk",
                "summary",
                "status",
                "approval_mode",
                "requested_at",
                "decided_at",
                "expires_at",
                "consumed_at",
            ];
            let filtered: Map<String, Value> = allowed
                .iter()
                .filter_map(|k| obj.get(*k).map(|v| (k.to_string(), v.clone())))
                .collect();
            item = Value::Object(filtered);
        }
        sanitized.push(item);
    }
    json!({ "items": sanitized })
}

pub fn project_attachments(raw: Value) -> Value {
    let raw = sanitize(raw);
    let mut out = pick_object(&raw, &["items", "usage"]);
    if let Some(items) = out.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            if let Some(obj) = item.as_object() {
                let allowed = [
                    "attachment_id",
                    "owner_id",
                    "session_id",
                    "original_name",
                    "declared_mime",
                    "detected_mime",
                    "kind",
                    "size_bytes",
                    "sha256",
                    "processing_status",
                    "summary",
                    "error",
                    "created_at",
                    "updated_at",
                ];
                let filtered = pick_object(&Value::Object(obj.clone()), &allowed);
                *item = Value::Object(filtered);
            }
        }
    }
    Value::Object(out)
}

pub fn project_attachment_item(raw: Value) -> Value {
    let raw = sanitize(raw);
    let allowed = [
        "attachment_id",
        "id",
        "owner_id",
        "session_id",
        "original_name",
        "declared_mime",
        "detected_mime",
        "kind",
        "size_bytes",
        "sha256",
        "processing_status",
        "summary",
        "error",
        "created_at",
        "updated_at",
    ];
    let m = pick_object(&raw, &allowed);
    if m.is_empty() {
        raw
    } else {
        Value::Object(m)
    }
}

pub fn project_runs(raw: Value) -> Value {
    let raw = sanitize(raw);
    let mut out = pick_object(&raw, &["items", "page", "pages", "page_size"]);
    if let Some(items) = out.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items.iter_mut() {
            if let Some(obj) = item.as_object() {
                let allowed = [
                    "id",
                    "session_id",
                    "provider",
                    "model",
                    "status",
                    "goal",
                    "started_at",
                    "finished_at",
                    "blocker_or_error",
                    "result",
                    "verification",
                    "tools",
                    "dependency_installs",
                ];
                let filtered = pick_object(&Value::Object(obj.clone()), &allowed);
                *item = Value::Object(filtered);
            }
        }
    }
    Value::Object(out)
}

pub fn project_run_item(raw: Value) -> Value {
    let raw = sanitize(raw);
    let allowed = [
        "id",
        "session_id",
        "provider",
        "model",
        "status",
        "goal",
        "started_at",
        "finished_at",
        "blocker_or_error",
        "result",
        "verification",
        "tools",
        "dependency_installs",
    ];
    let inner = if let Some(obj) = raw.as_object() {
        if let Some(r) = obj.get("run") {
            r
        } else {
            &raw
        }
    } else {
        &raw
    };
    let m = pick_object(inner, &allowed);
    if m.is_empty() {
        raw
    } else {
        Value::Object(m)
    }
}

pub fn project_doctor(raw: Value) -> Value {
    let raw = sanitize(raw);
    let mut out = Map::new();
    if let Some(obj) = raw.as_object() {
        if let Some(checks) = obj.get("checks").and_then(|v| v.as_array()) {
            let mut sanitized_checks = Vec::new();
            for c in checks {
                if let Some(cobj) = c.as_object() {
                    let allowed = ["status", "name", "evidence", "source", "ran_at"];
                    let filtered = pick_object(&Value::Object(cobj.clone()), &allowed);
                    sanitized_checks.push(Value::Object(filtered));
                } else {
                    sanitized_checks.push(sanitize(c.clone()));
                }
            }
            out.insert("checks".to_string(), Value::Array(sanitized_checks));
        }
        if let Some(ran) = obj.get("ran_at") {
            out.insert("ran_at".to_string(), ran.clone());
        }
        for k in ["summary", "ok"] {
            if let Some(v) = obj.get(k) {
                out.insert(k.to_string(), v.clone());
            }
        }
        if out.is_empty() {
            if let Some(items) = obj.get("items").and_then(|v| v.as_array()) {
                out.insert("checks".to_string(), Value::Array(items.clone()));
            }
        }
    }
    if out.is_empty() {
        return sanitize(raw);
    }
    Value::Object(out)
}

pub fn project_tools(raw: Value) -> Value {
    let raw = sanitize(raw);
    let out = pick_object(&raw, &["items"]);
    if out.is_empty() {
        if let Some(arr) = raw.as_array() {
            return json!({"items": sanitize(Value::Array(arr.clone()))});
        }
        return sanitize(raw);
    }
    Value::Object(out)
}

pub fn project_accounts(raw: Value) -> Value {
    let raw = sanitize(raw);
    let items = if let Some(obj) = raw.as_object() {
        if let Some(arr) = obj.get("accounts").and_then(|v| v.as_array()) {
            arr.clone()
        } else if let Some(arr) = obj.get("items").and_then(|v| v.as_array()) {
            arr.clone()
        } else {
            vec![]
        }
    } else {
        vec![]
    };
    let mut sanitized = Vec::new();
    for item in items {
        if let Some(obj) = item.as_object() {
            let allowed = [
                "id",
                "provider",
                "label",
                "email",
                "status",
                "access_expires_at",
                "credential_configured",
                "models",
            ];
            let filtered = pick_object(&Value::Object(obj.clone()), &allowed);
            sanitized.push(Value::Object(filtered));
        }
    }
    json!({ "items": sanitized })
}

pub fn project_account(raw: Value) -> Value {
    let raw = sanitize(raw);
    let allowed = [
        "id",
        "provider",
        "label",
        "email",
        "status",
        "access_expires_at",
        "credential_configured",
        "models",
    ];
    let m = pick_object(&raw, &allowed);
    if m.is_empty() {
        sanitize(raw)
    } else {
        Value::Object(m)
    }
}

pub fn project_custom_profiles(raw: Value) -> Value {
    let raw = sanitize(raw);
    let items = if let Some(obj) = raw.as_object() {
        if let Some(arr) = obj.get("custom_profiles").and_then(|v| v.as_array()) {
            arr.clone()
        } else if let Some(arr) = obj.get("items").and_then(|v| v.as_array()) {
            arr.clone()
        } else {
            vec![]
        }
    } else {
        vec![]
    };
    let mut sanitized = Vec::new();
    for item in items {
        if let Some(obj) = item.as_object() {
            let allowed = [
                "id",
                "alias",
                "endpoint",
                "protocol",
                "enabled",
                "reachability",
                "api_key_configured",
                "header_names",
                "model_count",
                "models",
                "last_probe_at",
            ];
            let filtered = pick_object(&Value::Object(obj.clone()), &allowed);
            sanitized.push(Value::Object(filtered));
        }
    }
    json!({ "items": sanitized })
}

pub fn project_custom_profile(raw: Value) -> Value {
    let raw = sanitize(raw);
    let allowed = [
        "id",
        "alias",
        "endpoint",
        "protocol",
        "enabled",
        "reachability",
        "api_key_configured",
        "header_names",
        "model_count",
        "models",
        "last_probe_at",
    ];
    let inner = if let Some(obj) = raw.as_object() {
        if let Some(p) = obj.get("profile") {
            p
        } else {
            &raw
        }
    } else {
        &raw
    };
    let m = pick_object(inner, &allowed);
    if m.is_empty() {
        sanitize(inner.clone())
    } else {
        Value::Object(m)
    }
}

pub fn project_model_list_for_session(raw: Value) -> Value {
    let raw = sanitize(raw);
    let allowed = [
        "session_id",
        "provider",
        "account_or_profile_id",
        "current_model",
        "models",
    ];
    Value::Object(pick_object(&raw, &allowed))
}

pub fn project_generic(raw: Value) -> Value {
    sanitize(raw)
}

// ---------------------------------------------------------------------------
// Human rendering helpers
// ---------------------------------------------------------------------------

pub fn human_status(value: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(obj) = value.as_object() {
        if let Some(owner) = obj.get("owner_id").and_then(|v| v.as_str()) {
            lines.push(format!("Owner:      {owner}"));
        }
        if let Some(health) = obj.get("health") {
            if let Some(hobj) = health.as_object() {
                let daemon = hobj
                    .get("daemon_running")
                    .and_then(|v| v.as_bool())
                    .map(|b| if b { "running" } else { "stopped" })
                    .unwrap_or("unknown");
                let uptime = hobj
                    .get("uptime_seconds")
                    .and_then(|v| v.as_u64())
                    .map(|s| format!(" (uptime {}s)", s))
                    .unwrap_or_default();
                lines.push(format!("Daemon:     {daemon}{uptime}"));
                if let Some(states) = hobj.get("provider_states").and_then(|v| v.as_object()) {
                    let summary: Vec<String> = states
                        .iter()
                        .map(|(k, v)| format!("{}={}", k, v.as_str().unwrap_or("?")))
                        .collect();
                    if !summary.is_empty() {
                        lines.push(format!("Providers:  {}", summary.join(", ")));
                    }
                }
            } else {
                lines.push(format!("Health:     {}", health));
            }
        }
        if let Some(ai) = obj.get("current_ai") {
            if ai.is_null() {
                lines.push("Current AI: none".into());
            } else if let Some(aobj) = ai.as_object() {
                let p = aobj.get("provider").and_then(|v| v.as_str()).unwrap_or("-");
                let m = aobj.get("model").and_then(|v| v.as_str()).unwrap_or("-");
                let sid = aobj
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if sid.is_empty() {
                    lines.push(format!("Current AI: {p}/{m}"));
                } else {
                    lines.push(format!("Current AI: {p}/{m} (session: {sid})"));
                }
            } else {
                lines.push(format!("Current AI: {ai}"));
            }
        }
        if let Some(counts) = obj.get("counts").and_then(|v| v.as_object()) {
            let c = |k: &str| counts.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
            lines.push(format!(
                "Counts:     sessions {} · messages {} · runs {} ({} running) · memory {} · skills {} · approvals {}",
                c("sessions"),
                c("messages"),
                c("agent_runs"),
                c("running_runs"),
                c("memories"),
                c("skills"),
                c("pending_approvals")
            ));
        }
        if let Some(rt) = obj.get("runtime").and_then(|v| v.as_object()) {
            let termux = rt.get("termux").and_then(|v| v.as_bool()).unwrap_or(false);
            let root = rt.get("root").and_then(|v| v.as_bool()).unwrap_or(false);
            let selinux = rt
                .get("selinux")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            lines.push(format!(
                "Runtime:    termux={termux} · root={root} · selinux={selinux}"
            ));
        }
    } else {
        lines.push(human_generic(value));
    }
    lines.join("\n")
}

pub fn human_context(value: &Value) -> String {
    let sess = value
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("-");
    let prov = value.get("provider").and_then(Value::as_str).unwrap_or("-");
    let model = value.get("model").and_then(Value::as_str).unwrap_or("-");
    let mode = value.get("mode").and_then(Value::as_str).unwrap_or("chat");
    let eff_msgs = value
        .get("effective_messages")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let main_msgs = value
        .get("main_messages")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let chars = value
        .get("stored_characters")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let budget = value
        .get("context_budget_characters")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let summary = value
        .get("summary_available")
        .and_then(Value::as_bool)
        .map(|b| if b { "available" } else { "none" })
        .unwrap_or("none");
    let mems = value
        .get("active_memory_entries")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let skills = value
        .get("skills_available")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let mut lines = Vec::new();
    lines.push(format!("Session:    {sess}"));
    lines.push(format!("Model:      {prov}/{model}"));
    lines.push(format!("Mode:       {mode}"));
    lines.push(format!(
        "Messages:   {eff_msgs} effective ({main_msgs} main)"
    ));
    lines.push(format!("Characters: {chars} / {budget} budget"));
    lines.push(format!("Summary:    {summary}"));
    lines.push(format!("Memory:     {mems} active entries"));
    lines.push(format!("Skills:     {skills} available"));
    lines.join("\n")
}

pub fn human_doctor(value: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(arr) = value.get("checks").and_then(|v| v.as_array()) {
        let ok = arr
            .iter()
            .filter(|c| c.get("status").and_then(|v| v.as_str()) == Some("OK"))
            .count();
        let warn = arr
            .iter()
            .filter(|c| c.get("status").and_then(|v| v.as_str()) == Some("WARN"))
            .count();
        let fail = arr
            .iter()
            .filter(|c| c.get("status").and_then(|v| v.as_str()) == Some("FAIL"))
            .count();
        lines.push(format!(
            "Doctor: {} checks ({} ok, {} warn, {} fail)",
            arr.len(),
            ok,
            warn,
            fail
        ));
        for check in arr {
            let name = check
                .get("name")
                .or_else(|| check.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("check");
            let status = check
                .get("status")
                .or_else(|| check.get("state"))
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let evidence = check
                .get("evidence")
                .or_else(|| check.get("detail"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let marker = match status {
                "OK" => "✓",
                "FAIL" => "✗",
                "WARN" => "!",
                _ => "·",
            };
            if evidence.is_empty() {
                lines.push(format!("  {marker} {name}: {status}"));
            } else {
                let ev = if evidence.chars().count() > 120 {
                    evidence.chars().take(120).collect::<String>() + "…"
                } else {
                    evidence.to_string()
                };
                lines.push(format!("  {marker} {name}: {status} · {ev}"));
            }
        }
        if let Some(ran) = value.get("ran_at").and_then(|v| v.as_str()) {
            lines.push(format!("Ran at: {ran}"));
        }
    } else {
        lines.push(human_generic(value));
    }
    lines.join("\n")
}

pub fn human_tools(value: &Value) -> String {
    let items = value.get("items").and_then(Value::as_array);
    if let Some(items) = items {
        if items.is_empty() {
            return "No tools registered.".into();
        }
        let mut lines = Vec::new();
        lines.push(format!("Tools ({})", items.len()));
        for t in items {
            let name = t.get("name").and_then(Value::as_str).unwrap_or("-");
            let risk = t.get("risk").and_then(Value::as_str).unwrap_or("low");
            let mode = t
                .get("approval_mode")
                .and_then(Value::as_str)
                .unwrap_or("auto");
            let desc = t.get("description").and_then(Value::as_str).unwrap_or("");
            if desc.is_empty() {
                lines.push(format!("  - {name:<14} risk: {risk:<6} mode: {mode}"));
            } else {
                lines.push(format!(
                    "  - {name:<14} risk: {risk:<6} mode: {mode:<6} {desc}"
                ));
            }
        }
        lines.join("\n")
    } else {
        human_generic(value)
    }
}

pub fn human_sessions(value: &Value) -> String {
    let active_id = value
        .get("active_cli_session_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        if items.is_empty() {
            return "No active sessions.".into();
        }
        let mut lines = Vec::new();
        lines.push(format!("Sessions ({})", items.len()));
        for s in items {
            let id = s.get("id").and_then(Value::as_str).unwrap_or("-");
            let name = s.get("name").and_then(Value::as_str).unwrap_or("Untitled");
            let prov = s.get("provider").and_then(Value::as_str).unwrap_or("-");
            let model = s.get("model").and_then(Value::as_str).unwrap_or("-");
            let msgs = s.get("message_count").and_then(Value::as_u64).unwrap_or(0);
            let yolo = s
                .get("yolo")
                .and_then(Value::as_bool)
                .map(|b| if b { "on" } else { "off" })
                .unwrap_or("off");
            let is_active = id == active_id;
            let marker = if is_active { "*" } else { " " };
            let active_tag = if is_active { " (active)" } else { "" };
            lines.push(format!(
                "  {marker} {id:<10} \"{name}\"  {prov}/{model}  msgs: {msgs}  yolo: {yolo}{active_tag}"
            ));
        }
        lines.join("\n")
    } else {
        human_generic(value)
    }
}

pub fn human_session_item(value: &Value) -> String {
    let id = value
        .get("id")
        .or_else(|| value.pointer("/session/id"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let name = value
        .get("name")
        .or_else(|| value.pointer("/session/name"))
        .and_then(Value::as_str)
        .unwrap_or("Untitled");
    let prov = value
        .get("provider")
        .or_else(|| value.pointer("/session/provider"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let model = value
        .get("model")
        .or_else(|| value.pointer("/session/model"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let msgs = value
        .get("message_count")
        .or_else(|| value.pointer("/session/message_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let yolo = value
        .get("yolo")
        .or_else(|| value.pointer("/session/yolo"))
        .and_then(Value::as_bool)
        .map(|b| if b { "on" } else { "off" })
        .unwrap_or("off");
    let created = value
        .get("created_at")
        .or_else(|| value.pointer("/session/created_at"))
        .and_then(Value::as_str);
    let active = value
        .get("last_active_at")
        .or_else(|| value.pointer("/session/last_active_at"))
        .and_then(Value::as_str);

    let mut lines = Vec::new();
    lines.push(format!("Session:     {id}"));
    lines.push(format!("Name:        {name}"));
    lines.push(format!("AI:          {prov}/{model}"));
    lines.push(format!("Messages:    {msgs}"));
    lines.push(format!("YOLO:        {yolo}"));
    if let Some(c) = created {
        lines.push(format!("Created:     {c}"));
    }
    if let Some(a) = active {
        lines.push(format!("Last active: {a}"));
    }
    lines.join("\n")
}

pub fn human_telegram(value: &Value) -> String {
    let t = value
        .get("telegram")
        .or_else(|| value.get("result").and_then(|r| r.get("status")))
        .or_else(|| value.get("status").filter(|s| s.is_object()))
        .unwrap_or(value);
    let mut lines = Vec::new();
    lines.push("Telegram:".into());
    let enabled = t
        .get("enabled")
        .and_then(Value::as_bool)
        .map(|b| if b { "yes" } else { "no" })
        .unwrap_or("no");
    let token = t
        .get("token_configured")
        .and_then(Value::as_bool)
        .map(|b| if b { "configured" } else { "not configured" })
        .unwrap_or("not configured");
    lines.push(format!("  Enabled:       {enabled}"));
    lines.push(format!("  Token:         {token}"));
    if let Some(bot) = t.get("bot").and_then(Value::as_object) {
        let username = bot.get("username").and_then(Value::as_str).unwrap_or("-");
        let bot_id = bot
            .get("id")
            .and_then(|v| v.as_i64().or_else(|| v.as_u64().map(|u| u as i64)));
        if let Some(bid) = bot_id {
            lines.push(format!("  Bot:           @{username} (id: {bid})"));
        } else {
            lines.push(format!("  Bot:           @{username}"));
        }
    }
    if let Some(owner) = t.get("owner_user_id").and_then(Value::as_i64) {
        let state = t
            .get("owner_state")
            .and_then(Value::as_str)
            .unwrap_or("active");
        lines.push(format!("  Owner:         {owner} ({state})"));
    }
    if let Some(chats) = t.get("allowed_chat_ids").and_then(Value::as_array) {
        let chat_strs: Vec<String> = chats.iter().map(|c| c.to_string()).collect();
        if !chat_strs.is_empty() {
            lines.push(format!("  Allowed Chats: {}", chat_strs.join(", ")));
        }
    }
    lines.join("\n")
}

pub fn human_model(value: &Value) -> String {
    if let Some(items) = value.get("items").and_then(|v| v.as_array()) {
        if items.is_empty() {
            return "No models available.".into();
        }
        if items[0].get("provider").is_some() && items[0].get("label").is_some() {
            let mut lines = Vec::new();
            lines.push(format!("Accounts ({})", items.len()));
            for item in items {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let provider = item.get("provider").and_then(|v| v.as_str()).unwrap_or("-");
                let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("");
                let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("-");
                let cred = item
                    .get("credential_configured")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let models = item
                    .get("models")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                lines.push(format!(
                    "  - {id}  {provider} \"{label}\"  status: {status}  cred: {cred}  models: {models}"
                ));
            }
            return lines.join("\n");
        }
        if items[0].get("alias").is_some() {
            let mut lines = Vec::new();
            lines.push(format!("Custom Profiles ({})", items.len()));
            for item in items {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let alias = item.get("alias").and_then(|v| v.as_str()).unwrap_or("-");
                let endpoint = item.get("endpoint").and_then(|v| v.as_str()).unwrap_or("-");
                let protocol = item.get("protocol").and_then(|v| v.as_str()).unwrap_or("-");
                let prov = item
                    .get("api_key_configured")
                    .and_then(|v| v.as_bool())
                    .map(|b| if b { "yes" } else { "no" })
                    .unwrap_or("no");
                let en = item
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .map(|b| if b { "yes" } else { "no" })
                    .unwrap_or("yes");
                let count = item
                    .get("model_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                lines.push(format!(
                    "  - {alias:<14} {endpoint} [{protocol}]  key: {prov}  enabled: {en}  models: {count}  (id: {id})"
                ));
            }
            return lines.join("\n");
        }
        return human_generic(value);
    }
    if value.get("models").is_some() {
        let mut lines = Vec::new();
        let provider = value
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let current = value
            .get("current_model")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let sess = value
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        lines.push(format!(
            "Session: {sess} · Provider: {provider} · Current: {current}"
        ));
        if let Some(models) = value.get("models").and_then(|v| v.as_array()) {
            if models.is_empty() {
                lines.push("  (no models available)".into());
            } else {
                lines.push(format!("Available models ({}):", models.len()));
                for m in models {
                    if let Some(s) = m.as_str() {
                        let marker = if s == current { "*" } else { " " };
                        let tag = if s == current { " (current)" } else { "" };
                        lines.push(format!("  {marker} {s}{tag}"));
                    } else if let Some(obj) = m.as_object() {
                        let id = obj
                            .get("model_id")
                            .or_else(|| obj.get("id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("-");
                        let marker = if id == current { "*" } else { " " };
                        let tag = if id == current { " (current)" } else { "" };
                        lines.push(format!("  {marker} {id}{tag}"));
                    } else {
                        lines.push(format!("    {m}"));
                    }
                }
            }
        }
        return lines.join("\n");
    }
    human_generic(value)
}

pub fn human_custom_profile(value: &Value) -> String {
    let p = value.get("profile").unwrap_or(value);
    let id = p.get("id").and_then(Value::as_str).unwrap_or("-");
    let alias = p.get("alias").and_then(Value::as_str).unwrap_or("-");
    let endpoint = p.get("endpoint").and_then(Value::as_str).unwrap_or("-");
    let protocol = p.get("protocol").and_then(Value::as_str).unwrap_or("-");
    let enabled = p
        .get("enabled")
        .and_then(Value::as_bool)
        .map(|b| if b { "yes" } else { "no" })
        .unwrap_or("-");
    let key = p
        .get("api_key_configured")
        .and_then(Value::as_bool)
        .map(|b| if b { "configured" } else { "not configured" })
        .unwrap_or("not configured");

    let mut lines = Vec::new();
    lines.push(format!("Custom Profile: {alias} (id: {id})"));
    lines.push(format!("Endpoint:       {endpoint}"));
    lines.push(format!("Protocol:       {protocol}"));
    lines.push(format!("Enabled:        {enabled}"));
    lines.push(format!("API Key:        {key}"));
    if let Some(headers) = p.get("header_names").and_then(Value::as_array) {
        let h_strs: Vec<String> = headers
            .iter()
            .filter_map(|h| h.as_str().map(str::to_owned))
            .collect();
        if !h_strs.is_empty() {
            lines.push(format!("Headers:        {}", h_strs.join(", ")));
        }
    }
    if let Some(models) = p.get("models").and_then(Value::as_array) {
        let m_strs: Vec<String> = models
            .iter()
            .filter_map(|m| {
                if let Some(s) = m.as_str() {
                    Some(s.to_string())
                } else if let Some(obj) = m.as_object() {
                    obj.get("model_id")
                        .or_else(|| obj.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                } else {
                    None
                }
            })
            .collect();
        if !m_strs.is_empty() {
            lines.push(format!(
                "Models ({}):     {}",
                m_strs.len(),
                m_strs.join(", ")
            ));
        }
    }
    lines.join("\n")
}

pub fn human_runs(value: &Value) -> String {
    if let Some(items) = value.get("items").and_then(|v| v.as_array()) {
        if items.is_empty() {
            return "No runs found.".into();
        }
        let mut lines = Vec::new();
        lines.push(format!("Runs ({})", items.len()));
        for r in items {
            let id = r.get("id").and_then(Value::as_str).unwrap_or("-");
            let sess = r.get("session_id").and_then(Value::as_str).unwrap_or("-");
            let prov = r.get("provider").and_then(Value::as_str).unwrap_or("-");
            let model = r.get("model").and_then(Value::as_str).unwrap_or("-");
            let status = r.get("status").and_then(Value::as_str).unwrap_or("-");
            let goal = r.get("goal").and_then(Value::as_str).unwrap_or("");
            if goal.is_empty() {
                lines.push(format!(
                    "  - {id}  sess: {sess}  {prov}/{model}  status: {status}"
                ));
            } else {
                lines.push(format!(
                    "  - {id}  sess: {sess}  {prov}/{model}  status: {status}  goal: \"{goal}\""
                ));
            }
        }
        lines.join("\n")
    } else {
        human_generic(value)
    }
}

pub fn human_run_item(value: &Value) -> String {
    let id = value
        .get("id")
        .or_else(|| value.pointer("/run/id"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let sess = value
        .get("session_id")
        .or_else(|| value.pointer("/run/session_id"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let status = value
        .get("status")
        .or_else(|| value.pointer("/run/status"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let prov = value
        .get("provider")
        .or_else(|| value.pointer("/run/provider"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let model = value
        .get("model")
        .or_else(|| value.pointer("/run/model"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let goal = value
        .get("goal")
        .or_else(|| value.pointer("/run/goal"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let started = value
        .get("started_at")
        .or_else(|| value.pointer("/run/started_at"))
        .and_then(Value::as_str);
    let finished = value
        .get("finished_at")
        .or_else(|| value.pointer("/run/finished_at"))
        .and_then(Value::as_str);
    let result = value
        .get("result")
        .or_else(|| value.pointer("/run/result"))
        .and_then(Value::as_str);
    let blocker = value
        .get("blocker_or_error")
        .or_else(|| value.pointer("/run/blocker_or_error"))
        .and_then(Value::as_str);

    let mut lines = Vec::new();
    lines.push(format!("Run:          {id}"));
    lines.push(format!("Session:      {sess}"));
    lines.push(format!("Status:       {status}"));
    lines.push(format!("AI:           {prov}/{model}"));
    lines.push(format!("Goal:         {goal}"));
    if let Some(s) = started {
        lines.push(format!("Started:      {s}"));
    }
    if let Some(f) = finished {
        lines.push(format!("Finished:     {f}"));
    }
    if let Some(r) = result {
        lines.push(format!("Result:       {r}"));
    }
    if let Some(b) = blocker {
        lines.push(format!("Error:        {b}"));
    }
    if let Some(v) = value
        .get("verification")
        .or_else(|| value.pointer("/run/verification"))
    {
        if let Some(state) = v.get("state").and_then(Value::as_str) {
            lines.push(format!("Verification: {state}"));
        }
    }
    lines.join("\n")
}

pub fn human_attachments(value: &Value) -> String {
    if let Some(items) = value.get("items").and_then(|v| v.as_array()) {
        if items.is_empty() {
            return "No attachments.".into();
        }
        let total_bytes = value.pointer("/usage/bytes").and_then(Value::as_u64);
        let count = items.len();
        let size_str = total_bytes.map(format_bytes).unwrap_or_default();
        let header = if size_str.is_empty() {
            format!("Attachments ({count})")
        } else {
            format!("Attachments ({count} · {size_str} total)")
        };
        let mut lines = Vec::new();
        lines.push(header);
        for att in items {
            let id = att
                .get("attachment_id")
                .and_then(Value::as_str)
                .unwrap_or("-");
            let name = att
                .get("original_name")
                .and_then(Value::as_str)
                .unwrap_or("-");
            let kind = att.get("kind").and_then(Value::as_str).unwrap_or("-");
            let size = att
                .get("size_bytes")
                .and_then(Value::as_u64)
                .map(format_bytes)
                .unwrap_or_else(|| "-".into());
            let status = att
                .get("processing_status")
                .and_then(Value::as_str)
                .unwrap_or("-");
            lines.push(format!(
                "  - {id}  {name}  kind: {kind}  size: {size}  status: {status}"
            ));
        }
        lines.join("\n")
    } else {
        human_generic(value)
    }
}

pub fn human_attachment_item(value: &Value) -> String {
    let id = value
        .get("attachment_id")
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let name = value
        .get("original_name")
        .or_else(|| value.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("-");
    let size = value
        .get("size_bytes")
        .and_then(Value::as_u64)
        .map(format_bytes)
        .unwrap_or_else(|| "-".into());
    let status = value
        .get("processing_status")
        .or_else(|| value.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("-");
    let mime = value
        .get("detected_mime")
        .or_else(|| value.get("declared_mime"))
        .and_then(Value::as_str);
    let sha = value.get("sha256").and_then(Value::as_str);

    let mut lines = Vec::new();
    lines.push(format!("Attachment:   {id}"));
    lines.push(format!("Name:         {name}"));
    lines.push(format!("Kind:         {kind}"));
    lines.push(format!("Size:         {size}"));
    lines.push(format!("Status:       {status}"));
    if let Some(m) = mime {
        lines.push(format!("MIME:         {m}"));
    }
    if let Some(s) = sha {
        lines.push(format!("SHA256:       {s}"));
    }
    lines.join("\n")
}

pub fn human_memory(value: &Value) -> String {
    if let Some(items) = value.get("items").and_then(|v| v.as_array()) {
        if items.is_empty() {
            return "No memory entries found.".into();
        }
        let mut lines = Vec::new();
        lines.push(format!("Memory Entries ({})", items.len()));
        for m in items {
            let id = m.get("id").and_then(Value::as_str).unwrap_or("-");
            let scope = m.get("scope").and_then(Value::as_str).unwrap_or("-");
            let cat = m.get("category").and_then(Value::as_str).unwrap_or("-");
            let key = m.get("key").and_then(Value::as_str).unwrap_or("-");
            let val = m.get("value").and_then(Value::as_str).unwrap_or("-");
            lines.push(format!("  - [{scope}/{cat}] {key} = \"{val}\" (id: {id})"));
        }
        lines.join("\n")
    } else {
        human_generic(value)
    }
}

pub fn human_memory_item(value: &Value) -> String {
    let id = value.get("id").and_then(Value::as_str).unwrap_or("-");
    let scope = value.get("scope").and_then(Value::as_str).unwrap_or("-");
    let cat = value.get("category").and_then(Value::as_str).unwrap_or("-");
    let key = value.get("key").and_then(Value::as_str).unwrap_or("-");
    let val = value.get("value").and_then(Value::as_str).unwrap_or("-");
    let src = value
        .get("source_kind")
        .and_then(Value::as_str)
        .unwrap_or("-");

    let mut lines = Vec::new();
    lines.push(format!("Memory:   {id}"));
    lines.push(format!("Scope:    {scope}"));
    lines.push(format!("Category: {cat}"));
    lines.push(format!("Key:      {key}"));
    lines.push(format!("Value:    {val}"));
    lines.push(format!("Source:   {src}"));
    lines.join("\n")
}

pub fn human_skills(value: &Value) -> String {
    if let Some(items) = value.get("items").and_then(|v| v.as_array()) {
        if items.is_empty() {
            return "No skills found.".into();
        }
        let mut lines = Vec::new();
        lines.push(format!("Skills ({})", items.len()));
        for s in items {
            let id = s.get("id").and_then(Value::as_str).unwrap_or("-");
            let name = s.get("name").and_then(Value::as_str).unwrap_or("-");
            let ver = s.get("version").and_then(Value::as_str).unwrap_or("1.0.0");
            let en = s
                .get("enabled")
                .and_then(Value::as_bool)
                .map(|b| if b { "yes" } else { "no" })
                .unwrap_or("-");
            let src = s.get("source_kind").and_then(Value::as_str).unwrap_or("-");
            let desc = s.get("description").and_then(Value::as_str).unwrap_or("");
            if desc.is_empty() {
                lines.push(format!(
                    "  - {name}  v{ver}  enabled: {en}  source: {src}  (id: {id})"
                ));
            } else {
                lines.push(format!(
                    "  - {name}  v{ver}  enabled: {en}  source: {src}  \"{desc}\"  (id: {id})"
                ));
            }
        }
        lines.join("\n")
    } else {
        human_generic(value)
    }
}

pub fn human_skill_item(value: &Value) -> String {
    let id = value.get("id").and_then(Value::as_str).unwrap_or("-");
    let name = value.get("name").and_then(Value::as_str).unwrap_or("-");
    let ver = value
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("1.0.0");
    let en = value
        .get("enabled")
        .and_then(Value::as_bool)
        .map(|b| if b { "yes" } else { "no" })
        .unwrap_or("-");
    let src = value
        .get("source_kind")
        .and_then(Value::as_str)
        .unwrap_or("-");
    let desc = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");

    let mut lines = Vec::new();
    lines.push(format!("Skill:        {name} (id: {id})"));
    lines.push(format!("Version:      {ver}"));
    lines.push(format!("Enabled:      {en}"));
    lines.push(format!("Source:       {src}"));
    if !desc.is_empty() {
        lines.push(format!("Description:  {desc}"));
    }
    if let Some(caps) = value.get("capabilities").and_then(Value::as_array) {
        let cap_strs: Vec<String> = caps
            .iter()
            .filter_map(|c| c.as_str().map(str::to_owned))
            .collect();
        if !cap_strs.is_empty() {
            lines.push(format!("Capabilities: {}", cap_strs.join(", ")));
        }
    }
    lines.join("\n")
}

pub fn human_approvals(value: &Value) -> String {
    if let Some(items) = value.get("items").and_then(|v| v.as_array()) {
        if items.is_empty() {
            return "No pending approvals.".into();
        }
        let mut lines = Vec::new();
        lines.push(format!("Pending Approvals ({})", items.len()));
        for a in items {
            let id = a.get("id").and_then(Value::as_str).unwrap_or("-");
            let tool = a.get("tool_name").and_then(Value::as_str).unwrap_or("-");
            let risk = a.get("risk").and_then(Value::as_str).unwrap_or("-");
            let sum = a.get("summary").and_then(Value::as_str).unwrap_or("");
            let sess = a.get("session_id").and_then(Value::as_str).unwrap_or("");
            let sess_str = if sess.is_empty() {
                String::new()
            } else {
                format!("  (session: {sess})")
            };
            lines.push(format!(
                "  - {id}  tool: {tool}  risk: {risk}  \"{sum}\"{sess_str}"
            ));
        }
        lines.join("\n")
    } else {
        human_generic(value)
    }
}

pub fn human_config(value: &Value) -> String {
    if let Some(path) = value.get("path").and_then(Value::as_str) {
        if value.get("valid").and_then(Value::as_bool) == Some(true) {
            return format!("Configuration valid: {path}");
        }
        if value.as_object().map(|m| m.len() == 1).unwrap_or(false) {
            return path.to_string();
        }
    }
    let mut lines = Vec::new();
    lines.push("Configuration:".into());
    if let Some(map) = value.as_object() {
        for (k, v) in map {
            if let Some(sub) = v.as_object() {
                for (subk, subv) in sub {
                    lines.push(format!("  {k}.{subk}: {}", scalar(subv)));
                }
            } else {
                lines.push(format!("  {k}: {}", scalar(v)));
            }
        }
    } else {
        lines.push(format!("  {value}"));
    }
    lines.join("\n")
}

pub fn human_daemon(value: &Value) -> String {
    if let Some(status) = value.get("exit_status").and_then(Value::as_str) {
        return format!("Daemon exited: {status}");
    }
    if let Some(lines_arr) = value.get("lines").and_then(Value::as_array) {
        return lines_arr
            .iter()
            .map(|l| l.as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
    }
    if value.get("already_running").is_some() {
        let pid = value
            .get("pid")
            .and_then(Value::as_u64)
            .map(|p| format!(" (PID: {p})"))
            .unwrap_or_default();
        let already = value
            .get("already_running")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if already {
            return format!("Daemon is already running{pid}");
        } else {
            return format!("Daemon started{pid}");
        }
    }
    if let Some(state) = value.get("state").and_then(Value::as_str) {
        let pid = value
            .get("pid")
            .and_then(Value::as_u64)
            .map(|p| format!(" (PID: {p})"))
            .unwrap_or_default();
        return format!("Daemon: {state}{pid}");
    }
    if value.get("reachable").is_some() || value.get("managed_pid").is_some() {
        let reachable = value
            .get("reachable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let state = if reachable { "running" } else { "stopped" };
        let pid = value
            .get("managed_pid")
            .and_then(Value::as_u64)
            .map(|p| format!(" (PID: {p})"))
            .unwrap_or_default();
        let mut lines = Vec::new();
        lines.push(format!("Daemon Status: {state}{pid}"));
        if let Some(endpoint) = value.get("endpoint").and_then(Value::as_str) {
            lines.push(format!("Endpoint:      {endpoint}"));
        }
        if let Some(log) = value.get("log").and_then(Value::as_str) {
            lines.push(format!("Log File:      {log}"));
        }
        return lines.join("\n");
    }
    human_generic(value)
}

pub fn human_logs(value: &Value) -> String {
    if let Some(lines) = value.get("lines").and_then(Value::as_array) {
        return lines
            .iter()
            .map(|l| l.as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
    }
    if let Some(arr) = value.as_array() {
        return arr
            .iter()
            .map(|l| l.as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
    }
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    human_generic(value)
}

pub fn human_chat(value: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(answer) = value.get("answer").and_then(Value::as_str) {
        lines.push(answer.to_string());
    }
    if let Some(artifacts) = value.get("artifacts").and_then(Value::as_array) {
        for art in artifacts {
            if let Some(path) = art.get("path").and_then(Value::as_str) {
                lines.push(format!("artifact: {path}"));
            }
        }
    }
    if lines.is_empty() {
        if let Some(ans) = value.as_str() {
            return ans.to_string();
        }
        return human_generic(value);
    }
    lines.join("\n")
}

fn format_inline_map(map: &Map<String, Value>) -> String {
    let preferred = [
        "id", "name", "alias", "key", "status", "value", "enabled", "provider", "model",
    ];
    let mut parts = Vec::new();
    for k in &preferred {
        if let Some(v) = map.get(*k) {
            if is_scalar(v) {
                parts.push(format!("{k}: {}", scalar(v)));
            }
        }
    }
    for (k, v) in map {
        if !preferred.contains(&k.as_str()) && is_scalar(v) {
            parts.push(format!("{k}: {}", scalar(v)));
        }
    }
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join("  ")
    }
}

pub fn human_generic(value: &Value) -> String {
    match value {
        Value::Null => "OK".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(arr) => {
            let mut lines = Vec::new();
            for item in arr {
                if let Some(obj) = item.as_object() {
                    lines.push(format!("- {}", format_inline_map(obj)));
                } else {
                    lines.push(format!("- {}", scalar(item)));
                }
            }
            lines.join("\n")
        }
        Value::Object(map) => {
            if let Some(items) = map.get("items").and_then(Value::as_array) {
                let mut lines = Vec::new();
                lines.push(format!("Items ({})", items.len()));
                for item in items {
                    if let Some(obj) = item.as_object() {
                        lines.push(format!("  - {}", format_inline_map(obj)));
                    } else {
                        lines.push(format!("  - {}", scalar(item)));
                    }
                }
                for (k, v) in map {
                    if k != "items" && is_scalar(v) {
                        lines.push(format!("{}: {}", human_key(k), scalar(v)));
                    }
                }
                return lines.join("\n");
            }
            let mut lines = Vec::new();
            for (k, v) in map {
                if is_scalar(v) {
                    lines.push(format!("{}: {}", human_key(k), scalar(v)));
                } else if let Some(sub) = v.as_object() {
                    lines.push(format!("{}:", human_key(k)));
                    for (subk, subv) in sub {
                        lines.push(format!("  {}: {}", human_key(subk), scalar(subv)));
                    }
                } else if let Some(arr) = v.as_array() {
                    if arr.iter().all(is_scalar) {
                        let formatted: Vec<String> = arr.iter().map(scalar).collect();
                        lines.push(format!("{}: {}", human_key(k), formatted.join(", ")));
                    } else {
                        lines.push(format!("{} ({}):", human_key(k), arr.len()));
                        for item in arr {
                            if let Some(obj) = item.as_object() {
                                lines.push(format!("  - {}", format_inline_map(obj)));
                            } else {
                                lines.push(format!("  - {}", scalar(item)));
                            }
                        }
                    }
                }
            }
            if lines.is_empty() {
                "OK".into()
            } else {
                lines.join("\n")
            }
        }
    }
}

pub fn render_human_for_command(command: &str, value: &Value) -> String {
    let cmd = command.trim();
    if cmd == "status" {
        return human_status(value);
    }
    if cmd == "context" {
        return human_context(value);
    }
    if cmd == "doctor" {
        return human_doctor(value);
    }
    if cmd == "tools" {
        return human_tools(value);
    }
    if cmd == "sessions list" {
        return human_sessions(value);
    }
    if cmd.starts_with("sessions") {
        return human_session_item(value);
    }
    if cmd.starts_with("telegram") {
        return human_telegram(value);
    }
    if cmd == "model show" {
        return human_session_item(value);
    }
    if cmd == "model list" || cmd == "model custom list" || cmd == "model custom models" {
        return human_model(value);
    }
    if cmd == "model custom show" || cmd == "model custom add" || cmd == "model custom edit" {
        return human_custom_profile(value);
    }
    if cmd == "runs list" {
        return human_runs(value);
    }
    if cmd.starts_with("runs") {
        return human_run_item(value);
    }
    if cmd == "attachments list" {
        return human_attachments(value);
    }
    if cmd.starts_with("attachments show") {
        return human_attachment_item(value);
    }
    if cmd == "memory list" || cmd == "memory search" {
        return human_memory(value);
    }
    if cmd == "memory get" {
        return human_memory_item(value);
    }
    if cmd == "skills list" || cmd == "skills search" {
        return human_skills(value);
    }
    if cmd == "skills show" {
        return human_skill_item(value);
    }
    if cmd == "approvals list" {
        return human_approvals(value);
    }
    if cmd.starts_with("config") {
        return human_config(value);
    }
    if cmd.starts_with("daemon logs") || cmd == "logs" {
        return human_logs(value);
    }
    if cmd.starts_with("daemon") {
        return human_daemon(value);
    }
    if cmd == "chat" || cmd == "ask" || cmd == "retry" {
        return human_chat(value);
    }

    if value.get("health").is_some() || value.get("counts").is_some() {
        return human_status(value);
    }
    if value.get("checks").is_some() {
        return human_doctor(value);
    }
    if value.get("stored_characters").is_some() && value.get("main_messages").is_some() {
        return human_context(value);
    }
    if value.get("telegram").is_some() {
        return human_telegram(value);
    }
    if value.get("active_cli_session_id").is_some() {
        return human_sessions(value);
    }
    if value.get("answer").is_some() {
        return human_chat(value);
    }
    if value.get("lines").is_some() {
        return human_logs(value);
    }
    human_generic(value)
}

// ---------------------------------------------------------------------------
// Snapshot / contract tests (run as unit tests, no network required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn no_view_no_secret(data: &Value) -> bool {
        let s = serde_json::to_string(data).unwrap();
        let low = s.to_ascii_lowercase();
        for needle in ["\"blocks\"", "\"actions\"", "\"buttons\"", "\"view\""] {
            if low.contains(needle) {
                return false;
            }
        }
        for secret in [
            "S3CR3T_TOKEN_XYZ",
            "APIKEY_SUPER_SECRET",
            "header_secret_value",
        ] {
            if s.contains(secret) {
                return false;
            }
        }
        true
    }

    #[test]
    fn contract_version_is_stable() {
        assert_eq!(CONTRACT_VERSION, "1");
    }

    #[test]
    fn status_projection_strips_secrets_and_view() {
        let raw = json!({
            "owner_id": "owner-1",
            "health": {"daemon_running": true, "uptime_seconds": 123, "provider_states": {"codex": "ok"}},
            "counts": {"sessions": 2, "messages": 10, "agent_runs": 1, "running_runs": 0, "memories": 5, "skills": 1, "pending_approvals": 0},
            "current_ai": {"provider": "codex", "model": "gpt-5", "session_id": "sess1"},
            "runtime": {"termux": false, "root": true, "selinux": "enforcing"},
            "blocks": [{"kind":"paragraph","text":"leak"}],
            "token": "S3CR3T_TOKEN_XYZ",
            "extra_internal": "should_be_dropped"
        });
        let out = project_status(raw);
        assert_eq!(
            out.get("owner_id").and_then(|v| v.as_str()),
            Some("owner-1")
        );
        assert!(out.get("health").is_some());
        assert!(out.get("counts").is_some());
        assert!(out.get("current_ai").is_some());
        assert!(out.get("runtime").is_some());
        assert!(out.get("extra_internal").is_none());
        assert!(no_view_no_secret(&out));
        let h = human_status(&out);
        assert!(h.contains("Owner:      owner-1"));
        assert!(h.contains("Daemon:     running"));
        assert!(!h.contains('{'));
    }

    #[test]
    fn telegram_projection_strips_token_keeps_flags() {
        let raw = json!({
            "telegram": {
                "enabled": true,
                "owner_user_id": 12345678,
                "owner_state": "active",
                "legacy_candidate_count": 0,
                "allowed_chat_ids": [12345678],
                "token_configured": true,
                "token": "BOT_SECRET_LEAK",
                "bot": {
                    "id": 999,
                    "username": "MyXiaoBot",
                    "first_name": "Xiao",
                    "token": "BOT_SECRET_LEAK2"
                }
            },
            "blocks": []
        });
        let out = project_telegram(raw);
        let s = serde_json::to_string(&out).unwrap();
        assert!(!s.contains("BOT_SECRET_LEAK"));
        assert!(!s.contains("BOT_SECRET_LEAK2"));
        assert!(s.contains("\"token_configured\":true"));
        assert!(no_view_no_secret(&out));
        let h = human_telegram(&out);
        assert!(h.contains("Enabled:       yes"));
        assert!(h.contains("Token:         configured"));
        assert!(h.contains("@MyXiaoBot"));
        assert!(!h.contains('{'));
    }

    #[test]
    fn sessions_projection_and_human() {
        let raw = json!({
            "items": [
                {
                    "id": "s1",
                    "name": "Default",
                    "provider": "codex",
                    "model": "gpt-5",
                    "message_count": 5,
                    "archived": false,
                    "yolo": false,
                    "created_at": "2025-01-01T00:00:00Z",
                    "last_active_at": "2025-01-01T00:05:00Z",
                    "token": "S3CR3T_TOKEN_XYZ",
                    "internal_blob": "drop_me"
                }
            ],
            "active_cli_session_id": "s1",
            "page": 1,
            "pages": 1,
            "page_size": 10
        });
        let out = project_sessions(raw);
        assert!(no_view_no_secret(&out));
        let h = human_sessions(&out);
        assert!(h.contains("Sessions (1)"));
        assert!(h.contains("* s1"));
        assert!(h.contains("(active)"));
        assert!(!h.contains('{'));
    }

    #[test]
    fn context_projection_and_human() {
        let raw = json!({
            "session_id": "s1",
            "main_session_id": "s1",
            "mode": "chat",
            "main_messages": 10,
            "effective_messages": 8,
            "stored_characters": 1200,
            "context_budget_characters": 32000,
            "summary_available": false,
            "active_memory_entries": 3,
            "skills_available": 2,
            "provider": "codex",
            "account_or_profile_id": "a1",
            "model": "gpt-5",
            "token": "S3CR3T_TOKEN_XYZ"
        });
        let out = project_context(raw);
        assert!(no_view_no_secret(&out));
        let h = human_context(&out);
        assert!(h.contains("Session:    s1"));
        assert!(h.contains("Model:      codex/gpt-5"));
        assert!(h.contains("Messages:   8 effective (10 main)"));
        assert!(!h.contains('{'));
    }

    #[test]
    fn memory_and_skills_projections_paged() {
        let raw_mem = json!({"items": [{"id":"m1","scope":"user","category":"bio","key":"name","value":"Ada","confidence":1.0,"source_kind":"manual","created_at":"2025-01-01T00:00:00Z","updated_at":"2025-01-02T00:00:00Z","token":"S3CR3T_TOKEN_XYZ"}],"page":1,"pages":1,"page_size":10});
        let out = project_memory(raw_mem);
        assert_eq!(
            out.get("items").and_then(|v| v.as_array()).unwrap().len(),
            1
        );
        assert!(no_view_no_secret(&out));
        let hm = human_memory(&out);
        assert!(hm.contains("Memory Entries (1)"));
        assert!(hm.contains("[user/bio] name = \"Ada\""));
        assert!(!hm.contains('{'));

        let raw_skill = json!({"items": [{"id":"sk1","name":"my-skill","source_kind":"learned","enabled":true,"version":"1.0.0","description":"test skill"}],"page":1,"pages":1,"page_size":10});
        let out2 = project_skills(raw_skill);
        assert_eq!(
            out2.get("items").and_then(|v| v.as_array()).unwrap().len(),
            1
        );
        assert!(no_view_no_secret(&out2));
        let hs = human_skills(&out2);
        assert!(hs.contains("Skills (1)"));
        assert!(hs.contains("my-skill"));
        assert!(!hs.contains('{'));
    }

    #[test]
    fn attachments_and_runs_filtered() {
        let raw_att = json!({"items": [{"attachment_id":"a1","original_name":"file.pdf","kind":"document","size_bytes":123,"sha256":"abc","processing_status":"ready","token":"S3CR3T_TOKEN_XYZ"}],"usage": {"count":1,"bytes":123}});
        let out = project_attachments(raw_att);
        assert!(out.get("usage").is_some());
        assert!(no_view_no_secret(&out));
        let ha = human_attachments(&out);
        assert!(ha.contains("Attachments (1"));
        assert!(ha.contains("file.pdf"));
        assert!(!ha.contains('{'));

        let raw_runs = json!({"items": [{"id":"r1","session_id":"s1","provider":"codex","model":"gpt-5","status":"completed","goal":"do thing","started_at":"2025-01-01T00:00:00Z","verification":{"state":"verified_success","evidence":[]}}],"page":1,"pages":1,"page_size":10});
        let out2 = project_runs(raw_runs);
        assert_eq!(
            out2.get("items").and_then(|v| v.as_array()).unwrap().len(),
            1
        );
        assert!(no_view_no_secret(&out2));
        let hr = human_runs(&out2);
        assert!(hr.contains("Runs (1)"));
        assert!(hr.contains("r1"));
        assert!(hr.contains("status: completed"));
        assert!(!hr.contains('{'));
    }

    #[test]
    fn doctor_and_tools_projections() {
        let raw = json!({"ran_at":"2025-01-01T00:00:00Z","checks":[{"status":"OK","name":"disk","evidence":"LIVE ok","source":"live"},{"status":"FAIL","name":"net","evidence":"unreachable","source":"live"}],"token":"S3CR3T_TOKEN_XYZ"});
        let out = project_doctor(raw);
        assert!(out.get("checks").is_some());
        assert!(no_view_no_secret(&out));
        let h = human_doctor(&out);
        assert!(h.contains("Doctor: 2 checks"));
        assert!(h.contains("✓ disk: OK"));
        assert!(h.contains("✗ net: FAIL"));
        assert!(!h.contains('{'));

        let raw_tools = json!({"items":[{"name":"terminal","risk":"high","approval_mode":"ask","description":"run shell"}]});
        let out2 = project_tools(raw_tools);
        assert!(out2.get("items").is_some());
        assert!(no_view_no_secret(&out2));
        let ht = human_tools(&out2);
        assert!(ht.contains("Tools (1)"));
        assert!(ht.contains("terminal"));
        assert!(!ht.contains('{'));
    }

    #[test]
    fn approvals_projection_and_human() {
        let raw = json!({"items":[{"id":"apr1","tool_name":"terminal","risk":"high","summary":"rm -rf /tmp","session_id":"s1"}]});
        let out = project_approvals(raw);
        assert!(no_view_no_secret(&out));
        let ha = human_approvals(&out);
        assert!(ha.contains("Pending Approvals (1)"));
        assert!(ha.contains("apr1"));
        assert!(ha.contains("rm -rf /tmp"));
        assert!(!ha.contains('{'));
    }

    #[test]
    fn model_accounts_and_custom_filtered() {
        let raw_prov = json!({"accounts":[{"id":"a1","provider":"codex","label":"work","email":"a@b.com","status":"ok","access_expires_at":null,"credential_configured":true,"models":["gpt-5"],"api_key":"APIKEY_SUPER_SECRET"}],"custom_profiles":[{"id":"p1","alias":"local","endpoint":"http://localhost:11434","protocol":"openai_chat_completions","enabled":true,"reachability":"unknown","api_key_configured":false,"header_names":["X-Custom"],"model_count":1,"models":[]}],"provider_states":{"codex":"ok"}});
        let acc = project_accounts(raw_prov.clone());
        let cust = project_custom_profiles(raw_prov);
        assert!(no_view_no_secret(&acc));
        assert!(no_view_no_secret(&cust));
        let s = serde_json::to_string(&acc).unwrap();
        assert!(!s.contains("APIKEY_SUPER_SECRET"));
        assert_eq!(
            acc.get("items").and_then(|v| v.as_array()).unwrap().len(),
            1
        );
        assert_eq!(
            cust.get("items").and_then(|v| v.as_array()).unwrap().len(),
            1
        );
        let h = human_model(&acc);
        assert!(h.contains("Accounts (1)"));
        let h2 = human_model(&cust);
        assert!(h2.contains("Custom Profiles (1)"));
    }

    #[test]
    fn model_list_for_session_human() {
        let raw = json!({"session_id":"s1","provider":"codex","account_or_profile_id":"a1","current_model":"gpt-5","models":["gpt-5","gpt-4"]});
        let out = project_model_list_for_session(raw);
        assert_eq!(
            out.get("current_model").and_then(|v| v.as_str()),
            Some("gpt-5")
        );
        let h = human_model(&out);
        assert!(h.contains("* gpt-5 (current)"));
        assert!(h.contains("  gpt-4"));
    }

    #[test]
    fn no_secret_snapshot_regression() {
        let raw = json!({"items":[{"id":"x","token":"S3CR3T_TOKEN_XYZ","headers":{"X-Secret":"header_secret_value"}}]});
        for f in [
            project_memory as fn(Value) -> Value,
            project_skills,
            project_attachments,
            project_runs,
            project_tools,
            project_sessions,
        ] {
            let out = f(raw.clone());
            assert!(no_view_no_secret(&out), "leak in {:?}", f as *const ());
        }
    }

    #[test]
    fn human_output_never_contains_raw_json_blocks() {
        let test_cases: &[(&str, Value)] = &[
            (
                "status",
                project_status(json!({
                    "owner_id": "1",
                    "health": {"daemon_running": true},
                    "counts": {"sessions": 0},
                    "current_ai": null,
                    "runtime": {}
                })),
            ),
            (
                "context",
                project_context(json!({
                    "session_id": "s1",
                    "provider": "codex",
                    "model": "m",
                    "mode": "chat",
                    "effective_messages": 0,
                    "main_messages": 0,
                    "stored_characters": 0,
                    "context_budget_characters": 1000
                })),
            ),
            (
                "doctor",
                project_doctor(json!({
                    "checks": [{"name": "disk", "status": "OK", "evidence": "ok"}]
                })),
            ),
            (
                "tools",
                project_tools(json!({
                    "items": [{"name": "terminal", "risk": "high", "approval_mode": "ask", "description": "sh"}]
                })),
            ),
            (
                "sessions list",
                project_sessions(json!({"items": [], "active_cli_session_id": ""})),
            ),
            ("runs list", project_runs(json!({"items": []}))),
            (
                "attachments list",
                project_attachments(json!({"items": []})),
            ),
            ("memory list", project_memory(json!({"items": []}))),
            ("skills list", project_skills(json!({"items": []}))),
            ("approvals list", project_approvals(json!({"items": []}))),
            (
                "config show",
                json!({"server": {"control_socket": "/tmp/xiao.sock"}}),
            ),
            (
                "daemon status",
                json!({"reachable": true, "managed_pid": 123, "endpoint": "http://localhost"}),
            ),
            (
                "logs",
                json!({"lines": ["first log line", "second log line"]}),
            ),
            (
                "chat",
                json!({"answer": "Hello human!", "artifacts": [{"path": "file.txt"}]}),
            ),
        ];

        for (cmd, val) in test_cases {
            let rendered = render_human_for_command(cmd, val);
            assert!(
                !rendered.contains("{\n"),
                "{cmd} rendered raw JSON opening: {rendered}"
            );
            assert!(
                !rendered.contains("}\n"),
                "{cmd} rendered raw JSON closing: {rendered}"
            );
            assert!(
                !rendered.contains("\": \""),
                "{cmd} rendered raw JSON key-value: {rendered}"
            );
        }
    }
    #[test]
    fn telegram_mutation_result_projection_and_human() {
        let raw = json!({
            "ok": true,
            "result": {
                "applied": true,
                "tested": false,
                "status": {
                    "enabled": true,
                    "token_configured": true,
                    "owner_user_id": 999888,
                    "owner_state": "active",
                    "allowed_chat_ids": [111, 222]
                }
            }
        });
        let projected = project_telegram(raw);
        assert!(projected.get("telegram").is_some());
        let t = projected.get("telegram").unwrap();
        assert_eq!(t.get("enabled").and_then(Value::as_bool), Some(true));
        assert_eq!(
            t.get("token_configured").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(t.get("owner_user_id").and_then(Value::as_i64), Some(999888));

        let human = human_telegram(&projected);
        assert!(human.contains("Enabled:       yes"));
        assert!(human.contains("Token:         configured"));
        assert!(human.contains("Owner:         999888 (active)"));
        assert!(human.contains("Allowed Chats: 111, 222"));
    }
}
