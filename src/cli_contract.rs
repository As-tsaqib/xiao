//! P1-9 Stable CLI success JSON contracts.
//!
//! The outer envelope `{status:"ok", data: <Dto>}` is stable in `bin_cli::CliPresenter`.
//! This module defines stable, application-facing DTOs / projections for the
//! *success data* payloads.  It is the contract between the daemon's raw admin
//! JSON (which may evolve) and the CLI's public JSON output.
//!
//! Invariants enforced here (and tested by snapshots below):
//! - No Telegram View/button schema (`blocks`, `actions`, `view`, `buttons`) ever
//!   surfaces on the public CLI, even if the daemon accidentally returns it.
//! - No secrets: tokens, api keys, header values, credential blobs are stripped.
//!   Only booleans such as `token_configured` / `api_key_configured` may appear.
//! - Stable keys: each projection emits exactly the documented keys; unknown
//!   raw keys are dropped.  Callers can pin against these snapshots.

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
        // recurse into nested objects/arrays
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
    // Remove any key that would leak a secret value, recursively.
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
        "headers", // values would leak; we expose header_names instead
        "header_value",
        "client_secret",
        "authorization",
    ];
    const FORBIDDEN_SUBSTR: &[&str] = &["secret", "token", "credential"];
    if let Value::Object(map) = value {
        // collect to avoid borrow issues
        let keys: Vec<String> = map.keys().cloned().collect();
        for k in keys {
            let lk = k.to_ascii_lowercase();
            let exact = FORBIDDEN_EXACT.iter().any(|e| lk == *e);
            let substr = FORBIDDEN_SUBSTR.iter().any(|s| lk.contains(*s))
                && lk != "token_configured"
                && lk != "api_key_configured"
                && lk != "credential_configured"
                && lk != "credential_metadata";
            // credential_metadata is allowed because it is sanitized separately
            if exact || substr {
                // keep the *_configured booleans
                if matches!(
                    k.as_str(),
                    "token_configured" | "api_key_configured" | "credential_configured"
                ) {
                    continue;
                }
                // header_names is allowed; headers (values) is forbidden – already exact above
                map.remove(&k);
                continue;
            }
            if let Some(v) = map.get_mut(&k) {
                strip_secret_keys(v);
            }
        }
        // also strip view schema at this level already handled, but keep
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

// ---------------------------------------------------------------------------
// Projections — each returns a stable Value ready for {status:"ok", data:…}
// ---------------------------------------------------------------------------

/// status = dashboard projection.
/// Stable keys: owner_id, health, counts, current_ai, runtime
pub fn project_status(raw: Value) -> Value {
    let raw = sanitize(raw);
    // dashboard shape may contain extra keys; pick only stable ones
    let allowed = ["owner_id", "health", "counts", "current_ai", "runtime"];
    let mut out = pick_object(&raw, &allowed);
    // Ensure health.counts etc don't leak secrets even if nested
    // health may contain provider_states – keep as-is (no secrets)
    Value::Object(out)
}

/// telegram status. Raw may be {telegram:{...}} or {ok:true, telegram:{...}} flattened.
pub fn project_telegram(raw: Value) -> Value {
    let raw = sanitize(raw);
    if let Some(obj) = raw.as_object() {
        if let Some(t) = obj.get("telegram") {
            // sanitize telegram object again
            let t = sanitize(t.clone());
            // allow only documented telegram fields; strip token/value-bearing fields
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
                    // bot identity is public username/id
                    for k in ["id", "username", "first_name"] {
                        if let Some(v) = bobj.get(k) {
                            bot_filtered.insert(k.to_string(), v.clone());
                        }
                    }
                } else if !bot.is_null() {
                    // bot may be null or non-object; preserve as-is if not leaking
                    bot_filtered.insert("value".to_string(), bot.clone());
                }
            }
            let mut telegram_out = pick_object(&t, &allowed);
            if !bot_filtered.is_empty() {
                // replace bot with sanitized version
                if t.get("bot").is_some() {
                    telegram_out.insert("bot".to_string(), Value::Object(bot_filtered));
                }
            } else if t.get("bot").is_some() && t.get("bot").unwrap().is_null() {
                telegram_out.insert("bot".to_string(), Value::Null);
            }
            return json!({ "telegram": Value::Object(telegram_out) });
        }
    }
    // fallback: if raw already looks like telegram object
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

/// sessions list
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
            // sanitize each session item to stable keys
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

/// single session item projection
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
    // raw may be wrapped as {session:{...}} or bare object
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
        // fallback to sanitized raw if not matching – but strip
        return sanitize(inner.clone());
    }
    Value::Object(m)
}

/// context projection
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

/// memory listing
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

/// skills listing
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

/// approvals listing
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
            // keep approval stable keys
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

/// attachments
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
    // usage is {count, bytes} etc – keep as-is but sanitized
    Value::Object(out)
}

/// runs
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

/// doctor / diagnostics
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
        // also preserve any generic summary but filtered
        for k in ["summary", "ok"] {
            if let Some(v) = obj.get(k) {
                out.insert(k.to_string(), v.clone());
            }
        }
        if out.is_empty() {
            // fallback: treat items as checks
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

/// tools listing
pub fn project_tools(raw: Value) -> Value {
    let raw = sanitize(raw);
    let mut out = pick_object(&raw, &["items"]);
    if out.is_empty() {
        // raw may already be {items:[...]} or bare array
        if let Some(arr) = raw.as_array() {
            return json!({"items": sanitize(Value::Array(arr.clone()))});
        }
        return sanitize(raw);
    }
    Value::Object(out)
}

/// model accounts list: raw providers response -> {items: accounts}
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

/// single account
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
    if m.is_empty() { sanitize(raw) } else { Value::Object(m) }
}

/// custom profiles list
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

/// single custom profile
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
        if let Some(p) = obj.get("profile") { p } else { &raw }
    } else { &raw };
    let m = pick_object(inner, &allowed);
    if m.is_empty() { sanitize(inner.clone()) } else { Value::Object(m) }
}

/// model list for a session: raw from models_for_session helper
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

/// memory single item
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
        "scope",
        "category",
        "key",
        "value",
    ];
    let m = pick_object(&raw, &allowed);
    if m.is_empty() { raw } else { Value::Object(m) }
}

/// generic sanitizer for ad-hoc success data: strip view/secrets and return
pub fn project_generic(raw: Value) -> Value {
    sanitize(raw)
}

// ---------------------------------------------------------------------------
// Human rendering helpers — intentional formatting, not generic nested-JSON.
// Each returns a String; bin_cli prints it line-by-line.
// ---------------------------------------------------------------------------

pub fn human_status(value: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(obj) = value.as_object() {
        if let Some(owner) = obj.get("owner_id").and_then(|v| v.as_str()) {
            lines.push(format!("owner: {owner}"));
        }
        if let Some(health) = obj.get("health") {
            if let Some(hobj) = health.as_object() {
                let daemon = hobj.get("daemon_running").and_then(|v| v.as_bool()).map(|b| if b {"up"} else {"down"}).unwrap_or("unknown");
                let uptime = hobj.get("uptime_seconds").and_then(|v| v.as_u64()).map(|s| format!("{}s", s)).unwrap_or_else(|| "-".into());
                lines.push(format!("health: daemon {daemon} · uptime {uptime}"));
                if let Some(states) = hobj.get("provider_states").and_then(|v| v.as_object()) {
                    let summary: Vec<String> = states.iter().map(|(k,v)| format!("{}={}", k, v.as_str().unwrap_or("?"))).collect();
                    if !summary.is_empty() {
                        lines.push(format!("providers: {}", summary.join(", ")));
                    }
                }
            } else {
                lines.push(format!("health: {}", health));
            }
        }
        if let Some(counts) = obj.get("counts").and_then(|v| v.as_object()) {
            let c = |k:&str| counts.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
            lines.push(format!("counts: sessions {} · messages {} · runs {} ({} running) · memories {} · skills {} · approvals {}", c("sessions"), c("messages"), c("agent_runs"), c("running_runs"), c("memories"), c("skills"), c("pending_approvals")));
        }
        if let Some(ai) = obj.get("current_ai") {
            if ai.is_null() {
                lines.push("current_ai: none".into());
            } else if let Some(aobj) = ai.as_object() {
                let p = aobj.get("provider").and_then(|v| v.as_str()).unwrap_or("-");
                let m = aobj.get("model").and_then(|v| v.as_str()).unwrap_or("-");
                let sid = aobj.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
                if sid.is_empty() {
                    lines.push(format!("current_ai: {p}/{m}"));
                } else {
                    lines.push(format!("current_ai: {p}/{m} @ {sid}"));
                }
            } else {
                lines.push(format!("current_ai: {ai}"));
            }
        }
        if let Some(rt) = obj.get("runtime").and_then(|v| v.as_object()) {
            let termux = rt.get("termux").and_then(|v| v.as_bool()).unwrap_or(false);
            let root = rt.get("root").and_then(|v| v.as_bool()).unwrap_or(false);
            let selinux = rt.get("selinux").and_then(|v| v.as_str()).unwrap_or("-");
            lines.push(format!("runtime: termux={} root={} selinux={}", termux, root, selinux));
        }
        if lines.is_empty() {
            // fallback generic scalar printing
            for (k,v) in obj {
                if matches!(v, Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null) {
                    lines.push(format!("{}: {}", k, scalar(v)));
                }
            }
        }
    } else {
        lines.push(format!("{value}"));
    }
    lines.join("\n")
}

pub fn human_sessions(value: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(items) = value.get("items").and_then(|v| v.as_array()) {
        if items.is_empty() {
            lines.push("no sessions".into());
        } else {
            lines.push(format!("sessions ({})", items.len()));
            for item in items {
                if let Some(obj) = item.as_object() {
                    let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                    let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let provider = obj.get("provider").and_then(|v| v.as_str()).unwrap_or("-");
                    let model = obj.get("model").and_then(|v| v.as_str()).unwrap_or("-");
                    let msgs = obj.get("message_count").and_then(|v| v.as_u64()).unwrap_or(0);
                    let archived = obj.get("archived").and_then(|v| v.as_bool()).unwrap_or(false);
                    let tag = if archived {" [archived]"} else {""};
                    let label = if name.is_empty() { id.to_string() } else { format!("{id} \"{name}\"") };
                    lines.push(format!("  - {label}  {provider}/{model}  msgs={msgs}{tag}"));
                } else {
                    lines.push(format!("  - {item}"));
                }
            }
        }
        if let Some(active) = value.get("active_cli_session_id").and_then(|v| v.as_str()) {
            lines.push(format!("active: {active}"));
        }
        if let Some(page) = value.get("page") {
            let pages = value.get("pages").and_then(|v| v.as_u64()).unwrap_or(1);
            lines.push(format!("page {} of {}", page, pages));
        }
    } else {
        lines.push(format!("{value}"));
    }
    lines.join("\n")
}

pub fn human_doctor(value: &Value) -> String {
    let mut lines = Vec::new();
    let checks = value.get("checks").and_then(|v| v.as_array())
        .or_else(|| value.get("items").and_then(|v| v.as_array()));
    if let Some(arr) = checks {
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
            "doctor: {} checks ({} ok, {} warn, {} fail)",
            arr.len(),
            ok,
            warn,
            fail
        ));
        for check in arr {
            let name = check.get("name").or_else(|| check.get("id")).and_then(|v| v.as_str()).unwrap_or("check");
            let status = check.get("status").or_else(|| check.get("state")).and_then(|v| v.as_str()).unwrap_or("-");
            let evidence = check.get("evidence").or_else(|| check.get("detail")).and_then(|v| v.as_str()).unwrap_or("");
            let marker = match status {
                "OK" => "✓",
                "FAIL" => "✗",
                "WARN" => "!",
                _ => "·",
            };
            if evidence.is_empty() {
                lines.push(format!("  {marker} {name}: {status}"));
            } else {
                // truncate long evidence
                let ev = if evidence.chars().count() > 120 { evidence.chars().take(120).collect::<String>() + "…" } else { evidence.to_string() };
                lines.push(format!("  {marker} {name}: {status} · {ev}"));
            }
        }
        if let Some(ran) = value.get("ran_at").and_then(|v| v.as_str()) {
            lines.push(format!("ran_at: {ran}"));
        }
    } else {
        // generic fallback
        if let Some(obj) = value.as_object() {
            for (k,v) in obj {
                lines.push(format!("{}: {}", k, scalar(v)));
            }
        } else {
            lines.push(format!("{value}"));
        }
    }
    lines.join("\n")
}

pub fn human_model(value: &Value) -> String {
    let mut lines = Vec::new();
    // account list or custom list or model list for session
    if let Some(items) = value.get("items").and_then(|v| v.as_array()) {
        // heuristic: detect account vs custom by inspecting first item
        if items.is_empty() {
            lines.push("no models".into());
        } else if items[0].get("provider").is_some() && items[0].get("label").is_some() {
            lines.push(format!("accounts ({})", items.len()));
            for item in items {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let provider = item.get("provider").and_then(|v| v.as_str()).unwrap_or("-");
                let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("");
                let status = item.get("status").and_then(|v| v.as_str()).unwrap_or("-");
                let cred = item.get("credential_configured").and_then(|v| v.as_bool()).unwrap_or(false);
                let models = item.get("models").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                lines.push(format!("  - {id}  {provider} \"{label}\"  status={status}  cred={}  models={}", cred, models));
            }
        } else if items[0].get("alias").is_some() {
            lines.push(format!("custom profiles ({})", items.len()));
            for item in items {
                let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                let alias = item.get("alias").and_then(|v| v.as_str()).unwrap_or("-");
                let endpoint = item.get("endpoint").and_then(|v| v.as_str()).unwrap_or("-");
                let protocol = item.get("protocol").and_then(|v| v.as_str()).unwrap_or("-");
                let prov = item.get("api_key_configured").and_then(|v| v.as_bool()).unwrap_or(false);
                let count = item.get("model_count").and_then(|v| v.as_u64()).unwrap_or(0);
                lines.push(format!("  - {id}  alias={alias}  {endpoint} [{protocol}]  key={prov}  models={count}"));
            }
        } else {
            lines.push(format!("items ({})", items.len()));
            for item in items {
                lines.push(format!("  - {item}"));
            }
        }
        return lines.join("\n");
    }
    // modelsForSession shape: session_id, provider, current_model, models
    if value.get("models").is_some() {
        let provider = value.get("provider").and_then(|v| v.as_str()).unwrap_or("-");
        let current = value.get("current_model").and_then(|v| v.as_str()).unwrap_or("-");
        let sess = value.get("session_id").and_then(|v| v.as_str()).unwrap_or("-");
        lines.push(format!("model for session {sess} · provider {provider} · current {current}"));
        if let Some(models) = value.get("models").and_then(|v| v.as_array()) {
            if models.is_empty() {
                lines.push("  (no models available)".into());
            } else {
                for m in models {
                    if let Some(s) = m.as_str() {
                        let marker = if s == current {"*"} else {" "};
                        lines.push(format!("  {marker} {s}"));
                    } else if let Some(obj) = m.as_object() {
                        let id = obj.get("model_id").or_else(|| obj.get("id")).and_then(|v| v.as_str()).unwrap_or("-");
                        let marker = if id == current {"*"} else {" "};
                        lines.push(format!("  {marker} {id}"));
                    } else {
                        lines.push(format!("    {m}"));
                    }
                }
            }
        }
        return lines.join("\n");
    }
    // single session/account
    if value.get("id").is_some() && value.get("provider").is_some() {
        let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("-");
        let provider = value.get("provider").and_then(|v| v.as_str()).unwrap_or("-");
        let model = value.get("model").and_then(|v| v.as_str()).unwrap_or("-");
        lines.push(format!("session {id}  {provider}/{model}"));
        for (k,v) in value.as_object().unwrap() {
            if matches!(k.as_str(), "id"|"provider"|"model") { continue; }
            if matches!(v, Value::String(_) | Value::Number(_) | Value::Bool(_)) {
                lines.push(format!("  {k}: {}", scalar(v)));
            }
        }
        return lines.join("\n");
    }
    // fallback generic
    if let Some(obj) = value.as_object() {
        for (k,v) in obj {
            if v.is_string() || v.is_number() || v.is_boolean() || v.is_null() {
                lines.push(format!("{k}: {}", scalar(v)));
            } else {
                lines.push(format!("{k}: {}", v));
            }
        }
    } else {
        lines.push(format!("{value}"));
    }
    lines.join("\n")
}

fn scalar(value: &Value) -> String {
    match value {
        Value::Null => "none".into(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
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
        // view schema must not appear
        for needle in ["\"blocks\"", "\"actions\"", "\"buttons\"", "\"view\""] {
            if low.contains(needle) { return false; }
        }
        // secret values must not appear (but booleans token_configured are allowed)
        // Check that raw secret tokens we inject are gone
        for secret in ["S3CR3T_TOKEN_XYZ", "APIKEY_SUPER_SECRET", "header_secret_value"] {
            if s.contains(secret) { return false; }
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
        assert_eq!(out.get("owner_id").and_then(|v| v.as_str()), Some("owner-1"));
        assert!(out.get("health").is_some());
        assert!(out.get("counts").is_some());
        assert!(out.get("current_ai").is_some());
        assert!(out.get("runtime").is_some());
        assert!(out.get("extra_internal").is_none());
        assert!(no_view_no_secret(&out));
        // snapshot: keys exactly
        let keys: Vec<String> = out.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), 5);
        // human
        let h = human_status(&out);
        assert!(h.contains("owner: owner-1"));
        assert!(h.contains("daemon up"));
    }

    #[test]
    fn telegram_projection_filters_bot_and_token() {
        let raw = json!({
            "ok": true,
            "telegram": {
                "enabled": true,
                "owner_user_id": 123,
                "owner_state": "configured",
                "legacy_candidate_count": 0,
                "allowed_chat_ids": [1,2],
                "token_configured": true,
                "bot": {"id": 999, "username": "xiao_bot", "first_name": "Xiao"},
                "token": "S3CR3T_TOKEN_XYZ",
                "blocks": []
            }
        });
        let out = project_telegram(raw);
        let tel = out.get("telegram").unwrap();
        assert_eq!(tel.get("owner_user_id").and_then(|v| v.as_i64()), Some(123));
        assert_eq!(tel.get("token_configured").and_then(|v| v.as_bool()), Some(true));
        assert!(tel.get("token").is_none());
        let bot = tel.get("bot").unwrap();
        assert_eq!(bot.get("username").and_then(|v| v.as_str()), Some("xiao_bot"));
        assert!(no_view_no_secret(&out));
    }

    #[test]
    fn sessions_projection_is_paged_and_filtered() {
        let raw = json!({
            "items": [{"id":"s1","name":"chat","provider":"codex","account_or_profile_id":"a1","model":"m1","message_count":5,"archived":false,"yolo":false,"created_at":"2025-01-01T00:00:00Z","last_active_at":"2025-01-02T00:00:00Z","extra":"drop","token":"S3CR3T_TOKEN_XYZ"}],
            "page": 1, "pages": 1, "page_size": 10, "active_cli_session_id": "s1",
            "blocks": []
        });
        let out = project_sessions(raw);
        assert_eq!(out.get("page").and_then(|v| v.as_u64()), Some(1));
        let item = out.get("items").and_then(|v| v.as_array()).unwrap()[0].as_object().unwrap();
        assert!(item.contains_key("id"));
        assert!(!item.contains_key("extra"));
        assert!(no_view_no_secret(&out));
        let h = human_sessions(&out);
        assert!(h.contains("s1"));
    }

    #[test]
    fn context_projection_stable_keys() {
        let raw = json!({
            "session_id":"s1","main_session_id":"m1","mode":"main","main_messages":10,"effective_messages":10,"stored_characters":1000,"context_budget_characters":8000,"summary_available":false,"active_memory_entries":2,"skills_available":3,"provider":"codex","account_or_profile_id":"a1","model":"gpt-5","blocks":[]
        });
        let out = project_context(raw);
        assert_eq!(out.get("session_id").and_then(|v| v.as_str()), Some("s1"));
        assert!(out.get("provider").is_some());
        assert!(no_view_no_secret(&out));
    }

    #[test]
    fn approvals_projection_normalizes_pending() {
        let raw = json!({
            "pending_approvals": [{"id":"ap1","tool_name":"terminal","risk":"high","status":"pending","summary":"run rm","approval_mode":"ask"}],
            "blocks": []
        });
        let out = project_approvals(raw);
        assert_eq!(
            out.get("items").and_then(|v| v.as_array()).unwrap().len(),
            1
        );
        assert!(no_view_no_secret(&out));
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

        let raw_skill = json!({"items": [{"id":"sk1","name":"my-skill","source_kind":"learned","enabled":true,"version":"1.0.0"}],"page":1,"pages":1,"page_size":10});
        let out2 = project_skills(raw_skill);
        assert_eq!(
            out2.get("items").and_then(|v| v.as_array()).unwrap().len(),
            1
        );
        assert!(no_view_no_secret(&out2));
    }

    #[test]
    fn attachments_and_runs_filtered() {
        let raw_att = json!({"items": [{"attachment_id":"a1","original_name":"file.pdf","kind":"document","size_bytes":123,"sha256":"abc","processing_status":"ready","token":"S3CR3T_TOKEN_XYZ"}],"usage": {"count":1,"bytes":123}});
        let out = project_attachments(raw_att);
        assert!(out.get("usage").is_some());
        assert!(no_view_no_secret(&out));

        let raw_runs = json!({"items": [{"id":"r1","session_id":"s1","provider":"codex","model":"gpt-5","status":"completed","goal":"do thing","started_at":"2025-01-01T00:00:00Z","verification":{"state":"verified_success","evidence":[]}}],"page":1,"pages":1,"page_size":10});
        let out2 = project_runs(raw_runs);
        assert_eq!(
            out2.get("items").and_then(|v| v.as_array()).unwrap().len(),
            1
        );
        assert!(no_view_no_secret(&out2));
    }

    #[test]
    fn doctor_and_tools_projections() {
        let raw = json!({"ran_at":"2025-01-01T00:00:00Z","checks":[{"status":"OK","name":"disk","evidence":"LIVE ok","source":"live"},{"status":"FAIL","name":"net","evidence":"unreachable","source":"live"}],"token":"S3CR3T_TOKEN_XYZ"});
        let out = project_doctor(raw);
        assert!(out.get("checks").is_some());
        assert!(no_view_no_secret(&out));
        let h = human_doctor(&out);
        assert!(h.contains("doctor:"));
        assert!(h.contains("✓ disk"));

        let raw_tools = json!({"items":[{"name":"terminal","risk":"high","approval_mode":"ask"}]});
        let out2 = project_tools(raw_tools);
        assert!(out2.get("items").is_some());
        assert!(no_view_no_secret(&out2));
    }

    #[test]
    fn model_accounts_and_custom_filtered() {
        let raw_prov = json!({"accounts":[{"id":"a1","provider":"codex","label":"work","email":"a@b.com","status":"ok","access_expires_at":null,"credential_configured":true,"models":["gpt-5"],"api_key":"APIKEY_SUPER_SECRET"}],"custom_profiles":[{"id":"p1","alias":"local","endpoint":"http://localhost:11434","protocol":"openai_chat_completions","enabled":true,"reachability":"unknown","api_key_configured":false,"header_names":["X-Custom"],"model_count":1,"models":[]}],"provider_states":{"codex":"ok"}});
        let acc = project_accounts(raw_prov.clone());
        let cust = project_custom_profiles(raw_prov);
        assert!(no_view_no_secret(&acc));
        assert!(no_view_no_secret(&cust));
        // ensure api_key not emitted
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
        assert!(h.contains("accounts"));
        let h2 = human_model(&cust);
        assert!(h2.contains("custom"));
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
        assert!(h.contains("* gpt-5"));
    }

    #[test]
    fn no_secret_snapshot_regression() {
        // ensure every projection strips injected secrets even if nested deeply
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
}
