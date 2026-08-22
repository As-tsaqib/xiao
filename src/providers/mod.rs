mod payload;

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    auth::{antigravity_user_agent, AuthManager},
    config::{AppConfig, CustomProviderConfig},
    security::redact::redact_text,
    storage::MessageRecord,
    tools::{ToolCall, ToolResult, ToolRouter},
};

use self::payload::{antigravity_body, chat_messages, responses_payload};

const CODEX_DEFAULT_INSTRUCTIONS: &str = "You are Xiao, a concise and helpful AI assistant.";
const MAX_UPSTREAM_ERROR_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderState {
    Disabled,
    NotConfigured,
    NeedsLogin,
    Authenticating,
    Ready,
    Expired,
    RateLimited,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum AgentEvent {
    GenerationStarted,
    Status(String),
    ToolStarted(String),
    ToolCompleted { tool: String, summary: String },
    StreamChunk { provider: String, bytes: usize },
    GenerationCompleted,
    GenerationFailed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub session_id: String,
    pub account_id: Option<String>,
    pub model: String,
    pub messages: Vec<MessageRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderResponse {
    pub events: Vec<AgentEvent>,
    pub final_answer: String,
}

#[derive(Debug, Clone)]
pub enum ProviderStep {
    Final(String),
    ToolCalls(Vec<ToolCall>),
}
#[derive(Debug, Clone)]
pub struct ProviderTurn {
    pub step: ProviderStep,
    pub continuation: Option<serde_json::Value>,
    pub events: Vec<AgentEvent>,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;
    fn models(&self) -> Vec<String>;
    fn enabled(&self) -> bool {
        true
    }
    fn configured(&self) -> bool {
        true
    }
    fn ready(&self) -> bool;
    async fn run(
        &self,
        req: ProviderRequest,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<ProviderResponse>;
    async fn run_turn(
        &self,
        req: ProviderRequest,
        continuation: Option<serde_json::Value>,
        tool_results: Vec<ToolResult>,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<ProviderTurn> {
        if continuation.is_some() || !tool_results.is_empty() {
            return Err(anyhow!("provider does not support tool continuation"));
        }
        let response = self.run(req, progress).await?;
        Ok(ProviderTurn {
            step: ProviderStep::Final(response.final_answer),
            continuation: None,
            events: response.events,
        })
    }
}

pub struct ProviderRegistry {
    providers: RwLock<HashMap<String, Arc<dyn Provider>>>,
    auth: Arc<AuthManager>,
}

impl ProviderRegistry {
    pub fn new(config: AppConfig, auth: Arc<AuthManager>) -> Self {
        Self {
            providers: RwLock::new(build_providers(&config, auth.clone())),
            auth,
        }
    }

    pub fn reload_config(&self, config: &AppConfig) {
        *self.providers.write().unwrap() = build_providers(config, self.auth.clone());
    }

    pub fn get(&self, id: &str) -> Result<Arc<dyn Provider>> {
        self.providers
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("provider {id} unavailable"))
    }

    pub fn list(&self) -> Vec<String> {
        let mut v = self
            .providers
            .read()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        v.sort();
        v
    }

    pub fn readiness(&self) -> usize {
        self.list()
            .into_iter()
            .filter(|id| self.state(id) == ProviderState::Ready)
            .count()
    }

    pub fn state(&self, id: &str) -> ProviderState {
        let Ok(provider) = self.get(id) else {
            return ProviderState::Disabled;
        };
        if !provider.enabled() {
            return ProviderState::Disabled;
        }
        if !provider.configured() {
            return ProviderState::NotConfigured;
        }
        if !provider.ready() {
            return ProviderState::Error;
        }
        if id == "custom" {
            return ProviderState::Ready;
        }
        let Ok(accounts) = self.auth.accounts(Some(id)) else {
            return ProviderState::Error;
        };
        if accounts.is_empty() {
            return ProviderState::NeedsLogin;
        }
        let mut expired = false;
        for account in accounts.into_iter().filter(|a| a.status == "connected") {
            match self.auth.credential(&account.id) {
                Ok(Some(c)) => {
                    let stale = c
                        .expires_at_unix
                        .map(|x| x <= chrono::Utc::now().timestamp())
                        .unwrap_or(false);
                    if !stale || c.refresh_token.is_some() {
                        return ProviderState::Ready;
                    }
                    expired = true;
                }
                Ok(None) => {}
                Err(_) => return ProviderState::Error,
            }
        }
        if expired {
            ProviderState::Expired
        } else {
            ProviderState::NeedsLogin
        }
    }
    pub fn states(&self) -> std::collections::BTreeMap<String, ProviderState> {
        self.list()
            .into_iter()
            .map(|id| {
                let s = self.state(&id);
                (id, s)
            })
            .collect()
    }

    pub fn models(&self, id: &str) -> Result<Vec<String>> {
        Ok(self.get(id)?.models())
    }
    pub fn preferred_model(&self, id: &str) -> Result<String> {
        self.models(id)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("provider {id} has no usable models"))
    }
    pub fn resolve_model(&self, id: &str, selected: &str) -> Result<String> {
        let models = self.models(id)?;
        if selected != "default" && models.iter().any(|model| model == selected) {
            return Ok(selected.to_owned());
        }
        models
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("provider {id} has no usable models"))
    }
    pub fn auth(&self) -> Arc<AuthManager> {
        self.auth.clone()
    }

    #[cfg(test)]
    pub(crate) fn from_single(
        id: &str,
        provider: Arc<dyn Provider>,
        auth: Arc<AuthManager>,
    ) -> Self {
        let mut providers = HashMap::new();
        providers.insert(id.to_owned(), provider);
        Self {
            providers: RwLock::new(providers),
            auth,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_test(
        providers: Vec<(&str, Arc<dyn Provider>)>,
        auth: Arc<AuthManager>,
    ) -> Self {
        let providers = providers
            .into_iter()
            .map(|(id, provider)| (id.to_owned(), provider))
            .collect();
        Self {
            providers: RwLock::new(providers),
            auth,
        }
    }
}

fn build_providers(
    config: &AppConfig,
    auth: Arc<AuthManager>,
) -> HashMap<String, Arc<dyn Provider>> {
    let mut p: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    p.insert(
        "codex".into(),
        Arc::new(CodexProvider::new(
            config.providers.codex.enabled,
            config.providers.codex.base_url.clone(),
            config.providers.codex.default_model.clone(),
            auth.clone(),
        )),
    );
    let antigravity = &config.providers.antigravity;
    let antigravity_base = antigravity.base_url.clone().unwrap_or_else(|| {
        format!(
            "{}/v1internal:streamGenerateContent?alt=sse",
            antigravity.daily_base.trim_end_matches('/')
        )
    });
    p.insert(
        "antigravity".into(),
        Arc::new(AntigravityProvider::new(
            antigravity.enabled,
            antigravity_base,
            antigravity.default_model.clone(),
            antigravity_user_agent(antigravity).to_owned(),
            antigravity.x_goog_api_client.clone(),
            auth.clone(),
        )),
    );
    p.insert(
        "custom".into(),
        Arc::new(CustomProvider::new(config.providers.custom.clone(), auth)),
    );
    p
}

fn http_client() -> Client {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .build()
        .expect("provider http client")
}

fn emit(progress: &Option<mpsc::UnboundedSender<AgentEvent>>, event: AgentEvent) {
    if let Some(tx) = progress {
        let _ = tx.send(event);
    }
}

fn models_with_default(default_model: Option<&str>, fallback_models: Vec<String>) -> Vec<String> {
    let mut models = Vec::with_capacity(fallback_models.len() + 1);
    if let Some(model) = default_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        models.push(model.to_owned());
    }
    for model in fallback_models {
        let model = model.trim();
        if !model.is_empty() && !models.iter().any(|existing| existing == model) {
            models.push(model.to_owned());
        }
    }
    models
}

struct CodexProvider {
    enabled: bool,
    base: String,
    default_model: Option<String>,
    auth: Arc<AuthManager>,
    client: Client,
}
impl CodexProvider {
    fn new(
        enabled: bool,
        base: Option<String>,
        default_model: Option<String>,
        auth: Arc<AuthManager>,
    ) -> Self {
        Self {
            enabled,
            base: base.unwrap_or_else(|| "https://chatgpt.com/backend-api/codex/responses".into()),
            default_model,
            auth,
            client: http_client(),
        }
    }
}
#[async_trait]
impl Provider for CodexProvider {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn models(&self) -> Vec<String> {
        models_with_default(
            self.default_model.as_deref(),
            vec!["gpt-5.6-sol".into(), "gpt-5.5".into()],
        )
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn ready(&self) -> bool {
        self.enabled
    }
    async fn run(
        &self,
        req: ProviderRequest,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<ProviderResponse> {
        let turn = self.run_turn(req, None, vec![], progress).await?;
        match turn.step {
            ProviderStep::Final(final_answer) => Ok(ProviderResponse {
                events: turn.events,
                final_answer,
            }),
            ProviderStep::ToolCalls(_) => {
                Err(anyhow!("Codex requested a tool outside the agent loop"))
            }
        }
    }
    async fn run_turn(
        &self,
        req: ProviderRequest,
        continuation: Option<serde_json::Value>,
        tool_results: Vec<ToolResult>,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<ProviderTurn> {
        if !self.enabled {
            return Err(anyhow!("Codex provider is disabled"));
        }
        emit(
            &progress,
            AgentEvent::Status("Preparing Codex request".into()),
        );
        let account = req
            .account_id
            .as_deref()
            .ok_or_else(|| anyhow!("Codex account not selected"))?;
        let cred = self
            .auth
            .credential_for_use(account)
            .await?
            .ok_or_else(|| anyhow!("Codex credential missing"))?;
        let token = cred
            .access_token
            .as_deref()
            .ok_or_else(|| anyhow!("Codex access token missing"))?;
        let native = cred
            .account_native_id
            .as_deref()
            .ok_or_else(|| anyhow!("ChatGPT account id missing"))?;
        let payload = responses_payload(&req.messages, Some(CODEX_DEFAULT_INSTRUCTIONS));
        let mut input = continuation
            .and_then(|value| value.get("input").and_then(|item| item.as_array()).cloned())
            .unwrap_or(payload.input);
        for result in tool_results {
            input.push(serde_json::json!({"type":"function_call_output","call_id":result.call_id,"output":result.output}));
        }
        let tools = ToolRouter.definitions();
        let body = serde_json::json!({
            "model": req.model,
            "instructions": payload.instructions.unwrap_or_else(|| CODEX_DEFAULT_INSTRUCTIONS.into()),
            "store": false,
            "stream": true,
            "input": input,
            "tools": tools,
        });
        emit(
            &progress,
            AgentEvent::Status("Generating with Codex".into()),
        );
        let response = self
            .client
            .post(&self.base)
            .bearer_auth(token)
            .header("chatgpt-account-id", native)
            .header("OpenAI-Beta", "responses=experimental")
            .header("originator", "codex_cli_rs")
            .header("Accept", "text/event-stream")
            .header("session-id", &req.session_id)
            .json(&body)
            .send()
            .await?;
        let response = ensure_success(response, "Codex").await?;
        let streamed = consume_responses_sse(response, "codex", progress.clone()).await?;
        if !streamed.tool_calls.is_empty() {
            let mut next_input = input;
            next_input.extend(streamed.function_items);
            return Ok(ProviderTurn {
                step: ProviderStep::ToolCalls(streamed.tool_calls),
                continuation: Some(serde_json::json!({"input":next_input})),
                events: vec![AgentEvent::Status(
                    "Codex requested an internal tool".into(),
                )],
            });
        }
        if streamed.text.is_empty() {
            return Err(anyhow!("Codex response had no output text"));
        }
        Ok(ProviderTurn {
            step: ProviderStep::Final(streamed.text),
            continuation: None,
            events: vec![AgentEvent::Status("Generating with Codex".into())],
        })
    }
}

struct AntigravityProvider {
    enabled: bool,
    base: String,
    default_model: Option<String>,
    user_agent: String,
    x_goog_api_client: String,
    auth: Arc<AuthManager>,
    client: Client,
}
impl AntigravityProvider {
    fn new(
        enabled: bool,
        base: String,
        default_model: Option<String>,
        user_agent: String,
        x_goog_api_client: String,
        auth: Arc<AuthManager>,
    ) -> Self {
        Self {
            enabled,
            base,
            default_model,
            user_agent,
            x_goog_api_client,
            auth,
            client: http_client(),
        }
    }

    fn request_builder(&self, token: &str, body: &serde_json::Value) -> reqwest::RequestBuilder {
        let metadata = serde_json::json!({"ideType":"ANTIGRAVITY"});
        self.client
            .post(&self.base)
            .bearer_auth(token)
            .header("Accept", "text/event-stream")
            .header("User-Agent", &self.user_agent)
            .header("X-Goog-Api-Client", &self.x_goog_api_client)
            .header("Client-Metadata", metadata.to_string())
            .json(body)
    }
}
#[async_trait]
impl Provider for AntigravityProvider {
    fn id(&self) -> &'static str {
        "antigravity"
    }
    fn models(&self) -> Vec<String> {
        // Conservative current fallback catalog. Authenticated upstream discovery remains
        // provider-owned; these IDs were cross-checked against active Antigravity routers.
        models_with_default(
            self.default_model.as_deref(),
            vec![
                "gemini-pro-agent".into(),
                "gemini-3.1-pro-low".into(),
                "gemini-3.7-flash-high".into(),
                "gemini-3.7-flash-medium".into(),
                "gemini-3.7-flash-low".into(),
                "claude-sonnet-4-6".into(),
                "claude-opus-4-6-thinking".into(),
            ],
        )
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn ready(&self) -> bool {
        self.enabled
    }
    async fn run(
        &self,
        req: ProviderRequest,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<ProviderResponse> {
        if !self.enabled {
            return Err(anyhow!("Antigravity provider is disabled"));
        }
        emit(
            &progress,
            AgentEvent::Status("Refreshing Antigravity session if needed".into()),
        );
        let account = req
            .account_id
            .as_deref()
            .ok_or_else(|| anyhow!("Antigravity account not selected"))?;
        let cred = self
            .auth
            .credential_for_use(account)
            .await?
            .ok_or_else(|| anyhow!("Antigravity credential missing"))?;
        let token = cred
            .access_token
            .as_deref()
            .ok_or_else(|| anyhow!("Antigravity access token missing"))?;
        let project = cred.project_id.as_deref().ok_or_else(|| {
            anyhow!("Antigravity project id missing; re-authentication/project discovery required")
        })?;
        let request_id = format!(
            "agent-{}-{}",
            chrono::Utc::now().timestamp_millis(),
            Uuid::new_v4().simple()
        );
        let body = antigravity_body(project, &req.model, &req.messages, &request_id);
        emit(
            &progress,
            AgentEvent::Status("Generating with Antigravity".into()),
        );
        let response = self.request_builder(token, &body).send().await?;
        let response = ensure_success(response, "Antigravity").await?;
        let answer = consume_antigravity_sse(response, progress.clone()).await?;
        if answer.is_empty() {
            return Err(anyhow!("Antigravity stream contained no assistant text"));
        }
        Ok(ProviderResponse {
            events: vec![AgentEvent::Status("Generating with Antigravity".into())],
            final_answer: answer,
        })
    }
}

struct CustomProvider {
    cfg: CustomProviderConfig,
    auth: Arc<AuthManager>,
    client: Client,
}
impl CustomProvider {
    fn new(cfg: CustomProviderConfig, auth: Arc<AuthManager>) -> Self {
        Self {
            cfg,
            auth,
            client: http_client(),
        }
    }
}
#[async_trait]
impl Provider for CustomProvider {
    fn id(&self) -> &'static str {
        "custom"
    }
    fn models(&self) -> Vec<String> {
        let fallback = if self.cfg.models.is_empty() && self.cfg.default_model.is_none() {
            vec!["default".into()]
        } else {
            self.cfg.models.clone()
        };
        models_with_default(self.cfg.default_model.as_deref(), fallback)
    }
    fn enabled(&self) -> bool {
        self.cfg.enabled
    }
    fn configured(&self) -> bool {
        self.cfg
            .base_url
            .as_deref()
            .is_some_and(|x| !x.trim().is_empty())
    }
    fn ready(&self) -> bool {
        self.enabled() && self.configured()
    }
    async fn run(
        &self,
        req: ProviderRequest,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<ProviderResponse> {
        if !self.cfg.enabled {
            return Err(anyhow!("custom provider is disabled"));
        }
        let base = self
            .cfg
            .base_url
            .clone()
            .ok_or_else(|| anyhow!("custom provider base_url is not configured"))?;
        emit(
            &progress,
            AgentEvent::Status("Sending request to custom provider".into()),
        );
        let endpoint = if self.cfg.protocol == "openai_chat_completions" {
            endpoint_with_suffix(&base, "/chat/completions")
        } else {
            endpoint_with_suffix(&base, "/responses")
        };
        let mut request = self.client.post(endpoint);
        for (name, value) in &self.cfg.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let selected_api_key = match req.account_id.as_deref() {
            Some(account) => self
                .auth
                .credential(account)?
                .and_then(|credential| credential.api_key),
            None => None,
        };
        let api_key = match selected_api_key {
            Some(key) if !key.trim().is_empty() => Some(key),
            _ => self.auth.provider_api_key("custom")?,
        };
        if let Some(key) = api_key {
            request = request.bearer_auth(key);
        }
        let body = if self.cfg.protocol == "openai_chat_completions" {
            serde_json::json!({
                "model": req.model,
                "messages": chat_messages(&req.messages),
                "stream": false,
            })
        } else {
            let payload = responses_payload(&req.messages, None);
            let mut body = serde_json::json!({
                "model": req.model,
                "input": payload.input,
                "stream": false,
            });
            if let Some(instructions) = payload.instructions {
                body["instructions"] = serde_json::Value::String(instructions);
            }
            body
        };
        let response = request.json(&body).send().await?;
        let response = ensure_success(response, "Custom provider").await?;
        let value: serde_json::Value = response.json().await?;
        let answer = if self.cfg.protocol == "openai_chat_completions" {
            extract_chat_content(&value)
        } else {
            extract_output_text(&value)
        }
        .filter(|answer| !answer.trim().is_empty())
        .ok_or_else(|| anyhow!("custom response contained no assistant text"))?;
        Ok(ProviderResponse {
            events: vec![AgentEvent::Status("Custom provider completed".into())],
            final_answer: answer,
        })
    }
}

fn endpoint_with_suffix(base: &str, suffix: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with(suffix) {
        base.to_owned()
    } else {
        format!("{base}{suffix}")
    }
}

async fn ensure_success(response: reqwest::Response, provider: &str) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let mut body = Vec::new();
    let mut truncated = false;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let remaining = MAX_UPSTREAM_ERROR_BYTES.saturating_sub(body.len());
        if chunk.len() > remaining {
            body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
        if body.len() == MAX_UPSTREAM_ERROR_BYTES {
            truncated = true;
            break;
        }
    }
    let summary = upstream_error_summary(&body, truncated);
    Err(anyhow!(
        "{provider} request failed with HTTP {status}: {summary}"
    ))
}

fn upstream_error_summary(body: &[u8], truncated: bool) -> String {
    let parsed = serde_json::from_slice::<serde_json::Value>(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(serde_json::Value::as_str)
                .or_else(|| value.get("message").and_then(serde_json::Value::as_str))
                .or_else(|| value.get("detail").and_then(serde_json::Value::as_str))
                .or_else(|| value.get("error").and_then(serde_json::Value::as_str))
        })
        .map(str::to_owned)
        .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned());
    let message = if message.trim().is_empty() {
        "upstream returned an empty error body".to_owned()
    } else {
        message.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let mut safe = redact_text(&message).chars().take(1200).collect::<String>();
    if truncated || message.chars().count() > 1200 {
        safe.push('…');
    }
    safe
}

fn extract_chat_content(value: &serde_json::Value) -> Option<String> {
    let content = value.pointer("/choices/0/message/content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_owned());
    }
    let mut output = String::new();
    for part in content.as_array()? {
        if let Some(text) = part.get("text").and_then(serde_json::Value::as_str) {
            output.push_str(text);
        }
    }
    (!output.is_empty()).then_some(output)
}

struct StreamedResponses {
    text: String,
    tool_calls: Vec<ToolCall>,
    function_items: Vec<serde_json::Value>,
}
async fn consume_responses_sse(
    response: reqwest::Response,
    provider: &str,
    progress: Option<mpsc::UnboundedSender<AgentEvent>>,
) -> Result<StreamedResponses> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut items = Vec::new();
    let mut stream_error = None;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        emit(
            &progress,
            AgentEvent::StreamChunk {
                provider: provider.into(),
                bytes: chunk.len(),
            },
        );
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim_end_matches('\r').to_owned();
            buffer.drain(..=pos);
            let Some(data) = line.strip_prefix("data:").map(str::trim_start) else {
                continue;
            };
            if data == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };

            match value.get("type").and_then(|value| value.as_str()) {
                Some("response.output_text.delta") => {
                    if let Some(delta) = value.get("delta").and_then(|value| value.as_str()) {
                        text.push_str(delta);
                    }
                }
                Some("response.output_item.done") => {
                    let Some(item) = value.get("item") else {
                        continue;
                    };
                    if item.get("type").and_then(|value| value.as_str()) != Some("function_call") {
                        continue;
                    }
                    let (Some(call_id), Some(name)) = (
                        item.get("call_id").and_then(|value| value.as_str()),
                        item.get("name").and_then(|value| value.as_str()),
                    ) else {
                        continue;
                    };
                    let arguments = item
                        .get("arguments")
                        .and_then(|value| value.as_str())
                        .and_then(|value| serde_json::from_str(value).ok())
                        .unwrap_or_else(|| serde_json::json!({}));
                    calls.push(ToolCall {
                        call_id: call_id.into(),
                        name: name.into(),
                        arguments,
                    });
                    items.push(item.clone());
                }
                Some("response.completed") if text.is_empty() => {
                    if let Some(response) = value.get("response") {
                        if let Some(output) = extract_output_text(response) {
                            text = output;
                        }
                    }
                }
                Some("error") => {
                    stream_error = Some(
                        value
                            .pointer("/error/message")
                            .or_else(|| value.get("message"))
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                            .unwrap_or_else(|| "Responses stream reported an error".into()),
                    );
                }
                Some("response.failed") => {
                    stream_error = Some(
                        value
                            .pointer("/response/error/message")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                            .unwrap_or_else(|| "Responses stream failed".into()),
                    );
                }
                _ => {}
            }
        }
    }

    if let Some(error) = stream_error {
        return Err(anyhow!("{provider} stream failed: {}", redact_text(&error)));
    }

    Ok(StreamedResponses {
        text,
        tool_calls: calls,
        function_items: items,
    })
}

async fn consume_antigravity_sse(
    response: reqwest::Response,
    progress: Option<mpsc::UnboundedSender<AgentEvent>>,
) -> Result<String> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut output = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        emit(
            &progress,
            AgentEvent::StreamChunk {
                provider: "antigravity".into(),
                bytes: chunk.len(),
            },
        );
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim_end_matches('\r').to_owned();
            buffer.drain(..=pos);
            let Some(data) = line.strip_prefix("data:").map(str::trim_start) else {
                continue;
            };
            if data == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            for pointer in [
                "/response/candidates/0/content/parts",
                "/candidates/0/content/parts",
            ] {
                let Some(parts) = value.pointer(pointer).and_then(|value| value.as_array()) else {
                    continue;
                };
                for part in parts {
                    if part.get("thought").and_then(serde_json::Value::as_bool) == Some(true) {
                        continue;
                    }
                    if let Some(text) = part.get("text").and_then(|value| value.as_str()) {
                        output.push_str(text);
                    }
                }
            }
        }
    }

    Ok(output)
}

fn extract_output_text(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.get("output_text").and_then(|x| x.as_str()) {
        return Some(s.to_owned());
    }
    let mut out = String::new();
    for item in v.get("output")?.as_array()? {
        for content in item
            .get("content")
            .and_then(|x| x.as_array())
            .into_iter()
            .flatten()
        {
            if content.get("type").and_then(|x| x.as_str()) == Some("output_text") {
                if let Some(text) = content.get("text").and_then(|x| x.as_str()) {
                    out.push_str(text);
                }
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;
    use axum::{http::StatusCode, routing::post, Json, Router};

    fn message(role: &str, content: &str) -> MessageRecord {
        MessageRecord {
            role: role.to_owned(),
            content: content.to_owned(),
            created_at: "now".into(),
        }
    }

    fn request(messages: Vec<MessageRecord>) -> ProviderRequest {
        ProviderRequest {
            session_id: "session-a".into(),
            account_id: None,
            model: "model-a".into(),
            messages,
        }
    }

    fn test_auth() -> (Arc<AuthManager>, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open_memory().unwrap());
        let auth = Arc::new(AuthManager::new(storage, directory.path().into()));
        (auth, directory)
    }

    #[test]
    fn configured_default_is_always_the_first_model() {
        assert_eq!(
            models_with_default(
                Some("model-z"),
                vec!["model-a".into(), "model-z".into(), "model-b".into()],
            ),
            vec!["model-z", "model-a", "model-b"]
        );
    }

    #[test]
    fn antigravity_request_has_the_headers_required_by_cloud_code_assist() {
        let (auth, _directory) = test_auth();
        let provider = AntigravityProvider::new(
            true,
            "https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse"
                .into(),
            Some("gemini-pro-agent".into()),
            "antigravity/hub/2.2.1 darwin/arm64".into(),
            "gl-node/22.21.1".into(),
            auth,
        );
        let body = antigravity_body(
            "project-a",
            "gemini-pro-agent",
            &[message("user", "hello")],
            "agent-test",
        );
        let request = provider
            .request_builder("access-token", &body)
            .build()
            .unwrap();
        assert_eq!(request.headers()["accept"], "text/event-stream");
        assert_eq!(
            request.headers()["user-agent"],
            "antigravity/hub/2.2.1 darwin/arm64"
        );
        assert_eq!(request.headers()["x-goog-api-client"], "gl-node/22.21.1");
        assert_eq!(
            request.headers()["client-metadata"],
            r#"{"ideType":"ANTIGRAVITY"}"#
        );
        let wire_body: serde_json::Value =
            serde_json::from_slice(request.body().and_then(reqwest::Body::as_bytes).unwrap())
                .unwrap();
        assert_eq!(wire_body["userAgent"], "antigravity");
        assert_eq!(wire_body["requestId"], "agent-test");
    }

    #[tokio::test]
    async fn custom_responses_request_reaches_the_wire_with_typed_input() {
        let (captured_tx, mut captured_rx) = mpsc::unbounded_channel();
        let app = Router::new().route(
            "/v1/responses",
            post(move |Json(body): Json<serde_json::Value>| {
                let captured_tx = captured_tx.clone();
                async move {
                    captured_tx.send(body).unwrap();
                    Json(serde_json::json!({"output_text":"ok"}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (auth, _directory) = test_auth();
        let config = CustomProviderConfig {
            enabled: true,
            base_url: Some(format!("http://{address}/v1")),
            protocol: "openai_responses".into(),
            ..Default::default()
        };
        let provider = CustomProvider::new(config, auth);
        let response = provider
            .run(
                request(vec![message("user", "one"), message("user", "two")]),
                None,
            )
            .await
            .unwrap();
        assert_eq!(response.final_answer, "ok");
        let body = captured_rx.recv().await.unwrap();
        assert_eq!(body["input"].as_array().unwrap().len(), 1);
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][0]["content"][0]["text"], "one\n\ntwo");
        server.abort();
    }

    #[tokio::test]
    async fn custom_provider_surfaces_a_bounded_upstream_error_message() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error":{"message":"model rejected the payload"}
                    })),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (auth, _directory) = test_auth();
        let config = CustomProviderConfig {
            enabled: true,
            base_url: Some(format!("http://{address}/v1")),
            ..Default::default()
        };
        let provider = CustomProvider::new(config, auth);
        let error = provider
            .run(request(vec![message("user", "hello")]), None)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("HTTP 400"));
        assert!(error.contains("model rejected the payload"));
        server.abort();
    }

    #[test]
    fn upstream_error_summary_redacts_secret_material() {
        let summary =
            upstream_error_summary(br#"{"error":{"message":"api_key=very-secret"}}"#, false);
        assert!(!summary.contains("very-secret"));
        assert!(summary.contains("<redacted>"));
    }
}
