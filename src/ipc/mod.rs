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

// P2-2: hidden legacy management paths are thin deprecated delegates.
// Unified control plane owns Telegram, profile, session and attachment logic;
// legacy admin/base64 routes delegate or reject to avoid duplicate business logic.

use crate::{
    app::AppState,
    control_plane::{TelegramConfigureInput, TelegramSetupService},
    event::AppEvent,
    memory::{MemoryScope, MemoryStore},
    presentation::Block,
    providers::{ProviderProfileStore, ToolProtocol},
    security::{redact::redact_text, secrets::SecretStore},
    skills::{FilesystemSkills, SkillStore},
    storage::ProviderProfileModelRecord,
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
    /// Legacy caller-supplied principal. The server ignores this value and
    /// always derives the canonical OwnerIdentity from trusted runtime state.
    #[serde(default)]
    pub principal: String,
    pub input: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionExecuteRequest {
    /// Legacy field retained for wire compatibility; server ignores it.
    #[serde(default)]
    pub principal: String,
    pub session_id: String,
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub retry: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AttachmentIngestRequest {
    /// Legacy field retained for wire compatibility; server ignores it.
    #[serde(default)]
    pub principal: String,
    pub session_id: String,
    pub name: String,
    #[serde(default)]
    pub mime: Option<String>,
    pub kind: String,
    pub data_base64: String,
}

#[derive(Debug, Deserialize)]
struct AttachmentActionRequest {
    action: String,
    attachment_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApplyRequest {
    pub gateway_enabled: Option<bool>,
    pub gateway_auto_restart: Option<bool>,
    /// Deprecated legacy telegram mutation. Use POST /v1/admin/telegram (TelegramSetupService).
    pub telegram_enabled: Option<bool>,
    /// Deprecated. Use POST /v1/admin/telegram.
    pub telegram_bot_token: Option<String>,
    /// Deprecated. Use POST /v1/admin/telegram.
    pub allowed_chat_ids: Option<String>,
    pub owner_user_id: Option<i64>,
    /// Legacy low-level field. v0.2.7 never accepts multiple owners.
    pub allowed_user_ids: Option<String>,
    #[serde(default)]
    pub confirm_owner_change: bool,
    pub log_level: Option<String>,
    pub progress_detail: Option<String>,
    pub menu_close_behavior: Option<String>,
    /// Deprecated v0.2.8 compatibility fields. Legacy credentials remain
    /// stored for history isolation, but this endpoint never enables or
    /// mutates an inactive provider.
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
#[derive(Deserialize)]
struct TelegramSetupActionRequest {
    action: String,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    owner_user_id: Option<i64>,
    #[serde(default)]
    confirm_owner_change: bool,
    #[serde(default)]
    allowed_chat_ids: Option<Vec<i64>>,
    #[serde(default)]
    enabled: Option<bool>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct FetchModelsRequest {
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub profile_id: Option<String>,
    pub api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManagerQuery {
    page: Option<usize>,
    limit: Option<usize>,
    query: Option<String>,
    scope: Option<String>,
    include_archived: Option<bool>,
    session_id: Option<String>,
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CustomProfileActionRequest {
    action: String,
    profile_id: Option<String>,
    alias: Option<String>,
    endpoint: Option<String>,
    protocol: Option<String>,
    api_key: Option<String>,
    #[serde(default)]
    remove_api_key: bool,
    #[serde(default)]
    keep_credential: bool,
    #[serde(default)]
    keep_safe_headers: bool,
    #[serde(default)]
    keep_secret_headers: bool,
    #[serde(default)]
    headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    secret_headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    clear_secret_headers: bool,
    session_id: Option<String>,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionActionRequest {
    action: String,
    #[serde(default)]
    session_id: Option<String>,
    value: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    account_or_profile_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RunActionRequest {
    action: String,
    run_id: String,
}

#[derive(Debug, Deserialize)]
struct MemoryActionRequest {
    action: String,
    scope: Option<String>,
    category: Option<String>,
    key: Option<String>,
    value: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SkillActionRequest {
    action: String,
    skill_id: Option<String>,
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ApprovalActionRequest {
    action: String,
    approval_id: String,
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
        // Deprecated legacy aliases: thin delegates to canonical chat handler
        .route("/v1/command", post(execute))
        .route("/v1/chat", post(execute))
        .route("/v1/session-chat", post(execute_session))
        .route("/v1/attachments/ingest", post(ingest_attachment))
        .route("/v1/logs", get(logs))
        .route("/v1/admin/snapshot", get(admin_snapshot))
        .route("/v1/admin/apply", post(admin_apply))
        .route("/v1/admin/telegram/test", post(test_telegram))
        .route(
            "/v1/admin/telegram",
            get(manager_telegram_status).post(manager_telegram_action),
        )
        .route("/v1/admin/custom/models", post(custom_models))
        .route("/v1/admin/client-config", get(client_config))
        .route("/v1/admin/dashboard", get(manager_dashboard))
        .route("/v1/admin/providers", get(manager_providers))
        .route(
            "/v1/admin/providers/custom",
            post(manager_custom_profile_action),
        )
        .route("/v1/admin/runtime", get(manager_runtime))
        .route("/v1/admin/context", get(manager_context))
        .route(
            "/v1/admin/sessions",
            get(manager_sessions).post(manager_session_action),
        )
        .route("/v1/admin/runs", get(manager_runs).post(manager_run_action))
        .route(
            "/v1/admin/attachments",
            get(manager_attachments).post(manager_attachment_action),
        )
        .route(
            "/v1/admin/memory",
            get(manager_memory).post(manager_memory_action),
        )
        .route(
            "/v1/admin/skills",
            get(manager_skills).post(manager_skill_action),
        )
        .route("/v1/admin/tools", get(manager_tools))
        .route(
            "/v1/admin/security",
            get(manager_security).post(manager_approval_action),
        )
        .route("/v1/admin/diagnostics", get(manager_diagnostics))
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
    let owner = management_owner(&state).await.map_err(bad)?;
    let result = state
        .app
        .commands
        .execute_text(&owner, &req.input)
        .await
        .map_err(bad)?;
    Ok(Json(serde_json::to_value(result).map_err(bad)?))
}

async fn execute_session(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<SessionExecuteRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_client(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let result = if req.retry {
        state
            .app
            .commands
            .retry_in_session(&owner, &req.session_id, None)
            .await
    } else {
        let input = req.input.trim();
        if input.is_empty() {
            return Err(bad("chat input is required"));
        }
        state
            .app
            .commands
            .chat_in_session(&owner, &req.session_id, input, None)
            .await
    }
    .map_err(bad)?;
    Ok(Json(serde_json::to_value(result).map_err(bad)?))
}

async fn ingest_attachment(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<AttachmentIngestRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_client(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let kind = match req.kind.as_str() {
        "file" | "document" => crate::attachments::AttachmentKind::Document,
        "image" => crate::attachments::AttachmentKind::Image,
        _ => return Err(bad("attachment kind must be file or image")),
    };
    let max = state.app.attachments.max_download_bytes(kind);
    if req.data_base64.len() as u64 > max.saturating_mul(4).div_ceil(3).saturating_add(16) {
        return Err(bad("attachment payload exceeds configured limit"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(req.data_base64.as_bytes())
        .map_err(|_| bad("attachment payload is not valid base64"))?;
    if bytes.len() as u64 > max {
        return Err(bad("attachment exceeds configured limit"));
    }
    let record = state
        .app
        .attachments
        .ingest(crate::attachments::AttachmentIngest {
            owner_id: owner,
            session_id: req.session_id,
            telegram_file_id: None,
            telegram_unique_id: None,
            original_name: req.name,
            declared_mime: req.mime,
            expected_kind: kind,
            bytes,
        })
        .map_err(bad)?;
    Ok(Json(json!({ "attachment": record })))
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
    Ok(Json(json!({ "lines": lines })))
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
    let control = state.app.storage.telegram_control_state().map_err(bad)?;
    let bot = control
        .as_ref()
        .and_then(|state| state.bot_token_ref.as_deref())
        .map(|reference| store.get(reference))
        .transpose()
        .map_err(bad)?
        .flatten();
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
            "allowed_chat_ids": control.as_ref().map(|state| state.allowed_chat_ids.clone()).unwrap_or_default()
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
    // P0-5 / P2-2: telegram owner/token mutation is authoritative via
    // TelegramSetupService (/v1/admin/telegram). Legacy admin fields are
    // thin delegates that direct callers to the canonical endpoint.
    if req.telegram_enabled.is_some()
        || req.owner_user_id.is_some()
        || req.telegram_bot_token.is_some()
        || req.allowed_chat_ids.is_some()
    {
        return Err(bad(
            "telegram owner/token must be mutated via POST /v1/admin/telegram (TelegramSetupService); legacy /v1/admin/apply no longer mutates telegram identity",
        ));
    }
    if req.antigravity_enabled.is_some()
        || req.antigravity_oauth_client_id.is_some()
        || req.antigravity_oauth_client_secret.is_some()
        || req.antigravity_default_model.is_some()
    {
        return Err(bad(
            "provider_configuration_required: Codex and Antigravity are legacy-only in v0.2.8; configure a Custom profile instead",
        ));
    }
    let old = state.app.config.read().await.clone();
    let mut next = old.clone();
    if let Some(v) = req.gateway_enabled {
        next.gateway.enabled = v;
    }
    if let Some(v) = req.gateway_auto_restart {
        next.gateway.auto_restart = v;
    }
    // telegram fields are handled exclusively via TelegramSetupService; see reject above.
    if req
        .allowed_user_ids
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(bad(
            "allowed_user_ids is legacy-only; set one owner_user_id explicitly",
        ));
    }
    if req.owner_user_id.is_some() {
        return Err(bad(
            "owner_user_id via /v1/admin/apply is deprecated; use POST /v1/admin/telegram",
        ));
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

    // External validation is complete before any config commit.
    // P2-2: legacy telegram fields are never mutated here; canonical
    // TelegramSetupService (/v1/admin/telegram) owns token/owner state.
    next.save_atomic(&state.config_path).map_err(bad)?;
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
    state.app.providers.reload_config(&next);
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

    // P2-2: legacy telegram token no longer triggers restart here.
    let restart_required = old.gateway.enabled != next.gateway.enabled
        || old.telegram.enabled != next.telegram.enabled
        || old.daemon.log_level != next.daemon.log_level;
    Ok(Json(json!({
        "ok": true,
        "applied": true,
        "restart_required": restart_required,
        "hot_reloaded": ["telegram.access.owner_user_id","telegram.access.allowed_chat_ids","telegram.ui","providers.custom","gateway.auto_restart"]
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
    let service = TelegramSetupService::new(state.app.clone(), state.config_path.clone());
    let bot = service
        .test_connection(req.token.as_deref())
        .await
        .map_err(bad)?;
    Ok(Json(
        json!({"ok":true,"bot":{"id":bot.id,"username":bot.username,"first_name":bot.first_name}}),
    ))
}

async fn manager_telegram_status(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let service = TelegramSetupService::new(state.app.clone(), state.config_path.clone());
    let status = service.status().await.map_err(bad)?;
    Ok(Json(json!({"ok":true,"telegram":status})))
}

async fn manager_telegram_action(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<TelegramSetupActionRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let service = TelegramSetupService::new(state.app.clone(), state.config_path.clone());
    match req.action.as_str() {
        "configure" | "save" => {
            let result = service
                .configure(TelegramConfigureInput {
                    enabled: req.enabled,
                    bot_token: req.token,
                    owner_user_id: req.owner_user_id,
                    confirm_owner_change: req.confirm_owner_change,
                    allowed_chat_ids: req.allowed_chat_ids,
                    test_connection: false,
                    resolve_legacy_owners: false,
                })
                .await
                .map_err(bad)?;
            Ok(Json(json!({"ok":true,"result":result})))
        }
        "save_and_test" => {
            let result = service
                .configure(TelegramConfigureInput {
                    enabled: req.enabled,
                    bot_token: req.token,
                    owner_user_id: req.owner_user_id,
                    confirm_owner_change: req.confirm_owner_change,
                    allowed_chat_ids: req.allowed_chat_ids,
                    test_connection: true,
                    resolve_legacy_owners: false,
                })
                .await
                .map_err(bad)?;
            Ok(Json(json!({"ok":true,"result":result})))
        }
        "test" => {
            let bot = service
                .test_connection(req.token.as_deref())
                .await
                .map_err(bad)?;
            Ok(Json(json!({"ok":true,"bot":bot})))
        }
        _ => Err(bad("unknown Telegram setup action")),
    }
}

async fn custom_models(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<FetchModelsRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let supplied_key = req
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let (base_url, profile_headers, profile_key) = if let Some(profile_id) = req.profile_id {
        let owner = management_owner(&state).await.map_err(bad)?;
        let profile = ProviderProfileStore::new(state.app.storage.clone())
            .get(&owner, &profile_id)
            .map_err(bad)?
            .ok_or_else(|| bad("Custom profile not found for owner"))?;
        let key = match supplied_key {
            Some(value) => Some(value),
            None => profile
                .credential_ref
                .as_deref()
                .map(|reference| state.app.auth.credential(reference))
                .transpose()
                .map_err(bad)?
                .flatten()
                .and_then(|credential| credential.api_key),
        };
        // P1-4: request merges only selected profile's safe+secret headers, no cross-profile fallback.
        let cfg = state.app.config.read().await.clone();
        let secrets = SecretStore::new(cfg.paths.secrets_dir.clone());
        let profile_headers = profile.merged_headers(&secrets).map_err(bad)?;
        (profile.endpoint, profile_headers, key)
    } else {
        // Legacy explicit discovery remains available for onboarding, but an
        // omitted key means no Authorization and no provider-wide headers.
        // It must never inherit credentials from another Custom profile.
        (
            req.base_url
                .ok_or_else(|| bad("base_url or profile_id is required"))?,
            BTreeMap::new(),
            supplied_key,
        )
    };
    let models = fetch_custom_models(&base_url, &profile_headers, profile_key.as_deref())
        .await
        .map_err(bad)?;
    Ok(Json(json!({"ok":true,"models":models})))
}

fn stored_provider_api_key(app: &AppState, provider: &str) -> Result<Option<String>> {
    app.auth.provider_api_key(provider)
}

async fn management_owner(state: &ApiState) -> Result<String> {
    // The installation owner is durable SQLite state. The TOML owner field is
    // only a compatibility projection and must never be allowed to rebind the
    // owner when it is stale or manually edited.
    let owner = state.app.storage.management_owner_id()?;
    MemoryStore::with_workspace(state.app.storage.clone(), state.app.identity.clone())
        .reconcile(&owner)?;
    FilesystemSkills::new(
        state.app.identity.clone(),
        Arc::new(SkillStore::new(state.app.storage.clone())),
    )
    .reconcile(&owner)?;
    Ok(owner)
}

fn page_bounds(
    total: usize,
    requested: Option<usize>,
    limit: Option<usize>,
) -> (usize, usize, usize) {
    let limit = limit.unwrap_or(5).clamp(1, 50);
    let pages = total.max(1).div_ceil(limit);
    let page = requested.unwrap_or(1).clamp(1, pages);
    (page, pages, limit)
}

async fn manager_dashboard(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let cfg = state.app.config.read().await.clone();
    let health = state
        .app
        .health
        .snapshot(
            &cfg,
            state.app.storage.health(),
            state.app.providers.states(),
        )
        .await;
    let counts = state.app.storage.manager_counts(&owner).map_err(bad)?;
    let current = state
        .app
        .storage
        .list_main_sessions(&owner, 1, 0, false)
        .map_err(bad)?
        .into_iter()
        .next();
    let environment = state.app.runtime.environment();
    Ok(Json(json!({
        "owner_id": owner,
        "health": health,
        "counts": counts,
        "current_ai": current.as_ref().map(|session| json!({
            "provider": session.provider,
            "account_or_profile_id": session.account_id,
            "model": session.model,
            "session_id": session.id,
        })),
        "runtime": {
            "termux": environment.termux.is_some(),
            "root": environment.effective_uid == 0,
            "selinux": environment.selinux,
        }
    })))
}

async fn manager_providers(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let profiles_store = ProviderProfileStore::new(state.app.storage.clone());
    let cfg = state.app.config.read().await.clone();
    let secrets = SecretStore::new(cfg.paths.secrets_dir.clone());
    let profiles = profiles_store
        .list(&owner)
        .map_err(bad)?
        .into_iter()
        .map(|profile| {
            let models = profiles_store
                .models(&profile.profile_id)
                .unwrap_or_default();
            // Header values are write-only just like API keys. The manager may expose
            // names for inspection, but never returns stored values to a frontend.
            // P1-4: safe_headers -> DB, secret_headers -> SecretStore, only names in JSON.
            let header_names = profile.all_header_names(&secrets).unwrap_or_else(|_| {
                profile
                    .safe_headers()
                    .unwrap_or_default()
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
            });
            // Redacted: never include header values in serialized JSON.
            json!({
                "id": profile.profile_id,
                "alias": profile.alias,
                "endpoint": profile.endpoint,
                "protocol": profile.protocol,
                "enabled": profile.enabled,
                "reachability": profile.reachability,
                "api_key_configured": profile.credential_ref.is_some(),
                "header_names": header_names,
                "model_count": models.len(),
                "models": models,
                "last_probe_at": profile.last_probe_at,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "provider_states": state.app.providers.states(),
        // Keep the field empty for a bounded wire-compatible transition, but
        // never expose legacy account metadata or credentials to normal
        // product surfaces. Custom profile secrets are distinct write-only
        // references and cannot be inherited by a legacy session.
        "accounts": [],
        "custom_profiles": profiles,
    })))
}

async fn manager_custom_profile_action(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<CustomProfileActionRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let profiles = ProviderProfileStore::new(state.app.storage.clone());
    let cfg = state.app.config.read().await.clone();
    let secrets = SecretStore::new(cfg.paths.secrets_dir.clone());
    match req.action.as_str() {
        "create" => {
            let alias = req
                .alias
                .as_deref()
                .ok_or_else(|| bad("alias is required"))?;
            let endpoint = req
                .endpoint
                .as_deref()
                .ok_or_else(|| bad("endpoint is required"))?;
            let protocol = req.protocol.as_deref().unwrap_or("openai_chat_completions");
            let safe_headers = req.headers.clone().unwrap_or_default();
            let result = crate::providers::CustomProfileService::with_auth(
                state.app.storage.clone(),
                secrets.clone(),
                state.app.auth.clone(),
            )
            .create_profile(
                &owner,
                alias,
                endpoint,
                protocol,
                safe_headers,
                req.secret_headers.clone().unwrap_or_default(),
                req.api_key.as_deref(),
            )
            .map_err(|error| bad(redact_text(&error.to_string())))?;
            let profile = result.profile;
            state
                .app
                .storage
                .audit(
                    &owner,
                    "custom_profile_created",
                    &redact_text(&format!("profile_id={}", profile.profile_id)),
                )
                .map_err(bad)?;
            Ok(Json(json!({"ok":true,"profile_id":profile.profile_id})))
        }
        "edit" => {
            let profile_id = req
                .profile_id
                .as_deref()
                .ok_or_else(|| bad("profile_id is required"))?;
            let prior = profiles
                .get(&owner, profile_id)
                .map_err(bad)?
                .ok_or_else(|| bad("Custom profile not found"))?;
            let endpoint_changed = req
                .endpoint
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_some_and(|value| value != prior.endpoint);
            // P1-5: atomic edit via CustomProfileService (validate -> stage secret -> DB txn -> commit -> delete old -> rollback on fail -> invalidate models on endpoint change)
            let service = crate::providers::CustomProfileService::with_auth(
                state.app.storage.clone(),
                secrets.clone(),
                state.app.auth.clone(),
            );
            let edit = crate::providers::CustomProfileEdit {
                alias: req.alias.clone(),
                endpoint: req.endpoint.clone(),
                protocol: req.protocol.clone(),
                safe_headers: req.headers.clone(),
                secret_headers: req.secret_headers.clone(),
                clear_secret_headers: req.clear_secret_headers,
                keep_credential_on_endpoint_change: req.keep_credential,
                keep_safe_headers_on_endpoint_change: req.keep_safe_headers,
                keep_secret_headers_on_endpoint_change: req.keep_secret_headers,
                api_key: req.api_key.clone(),
                remove_api_key: req.remove_api_key,
            };
            let result = service
                .edit_with_warnings(&owner, profile_id, edit)
                .map_err(bad)?;
            let profile = result.profile;
            state
                .app
                .storage
                .audit(
                    &owner,
                    "custom_profile_edited",
                    &redact_text(&format!(
                        "profile_id={profile_id};endpoint_changed={endpoint_changed};credential_kept={};safe_headers_kept={};secret_headers_kept={}",
                        endpoint_changed && req.keep_credential,
                        endpoint_changed && req.keep_safe_headers,
                        endpoint_changed && req.keep_secret_headers
                    )),
                )
                .map_err(bad)?;
            let header_names = profile.all_header_names(&secrets).unwrap_or_else(|_| {
                profile
                    .safe_headers()
                    .unwrap_or_default()
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
            });
            Ok(Json(json!({
                "ok": true,
                "profile": {
                    "id": profile.profile_id,
                    "alias": profile.alias,
                    "endpoint": profile.endpoint,
                    "protocol": profile.protocol,
                    "api_key_configured": profile.credential_ref.is_some(),
                    "header_names": header_names,
                },
                "credential_cleared_on_endpoint_change": endpoint_changed && !req.keep_credential,
                "safe_headers_cleared_on_endpoint_change": endpoint_changed && !req.keep_safe_headers,
                "secret_headers_cleared_on_endpoint_change": endpoint_changed && !req.keep_secret_headers,
                "cleanup_warnings": result.cleanup_warnings,
            })))
        }
        "edit_endpoint" => {
            let profile_id = req
                .profile_id
                .as_deref()
                .ok_or_else(|| bad("profile_id is required"))?;
            let endpoint = req
                .endpoint
                .as_deref()
                .ok_or_else(|| bad("endpoint is required"))?;
            let result = crate::providers::CustomProfileService::with_auth(
                state.app.storage.clone(),
                secrets.clone(),
                state.app.auth.clone(),
            )
            .edit_with_warnings(
                &owner,
                profile_id,
                crate::providers::CustomProfileEdit {
                    endpoint: Some(endpoint.to_owned()),
                    ..Default::default()
                },
            )
            .map_err(bad)?;
            state
                .app
                .storage
                .audit(
                    &owner,
                    "custom_profile_endpoint_changed",
                    &redact_text(&format!(
                        "profile_id={profile_id};credential_cleared=true;cleanup_warnings={}",
                        result.cleanup_warnings.len()
                    )),
                )
                .map_err(bad)?;
            Ok(Json(
                json!({"ok":true,"credential_cleared":true,"headers_cleared":true,"cleanup_warnings":result.cleanup_warnings}),
            ))
        }
        "test" => {
            let profile_id = req
                .profile_id
                .as_deref()
                .ok_or_else(|| bad("profile_id is required"))?;
            let profile = profiles
                .get(&owner, profile_id)
                .map_err(bad)?
                .ok_or_else(|| bad("Custom profile not found"))?;
            let api_key = profile
                .credential_ref
                .as_deref()
                .map(|reference| state.app.auth.credential(reference))
                .transpose()
                .map_err(bad)?
                .flatten()
                .and_then(|credential| credential.api_key);
            // P1-4: request merges only selected profile's headers, no cross-profile fallback.
            let headers = profile.merged_headers(&secrets).map_err(bad)?;
            let ids = fetch_custom_models(&profile.endpoint, &headers, api_key.as_deref())
                .await
                .map_err(|error| {
                    let _ = profiles.set_reachability(&owner, profile_id, "unreachable");
                    bad(error)
                })?;
            let prior = profiles
                .models(profile_id)
                .map_err(bad)?
                .into_iter()
                .map(|model| (model.model_id.clone(), model))
                .collect::<BTreeMap<_, _>>();

            let mut models = Vec::with_capacity(ids.len());
            // Keep a hard network budget for a catalog with many models. The
            // selected/requested model is probed first; at most eight models
            // are actively probed per Test operation, all others remain
            // explicitly Unknown rather than silently Unsupported.
            let requested_model = req.model.as_deref();
            let mut ordered = ids;
            if let Some(model) = requested_model {
                if let Some(index) = ordered.iter().position(|candidate| candidate == model) {
                    let selected = ordered.remove(index);
                    ordered.insert(0, selected);
                } else {
                    return Err(bad("requested model is not in the Custom catalog"));
                }
            }
            let now = chrono::Utc::now().to_rfc3339();
            for (index, model_id) in ordered.into_iter().enumerate() {
                if index < 8 {
                    let probe = crate::providers::probe_custom_capabilities(
                        &profile.endpoint,
                        &headers,
                        api_key.as_deref(),
                        &profile.protocol,
                        &model_id,
                    )
                    .await;
                    models.push(crate::providers::profile_model_from_probe(
                        profile_id, &model_id, &probe, &now,
                    ));
                } else {
                    models.push(prior.get(&model_id).cloned().unwrap_or(
                        ProviderProfileModelRecord {
                            profile_id: profile_id.to_owned(),
                            model_id,
                            text_capable: true,
                            vision_capable: false,
                            file_input_capable: false,
                            native_tools: false,
                            structured_output: false,
                            continuation: false,
                            native_tools_state: "unknown".into(),
                            structured_output_state: "unknown".into(),
                            continuation_state: "unknown".into(),
                            vision_state: "unknown".into(),
                            file_input_state: "unknown".into(),
                            model_discovery: true,
                            tool_protocol: ToolProtocol::ChatOnly.as_str().into(),
                            evidence:
                                "model discovered; active capability probe budget not spent".into(),
                            probe_status: "unprobed".into(),
                            probe_version: 1,
                            probed_at: now.clone(),
                        },
                    ));
                }
            }
            profiles
                .replace_models(&owner, profile_id, &models)
                .map_err(bad)?;
            Ok(Json(json!({"ok":true,"models":models})))
        }
        "probe" => {
            // P0-4 WebUI exact-model probe: bounded single-model probe -> persist, no catalog discovery.
            let profile_id = req
                .profile_id
                .as_deref()
                .ok_or_else(|| bad("profile_id is required"))?;
            let model = req
                .model
                .as_deref()
                .ok_or_else(|| bad("model is required"))?;
            let profile = profiles
                .get(&owner, profile_id)
                .map_err(bad)?
                .ok_or_else(|| bad("Custom profile not found"))?;
            if profiles.model(profile_id, model).map_err(bad)?.is_none() {
                return Err(bad("model has not been discovered for this Custom profile"));
            }
            let headers = profile.merged_headers(&secrets).map_err(bad)?;
            let api_key = profile
                .credential_ref
                .as_deref()
                .map(|ref_id| state.app.auth.credential(ref_id))
                .transpose()
                .map_err(bad)?
                .flatten()
                .and_then(|c| c.api_key);
            let probe = crate::providers::probe_custom_capabilities(
                &profile.endpoint,
                &headers,
                api_key.as_deref(),
                &profile.protocol,
                model,
            )
            .await;
            let now = chrono::Utc::now().to_rfc3339();
            let mut models = profiles.models(profile_id).map_err(bad)?;
            if let Some(pos) = models.iter().position(|m| m.model_id == model) {
                models[pos] =
                    crate::providers::profile_model_from_probe(profile_id, model, &probe, &now);
                profiles
                    .replace_models(&owner, profile_id, &models)
                    .map_err(bad)?;
                Ok(Json(json!({"ok":true,"model":models[pos]})))
            } else {
                Err(bad("model has not been discovered for this Custom profile"))
            }
        }
        "use" => {
            let profile_id = req
                .profile_id
                .as_deref()
                .ok_or_else(|| bad("profile_id is required"))?;
            let session_id = req
                .session_id
                .as_deref()
                .ok_or_else(|| bad("session_id is required"))?;
            let model = req
                .model
                .as_deref()
                .ok_or_else(|| bad("model is required"))?;
            state
                .app
                .commands
                .management_set_session_ai(
                    &owner,
                    session_id,
                    "custom",
                    Some(profile_id.to_owned()),
                    model,
                )
                .map_err(bad)?;
            Ok(Json(json!({"ok":true})))
        }
        "delete" => {
            let profile_id = req
                .profile_id
                .as_deref()
                .ok_or_else(|| bad("profile_id is required"))?;
            let result = crate::providers::CustomProfileService::with_auth(
                state.app.storage.clone(),
                secrets.clone(),
                state.app.auth.clone(),
            )
            .delete_with_warnings(&owner, profile_id)
            .map_err(bad)?;
            state
                .app
                .storage
                .audit(
                    &owner,
                    "custom_profile_deleted",
                    &redact_text(&format!(
                        "profile_id={profile_id};cleanup_warnings={}",
                        result.cleanup_warnings.len()
                    )),
                )
                .map_err(bad)?;
            Ok(Json(json!({
                "ok": true,
                "cleanup_warnings": result.cleanup_warnings
            })))
        }
        _ => Err(bad("unsupported Custom profile action")),
    }
}

async fn manager_runtime(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let cfg = state.app.config.read().await.clone();
    Ok(Json(json!({
        "environment": state.app.runtime.environment(),
        "capabilities": state.app.runtime.capabilities().list(),
        "paths": {
            "data": cfg.paths.data_dir,
            "database": cfg.storage.database,
            "logs": cfg.paths.logs_dir,
            "secrets": "configured (private)",
        },
    })))
}

async fn manager_context(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<ManagerQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let context = if let Some(session_id) = query.session_id.as_deref() {
        state
            .app
            .sessions
            .context_for_session(&owner, session_id)
            .map_err(bad)?
    } else {
        state.app.sessions.context_for(&owner).map_err(bad)?
    };
    let main_messages = state
        .app
        .storage
        .messages(&owner, &context.main.id)
        .map_err(bad)?;
    let side_messages = if context.mode == crate::session::ChatMode::Side {
        state
            .app
            .storage
            .messages(&owner, &context.active.id)
            .map_err(bad)?
    } else {
        Vec::new()
    };
    let chars = main_messages
        .iter()
        .chain(side_messages.iter())
        .map(|message| message.content.chars().count())
        .sum::<usize>();
    let memories = MemoryStore::new(state.app.storage.clone())
        .list(&owner, None, 200)
        .map_err(bad)?
        .len();
    let skills = SkillStore::new(state.app.storage.clone())
        .list(&owner, 500)
        .map_err(bad)?
        .len();
    let summarized = state
        .app
        .storage
        .session_summary(&owner, &context.main.id)
        .map_err(bad)?
        .is_some();
    let budget = state.app.config.read().await.agent.context_max_chars;
    Ok(Json(json!({
        "session_id":context.active.id,
        "main_session_id":context.main.id,
        "mode":context.mode.as_str(),
        "main_messages":context.main.message_count,
        "effective_messages":main_messages.len()+side_messages.len(),
        "stored_characters":chars,
        "context_budget_characters":budget,
        "summary_available":summarized,
        "active_memory_entries":memories,
        "skills_available":skills,
        "provider":context.active.provider,
        "account_or_profile_id":context.active.account_id,
        "model":context.active.model,
    })))
}

async fn manager_sessions(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<ManagerQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let mut rows = state
        .app
        .storage
        .list_main_sessions(&owner, 500, 0, query.include_archived.unwrap_or(true))
        .map_err(bad)?;
    if let Some(id) = query.id.as_deref() {
        rows.retain(|session| session.id == id);
    }
    let active_cli_session_id = state
        .app
        .storage
        .frontend_state(&owner)
        .map_err(bad)?
        .map(|(main_id, _, _)| main_id);
    let (page, pages, limit) = page_bounds(rows.len(), query.page, query.limit);
    let start = (page - 1) * limit;
    let items = rows
        .into_iter()
        .skip(start)
        .take(limit)
        .map(|session| {
            let scope = state
                .app
                .storage
                .telegram_scope_for_session(&owner, &session.id)
                .ok()
                .flatten();
            json!({
                "id": session.id,
                "name": session.name,
                "provider": session.provider,
                "account_or_profile_id": session.account_id,
                "model": session.model,
                "message_count": session.message_count,
                "archived": session.archived,
                "yolo": session.yolo_mode,
                "created_at": session.created_at,
                "last_active_at": session.last_active_at,
                "telegram_scope": scope.map(|(chat_id, message_thread_id)| json!({
                    "chat_id": chat_id,
                    "message_thread_id": message_thread_id,
                })),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "items":items,
        "page":page,
        "pages":pages,
        "page_size":limit,
        "active_cli_session_id":active_cli_session_id,
    })))
}

async fn manager_session_action(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<SessionActionRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let required_session = || {
        req.session_id
            .as_deref()
            .ok_or_else(|| bad("session_id is required"))
    };
    match req.action.as_str() {
        "btw" => {
            let context = state.app.sessions.toggle_side(&owner).map_err(bad)?;
            return Ok(Json(json!({
                "ok":true,
                "mode":context.mode.as_str(),
                "main_session_id":context.main.id,
                "active_session_id":context.active.id,
            })));
        }
        "stop" => {
            let session_id = required_session()?;
            let cancelled = state
                .app
                .commands
                .cancel_session_run(&owner, session_id)
                .map_err(bad)?;
            return Ok(Json(json!({"ok":cancelled,"cancel_requested":cancelled})));
        }
        "new" => {
            let session = state.app.sessions.create_and_switch(&owner).map_err(bad)?;
            return Ok(Json(json!({"ok":true,"session":session})));
        }
        "use" => {
            let session = state
                .app
                .sessions
                .switch_main(&owner, required_session()?)
                .map_err(bad)?;
            return Ok(Json(json!({"ok":true,"session":session})));
        }
        "rename" => {
            let session_id = required_session()?;
            let value = req
                .value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty() && value.chars().count() <= 120)
                .ok_or_else(|| bad("session name must contain 1-120 characters"))?;
            state
                .app
                .storage
                .rename_session(&owner, session_id, value)
                .map_err(bad)?;
        }
        "delete" => {
            let session_id = required_session()?;
            let scope = state
                .app
                .storage
                .telegram_scope_for_session(&owner, session_id)
                .map_err(bad)?;
            let (active, deleted) = if let Some((chat_id, message_thread_id)) = scope {
                state
                    .app
                    .sessions
                    .delete_and_recover_telegram(
                        &owner,
                        crate::telegram::TelegramScope::new(chat_id, message_thread_id),
                        session_id,
                    )
                    .map_err(bad)?
            } else {
                state
                    .app
                    .sessions
                    .delete_and_recover(&owner, session_id)
                    .map_err(bad)?
            };
            let cleanup_warning = state
                .app
                .attachments
                .cleanup_deleted_session_paths(&deleted.attachment_paths)
                .err()
                .map(|error| redact_text(&error.to_string()));
            state
                .app
                .storage
                .audit(
                    &owner,
                    "session_deleted",
                    &format!(
                        "session_id={session_id};replacement_session_id={};attachment_cleanup_warning={}",
                        active.id,
                        cleanup_warning.as_deref().unwrap_or("none")
                    ),
                )
                .map_err(bad)?;
            return Ok(Json(json!({
                "ok": true,
                "deleted": true,
                "active_session": active,
                "cleanup_warning": cleanup_warning,
            })));
        }
        "yolo" => {
            let session_id = required_session()?;
            let enabled = req
                .value
                .as_deref()
                .and_then(|value| match value {
                    "true" | "on" => Some(true),
                    "false" | "off" => Some(false),
                    _ => None,
                })
                .ok_or_else(|| bad("YOLO value must be true or false"))?;
            state
                .app
                .storage
                .set_session_yolo(&owner, session_id, enabled)
                .map_err(bad)?;
            state
                .app
                .storage
                .audit(
                    &owner,
                    if enabled {
                        "yolo_enabled"
                    } else {
                        "yolo_disabled"
                    },
                    &format!("session_id={session_id}"),
                )
                .map_err(bad)?;
        }
        "ai_config" => {
            let session = state
                .app
                .commands
                .management_set_session_ai(
                    &owner,
                    required_session()?,
                    req.provider
                        .as_deref()
                        .ok_or_else(|| bad("provider is required"))?,
                    req.account_or_profile_id.clone(),
                    req.model
                        .as_deref()
                        .ok_or_else(|| bad("model is required"))?,
                )
                .map_err(bad)?;
            state
                .app
                .storage
                .audit(
                    &owner,
                    "session_ai_config_changed",
                    &format!("session_id={}", session.id),
                )
                .map_err(bad)?;
            return Ok(Json(json!({"ok":true,"session":session})));
        }
        _ => return Err(bad("unsupported session action")),
    }
    let session_id = required_session()?;
    let session = state.app.storage.session(&owner, session_id).map_err(bad)?;
    Ok(Json(json!({"ok":true,"session":session})))
}

async fn manager_runs(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<ManagerQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let rows = state.app.storage.agent_runs(&owner, 500).map_err(bad)?;
    let (page, pages, limit) = page_bounds(rows.len(), query.page, query.limit);
    let items = rows
        .into_iter()
        .skip((page - 1) * limit)
        .take(limit)
        .map(|run| {
            let tool_records = state
                .app
                .storage
                .tool_runs(&owner, &run.id)
                .unwrap_or_default();
            let verification_evidence = tool_records
                .iter()
                .filter(|tool| tool.status == "succeeded")
                .filter_map(|tool| {
                    tool.output.as_deref().map(|output| {
                        format!("{}: {}", tool.tool_name, bounded_redacted(output, 1_000))
                    })
                })
                .take(5)
                .collect::<Vec<_>>();
            let tools = tool_records
                .into_iter()
                .map(|tool| {
                    json!({
                        "id": tool.id,
                        "tool_name": tool.tool_name,
                        "risk": tool.risk,
                        "status": tool.status,
                        "approval_mode": tool.approval_mode,
                        "output": tool.output.map(|value| redact_text(&value)),
                        "error": tool.error.map(|value| redact_text(&value)),
                    })
                })
                .collect::<Vec<_>>();
            let dependencies = state
                .app
                .storage
                .dependency_installs(&run.id)
                .unwrap_or_default();
            let result = state
                .app
                .storage
                .agent_run_result(&owner, &run)
                .ok()
                .flatten()
                .map(|value| bounded_redacted(&value, 4_000));
            let verification_state = match run.status.as_str() {
                "completed" => "verified_success",
                "blocked" => "blocked",
                "failed" | "cancelled" | "interrupted" => "failed",
                _ => "not_yet_verified",
            };
            json!({
                "id": run.id,
                "session_id": run.session_id,
                "provider": run.provider,
                "model": run.model,
                "status": run.status,
                "goal": run.goal.map(|value| redact_text(&value)),
                "started_at": run.started_at,
                "finished_at": run.finished_at,
                "blocker_or_error": run.error.map(|value| redact_text(&value)),
                "result": result,
                "verification": {
                    "state": verification_state,
                    "evidence": verification_evidence,
                },
                "tools": tools,
                "dependency_installs": dependencies,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(
        json!({"items":items,"page":page,"pages":pages,"page_size":limit}),
    ))
}

async fn manager_attachments(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<ManagerQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let mut items = state
        .app
        .storage
        .list_attachments(
            &owner,
            query.session_id.as_deref(),
            query.limit.unwrap_or(200),
        )
        .map_err(bad)?;
    if let Some(id) = query.id.as_deref() {
        items.retain(|item| item.attachment_id == id);
    }
    let usage = state.app.attachments.usage(&owner).map_err(bad)?;
    Ok(Json(json!({"items":items,"usage":usage})))
}

async fn manager_attachment_action(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<AttachmentActionRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    match req.action.as_str() {
        "remove" => {
            let removed = state
                .app
                .attachments
                .remove(&owner, &req.attachment_id)
                .map_err(bad)?;
            if !removed {
                return Err(bad("attachment not found"));
            }
            Ok(Json(json!({"ok":true,"removed":true})))
        }
        _ => Err(bad("unsupported attachment action")),
    }
}

fn bounded_redacted(value: &str, max_chars: usize) -> String {
    let value = redact_text(value);
    if value.chars().count() <= max_chars {
        value
    } else {
        value.chars().take(max_chars).collect::<String>() + "…"
    }
}

async fn manager_run_action(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<RunActionRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    if req.action != "cancel" {
        return Err(bad("unsupported run action"));
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let run = state
        .app
        .storage
        .agent_run(&owner, &req.run_id)
        .map_err(bad)?
        .ok_or_else(|| bad("run not found"))?;
    let cancelled = state
        .app
        .commands
        .cancel_session_run(&owner, &run.session_id)
        .map_err(bad)?;
    if cancelled {
        state
            .app
            .storage
            .audit(
                &owner,
                "agent_run_cancel_requested",
                &format!("run_id={}", run.id),
            )
            .map_err(bad)?;
    }
    Ok(Json(json!({"ok":cancelled,"cancel_requested":cancelled})))
}

fn manager_memory_store(state: &ApiState) -> MemoryStore {
    MemoryStore::with_workspace(state.app.storage.clone(), state.app.identity.clone())
}

async fn manager_memory(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<ManagerQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let store = manager_memory_store(&state);
    store.reconcile(&owner).map_err(bad)?;
    if query.scope.as_deref() == Some("history") {
        return Ok(Json(
            json!({"items":store.history(&owner, query.limit.unwrap_or(100)).map_err(bad)?}),
        ));
    }
    let memory_scope = query
        .scope
        .as_deref()
        .filter(|scope| !scope.is_empty() && *scope != "all")
        .map(MemoryScope::try_from)
        .transpose()
        .map_err(bad)?;
    let rows = match query
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
    {
        Some(search) => store.search(&owner, search, 500).map_err(bad)?,
        None => store.list(&owner, memory_scope, 500).map_err(bad)?,
    };
    let (page, pages, limit) = page_bounds(rows.len(), query.page, query.limit);
    let items = rows
        .into_iter()
        .skip((page - 1) * limit)
        .take(limit)
        .collect::<Vec<_>>();
    Ok(Json(
        json!({"items":items,"page":page,"pages":pages,"page_size":limit}),
    ))
}

async fn manager_memory_action(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<MemoryActionRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let store = manager_memory_store(&state);
    let changed = match req.action.as_str() {
        "reconcile" => store.reconcile(&owner).map_err(bad)?,
        "upsert" => {
            let scope = MemoryScope::try_from(
                req.scope
                    .as_deref()
                    .ok_or_else(|| bad("scope is required"))?,
            )
            .map_err(bad)?;
            let category = req
                .category
                .as_deref()
                .ok_or_else(|| bad("category is required"))?;
            let key = req.key.as_deref().ok_or_else(|| bad("key is required"))?;
            let value = req
                .value
                .as_deref()
                .ok_or_else(|| bad("value is required"))?;
            state
                .app
                .commands
                .management_memory_set(&owner, scope, category, key, value, "owner_management_edit")
                .map_err(bad)?;
            1
        }
        "delete" => {
            let scope = MemoryScope::try_from(
                req.scope
                    .as_deref()
                    .ok_or_else(|| bad("scope is required"))?,
            )
            .map_err(bad)?;
            state
                .app
                .commands
                .management_memory_forget(
                    &owner,
                    scope,
                    req.category
                        .as_deref()
                        .ok_or_else(|| bad("category is required"))?,
                    req.key.as_deref().ok_or_else(|| bad("key is required"))?,
                )
                .map_err(bad)? as usize
        }
        _ => return Err(bad("unsupported memory action")),
    };
    state
        .app
        .storage
        .audit(
            &owner,
            "memory_manager_action",
            &format!("action={};changed={changed}", req.action),
        )
        .map_err(bad)?;
    Ok(Json(json!({"ok":true,"changed":changed})))
}

async fn manager_skills(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(query): Query<ManagerQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let skills = FilesystemSkills::new(
        state.app.identity.clone(),
        Arc::new(SkillStore::new(state.app.storage.clone())),
    );
    let reconciled = skills.reconcile(&owner).map_err(bad)?;
    let store = SkillStore::new(state.app.storage.clone());
    let rows = match query
        .query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
    {
        Some(search) => store.search(&owner, search, 500).map_err(bad)?,
        None => store.list_all(&owner, 500).map_err(bad)?,
    };
    let (page, pages, limit) = page_bounds(rows.len(), query.page, query.limit);
    let items = rows
        .into_iter()
        .skip((page - 1) * limit)
        .take(limit)
        .collect::<Vec<_>>();
    Ok(Json(
        json!({"items":items,"page":page,"pages":pages,"page_size":limit,"reconciled":reconciled}),
    ))
}

async fn manager_skill_action(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<SkillActionRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let store = SkillStore::new(state.app.storage.clone());
    match req.action.as_str() {
        "refresh" => {
            FilesystemSkills::new(state.app.identity.clone(), Arc::new(store))
                .reconcile(&owner)
                .map_err(bad)?;
        }
        "set_enabled" => {
            state
                .app
                .commands
                .management_skill_set_enabled(
                    &owner,
                    req.skill_id
                        .as_deref()
                        .ok_or_else(|| bad("skill_id is required"))?,
                    req.enabled.ok_or_else(|| bad("enabled is required"))?,
                )
                .map_err(bad)?;
        }
        "delete" => {
            let id = req
                .skill_id
                .as_deref()
                .ok_or_else(|| bad("skill_id is required"))?;
            let skill = store
                .view(&owner, id)
                .map_err(bad)?
                .ok_or_else(|| bad("skill not found"))?;
            if skill.source_kind != "learned" {
                return Err(bad("only learned owner-created skills can be deleted"));
            }
            state
                .app
                .commands
                .management_skill_delete(&owner, id)
                .map_err(bad)?;
        }
        _ => return Err(bad("unsupported skill action")),
    }
    state
        .app
        .storage
        .audit(
            &owner,
            "skill_manager_action",
            &format!("action={}", req.action),
        )
        .map_err(bad)?;
    Ok(Json(json!({"ok":true})))
}

async fn manager_tools(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    Ok(Json(
        json!({"items":state.app.runtime.capabilities().list()}),
    ))
}

async fn manager_security(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let approvals = state.app.storage.pending_approvals(&owner).map_err(bad)?;
    let yolo_sessions = state
        .app
        .storage
        .list_main_sessions(&owner, 500, 0, false)
        .map_err(bad)?
        .into_iter()
        .filter(|session| session.yolo_mode)
        .collect::<Vec<_>>();
    let profiles = ProviderProfileStore::new(state.app.storage.clone())
        .list(&owner)
        .map_err(bad)?;
    let credentials = profiles
        .into_iter()
        .map(|profile| {
            json!({
                "id": profile.profile_id,
                "provider": "custom",
                "label": profile.alias,
                "configured": profile.credential_ref.is_some() || profile.secret_headers_ref.is_some(),
                "status": if profile.enabled { "enabled" } else { "disabled" },
            })
        })
        .collect::<Vec<_>>();
    let audit = state
        .app
        .storage
        .audit_events(&owner, 100)
        .map_err(bad)?
        .into_iter()
        .map(|mut event| {
            event.detail = redact_text(&event.detail);
            event
        })
        .collect::<Vec<_>>();
    let denied_actions = state
        .app
        .storage
        .agent_runs(&owner, 100)
        .map_err(bad)?
        .into_iter()
        .flat_map(|run| {
            let run_id = run.id;
            let session_id = run.session_id;
            state
                .app
                .storage
                .tool_runs(&owner, &run_id)
                .unwrap_or_default()
                .into_iter()
                .filter(|tool| tool.status == "denied")
                .map(move |tool| {
                    json!({
                        "run_id": run_id,
                        "session_id": session_id,
                        "tool_name": tool.tool_name,
                        "risk": tool.risk,
                        "error": tool.error.map(|value| bounded_redacted(&value, 1_000)),
                        "finished_at": tool.finished_at,
                    })
                })
        })
        .take(50)
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "pending_approvals": approvals,
        "yolo_sessions": yolo_sessions,
        "credential_metadata": credentials,
        "recent_denied_actions": denied_actions,
        "recent_audit": audit,
        "root_shell_exposed": false,
        "admin_bind_loopback": state.app.config.read().await.ipc.ip().is_ok_and(|ip| ip.is_loopback()),
    })))
}

async fn manager_approval_action(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(req): Json<ApprovalActionRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let approve = match req.action.as_str() {
        "approve" => true,
        "deny" => false,
        _ => return Err(bad("approval action must be approve or deny")),
    };
    let changed = state
        .app
        .commands
        .management_approval_decide(&owner, &req.approval_id, approve)
        .map_err(bad)?;
    if !changed {
        return Err(bad("pending approval not found or expired"));
    }
    Ok(Json(json!({"ok":true})))
}

async fn manager_diagnostics(
    State(state): State<ApiState>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !authorized_admin(&headers, &state) {
        return Err(deny());
    }
    let owner = management_owner(&state).await.map_err(bad)?;
    let view = state.app.commands.management_doctor(&owner).await;
    let mut checks = Vec::new();
    for block in view.blocks {
        if let Block::List { items, .. } = block {
            for item in items {
                let mut parts = item.splitn(3, " · ");
                let status = parts.next().unwrap_or("WARN");
                let name = parts.next().unwrap_or("Unknown probe");
                let evidence = parts.next().unwrap_or_default();
                let source = if evidence.starts_with("LIVE") {
                    "live"
                } else if evidence.starts_with("CACHED") {
                    "cached"
                } else if evidence.starts_with("LOCAL") {
                    "local"
                } else {
                    "probe"
                };
                checks.push(json!({
                    "status": status,
                    "name": name,
                    "evidence": evidence,
                    "source": source,
                }));
            }
        }
    }
    Ok(Json(json!({
        "ran_at": chrono::Utc::now().to_rfc3339(),
        "checks": checks,
    })))
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

// P2-2: deprecated thin delegate helpers for the legacy WebUI/CLI base64
// envelope (manager-get-base64 / manager-post-base64). No business logic
// lives here; the base64 layer only transports JSON to the canonical typed
// manager endpoints defined in `serve`.
pub fn encode_admin_payload(json: &str) -> String {
    URL_SAFE_NO_PAD.encode(json.as_bytes())
}
/// Deprecated thin delegate. Decodes the base64 envelope used by the legacy
/// WebUI/CLI bridge and returns the inner JSON; caller must route to the
/// canonical manager handler. No independent validation is performed here.
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
    use crate::identity::WorkspaceDocument;
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

    fn admin_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse().unwrap());
        headers
    }

    async fn test_state() -> (ApiState, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let mut config = crate::config::AppConfig::default();
        config.storage.database = directory.path().join("xiao.db");
        config.paths.data_dir = directory.path().join("data");
        config.paths.logs_dir = directory.path().join("logs");
        config.paths.secrets_dir = directory.path().join("secrets");
        let app = AppState::build(config).await.unwrap();
        (
            ApiState {
                app,
                config_path: directory.path().join("config.toml"),
                client_token: Arc::new("client-test-token".into()),
                admin_token: Arc::new("admin-test-token".into()),
            },
            directory,
        )
    }

    #[tokio::test]
    async fn manager_provider_json_masks_write_only_secrets_and_header_values() {
        let (state, _directory) = test_state().await;
        let headers = admin_headers("admin-test-token");
        let mut safe_headers = BTreeMap::new();
        safe_headers.insert("X-Workspace".into(), "HEADER_VALUE_SENTINEL".into());
        let _ = manager_custom_profile_action(
            State(state.clone()),
            headers.clone(),
            Json(CustomProfileActionRequest {
                action: "create".into(),
                profile_id: None,
                alias: Some("isolated".into()),
                endpoint: Some("https://isolated.example/v1".into()),
                protocol: Some("openai_chat_completions".into()),
                api_key: Some("SECRET_BROWSER_SENTINEL".into()),
                remove_api_key: false,
                keep_credential: false,
                keep_safe_headers: false,
                keep_secret_headers: false,
                headers: Some(safe_headers),
                secret_headers: None,
                clear_secret_headers: false,
                session_id: None,
                model: None,
            }),
        )
        .await
        .unwrap();
        let response = manager_providers(State(state), headers).await.unwrap().0;
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains("SECRET_BROWSER_SENTINEL"));
        assert!(!serialized.contains("HEADER_VALUE_SENTINEL"));
        assert!(serialized.contains("X-Workspace"));
        assert_eq!(response["custom_profiles"][0]["api_key_configured"], true);
    }

    #[tokio::test]
    async fn manager_custom_create_rolls_back_profile_when_credential_creation_fails() {
        let (state, _directory) = test_state().await;
        let headers = admin_headers("admin-test-token");
        let result = manager_custom_profile_action(
            State(state.clone()),
            headers,
            Json(CustomProfileActionRequest {
                action: "create".into(),
                profile_id: None,
                alias: Some("must-rollback".into()),
                endpoint: Some("https://rollback.example/v1".into()),
                protocol: Some("openai_chat_completions".into()),
                api_key: Some("x".repeat(16_385)),
                remove_api_key: false,
                keep_credential: false,
                keep_safe_headers: false,
                keep_secret_headers: false,
                headers: Some(BTreeMap::new()),
                secret_headers: None,
                clear_secret_headers: false,
                session_id: None,
                model: None,
            }),
        )
        .await;
        assert!(result.is_err());
        let owner = state.app.storage.management_owner_id().unwrap();
        assert!(ProviderProfileStore::new(state.app.storage.clone())
            .list(&owner)
            .unwrap()
            .is_empty());
        assert!(state
            .app
            .storage
            .accounts_for_owner(&owner, Some("custom"))
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn legacy_provider_accounts_are_hidden_and_cannot_be_selected() {
        let (state, _directory) = test_state().await;
        let headers = admin_headers("admin-test-token");
        let owner = management_owner(&state).await.unwrap();
        let account = state
            .app
            .auth
            .create_api_key_credential("codex", "managed-codex", "ACCOUNT_SECRET_SENTINEL")
            .unwrap();
        state
            .app
            .storage
            .set_account_owner(&owner, &account.id)
            .unwrap();
        let session = state
            .app
            .storage
            .create_session(
                &owner,
                "legacy history",
                "codex",
                Some(&account.id),
                "gpt-legacy",
                false,
                None,
            )
            .unwrap();
        state
            .app
            .storage
            .append_message(&owner, &session.id, "assistant", "historical legacy answer")
            .unwrap();

        let providers = manager_providers(State(state.clone()), headers.clone())
            .await
            .unwrap()
            .0;
        let serialized = serde_json::to_string(&providers).unwrap();
        assert!(!serialized.contains("managed-codex"));
        assert!(!serialized.contains("ACCOUNT_SECRET_SENTINEL"));
        assert_eq!(providers["accounts"], json!([]));
        assert_eq!(
            state.app.storage.messages(&owner, &session.id).unwrap()[0].content,
            "historical legacy answer"
        );
        let error = state
            .app
            .commands
            .management_set_session_ai(&owner, &session.id, "codex", Some(account.id), "gpt-legacy")
            .unwrap_err()
            .to_string();
        assert!(error.contains("provider_configuration_required"));
    }

    #[tokio::test]
    async fn manager_dashboard_and_tasks_expose_blockers_results_and_verification_evidence() {
        let (state, _directory) = test_state().await;
        let headers = admin_headers("admin-test-token");
        let owner = management_owner(&state).await.unwrap();
        let session = state
            .app
            .storage
            .create_session(
                &owner,
                "observability",
                "codex",
                None,
                "gpt-5.6-sol",
                false,
                None,
            )
            .unwrap();
        let completed = state
            .app
            .storage
            .create_agent_run(
                &owner,
                &session.id,
                "codex",
                "gpt-5.6-sol",
                Some("create observable artifact"),
            )
            .unwrap();
        let tool = state
            .app
            .storage
            .create_tool_run(
                &completed,
                "call-observable",
                "write_file",
                r#"{"path":"result.txt"}"#,
                "safe",
            )
            .unwrap();
        state
            .app
            .storage
            .set_tool_run_status(
                &tool,
                "succeeded",
                Some("created result.txt with verified content"),
                None,
            )
            .unwrap();
        state
            .app
            .storage
            .append_message(&owner, &session.id, "assistant", "Observable final result")
            .unwrap();
        state
            .app
            .storage
            .set_agent_run_status(&owner, &completed, "completed", None)
            .unwrap();
        let blocked = state
            .app
            .storage
            .create_agent_run(
                &owner,
                &session.id,
                "codex",
                "gpt-5.6-sol",
                Some("needs physical confirmation"),
            )
            .unwrap();
        state
            .app
            .storage
            .set_agent_run_status(
                &owner,
                &blocked,
                "blocked",
                Some("owner physical interaction required"),
            )
            .unwrap();

        let dashboard = manager_dashboard(State(state.clone()), headers.clone())
            .await
            .unwrap()
            .0;
        assert_eq!(dashboard["counts"]["blocked_runs"], 1);
        let runs = manager_runs(
            State(state),
            headers,
            Query(ManagerQuery {
                page: Some(1),
                limit: Some(5),
                query: None,
                scope: None,
                include_archived: None,
                session_id: None,
                id: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let item = runs["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == completed)
            .unwrap();
        assert_eq!(item["result"], "Observable final result");
        assert_eq!(item["verification"]["state"], "verified_success");
        assert!(item["verification"]["evidence"][0]
            .as_str()
            .unwrap()
            .contains("created result.txt"));
    }

    #[tokio::test]
    async fn manager_memory_write_flows_through_living_memory_manager() {
        let (state, _directory) = test_state().await;
        let headers = admin_headers("admin-test-token");
        let _ = manager_memory_action(
            State(state.clone()),
            headers,
            Json(MemoryActionRequest {
                action: "upsert".into(),
                scope: Some("user".into()),
                category: Some("preferences".into()),
                key: Some("webui.integration".into()),
                value: Some("Visible canonical manager edit".into()),
            }),
        )
        .await
        .unwrap();
        let user = state.app.identity.read(WorkspaceDocument::User).unwrap();
        assert!(user.contains("Visible canonical manager edit"));
        let owner = state.app.storage.management_owner_id().unwrap();
        let records =
            MemoryStore::with_workspace(state.app.storage.clone(), state.app.identity.clone())
                .list(&owner, Some(MemoryScope::User), 10)
                .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key, "webui_integration");
    }

    #[test]
    fn webui_uses_only_typed_xiaod_manager_actions() {
        // Keep the source contract explicit and let CI prove that the checked-in
        // Vite bundle is regenerated from it. The browser may transport only
        // typed manager resources; all authority remains in xiaod.
        let javascript = include_str!("../../module/webroot/assets/app.js");
        let html = include_str!("../../module/webroot/index.html");
        let app = include_str!("../../webui/src/App.jsx");
        let bridge = include_str!("../../webui/src/bridge.js");
        assert!(html.contains("id=\"root\""));
        assert!(javascript.contains("manager-get-base64"));
        assert!(javascript.contains("manager-post-base64"));
        assert!(bridge.contains("const GET_RESOURCES"));
        assert!(bridge.contains("const POST_RESOURCES"));
        for forbidden in [
            "sqlite3",
            "xiao.db",
            "writeFile",
            "ksuExec",
            "raw-root",
            "/system/bin/su",
        ] {
            assert!(!javascript.contains(forbidden));
            assert!(!bridge.contains(forbidden));
        }
        for resource in [
            "dashboard",
            "telegram",
            "providers",
            "provider-custom",
            "runtime",
            "context",
            "sessions",
            "runs",
            "attachments",
            "memory",
            "skills",
            "tools",
            "security",
            "diagnostics",
            "logs",
        ] {
            assert!(
                bridge.contains(resource),
                "WebUI bridge is missing typed manager resource {resource}"
            );
        }
        for section in [
            "Overview",
            "Telegram",
            "Custom AI",
            "Sessions",
            "Attachments",
            "Runs",
            "Memory",
            "Skills",
            "Tools",
            "Security",
            "Runtime",
            "Diagnostics",
            "Logs",
        ] {
            assert!(app.contains(section), "missing WebUI manager section {section}");
        }
        for required in [
            "write-only",
            "provider: 'custom'",
            "managerPost('provider-custom'",
            "managerPost('sessions'",
            "ProfileEditor",
            "SessionAiDialog",
            "secret headers",
            "Custom profile",
        ] {
            assert!(
                app.contains(required),
                "missing Custom-only WebUI behavior {required}"
            );
        }
        for removed in [
            "provider-accounts",
            "addCodex",
            "addAgy",
            "beginProviderLogin",
            "action: 'reconnect'",
            "action: 'oauth'",
        ] {
            assert!(
                !app.contains(removed) && !bridge.contains(removed),
                "removed provider-manager surface remains: {removed}"
            );
        }
    }

    // P2-2: legacy delegates must remain thin — no independent business logic.
    #[test]
    fn legacy_admin_payload_roundtrip_is_thin_delegate() {
        let original = r#"{"resource":"dashboard","query":{"page":1}}"#;
        let encoded = encode_admin_payload(original);
        let decoded = decode_admin_payload(&encoded).unwrap();
        assert_eq!(decoded, original);
        assert!(decode_admin_payload("!!!not-base64!!!").is_err());
    }

    #[tokio::test]
    async fn legacy_admin_apply_rejects_telegram_fields_via_canonical_delegate() {
        let (state, _directory) = test_state().await;
        let headers = admin_headers("admin-test-token");
        for body in [
            json!({"telegram_enabled": true}),
            json!({"owner_user_id": 123}),
            json!({"telegram_bot_token": "123:abc"}),
            json!({"allowed_chat_ids": "-100"}),
            json!({"allowed_user_ids": "1,2"}),
        ] {
            let result = admin_apply(
                State(state.clone()),
                headers.clone(),
                Json(serde_json::from_value(body).unwrap()),
            )
            .await;
            assert!(result.is_err(), "legacy field should be rejected");
            let (status, Json(value)) = result.unwrap_err();
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert!(value.get("error").is_some());
        }
    }

    #[test]
    fn apply_request_legacy_fields_are_deprecated_delegates() {
        // Ensure ApplyRequest still deserializes legacy fields for wire compat
        // but the handler treats them as deprecated delegates (see test above).
        let raw = r#"{"telegram_enabled":true,"owner_user_id":99,"allowed_chat_ids":"-100","allowed_user_ids":"1,2"}"#;
        let req: ApplyRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.telegram_enabled, Some(true));
        assert_eq!(req.owner_user_id, Some(99));
        assert_eq!(req.allowed_chat_ids.as_deref(), Some("-100"));
        assert_eq!(req.allowed_user_ids.as_deref(), Some("1,2"));
    }
}
