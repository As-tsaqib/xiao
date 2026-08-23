use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use futures_util::StreamExt;
use rand::{distributions::Alphanumeric, Rng};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use subtle::ConstantTimeEq;

use crate::{
    app::AppState,
    config::parse_id_list,
    event::AppEvent,
    security::{redact::redact_text, secrets::SecretStore},
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
pub struct FetchModelsRequest {
    pub base_url: String,
    pub api_key: Option<String>,
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
        .route("/v1/admin/custom/models", post(custom_models))
        .route("/v1/admin/client-config", get(client_config))
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
    let custom_api_key_configured = stored_provider_api_key(&state.app, "custom")
        .map_err(bad)?
        .is_some();
    let health = state
        .app
        .health
        .snapshot(
            &cfg,
            state.app.storage.health(),
            state.app.providers.states(),
        )
        .await;
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
            "token_configured": bot.is_some(),
            "allowed_chat_ids": cfg.telegram.access.allowed_chat_ids
        },
        "config": {
            "custom": {
                "enabled": cfg.providers.custom.enabled,
                "base_url": cfg.providers.custom.base_url,
                "protocol": cfg.providers.custom.protocol,
                "models": cfg.providers.custom.models,
                "default_model": cfg.providers.custom.default_model,
                "api_key_configured": custom_api_key_configured
            }
        }
    })))
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
    if old.providers.antigravity.default_model != next.providers.antigravity.default_model {
        let models = state.app.providers.models("antigravity").map_err(bad)?;
        let preferred = models
            .first()
            .ok_or_else(|| bad("provider antigravity has no usable models"))?;
        state
            .app
            .storage
            .reconcile_provider_models(
                "antigravity",
                old.providers.antigravity.default_model.as_deref(),
                preferred,
                &models,
            )
            .map_err(bad)?;
    }
    if old.providers.custom.default_model != next.providers.custom.default_model
        || old.providers.custom.models != next.providers.custom.models
    {
        let models = state.app.providers.models("custom").map_err(bad)?;
        let preferred = models
            .first()
            .ok_or_else(|| bad("provider custom has no usable models"))?;
        state
            .app
            .storage
            .reconcile_provider_models(
                "custom",
                old.providers.custom.default_model.as_deref(),
                preferred,
                &models,
            )
            .map_err(bad)?;
    }
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

async fn custom_models(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<FetchModelsRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let cfg = state.app.config.read().await.clone();
    let supplied_key = req
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let api_key = match supplied_key {
        Some(value) => Some(value),
        None => stored_provider_api_key(&state.app, "custom").map_err(bad)?,
    };
    let models = fetch_custom_models(
        &req.base_url,
        &cfg.providers.custom.headers,
        api_key.as_deref(),
    )
    .await
    .map_err(bad)?;
    Ok(Json(json!({"ok":true,"models":models})))
}

fn stored_provider_api_key(app: &AppState, provider: &str) -> Result<Option<String>> {
    app.auth.provider_api_key(provider)
}

pub(crate) async fn fetch_custom_models(
    base_url: &str,
    headers: &BTreeMap<String, String>,
    api_key: Option<&str>,
) -> Result<Vec<String>> {
    const MAX_CATALOG_BYTES: usize = 4 * 1024 * 1024;
    let endpoint = custom_models_endpoint(base_url)?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut request = client.get(endpoint);
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!(
            "custom model discovery failed with HTTP {status}; check Base URL and API key"
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CATALOG_BYTES as u64)
    {
        return Err(anyhow!("custom model catalog exceeds 4 MiB"));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len().saturating_add(chunk.len()) > MAX_CATALOG_BYTES {
            return Err(anyhow!("custom model catalog exceeds 4 MiB"));
        }
        body.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&body).context("parse custom model catalog")?;
    let models = custom_model_ids(&value);
    if models.is_empty() {
        return Err(anyhow!(
            "custom /models response does not contain any model IDs"
        ));
    }
    Ok(models)
}

fn custom_models_endpoint(base_url: &str) -> Result<url::Url> {
    let mut url = url::Url::parse(base_url.trim()).context("invalid custom provider Base URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(anyhow!("custom provider Base URL must use HTTP or HTTPS"));
    }
    if url.host_str().is_none() {
        return Err(anyhow!("custom provider Base URL must include a host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(anyhow!(
            "custom provider Base URL must not contain credentials"
        ));
    }
    let path = format!("{}/models", url.path().trim_end_matches('/'));
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn custom_model_ids(value: &Value) -> Vec<String> {
    let items = if let Some(items) = value.get("data").and_then(Value::as_array) {
        items
    } else if let Some(items) = value.get("models").and_then(Value::as_array) {
        items
    } else if let Some(items) = value.as_array() {
        items
    } else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            item.as_str()
                .or_else(|| item.get("id").and_then(Value::as_str))
                .or_else(|| item.get("name").and_then(Value::as_str))
        })
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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

    #[test]
    fn custom_models_url_appends_models_to_versioned_base() {
        let url = custom_models_endpoint("http://127.0.0.1:8317/v1/?ignored=true").unwrap();
        assert_eq!(url.as_str(), "http://127.0.0.1:8317/v1/models");
    }

    #[test]
    fn custom_model_catalog_is_sorted_deduplicated_and_accepts_openai_shape() {
        let value = json!({"data":[{"id":"z-model"},{"id":"a-model"},{"id":"a-model"}]});
        assert_eq!(custom_model_ids(&value), vec!["a-model", "z-model"]);
    }
}
