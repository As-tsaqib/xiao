use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Html,
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use subtle::ConstantTimeEq;

use crate::{
    app::AppState,
    config::parse_id_list,
    event::AppEvent,
    security::{
        redact::{mask_token, redact_text},
        secrets::SecretStore,
    },
    telegram::client::TelegramClient,
};

#[derive(Clone)]
struct ApiState {
    app: AppState,
    config_path: PathBuf,
    client_token: Arc<String>,
    admin_token: Arc<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExecuteRequest {
    pub principal: String,
    pub input: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApplyRequest {
    pub gateway_enabled: Option<bool>,
    pub gateway_auto_restart: Option<bool>,
    pub telegram_enabled: Option<bool>,
    pub telegram_bot_token: Option<String>,
    pub allowed_chat_ids: Option<String>,
    pub allowed_user_ids: Option<String>,
    pub log_level: Option<String>,
    pub progress_detail: Option<String>,
    pub menu_close_behavior: Option<String>,
    pub antigravity_enabled: Option<bool>,
    pub antigravity_oauth_client_id: Option<String>,
    pub antigravity_oauth_client_secret: Option<String>,
    pub antigravity_default_model: Option<String>,
    pub custom_enabled: Option<bool>,
    pub custom_name: Option<String>,
    pub custom_base_url: Option<String>,
    pub custom_protocol: Option<String>,
    pub custom_models: Option<Vec<String>>,
    pub custom_default_model: Option<String>,
    pub custom_headers: Option<BTreeMap<String, String>>,
    pub custom_api_key: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TestTokenRequest {
    pub token: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct AntigravityCallbackRequest {
    pub transaction_id: String,
    pub code: String,
    pub state: String,
}
#[derive(Debug, Deserialize)]
struct AntigravityBrowserQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}
#[derive(Debug, Deserialize)]
struct LogsQuery {
    lines: Option<usize>,
}

pub async fn serve(app: AppState, config_path: impl AsRef<Path>) -> Result<()> {
    let cfg = app.config.read().await.clone();
    let addr = cfg.ipc.socket_addr()?;
    if !addr.ip().is_loopback() {
        return Err(anyhow!("refusing non-loopback IPC bind"));
    }
    let secrets = SecretStore::new(cfg.paths.secrets_dir.clone());
    let client_token = load_or_create_token(&secrets, "ipc-client-token")?;
    let admin_token = load_or_create_token(&secrets, "ipc-admin-token")?;
    let state = ApiState {
        app,
        config_path: config_path.as_ref().to_owned(),
        client_token: Arc::new(client_token),
        admin_token: Arc::new(admin_token),
    };
    let router = Router::new()
        .route("/v1/status", get(status))
        .route("/v1/command", post(execute))
        .route("/v1/chat", post(execute))
        .route("/v1/logs", get(logs))
        .route("/v1/admin/snapshot", get(admin_snapshot))
        .route("/v1/admin/apply", post(admin_apply))
        .route("/v1/admin/telegram/test", post(test_telegram))
        .route("/v1/admin/client-config", get(client_config))
        .route("/v1/auth/antigravity/callback", post(antigravity_callback))
        .route(
            "/v1/auth/antigravity/browser-callback",
            get(antigravity_browser_callback),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "local authenticated IPC listening");
    axum::serve(listener, router).await?;
    Ok(())
}

fn load_or_create_token(store: &SecretStore, key: &str) -> Result<String> {
    if let Some(value) = store.get(key)? {
        return Ok(value);
    }
    let value: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(48)
        .map(char::from)
        .collect();
    store.put(key, &value)?;
    Ok(value)
}
fn authorized_client(headers: &HeaderMap, state: &ApiState) -> bool {
    bearer_matches(headers, state.client_token.as_str())
        || bearer_matches(headers, state.admin_token.as_str())
}
fn authorized_admin(headers: &HeaderMap, state: &ApiState) -> bool {
    bearer_matches(headers, state.admin_token.as_str())
}
fn bearer_matches(headers: &HeaderMap, expected: &str) -> bool {
    let Some(value) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    let a = value.as_bytes();
    let b = expected.as_bytes();
    a.len() == b.len() && bool::from(a.ct_eq(b))
}
fn deny() -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error":"unauthorized"})),
    )
}
fn bad(error: impl std::fmt::Display) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":redact_text(&error.to_string())})),
    )
}

async fn status(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_client(&headers, &state) {
        return Err(deny());
    }
    let cfg = state.app.config.read().await.clone();
    let snapshot = state
        .app
        .health
        .snapshot(
            &cfg,
            state.app.storage.health(),
            state.app.providers.states(),
        )
        .await;
    Ok(Json(serde_json::to_value(snapshot).map_err(bad)?))
}

async fn execute(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<ExecuteRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_client(&headers, &state) {
        return Err(deny());
    }
    let result = state
        .app
        .commands
        .execute_text(&req.principal, &req.input)
        .await
        .map_err(bad)?;
    Ok(Json(serde_json::to_value(result).map_err(bad)?))
}

async fn logs(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<LogsQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_client(&headers, &state) {
        return Err(deny());
    }
    let cfg = state.app.config.read().await.clone();
    let path = cfg.paths.logs_dir.join("daemon.log");
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let limit = query.lines.unwrap_or(120).clamp(1, 500);
    let lines = content
        .lines()
        .rev()
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(redact_text)
        .collect::<Vec<_>>();
    Ok(Json(json!({"lines":lines})))
}

async fn admin_snapshot(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let cfg = state.app.config.read().await.clone();
    let store = SecretStore::new(cfg.paths.secrets_dir.clone());
    let bot = store.get("telegram-bot-token").map_err(bad)?;
    let health = state
        .app
        .health
        .snapshot(
            &cfg,
            state.app.storage.health(),
            state.app.providers.states(),
        )
        .await;
    let provider_status = provider_status(&state.app).map_err(bad)?;
    Ok(Json(json!({
        "gateway": health.clone(),
        "daemon": {
            "status": if health.daemon_running {"running"} else {"stopped"},
            "pid": std::process::id(),
            "uptime_seconds": health.uptime_seconds,
            "memory_bytes": health.memory_bytes,
            "boot_start": std::env::var("XIAO_BOOT_START").ok().as_deref() == Some("1"),
            "auto_restart": cfg.gateway.auto_restart
        },
        "telegram": {
            "enabled": cfg.telegram.enabled,
            "transport": cfg.telegram.transport,
            "polling": health.telegram_polling,
            "last_update_at": health.telegram_last_update_at,
            "inbox_problem_count": state.app.storage.telegram_inbox_problem_count().unwrap_or(0),
            "token_configured": bot.is_some(),
            "masked_token": bot.as_deref().map(mask_token),
            "allowed_chat_ids": cfg.telegram.access.allowed_chat_ids,
            "allowed_user_ids": cfg.telegram.access.allowed_user_ids,
            "bot_identity": state.app.storage.setting("telegram_bot_identity").ok().flatten().and_then(|x| serde_json::from_str::<Value>(&x).ok())
        },
        "providers": provider_status,
        "config": {
            "gateway": {"enabled":cfg.gateway.enabled,"auto_restart":cfg.gateway.auto_restart},
            "log_level": cfg.daemon.log_level,
            "telegram_ui":{"progress_detail":cfg.telegram.ui.progress_detail,"menu_close_behavior":cfg.telegram.ui.menu_close_behavior},
            "antigravity":{
                "enabled":cfg.providers.antigravity.enabled,
                "oauth_client_id":cfg.providers.antigravity.oauth_client_id,
                "oauth_client_secret_configured":state.app.auth.antigravity_client_secret_configured(),
                "default_model":cfg.providers.antigravity.default_model
            },
            "custom": {
                "enabled": cfg.providers.custom.enabled,
                "name": cfg.providers.custom.name,
                "base_url": cfg.providers.custom.base_url,
                "protocol": cfg.providers.custom.protocol,
                "models": cfg.providers.custom.models,
                "default_model": cfg.providers.custom.default_model,
                "headers": cfg.providers.custom.headers
            }
        }
    })))
}

fn provider_status(app: &AppState) -> Result<Value> {
    let mut map = serde_json::Map::new();
    for provider in ["codex", "antigravity", "custom"] {
        let accounts = app.auth.accounts(Some(provider))?;
        let state = app.providers.state(provider);
        let status = serde_json::to_value(&state)?
            .as_str()
            .unwrap_or("error")
            .to_owned();
        map.insert(provider.into(), json!({
            "status": status,
            "runtime_ready": matches!(state,crate::providers::ProviderState::Ready),
            "accounts": accounts.iter().map(|a| json!({"id":a.id,"label":a.label,"status":a.status})).collect::<Vec<_>>()
        }));
    }
    Ok(Value::Object(map))
}

async fn admin_apply(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<ApplyRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let old = state.app.config.read().await.clone();
    let mut next = old.clone();
    if let Some(v) = req.gateway_enabled {
        next.gateway.enabled = v;
    }
    if let Some(v) = req.gateway_auto_restart {
        next.gateway.auto_restart = v;
    }
    if let Some(v) = req.telegram_enabled {
        next.telegram.enabled = v;
    }
    if let Some(v) = req.allowed_chat_ids.as_deref() {
        next.telegram.access.allowed_chat_ids = parse_id_list(v).map_err(bad)?;
    }
    if let Some(v) = req.allowed_user_ids.as_deref() {
        next.telegram.access.allowed_user_ids = parse_id_list(v).map_err(bad)?;
    }
    if let Some(v) = req.log_level {
        next.daemon.log_level = v;
    }
    if let Some(v) = req.progress_detail {
        next.telegram.ui.progress_detail = v;
    }
    if let Some(v) = req.menu_close_behavior {
        next.telegram.ui.menu_close_behavior = v;
    }
    if let Some(v) = req.antigravity_enabled {
        next.providers.antigravity.enabled = v;
    }
    if let Some(v) = req.antigravity_oauth_client_id {
        next.providers.antigravity.oauth_client_id = if v.trim().is_empty() {
            None
        } else {
            Some(v.trim().to_owned())
        };
    }
    if let Some(v) = req.antigravity_default_model {
        next.providers.antigravity.default_model = if v.trim().is_empty() {
            None
        } else {
            Some(v.trim().to_owned())
        };
    }
    if let Some(v) = req.custom_enabled {
        next.providers.custom.enabled = v;
    }
    if let Some(v) = req.custom_name {
        next.providers.custom.name = Some(v);
    }
    if let Some(v) = req.custom_base_url {
        next.providers.custom.base_url = if v.trim().is_empty() { None } else { Some(v) };
    }
    if let Some(v) = req.custom_protocol {
        next.providers.custom.protocol = v;
    }
    if let Some(v) = req.custom_models {
        next.providers.custom.models = v
            .into_iter()
            .map(|x| x.trim().to_owned())
            .filter(|x| !x.is_empty())
            .collect();
    }
    if let Some(v) = req.custom_default_model {
        next.providers.custom.default_model = if v.trim().is_empty() { None } else { Some(v) };
    }
    if let Some(v) = req.custom_headers {
        next.providers.custom.headers = v;
    }
    next.validate().map_err(bad)?;

    let store = SecretStore::new(next.paths.secrets_dir.clone());
    let new_bot = req
        .telegram_bot_token
        .as_deref()
        .map(str::trim)
        .filter(|x| !x.is_empty());
    let identity = if let Some(token) = new_bot {
        Some(
            TelegramClient::new(token.to_owned())
                .map_err(bad)?
                .get_me()
                .await
                .map_err(bad)?,
        )
    } else {
        None
    };

    // External validation is complete before any config commit.
    next.save_atomic(&state.config_path).map_err(bad)?;
    if let Some(token) = new_bot {
        store.put("telegram-bot-token", token).map_err(bad)?;
        if let Some(identity) = identity {
            state
                .app
                .storage
                .put_setting(
                    "telegram_bot_identity",
                    &serde_json::to_string(&identity).map_err(bad)?,
                )
                .map_err(bad)?;
        }
    }
    if let Some(key) = req
        .custom_api_key
        .as_deref()
        .map(str::trim)
        .filter(|x| !x.is_empty())
    {
        state
            .app
            .auth
            .configure_api_key(
                "custom",
                next.providers.custom.name.as_deref().unwrap_or("Custom"),
                key,
            )
            .map_err(bad)?;
    }
    if let Some(secret) = req
        .antigravity_oauth_client_secret
        .as_deref()
        .map(str::trim)
        .filter(|x| !x.is_empty())
    {
        state
            .app
            .auth
            .set_antigravity_client_secret(Some(secret))
            .map_err(bad)?;
    }
    state.app.providers.reload_config(&next);
    *state.app.config.write().await = next.clone();
    state.app.events.publish(AppEvent::ConfigReloaded);

    let restart_required = new_bot.is_some()
        || old.gateway.enabled != next.gateway.enabled
        || old.telegram.enabled != next.telegram.enabled
        || old.daemon.log_level != next.daemon.log_level;
    Ok(Json(json!({
        "ok": true,
        "applied": true,
        "restart_required": restart_required,
        "hot_reloaded": ["telegram.access.allowed_chat_ids","telegram.access.allowed_user_ids","telegram.ui","providers.antigravity","providers.custom","gateway.auto_restart"]
    })))
}

async fn test_telegram(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<TestTokenRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let cfg = state.app.config.read().await.clone();
    let token = match req.token.filter(|x| !x.trim().is_empty()) {
        Some(v) => v,
        None => SecretStore::new(cfg.paths.secrets_dir)
            .get("telegram-bot-token")
            .map_err(bad)?
            .ok_or_else(|| bad("Telegram token is not configured"))?,
    };
    let bot = TelegramClient::new(token)
        .map_err(bad)?
        .get_me()
        .await
        .map_err(bad)?;
    Ok(Json(
        json!({"ok":true,"bot":{"id":bot.id,"username":bot.username,"first_name":bot.first_name}}),
    ))
}

async fn client_config(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let cfg = state.app.config.read().await.clone();
    Ok(Json(json!({
        "endpoint": format!("http://{}", cfg.ipc.bind),
        "token": state.client_token.as_str(),
        "principal": "termux:default"
    })))
}

async fn antigravity_callback(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<AntigravityCallbackRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let account = state
        .app
        .auth
        .complete_antigravity(&req.transaction_id, &req.code, &req.state)
        .await
        .map_err(bad)?;
    Ok(Json(
        json!({"ok":true,"account":{"id":account.id,"label":account.label,"provider":account.provider}}),
    ))
}

async fn antigravity_browser_callback(
    State(state): State<ApiState>,
    Query(query): Query<AntigravityBrowserQuery>,
) -> (StatusCode, Html<String>) {
    if let Some(error) = query.error {
        return (
            StatusCode::BAD_REQUEST,
            Html(format!(
                "<h1>xiao login failed</h1><p>{}</p>",
                html_escape(&error)
            )),
        );
    }
    let (Some(code), Some(returned_state)) = (query.code, query.state) else {
        return (
            StatusCode::BAD_REQUEST,
            Html("<h1>xiao login failed</h1><p>Missing OAuth code/state.</p>".into()),
        );
    };
    match state
        .app
        .auth
        .complete_antigravity_by_state(&code, &returned_state)
        .await
    {
        Ok((_tx, account)) => (
            StatusCode::OK,
            Html(format!(
                "<h1>xiao connected</h1><p>{}</p><p>You can close this tab.</p>",
                html_escape(&account.label)
            )),
        ),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Html(format!(
                "<h1>xiao login failed</h1><p>{}</p>",
                html_escape(&redact_text(&error.to_string()))
            )),
        ),
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn encode_admin_payload(json: &str) -> String {
    URL_SAFE_NO_PAD.encode(json.as_bytes())
}
pub fn decode_admin_payload(value: &str) -> Result<String> {
    Ok(String::from_utf8(
        URL_SAFE_NO_PAD
            .decode(value)
            .context("invalid base64 admin payload")?,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ipc_requires_exact_bearer_token() {
        let mut headers = HeaderMap::new();
        assert!(!bearer_matches(&headers, "secret"));
        headers.insert("authorization", "Bearer wrong".parse().unwrap());
        assert!(!bearer_matches(&headers, "secret"));
        headers.insert("authorization", "Bearer secret".parse().unwrap());
        assert!(bearer_matches(&headers, "secret"));
    }
}
