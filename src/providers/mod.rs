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

use crate::{
    auth::AuthManager,
    config::{AppConfig, CustomProviderConfig},
    storage::MessageRecord,
    tools::{ToolCall, ToolResult, ToolRouter},
};

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
    p.insert(
        "antigravity".into(),
        Arc::new(AntigravityProvider::new(
            config.providers.antigravity.enabled,
            config
                .providers
                .antigravity
                .oauth_client_id
                .as_deref()
                .map(str::trim)
                .is_some_and(|x| !x.is_empty()),
            config.providers.antigravity.base_url.clone(),
            config.providers.antigravity.default_model.clone(),
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
        let mut models = vec!["gpt-5.6-sol".into(), "gpt-5.5".into()];
        if let Some(v) = &self.default_model {
            if !models.contains(v) {
                models.insert(0, v.clone());
            }
        }
        models
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
        let mut input = continuation.and_then(|v|v.get("input").and_then(|x|x.as_array()).cloned()).unwrap_or_else(||req.messages.iter().map(|m| serde_json::json!({
            "role": m.role,
            "content": [{"type": if m.role == "assistant" { "output_text" } else { "input_text" }, "text": m.content}]
        })).collect::<Vec<_>>());
        for result in tool_results {
            input.push(serde_json::json!({"type":"function_call_output","call_id":result.call_id,"output":result.output}));
        }
        let tools = ToolRouter.definitions();
        let body = serde_json::json!({"model": req.model, "store": false, "stream": true, "input": input, "tools":tools});
        emit(
            &progress,
            AgentEvent::Status("Generating with Codex".into()),
        );
        let response = self
            .client
            .post(&self.base)
            .bearer_auth(token)
            .header("chatgpt-account-id", native)
            .header("originator", "xiao")
            .header("session-id", &req.session_id)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
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
    configured: bool,
    base: String,
    default_model: Option<String>,
    auth: Arc<AuthManager>,
    client: Client,
}
impl AntigravityProvider {
    fn new(
        enabled: bool,
        configured: bool,
        base: Option<String>,
        default_model: Option<String>,
        auth: Arc<AuthManager>,
    ) -> Self {
        Self { enabled, configured, base: base.unwrap_or_else(|| "https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse".into()), default_model, auth, client: http_client() }
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
        let mut models = vec![
            "gemini-pro-agent".into(),
            "gemini-3.1-pro-low".into(),
            "gemini-3.7-flash-high".into(),
            "gemini-3.7-flash-medium".into(),
            "gemini-3.7-flash-low".into(),
            "claude-sonnet-4-6".into(),
            "claude-opus-4-6-thinking".into(),
        ];
        if let Some(v) = &self.default_model {
            if !models.contains(v) {
                models.insert(0, v.clone());
            }
        }
        models
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn configured(&self) -> bool {
        self.configured
    }
    fn ready(&self) -> bool {
        self.enabled && self.configured
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
        let contents = req.messages.iter().map(|m| serde_json::json!({"role": if m.role == "assistant" { "model" } else { "user" }, "parts": [{"text": m.content}]})).collect::<Vec<_>>();
        let metadata = serde_json::json!({"ideType":"ANTIGRAVITY","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"});
        let body = serde_json::json!({"project": project, "model": req.model, "request": {"contents": contents}, "requestType": "agent"});
        emit(
            &progress,
            AgentEvent::Status("Generating with Antigravity".into()),
        );
        let response = self
            .client
            .post(&self.base)
            .bearer_auth(token)
            .header("User-Agent", format!("xiao/{}", crate::VERSION))
            .header("Client-Metadata", metadata.to_string())
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
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
        if self.cfg.models.is_empty() {
            vec![self
                .cfg
                .default_model
                .clone()
                .unwrap_or_else(|| "default".into())]
        } else {
            self.cfg.models.clone()
        }
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
            format!("{}/chat/completions", base.trim_end_matches('/'))
        } else {
            format!("{}/responses", base.trim_end_matches('/'))
        };
        let mut request = self.client.post(endpoint);
        for (name, value) in &self.cfg.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(account) = req.account_id.as_deref() {
            if let Some(cred) = self.auth.credential(account)? {
                if let Some(key) = cred.api_key {
                    request = request.bearer_auth(key);
                }
            }
        }
        let body = if self.cfg.protocol == "openai_chat_completions" {
            serde_json::json!({"model": req.model, "messages": req.messages.iter().map(|m| serde_json::json!({"role":m.role,"content":m.content})).collect::<Vec<_>>(), "stream": false})
        } else {
            serde_json::json!({"model": req.model, "input": req.messages.iter().map(|m| serde_json::json!({"role":m.role,"content":m.content})).collect::<Vec<_>>(), "stream": false})
        };
        let value: serde_json::Value = request
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let answer = if self.cfg.protocol == "openai_chat_completions" {
            value
                .pointer("/choices/0/message/content")
                .and_then(|x| x.as_str())
                .map(str::to_owned)
        } else {
            extract_output_text(&value)
        }
        .ok_or_else(|| anyhow!("custom response contained no assistant text"))?;
        Ok(ProviderResponse {
            events: vec![AgentEvent::Status("Custom provider completed".into())],
            final_answer: answer,
        })
    }
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
            let Some(data) = line.strip_prefix("data: ") else {
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
                _ => {}
            }
        }
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
            let Some(data) = line.strip_prefix("data: ") else {
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
