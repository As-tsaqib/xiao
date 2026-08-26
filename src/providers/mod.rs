mod payload;
pub mod profiles;

pub use profiles::{
    secret_headers_ref_for, CustomProfileEdit, CustomProfileService, ProviderProfileStore,
};

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    attachments::{
        NormalizedFile, NormalizedImage, PdfFallbackCapabilities, PdfFallbackProvider,
        PdfFallbackRequest,
    },
    auth::AuthManager,
    config::{AppConfig, CustomProviderConfig},
    security::redact::redact_text,
    storage::MessageRecord,
    tools::{ToolCall, ToolResult, ToolSpec},
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

/// The normalized agent protocol exposed by a provider/model pair. The agent
/// runtime uses this explicit value instead of treating a missing provider
/// override as permission to silently remove all tools.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolProtocol {
    Native,
    StructuredJsonFallback,
    ChatOnly,
}

impl ToolProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::StructuredJsonFallback => "structured_json_fallback",
            Self::ChatOnly => "chat_only",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Supported,
    Unsupported,
    Unknown,
}

impl CapabilityState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityOverride {
    Auto,
    ForceSupported,
    ForceUnsupported,
}

impl CapabilityOverride {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::ForceSupported => "force_supported",
            Self::ForceUnsupported => "force_unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityEvidenceSource {
    ProbeSuccess,
    RuntimeSuccess,
    ProviderExplicitUnsupported,
    OwnerOverride,
    Migration,
}

impl CapabilityEvidenceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProbeSuccess => "probe_success",
            Self::RuntimeSuccess => "runtime_success",
            Self::ProviderExplicitUnsupported => "provider_explicit_unsupported",
            Self::OwnerOverride => "owner_override",
            Self::Migration => "migration",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomCapabilityProbe {
    pub capabilities: ProviderCapabilities,
    pub native_tools: CapabilityState,
    pub structured_output: CapabilityState,
    pub continuation: CapabilityState,
    pub vision: CapabilityState,
    pub file_input: CapabilityState,
}

/// Canonical conversion from probe tri-state to runtime record. Ensures:
///
/// Supported   -> runtime bool true
/// Unsupported -> runtime bool false
/// Unknown     -> runtime bool false, metadata remains Unknown
pub fn profile_model_from_probe(
    profile_id: &str,
    model_id: &str,
    probe: &CustomCapabilityProbe,
    probed_at: &str,
) -> crate::storage::ProviderProfileModelRecord {
    crate::storage::ProviderProfileModelRecord {
        profile_id: profile_id.to_owned(),
        model_id: model_id.to_owned(),
        text_capable: probe.capabilities.text,
        vision_capable: matches!(probe.vision, CapabilityState::Supported),
        file_input_capable: matches!(probe.file_input, CapabilityState::Supported),
        native_tools: matches!(probe.native_tools, CapabilityState::Supported),
        structured_output: matches!(probe.structured_output, CapabilityState::Supported),
        continuation: matches!(probe.continuation, CapabilityState::Supported),
        native_tools_state: probe.native_tools.as_str().into(),
        structured_output_state: probe.structured_output.as_str().into(),
        continuation_state: probe.continuation.as_str().into(),
        vision_state: probe.vision.as_str().into(),
        file_input_state: probe.file_input.as_str().into(),
        model_discovery: probe.capabilities.model_discovery,
        tool_protocol: probe.capabilities.tool_protocol.as_str().into(),
        evidence: probe.capabilities.evidence.clone(),
        probe_status: "completed".into(),
        probe_version: 1,
        probed_at: probed_at.to_owned(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub text: bool,
    pub vision: bool,
    pub file_input: bool,
    pub native_tools: bool,
    pub tool_protocol: ToolProtocol,
    pub model_discovery: bool,
    pub structured_output: bool,
    pub continuation: bool,
    /// Concise inspectable evidence, never provider reasoning.
    pub evidence: String,
}

impl ProviderCapabilities {
    pub fn native(evidence: impl Into<String>) -> Self {
        Self {
            text: true,
            vision: false,
            file_input: false,
            native_tools: true,
            tool_protocol: ToolProtocol::Native,
            model_discovery: false,
            structured_output: true,
            continuation: true,
            evidence: evidence.into(),
        }
    }

    pub fn chat_only(evidence: impl Into<String>) -> Self {
        Self {
            text: true,
            vision: false,
            file_input: false,
            native_tools: false,
            tool_protocol: ToolProtocol::ChatOnly,
            model_discovery: false,
            structured_output: false,
            continuation: false,
            evidence: evidence.into(),
        }
    }

    pub fn is_agent_capable(&self) -> bool {
        self.tool_protocol != ToolProtocol::ChatOnly
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum AgentEvent {
    GenerationStarted,
    Status(String),
    ToolStarted(String),
    ToolCompleted {
        tool: String,
        summary: String,
    },
    ToolStartedWithId {
        tool: String,
        call_id: String,
    },
    ToolCompletedWithId {
        tool: String,
        call_id: String,
        summary: String,
    },
    /// Observable state for a typed ASK decision.  This never includes tool
    /// arguments, credentials, model reasoning, or a reusable grant.
    ApprovalRequested {
        approval_id: String,
        tool: String,
        call_id: String,
        summary: String,
    },
    StreamChunk {
        provider: String,
        bytes: usize,
    },
    TextDelta(String),
    GenerationCompleted,
    GenerationFailed(String),
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub session_id: String,
    pub account_id: Option<String>,
    pub model: String,
    pub messages: Vec<MessageRecord>,
    /// Canonical Xiao tool specifications selected by the agent runtime. A
    /// provider may translate these, but never discovers or executes tools.
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    /// Runtime-validated normalized images. Telegram wire details never enter
    /// provider adapters, and providers may serialize these only when their
    /// selected model explicitly declares vision capability.
    #[serde(default)]
    pub images: Vec<NormalizedImage>,
    /// Runtime-validated bounded files. Adapters serialize these only for an
    /// explicitly file-input-capable selected model.
    #[serde(default)]
    pub files: Vec<NormalizedFile>,
    #[serde(default = "default_true")]
    pub streaming: bool,
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
    fn capabilities(&self, _model: &str) -> ProviderCapabilities {
        ProviderCapabilities::chat_only("provider did not declare an agent tool protocol")
    }
    fn capabilities_for(
        &self,
        model: &str,
        _account_or_profile_id: Option<&str>,
    ) -> ProviderCapabilities {
        self.capabilities(model)
    }
    fn pdf_fallback_capabilities(
        &self,
        model: &str,
        account_or_profile_id: Option<&str>,
    ) -> PdfFallbackCapabilities {
        let capabilities = self.capabilities_for(model, account_or_profile_id);
        PdfFallbackCapabilities {
            file_input: capabilities.file_input,
            vision: capabilities.vision,
        }
    }
    async fn pdf_file_input(
        &self,
        request: &PdfFallbackRequest,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<String> {
        if cancellation.is_cancelled() {
            return Err(anyhow!("attachment provider file input cancelled"));
        }
        let capabilities =
            self.capabilities_for(&request.model, request.account_or_profile_id.as_deref());
        if !capabilities.file_input {
            return Err(anyhow!(
                "selected provider/model does not declare file-input capability"
            ));
        }
        let provider_request = ProviderRequest {
            session_id: request.session_id.clone(),
            account_id: request.account_or_profile_id.clone(),
            model: request.model.clone(),
            messages: vec![MessageRecord {
                role: "user".into(),
                content: request.prompt.clone(),
                created_at: chrono::Utc::now().to_rfc3339(),
            }],
            tools: Vec::new(),
            images: Vec::new(),
            files: vec![NormalizedFile {
                attachment_id: request.attachment_id.clone(),
                filename: request.original_name.clone(),
                mime_type: "application/pdf".into(),
                bytes: request.pdf.clone(),
                caption: request.prompt.clone(),
            }],
            streaming: false,
        };
        tokio::select! {
            _ = cancellation.cancelled() => Err(anyhow!("attachment provider file input cancelled")),
            result = self.generate_text(provider_request) => result,
        }
    }
    async fn pdf_vision(
        &self,
        request: &PdfFallbackRequest,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<String> {
        if request.rendered_pages.is_empty() {
            return Err(anyhow!("PDF vision adapter received no rendered pages"));
        }
        let images = request
            .rendered_pages
            .iter()
            .take(4)
            .map(|page| NormalizedImage {
                attachment_id: format!("{}:page:{}", request.attachment_id, page.page_no),
                mime_type: page.mime_type.clone(),
                bytes: page.bytes.clone(),
                width: page.width,
                height: page.height,
                caption: request.prompt.clone(),
            })
            .collect();
        let request = ProviderRequest {
            session_id: request.attachment_id.clone(),
            account_id: request.account_or_profile_id.clone(),
            model: request.model.clone(),
            messages: vec![MessageRecord {
                role: "user".into(),
                content: request.prompt.clone(),
                created_at: chrono::Utc::now().to_rfc3339(),
            }],
            tools: Vec::new(),
            images,
            files: Vec::new(),
            streaming: false,
        };
        tokio::select! {
            _ = cancellation.cancelled() => Err(anyhow!("attachment provider vision cancelled")),
            result = self.generate_text(request) => result,
        }
    }
    fn models_for(&self, _account_or_profile_id: Option<&str>) -> Vec<String> {
        self.models()
    }
    /// Whether this adapter supports the ordinary no-tool generation path
    /// used by Xiao's internal schema evaluator. Test/fake providers opt in
    /// explicitly so semantic side calls cannot accidentally consume their
    /// scripted agent-turn queues.
    fn supports_semantic_evaluation(&self, _model: &str) -> bool {
        false
    }
    fn supports_semantic_evaluation_for(
        &self,
        model: &str,
        _account_or_profile_id: Option<&str>,
    ) -> bool {
        self.supports_semantic_evaluation(model)
    }
    async fn generate_text(&self, mut req: ProviderRequest) -> Result<String> {
        req.tools.clear();
        Ok(self.run(req, None).await?.final_answer)
    }
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

pub struct ProviderPdfFallback<'a> {
    provider: &'a dyn Provider,
}

impl<'a> ProviderPdfFallback<'a> {
    pub fn new(provider: &'a dyn Provider) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl PdfFallbackProvider for ProviderPdfFallback<'_> {
    async fn file_input(
        &self,
        request: &PdfFallbackRequest,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<String> {
        self.provider.pdf_file_input(request, cancellation).await
    }

    async fn vision(
        &self,
        request: &PdfFallbackRequest,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<String> {
        self.provider.pdf_vision(request, cancellation).await
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
    pub fn models_for(&self, id: &str, account_or_profile_id: Option<&str>) -> Result<Vec<String>> {
        Ok(self.get(id)?.models_for(account_or_profile_id))
    }
    pub fn capabilities(&self, id: &str, model: &str) -> Result<ProviderCapabilities> {
        Ok(self.get(id)?.capabilities(model))
    }
    pub fn capabilities_for(
        &self,
        id: &str,
        model: &str,
        account_or_profile_id: Option<&str>,
    ) -> Result<ProviderCapabilities> {
        Ok(self.get(id)?.capabilities_for(model, account_or_profile_id))
    }
    pub fn preferred_model(&self, id: &str) -> Result<String> {
        self.models(id)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("provider {id} has no usable models"))
    }
    pub fn resolve_model(&self, id: &str, selected: &str) -> Result<String> {
        self.resolve_model_for(id, selected, None)
    }
    pub fn resolve_model_for(
        &self,
        id: &str,
        selected: &str,
        account_or_profile_id: Option<&str>,
    ) -> Result<String> {
        let models = self.models_for(id, account_or_profile_id)?;
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

    pub fn from_single(id: &str, provider: Arc<dyn Provider>, auth: Arc<AuthManager>) -> Self {
        let mut providers = HashMap::new();
        providers.insert(id.to_owned(), provider);
        Self {
            providers: RwLock::new(providers),
            auth,
        }
    }

    pub fn from_test(providers: Vec<(&str, Arc<dyn Provider>)>, auth: Arc<AuthManager>) -> Self {
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
    // v0.2.8 deliberately exposes exactly one active provider family. The
    // legacy adapters remain in this module only so old serialized sessions
    // and archived history can still be decoded during migration; they are
    // not registered, reachable, or advertised by normal runtime surfaces.
    let mut p: HashMap<String, Arc<dyn Provider>> = HashMap::new();
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

fn codex_model_supports_vision(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model.starts_with("gpt-5")
}

fn antigravity_model_supports_vision(model: &str) -> bool {
    model.trim().to_ascii_lowercase().starts_with("gemini-")
}

fn capabilities_from_record(
    record: crate::storage::ProviderCapabilityRecord,
) -> ProviderCapabilities {
    if !probe_is_completed(
        &record.probe_status,
        record.probe_version,
        &record.probed_at,
    ) {
        return ProviderCapabilities {
            model_discovery: true,
            ..ProviderCapabilities::chat_only(format!(
                "model capability probe is {}",
                normalized_probe_status(&record.probe_status)
            ))
        };
    }
    let tool_protocol = match record.tool_protocol.as_str() {
        "native" => ToolProtocol::Native,
        "structured_json_fallback" | "structured_json" => ToolProtocol::StructuredJsonFallback,
        "chat_only" => ToolProtocol::ChatOnly,
        _ => ToolProtocol::ChatOnly,
    };
    let tool_protocol = match tool_protocol {
        ToolProtocol::Native if record.native_tool_calls && record.continuation => {
            ToolProtocol::Native
        }
        ToolProtocol::StructuredJsonFallback if record.structured_output && record.continuation => {
            ToolProtocol::StructuredJsonFallback
        }
        _ => ToolProtocol::ChatOnly,
    };
    ProviderCapabilities {
        text: true,
        vision: false,
        file_input: false,
        native_tools: tool_protocol == ToolProtocol::Native,
        tool_protocol,
        model_discovery: true,
        structured_output: record.structured_output,
        continuation: record.continuation,
        evidence: record.evidence,
    }
}

fn normalized_probe_status(status: &str) -> &str {
    match status {
        "completed" | "indeterminate" | "unprobed" => status,
        _ => "indeterminate",
    }
}

fn probe_is_completed(status: &str, version: u32, probed_at: &str) -> bool {
    status == "completed" && version > 0 && !probed_at.trim().is_empty()
}

fn profile_capabilities_from_record(
    record: crate::storage::ProviderProfileModelRecord,
) -> ProviderCapabilities {
    let completed = probe_is_completed(
        &record.probe_status,
        record.probe_version,
        &record.probed_at,
    );
    if !completed {
        return ProviderCapabilities {
            model_discovery: record.model_discovery,
            ..ProviderCapabilities::chat_only(format!(
                "model capability probe is {}",
                normalized_probe_status(&record.probe_status)
            ))
        };
    }

    let protocol = match record.tool_protocol.as_str() {
        "native"
            if record.native_tools_state == "supported"
                && record.continuation_state == "supported" =>
        {
            ToolProtocol::Native
        }
        "structured_json_fallback" | "structured_json"
            if record.structured_output_state == "supported"
                && record.continuation_state == "supported" =>
        {
            ToolProtocol::StructuredJsonFallback
        }
        _ => ToolProtocol::ChatOnly,
    };
    ProviderCapabilities {
        text: record.text_capable,
        // Unknown is an optimistic real-request state. Only explicit
        // Unsupported evidence closes the route.
        vision: record.vision_state != "unsupported",
        file_input: record.file_input_state != "unsupported",
        native_tools: protocol == ToolProtocol::Native,
        tool_protocol: protocol,
        model_discovery: record.model_discovery,
        structured_output: record.structured_output_state == "supported",
        continuation: record.continuation_state == "supported",
        evidence: record.evidence,
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
    fn capabilities(&self, model: &str) -> ProviderCapabilities {
        ProviderCapabilities {
            vision: codex_model_supports_vision(model),
            ..ProviderCapabilities::native(
                "OpenAI Responses native function calls and continuation; vision is enabled only for Xiao's verified GPT-5 family",
            )
        }
    }
    fn supports_semantic_evaluation(&self, _model: &str) -> bool {
        true
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
        if !req.images.is_empty() && !self.capabilities(&req.model).vision {
            return Err(anyhow!(
                "selected Codex model does not declare vision capability"
            ));
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
        let mut payload = responses_payload(&req.messages, Some(CODEX_DEFAULT_INSTRUCTIONS));
        append_responses_images(&mut payload.input, &req.images)?;
        append_responses_files(&mut payload.input, &req.files)?;
        let mut input = continuation
            .and_then(|value| value.get("input").and_then(|item| item.as_array()).cloned())
            .unwrap_or(payload.input);
        for result in tool_results {
            input.push(serde_json::json!({"type":"function_call_output","call_id":result.call_id,"output":result.output}));
        }
        let tools = responses_tool_specs(&req.tools);
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
        let streamed = consume_responses_sse(response, "codex", progress.clone(), true).await?;
        if !streamed.tool_calls.is_empty() {
            let mut next_input = input;
            next_input.extend(streamed.function_items);
            return Ok(ProviderTurn {
                step: ProviderStep::ToolCalls(streamed.tool_calls),
                continuation: Some(serde_json::json!({ "input": next_input })),
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

fn responses_tool_specs(specs: &[ToolSpec]) -> Vec<serde_json::Value> {
    specs
        .iter()
        .map(|spec| {
            serde_json::json!({
                "type": "function",
                "name": spec.name,
                "description": spec.description,
                "parameters": spec.parameters,
                "strict": true,
            })
        })
        .collect()
}

fn image_data_url(image: &NormalizedImage) -> Result<String> {
    if !matches!(
        image.mime_type.as_str(),
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) || image.bytes.is_empty()
        || image.bytes.len() > 20 * 1024 * 1024
    {
        return Err(anyhow!("normalized image violates provider input bounds"));
    }
    Ok(format!(
        "data:{};base64,{}",
        image.mime_type,
        STANDARD.encode(&image.bytes)
    ))
}

fn file_data_url(file: &NormalizedFile) -> Result<String> {
    if file.mime_type != "application/pdf"
        || file.bytes.is_empty()
        || file.bytes.len() > 25 * 1024 * 1024
        || file.filename.trim().is_empty()
    {
        return Err(anyhow!("normalized file violates provider input bounds"));
    }
    Ok(format!(
        "data:{};base64,{}",
        file.mime_type,
        STANDARD.encode(&file.bytes)
    ))
}

fn append_responses_images(
    input: &mut Vec<serde_json::Value>,
    images: &[NormalizedImage],
) -> Result<()> {
    for image in images.iter().take(4) {
        input.push(serde_json::json!({
            "type":"message",
            "role":"user",
            "content":[
                {"type":"input_text","text":image.caption},
                {"type":"input_image","image_url":image_data_url(image)?,"detail":"auto"}
            ]
        }));
    }
    Ok(())
}

fn append_responses_files(
    input: &mut Vec<serde_json::Value>,
    files: &[NormalizedFile],
) -> Result<()> {
    for file in files.iter().take(2) {
        input.push(serde_json::json!({
            "type": "message",
            "role": "user",
            "content": [
                {"type": "input_text", "text": file.caption},
                {"type": "input_file", "filename": file.filename, "file_data": file_data_url(file)?}
            ]
        }));
    }
    Ok(())
}

fn append_chat_images(messages: &mut Vec<serde_json::Value>, images: &[NormalizedImage]) {
    for image in images.iter().take(4) {
        if let Ok(url) = image_data_url(image) {
            messages.push(serde_json::json!({
                "role":"user",
                "content":[
                    {"type":"text","text":image.caption},
                    {"type":"image_url","image_url":{"url":url,"detail":"auto"}}
                ]
            }));
        }
    }
}

fn append_chat_files(
    messages: &mut Vec<serde_json::Value>,
    files: &[NormalizedFile],
) -> Result<()> {
    for file in files.iter().take(2) {
        messages.push(serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": file.caption},
                {"type": "file", "file": {"filename": file.filename, "file_data": file_data_url(file)?}}
            ]
        }));
    }
    Ok(())
}

fn append_antigravity_images(
    body: &mut serde_json::Value,
    images: &[NormalizedImage],
) -> Result<()> {
    let Some(contents) = body
        .pointer_mut("/request/contents")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Err(anyhow!("Antigravity request has no contents array"));
    };
    for image in images.iter().take(4) {
        image_data_url(image)?;
        contents.push(serde_json::json!({
            "role":"user",
            "parts":[
                {"text":image.caption},
                {"inlineData":{"mimeType":image.mime_type,"data":STANDARD.encode(&image.bytes)}}
            ]
        }));
    }
    Ok(())
}

fn append_antigravity_files(body: &mut serde_json::Value, files: &[NormalizedFile]) -> Result<()> {
    let Some(contents) = body
        .pointer_mut("/request/contents")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Err(anyhow!("Antigravity request has no contents array"));
    };
    for file in files.iter().take(2) {
        let data_url = file_data_url(file)?;
        let encoded = data_url
            .split_once(',')
            .map(|(_, value)| value)
            .unwrap_or_default();
        contents.push(serde_json::json!({
            "role": "user",
            "parts": [
                {"text": file.caption},
                {"inlineData": {"mimeType": file.mime_type, "data": encoded}}
            ]
        }));
    }
    Ok(())
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
    fn capabilities(&self, model: &str) -> ProviderCapabilities {
        ProviderCapabilities {
            vision: antigravity_model_supports_vision(model),
            ..ProviderCapabilities::native(
                "Cloud Code Assist functionDeclarations continuation; inlineData vision is enabled only for verified Gemini families",
            )
        }
    }
    fn supports_semantic_evaluation(&self, _model: &str) -> bool {
        true
    }
    async fn run(
        &self,
        req: ProviderRequest,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<ProviderResponse> {
        let turn = self.run_turn(req, None, Vec::new(), progress).await?;
        match turn.step {
            ProviderStep::Final(final_answer) => Ok(ProviderResponse {
                events: turn.events,
                final_answer,
            }),
            ProviderStep::ToolCalls(_) => Err(anyhow!(
                "Antigravity requested a tool outside the agent loop"
            )),
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
            return Err(anyhow!("Antigravity provider is disabled"));
        }
        if !req.images.is_empty() && !self.capabilities(&req.model).vision {
            return Err(anyhow!(
                "selected Antigravity model does not declare vision capability"
            ));
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
        let mut body = antigravity_body(project, &req.model, &req.messages, &request_id);
        append_antigravity_images(&mut body, &req.images)?;
        append_antigravity_files(&mut body, &req.files)?;
        let mut contents = continuation
            .as_ref()
            .and_then(|value| value.get("contents"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .or_else(|| {
                body.pointer("/request/contents")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
            })
            .unwrap_or_default();
        if !tool_results.is_empty() {
            contents.push(serde_json::json!({
                "role": "user",
                "parts": tool_results.into_iter().map(|result| serde_json::json!({
                    "functionResponse": {
                        "name": result.name,
                        "response": {
                            "output": result.output,
                            "is_error": result.is_error,
                        }
                    }
                })).collect::<Vec<_>>()
            }));
        }
        body["request"]["contents"] = serde_json::Value::Array(contents.clone());
        if !req.tools.is_empty() {
            body["request"]["tools"] = serde_json::json!([{
                "functionDeclarations": antigravity_tool_specs(&req.tools)
            }]);
        }
        emit(
            &progress,
            AgentEvent::Status("Generating with Antigravity".into()),
        );
        let response = self.request_builder(token, &body).send().await?;
        let response = ensure_success(response, "Antigravity").await?;
        let streamed = consume_antigravity_sse(response, progress.clone()).await?;
        if !streamed.tool_calls.is_empty() {
            contents.push(serde_json::json!({
                "role": "model",
                "parts": streamed.function_parts,
            }));
            return Ok(ProviderTurn {
                step: ProviderStep::ToolCalls(streamed.tool_calls),
                continuation: Some(serde_json::json!({ "contents": contents })),
                events: vec![AgentEvent::Status(
                    "Antigravity requested an internal tool".into(),
                )],
            });
        }
        if streamed.text.is_empty() {
            return Err(anyhow!("Antigravity stream contained no assistant text"));
        }
        Ok(ProviderTurn {
            step: ProviderStep::Final(streamed.text),
            continuation: None,
            events: vec![AgentEvent::Status("Generating with Antigravity".into())],
        })
    }
}

fn antigravity_tool_specs(specs: &[ToolSpec]) -> Vec<serde_json::Value> {
    specs
        .iter()
        .map(|spec| {
            serde_json::json!({
                "name": spec.name,
                "description": spec.description,
                "parameters": spec.parameters,
            })
        })
        .collect()
}

struct CustomProvider {
    cfg: CustomProviderConfig,
    auth: Arc<AuthManager>,
    storage: Arc<crate::storage::Storage>,
    profiles: ProviderProfileStore,
    client: Client,
}

struct CustomTarget {
    base_url: String,
    protocol: String,
    headers: std::collections::BTreeMap<String, String>,
    api_key: Option<String>,
}

impl CustomProvider {
    fn new(cfg: CustomProviderConfig, auth: Arc<AuthManager>) -> Self {
        let storage = auth.storage();
        Self {
            cfg,
            auth,
            profiles: ProviderProfileStore::new(storage.clone()),
            storage,
            client: http_client(),
        }
    }

    fn target(&self, profile_id: Option<&str>) -> Result<CustomTarget> {
        if let Some(profile_id) = profile_id {
            let profile = self
                .profiles
                .get_by_id(profile_id)?
                .ok_or_else(|| anyhow!("selected Custom profile does not exist"))?;
            if !profile.enabled {
                return Err(anyhow!("selected Custom profile is disabled"));
            }
            // Critical isolation rule: resolve only the selected profile's
            // credential reference. Absence means no Authorization header.
            let api_key = self
                .profiles
                .resolve_api_key(self.auth.secrets(), &profile)?
                .filter(|key| !key.trim().is_empty());
            let headers = profile.merged_headers(self.auth.secrets())?;
            return Ok(CustomTarget {
                base_url: profile.endpoint,
                protocol: profile.protocol,
                headers,
                api_key,
            });
        }
        // Compatibility boundary for a config-only v0.2.5 installation
        // before an authorized Telegram owner can be derived. It deliberately
        // does not use a provider-wide API-key fallback.
        Ok(CustomTarget {
            base_url: self
                .cfg
                .base_url
                .clone()
                .ok_or_else(|| anyhow!("Custom profile is not selected"))?,
            protocol: self.cfg.protocol.clone(),
            headers: self.cfg.headers.clone(),
            api_key: None,
        })
    }

    fn unprobed_capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            model_discovery: true,
            ..ProviderCapabilities::chat_only(
                "custom model capability has not been probed; run an exact-model probe",
            )
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
        let mut models = models_with_default(self.cfg.default_model.as_deref(), fallback);
        if let Ok(profile_models) = self.profiles.all_models() {
            for model in profile_models {
                if !models.contains(&model) {
                    models.push(model);
                }
            }
        }
        models
    }
    fn models_for(&self, profile_id: Option<&str>) -> Vec<String> {
        let Some(profile_id) = profile_id else {
            return self.models();
        };
        self.profiles
            .models(profile_id)
            .map(|models| models.into_iter().map(|model| model.model_id).collect())
            .unwrap_or_default()
    }
    fn enabled(&self) -> bool {
        self.cfg.enabled
            || self
                .profiles
                .all_models()
                .is_ok_and(|models| !models.is_empty())
    }
    fn configured(&self) -> bool {
        self.profiles
            .all_models()
            .is_ok_and(|models| !models.is_empty())
            || self
                .cfg
                .base_url
                .as_deref()
                .is_some_and(|x| !x.trim().is_empty())
    }
    fn ready(&self) -> bool {
        self.enabled() && self.configured()
    }
    fn capabilities(&self, model: &str) -> ProviderCapabilities {
        self.storage
            .provider_capability("custom", model)
            .ok()
            .flatten()
            .map(capabilities_from_record)
            .unwrap_or_else(|| self.unprobed_capabilities())
    }
    fn capabilities_for(&self, model: &str, profile_id: Option<&str>) -> ProviderCapabilities {
        let Some(profile_id) = profile_id else {
            return self.capabilities(model);
        };
        let mut capabilities = self
            .profiles
            .model(profile_id, model)
            .ok()
            .flatten()
            .map(profile_capabilities_from_record)
            .unwrap_or_else(|| {
                ProviderCapabilities::chat_only(
                    "selected Custom profile/model has not passed capability probing",
                )
            });
        if let Ok(Some(profile)) = self.profiles.get_by_id(profile_id) {
            for (capability, target) in [
                ("vision", &mut capabilities.vision),
                ("file_input", &mut capabilities.file_input),
            ] {
                match self
                    .profiles
                    .capability_override(profile_id, model, &profile.protocol, capability)
                    .as_deref()
                {
                    Ok("force_supported") => *target = true,
                    Ok("force_unsupported") => *target = false,
                    _ => {}
                }
            }
        }
        capabilities
    }
    fn supports_semantic_evaluation(&self, model: &str) -> bool {
        (self
            .storage
            .provider_capability("custom", model)
            .ok()
            .flatten()
            .is_some())
            && self.capabilities(model).structured_output
    }
    fn supports_semantic_evaluation_for(&self, model: &str, profile_id: Option<&str>) -> bool {
        self.capabilities_for(model, profile_id).structured_output
    }
    async fn generate_text(&self, mut req: ProviderRequest) -> Result<String> {
        req.tools.clear();
        if req.account_id.is_none() && !self.cfg.enabled {
            return Err(anyhow!("custom provider is disabled"));
        }
        let target = self.target(req.account_id.as_deref())?;
        let endpoint = if target.protocol == "openai_chat_completions" {
            endpoint_with_suffix(&target.base_url, "/chat/completions")
        } else {
            endpoint_with_suffix(&target.base_url, "/responses")
        };
        let mut request = self.client.post(endpoint);
        for (name, value) in &target.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(key) = target.api_key.as_deref() {
            request = request.bearer_auth(key);
        }
        let body = if target.protocol == "openai_chat_completions" {
            custom_chat_body(&req, None, &[], ToolProtocol::ChatOnly)?
        } else {
            custom_responses_body(&req, None, &[], ToolProtocol::ChatOnly)?
        };
        let value: serde_json::Value = ensure_success(
            request.json(&body).send().await?,
            "Custom semantic provider",
        )
        .await?
        .json()
        .await?;
        let text = if target.protocol == "openai_chat_completions" {
            extract_chat_content(&value)
        } else {
            extract_output_text(&value)
        };
        text.filter(|text| !text.trim().is_empty())
            .ok_or_else(|| anyhow!("custom semantic response contained no text"))
    }
    async fn run(
        &self,
        req: ProviderRequest,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<ProviderResponse> {
        let turn = self.run_turn(req, None, Vec::new(), progress).await?;
        match turn.step {
            ProviderStep::Final(final_answer) => Ok(ProviderResponse {
                events: turn.events,
                final_answer,
            }),
            ProviderStep::ToolCalls(_) => Err(anyhow!(
                "custom provider requested a tool outside the agent loop"
            )),
        }
    }

    async fn run_turn(
        &self,
        mut req: ProviderRequest,
        continuation: Option<serde_json::Value>,
        tool_results: Vec<ToolResult>,
        progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<ProviderTurn> {
        if req.account_id.is_none() && !self.cfg.enabled {
            return Err(anyhow!("custom provider is disabled"));
        }
        let profile_id = req.account_id.clone();
        let had_images = !req.images.is_empty();
        let target = self.target(req.account_id.as_deref())?;
        let capabilities = self.capabilities_for(&req.model, req.account_id.as_deref());
        if !req.images.is_empty() && !capabilities.vision {
            return Err(anyhow!(
                "selected Custom profile/model does not declare vision capability"
            ));
        }
        if !req.files.is_empty() && !capabilities.file_input {
            return Err(anyhow!(
                "selected Custom profile/model does not declare file-input capability"
            ));
        }
        emit(
            &progress,
            AgentEvent::Status("Sending request to custom provider".into()),
        );
        let is_streaming = req.streaming;
        let endpoint = if target.protocol == "openai_chat_completions" {
            endpoint_with_suffix(&target.base_url, "/chat/completions")
        } else {
            endpoint_with_suffix(&target.base_url, "/responses")
        };
        let mut request = self.client.post(endpoint.clone());
        for (name, value) in &target.headers {
            request = request.header(name.as_str(), value.as_str());
        }
        if let Some(key) = target.api_key.as_deref() {
            request = request.bearer_auth(key);
        }
        if is_streaming {
            request = request.header("Accept", "text/event-stream");
        }
        let mut structured_transcript = None;
        if capabilities.tool_protocol == ToolProtocol::StructuredJsonFallback {
            let transcript = structured_fallback_transcript(continuation.as_ref(), &tool_results)?;
            append_structured_fallback_context(&mut req.messages, &req.tools, &transcript);
            structured_transcript = Some(transcript);
        }
        let body = if target.protocol == "openai_chat_completions" {
            custom_chat_body(
                &req,
                continuation.as_ref(),
                &tool_results,
                capabilities.tool_protocol,
            )?
        } else {
            custom_responses_body(
                &req,
                continuation.as_ref(),
                &tool_results,
                capabilities.tool_protocol,
            )?
        };
        let response_res = request.json(&body).send().await;
        let mut response = match response_res {
            Ok(resp) => resp,
            Err(err) => return Err(err.into()),
        };
        let mut parsed_non_stream = false;
        if !response.status().is_success() && is_streaming {
            let status = response.status();
            let body_bytes = response.bytes().await.unwrap_or_default();
            let body_text = String::from_utf8_lossy(&body_bytes);
            if is_explicit_streaming_unsupported(status.as_u16(), &body_text) {
                if let Some(profile_id) = profile_id.as_deref() {
                    let _ = self.profiles.record_runtime_capability(
                        profile_id,
                        &req.model,
                        &target.protocol,
                        "streaming",
                        "unsupported",
                        "provider_explicit_unsupported",
                    );
                }
                let mut fallback_req = req.clone();
                fallback_req.streaming = false;
                let fallback_body = if target.protocol == "openai_chat_completions" {
                    custom_chat_body(
                        &fallback_req,
                        continuation.as_ref(),
                        &tool_results,
                        capabilities.tool_protocol,
                    )?
                } else {
                    custom_responses_body(
                        &fallback_req,
                        continuation.as_ref(),
                        &tool_results,
                        capabilities.tool_protocol,
                    )?
                };
                let mut retry_request = self.client.post(endpoint);
                for (name, value) in &target.headers {
                    retry_request = retry_request.header(name.as_str(), value.as_str());
                }
                if let Some(key) = target.api_key.as_deref() {
                    retry_request = retry_request.bearer_auth(key);
                }
                response = retry_request.json(&fallback_body).send().await?;
                parsed_non_stream = true;
            } else {
                let summary = upstream_error_summary(&body_bytes, false);
                if had_images && explicit_image_unsupported(&summary) {
                    if let Some(profile_id) = profile_id.as_deref() {
                        let _ = self.profiles.record_runtime_capability(
                            profile_id,
                            &req.model,
                            &target.protocol,
                            "vision",
                            "unsupported",
                            "provider_explicit_unsupported",
                        );
                    }
                }
                return Err(anyhow!(
                    "Custom provider request failed with HTTP {status}: {summary}"
                ));
            }
        }
        let response = match ensure_success(response, "Custom provider").await {
            Ok(response) => response,
            Err(error) => {
                if had_images && explicit_image_unsupported(&error.to_string()) {
                    if let Some(profile_id) = profile_id.as_deref() {
                        let _ = self.profiles.record_runtime_capability(
                            profile_id,
                            &req.model,
                            &target.protocol,
                            "vision",
                            "unsupported",
                            "provider_explicit_unsupported",
                        );
                    }
                }
                return Err(error);
            }
        };
        if parsed_non_stream
            || !req.streaming
            || !response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.contains("text/event-stream"))
        {
            let value: serde_json::Value = response.json().await?;
            let (step, continuation) = match capabilities.tool_protocol {
                ToolProtocol::Native if target.protocol == "openai_chat_completions" => {
                    parse_custom_chat_turn(&value, body["messages"].clone())?
                }
                ToolProtocol::Native => parse_custom_responses_turn(&value, body["input"].clone())?,
                ToolProtocol::StructuredJsonFallback => {
                    let text = if target.protocol == "openai_chat_completions" {
                        extract_chat_content(&value)
                    } else {
                        extract_output_text(&value)
                    }
                    .ok_or_else(|| anyhow!("custom structured response contained no JSON text"))?;
                    let step = parse_structured_agent_output(&text, &req.tools)?;
                    let continuation = match &step {
                        ProviderStep::ToolCalls(calls) => {
                            let mut transcript = structured_transcript.take().unwrap_or_default();
                            transcript.push(serde_json::json!({
                                "role":"assistant", "kind":"tool_calls",
                                "calls":calls.iter().map(|call| serde_json::json!({
                                    "call_id":call.call_id,"name":call.name,"arguments":call.arguments,
                                })).collect::<Vec<_>>()
                            }));
                            Some(serde_json::json!({
                                "kind":"xiao_structured_continuation_v1",
                                "transcript":bound_structured_transcript(transcript)?,
                            }))
                        }
                        ProviderStep::Final(_) => None,
                    };
                    (step, continuation)
                }
                ToolProtocol::ChatOnly => {
                    let answer = if target.protocol == "openai_chat_completions" {
                        extract_chat_content(&value)
                    } else {
                        extract_output_text(&value)
                    }
                    .filter(|answer| !answer.trim().is_empty())
                    .ok_or_else(|| anyhow!("custom response contained no assistant text"))?;
                    (ProviderStep::Final(answer), None)
                }
            };
            if had_images {
                if let Some(profile_id) = profile_id.as_deref() {
                    let _ = self.profiles.record_runtime_capability(
                        profile_id,
                        &req.model,
                        &target.protocol,
                        "vision",
                        "supported",
                        "runtime_success",
                    );
                }
            }
            return Ok(ProviderTurn {
                step,
                continuation,
                events: vec![AgentEvent::Status("Custom provider completed".into())],
            });
        }
        if target.protocol == "openai_chat_completions" {
            let streamed = consume_custom_chat_sse(
                response,
                progress.clone(),
                body["messages"].as_array().cloned().unwrap_or_default(),
                capabilities.tool_protocol != ToolProtocol::StructuredJsonFallback,
            )
            .await?;
            let (step, continuation) = if capabilities.tool_protocol
                == ToolProtocol::StructuredJsonFallback
            {
                let step = parse_structured_agent_output(&streamed.text, &req.tools)?;
                let continuation = match &step {
                    ProviderStep::ToolCalls(calls) => {
                        let mut transcript = structured_transcript.take().unwrap_or_default();
                        transcript.push(serde_json::json!({"role":"assistant","kind":"tool_calls","calls":calls}));
                        Some(
                            serde_json::json!({"kind":"xiao_structured_continuation_v1","transcript":bound_structured_transcript(transcript)?}),
                        )
                    }
                    ProviderStep::Final(_) => None,
                };
                (step, continuation)
            } else if streamed.tool_calls.is_empty() {
                (ProviderStep::Final(streamed.text), None)
            } else {
                (
                    ProviderStep::ToolCalls(streamed.tool_calls),
                    streamed.continuation,
                )
            };
            if let Some(profile_id) = profile_id.as_deref() {
                let _ = self.profiles.record_runtime_capability(
                    profile_id,
                    &req.model,
                    &target.protocol,
                    "streaming",
                    "supported",
                    "runtime_success",
                );
                if had_images {
                    let _ = self.profiles.record_runtime_capability(
                        profile_id,
                        &req.model,
                        &target.protocol,
                        "vision",
                        "supported",
                        "runtime_success",
                    );
                }
            }
            return Ok(ProviderTurn {
                step,
                continuation,
                events: vec![AgentEvent::Status("Custom provider completed".into())],
            });
        }
        let streamed = consume_responses_sse(
            response,
            "custom",
            progress.clone(),
            capabilities.tool_protocol != ToolProtocol::StructuredJsonFallback,
        )
        .await?;
        let (step, continuation) = if capabilities.tool_protocol
            == ToolProtocol::StructuredJsonFallback
        {
            let step = parse_structured_agent_output(&streamed.text, &req.tools)?;
            let continuation = match &step {
                ProviderStep::ToolCalls(calls) => {
                    let mut transcript = structured_transcript.take().unwrap_or_default();
                    transcript.push(
                        serde_json::json!({"role":"assistant","kind":"tool_calls","calls":calls}),
                    );
                    Some(
                        serde_json::json!({"kind":"xiao_structured_continuation_v1","transcript":bound_structured_transcript(transcript)?}),
                    )
                }
                ProviderStep::Final(_) => None,
            };
            (step, continuation)
        } else if streamed.tool_calls.is_empty() {
            (ProviderStep::Final(streamed.text), None)
        } else {
            let mut input = body["input"].as_array().cloned().unwrap_or_default();
            input.extend(streamed.function_items);
            (
                ProviderStep::ToolCalls(streamed.tool_calls),
                Some(serde_json::json!({ "input": input })),
            )
        };
        if let Some(profile_id) = profile_id.as_deref() {
            let _ = self.profiles.record_runtime_capability(
                profile_id,
                &req.model,
                &target.protocol,
                "streaming",
                "supported",
                "runtime_success",
            );
            if had_images {
                let _ = self.profiles.record_runtime_capability(
                    profile_id,
                    &req.model,
                    &target.protocol,
                    "vision",
                    "supported",
                    "runtime_success",
                );
            }
        }
        Ok(ProviderTurn {
            step,
            continuation,
            events: vec![AgentEvent::Status("Custom provider completed".into())],
        })
    }
}

fn custom_chat_body(
    req: &ProviderRequest,
    continuation: Option<&serde_json::Value>,
    tool_results: &[ToolResult],
    protocol: ToolProtocol,
) -> Result<serde_json::Value> {
    let mut messages = continuation
        .and_then(|value| value.get("messages"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_else(|| chat_messages(&req.messages));
    if continuation.is_none() {
        append_chat_images(&mut messages, &req.images);
        append_chat_files(&mut messages, &req.files)?;
    }
    if protocol == ToolProtocol::Native {
        messages.extend(tool_results.iter().map(|result| {
            serde_json::json!({
                "role": "tool",
                "tool_call_id": result.call_id,
                "name": result.name,
                "content": result.output,
            })
        }));
    }
    let mut body = serde_json::json!({
        "model": req.model,
        "messages": messages,
        "stream": req.streaming,
    });
    if protocol == ToolProtocol::Native && !req.tools.is_empty() {
        body["tools"] = serde_json::Value::Array(
            req.tools
                .iter()
                .map(|spec| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": spec.name,
                            "description": spec.description,
                            "parameters": spec.parameters,
                            "strict": true,
                        }
                    })
                })
                .collect(),
        );
        body["tool_choice"] = serde_json::Value::String("auto".into());
    }
    Ok(body)
}

fn custom_responses_body(
    req: &ProviderRequest,
    continuation: Option<&serde_json::Value>,
    tool_results: &[ToolResult],
    protocol: ToolProtocol,
) -> Result<serde_json::Value> {
    let mut payload = responses_payload(&req.messages, None);
    if continuation.is_none() {
        // Any image reaching this point was validated by the runtime and the
        // selected profile/model capability gate.
        let _ = append_responses_images(&mut payload.input, &req.images);
        append_responses_files(&mut payload.input, &req.files)?;
    }
    let mut input = continuation
        .and_then(|value| value.get("input"))
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or(payload.input);
    if protocol == ToolProtocol::Native {
        input.extend(tool_results.iter().map(|result| {
            serde_json::json!({
                "type": "function_call_output",
                "call_id": result.call_id,
                "output": result.output,
            })
        }));
    }
    let mut body = serde_json::json!({
        "model": req.model,
        "input": input,
        "stream": req.streaming,
    });
    if let Some(instructions) = payload.instructions {
        body["instructions"] = serde_json::Value::String(instructions);
    }
    if protocol == ToolProtocol::Native && !req.tools.is_empty() {
        body["tools"] = serde_json::Value::Array(responses_tool_specs(&req.tools));
    }
    Ok(body)
}

fn parse_custom_chat_turn(
    value: &serde_json::Value,
    messages_value: serde_json::Value,
) -> Result<(ProviderStep, Option<serde_json::Value>)> {
    let message = value
        .pointer("/choices/0/message")
        .ok_or_else(|| anyhow!("custom chat response missing choices[0].message"))?;
    let mut calls = Vec::new();
    for item in message
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let call_id = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("custom chat tool call missing id"))?;
        let function = item
            .get("function")
            .ok_or_else(|| anyhow!("custom chat tool call missing function"))?;
        let name = function
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("custom chat tool call missing function name"))?;
        let raw_arguments = function
            .get("arguments")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("custom chat tool call arguments must be JSON text"))?;
        let arguments = serde_json::from_str(raw_arguments)
            .map_err(|_| anyhow!("custom chat tool call arguments are malformed JSON"))?;
        calls.push(ToolCall {
            call_id: call_id.into(),
            name: name.into(),
            arguments,
        });
    }
    if !calls.is_empty() {
        let mut messages = messages_value.as_array().cloned().unwrap_or_default();
        messages.push(message.clone());
        return Ok((
            ProviderStep::ToolCalls(calls),
            Some(serde_json::json!({ "messages": messages })),
        ));
    }
    let answer = extract_chat_content(value)
        .filter(|answer| !answer.trim().is_empty())
        .ok_or_else(|| anyhow!("custom chat response contained no assistant text or tool call"))?;
    Ok((ProviderStep::Final(answer), None))
}

fn parse_custom_responses_turn(
    value: &serde_json::Value,
    input_value: serde_json::Value,
) -> Result<(ProviderStep, Option<serde_json::Value>)> {
    let mut calls = Vec::new();
    let output = value
        .get("output")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in &output {
        if item.get("type").and_then(serde_json::Value::as_str) != Some("function_call") {
            continue;
        }
        let call_id = item
            .get("call_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("custom Responses function call missing call_id"))?;
        let name = item
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("custom Responses function call missing name"))?;
        let arguments = match item.get("arguments") {
            Some(serde_json::Value::String(raw)) => serde_json::from_str(raw)
                .map_err(|_| anyhow!("custom Responses function arguments are malformed JSON"))?,
            Some(value @ serde_json::Value::Object(_)) => value.clone(),
            _ => return Err(anyhow!("custom Responses function call missing arguments")),
        };
        calls.push(ToolCall {
            call_id: call_id.into(),
            name: name.into(),
            arguments,
        });
    }
    if !calls.is_empty() {
        let mut input = input_value.as_array().cloned().unwrap_or_default();
        input.extend(output);
        return Ok((
            ProviderStep::ToolCalls(calls),
            Some(serde_json::json!({ "input": input })),
        ));
    }
    let answer = extract_output_text(value)
        .filter(|answer| !answer.trim().is_empty())
        .ok_or_else(|| anyhow!("custom Responses output contained no text or function call"))?;
    Ok((ProviderStep::Final(answer), None))
}

fn append_structured_fallback_context(
    messages: &mut Vec<MessageRecord>,
    tools: &[ToolSpec],
    transcript: &[serde_json::Value],
) {
    let tools = tools
        .iter()
        .take(64)
        .map(|spec| {
            serde_json::json!({
                "name": spec.name,
                "description": spec.description,
                "parameters": spec.parameters,
            })
        })
        .collect::<Vec<_>>();
    messages.push(MessageRecord {
        role: "system".into(),
        content: format!(
            "<XIAO_STRUCTURED_AGENT_PROTOCOL>Return exactly one JSON object, with no markdown: {{\"kind\":\"tool_calls\",\"calls\":[{{\"call_id\":\"unique\",\"name\":\"tool\",\"arguments\":{{}}}}]}} or {{\"kind\":\"final\",\"text\":\"answer\"}}. Only request listed tools. Tool policy is enforced by Xiao. TOOLS={} NORMALIZED_TRANSCRIPT={}</XIAO_STRUCTURED_AGENT_PROTOCOL>",
            serde_json::to_string(&tools).unwrap_or_else(|_| "[]".into()),
            serde_json::to_string(transcript).unwrap_or_else(|_| "[]".into()),
        )
        .chars()
        .take(48_000)
        .collect(),
        created_at: chrono::Utc::now().to_rfc3339(),
    });
}

const STRUCTURED_MAX_TRANSCRIPT_ITEMS: usize = 48;
const STRUCTURED_MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;
const STRUCTURED_MAX_RESULT_BYTES: usize = 16 * 1024;

fn structured_fallback_transcript(
    continuation: Option<&serde_json::Value>,
    tool_results: &[ToolResult],
) -> Result<Vec<serde_json::Value>> {
    let mut transcript = match continuation {
        Some(value)
            if value.get("kind").and_then(serde_json::Value::as_str)
                == Some("xiao_structured_continuation_v1") =>
        {
            value
                .get("transcript")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default()
        }
        Some(_) => return Err(anyhow!("invalid Custom structured continuation state")),
        None => Vec::new(),
    };
    for result in tool_results.iter().take(16) {
        transcript.push(serde_json::json!({
            "role":"tool",
            "call_id":result.call_id,
            "name":result.name,
            "output":bound_utf8(&result.output, STRUCTURED_MAX_RESULT_BYTES),
            "is_error":result.is_error,
        }));
    }
    bound_structured_transcript(transcript)
}

fn bound_structured_transcript(
    mut transcript: Vec<serde_json::Value>,
) -> Result<Vec<serde_json::Value>> {
    if transcript.len() > STRUCTURED_MAX_TRANSCRIPT_ITEMS {
        let drain = transcript.len() - STRUCTURED_MAX_TRANSCRIPT_ITEMS;
        transcript.drain(0..drain);
    }
    while serde_json::to_vec(&transcript)?.len() > STRUCTURED_MAX_TRANSCRIPT_BYTES {
        if transcript.len() <= 1 {
            return Err(anyhow!(
                "Custom structured continuation exceeds bounded transcript size"
            ));
        }
        transcript.remove(0);
    }
    Ok(transcript)
}

fn bound_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value[..end].to_owned() + "…"
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StructuredAgentOutput {
    ToolCalls { calls: Vec<StructuredToolCall> },
    Final { text: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StructuredToolCall {
    call_id: String,
    name: String,
    arguments: serde_json::Value,
}

fn parse_structured_agent_output(text: &str, tools: &[ToolSpec]) -> Result<ProviderStep> {
    if text.chars().count() > 64_000 {
        return Err(anyhow!(
            "structured agent response exceeded the 64000 character bound"
        ));
    }
    let parsed: StructuredAgentOutput = serde_json::from_str(text.trim())
        .map_err(|_| anyhow!("structured agent response failed strict schema validation"))?;
    match parsed {
        StructuredAgentOutput::Final { text } if !text.trim().is_empty() => {
            Ok(ProviderStep::Final(text))
        }
        StructuredAgentOutput::Final { .. } => {
            Err(anyhow!("structured agent final text was empty"))
        }
        StructuredAgentOutput::ToolCalls { calls } => {
            if calls.is_empty() || calls.len() > 16 {
                return Err(anyhow!(
                    "structured agent tool_calls must contain between 1 and 16 calls"
                ));
            }
            calls
                .into_iter()
                .map(|call| {
                    if call.call_id.trim().is_empty()
                        || !tools.iter().any(|spec| spec.name == call.name)
                        || !call.arguments.is_object()
                    {
                        return Err(anyhow!(
                            "structured agent requested an invalid or undeclared tool call"
                        ));
                    }
                    Ok(ToolCall {
                        call_id: call.call_id,
                        name: call.name,
                        arguments: call.arguments,
                    })
                })
                .collect::<Result<Vec<_>>>()
                .map(ProviderStep::ToolCalls)
        }
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

/// Probe a selected Custom model without exposing Xiao's real tools. The
/// synthetic function has no side effect, and every response is bounded and
/// schema-checked before capability metadata is persisted.
pub(crate) async fn probe_custom_capabilities(
    base: &str,
    headers: &std::collections::BTreeMap<String, String>,
    api_key: Option<&str>,
    protocol: &str,
    model: &str,
) -> CustomCapabilityProbe {
    let nonce = Uuid::new_v4().simple().to_string();
    let native_result = custom_native_probe(base, headers, api_key, protocol, model, &nonce).await;
    let structured_result =
        custom_structured_probe(base, headers, api_key, protocol, model, &nonce).await;
    let vision_result = custom_vision_probe(base, headers, api_key, protocol, model, &nonce).await;
    let file_result =
        custom_file_input_probe(base, headers, api_key, protocol, model, &nonce).await;

    let native_tools = result_state(&native_result);
    let structured_output = result_state(&structured_result);
    let vision = positive_or_unknown(&vision_result);
    let file_input = positive_or_unknown(&file_result);
    let continuation = if matches!(native_tools, CapabilityState::Supported)
        || matches!(structured_output, CapabilityState::Supported)
    {
        CapabilityState::Supported
    } else if matches!(native_tools, CapabilityState::Unsupported)
        && matches!(structured_output, CapabilityState::Unsupported)
    {
        CapabilityState::Unsupported
    } else {
        CapabilityState::Unknown
    };

    let tool_protocol = if matches!(native_tools, CapabilityState::Supported) {
        ToolProtocol::Native
    } else if matches!(structured_output, CapabilityState::Supported) {
        ToolProtocol::StructuredJsonFallback
    } else {
        ToolProtocol::ChatOnly
    };
    let capabilities = ProviderCapabilities {
        text: true,
        vision: matches!(vision, CapabilityState::Supported),
        file_input: matches!(file_input, CapabilityState::Supported),
        native_tools: matches!(native_tools, CapabilityState::Supported),
        tool_protocol,
        model_discovery: true,
        structured_output: matches!(structured_output, CapabilityState::Supported),
        continuation: matches!(continuation, CapabilityState::Supported),
        evidence: format!(
            "bounded custom probe: native={}; structured={}; continuation={}; vision={}; file_input={}",
            native_tools.as_str(),
            structured_output.as_str(),
            continuation.as_str(),
            vision.as_str(),
            file_input.as_str(),
        ),
    };
    CustomCapabilityProbe {
        capabilities,
        native_tools,
        structured_output,
        continuation,
        vision,
        file_input,
    }
}

#[cfg(test)]
pub(crate) async fn probe_custom_tool_capability(
    base: &str,
    headers: &std::collections::BTreeMap<String, String>,
    api_key: Option<&str>,
    protocol: &str,
    model: &str,
) -> ProviderCapabilities {
    probe_custom_capabilities(base, headers, api_key, protocol, model)
        .await
        .capabilities
}

fn result_state(result: &Result<bool>) -> CapabilityState {
    match result {
        Ok(true) => CapabilityState::Supported,
        Ok(false) => CapabilityState::Unsupported,
        Err(_) => CapabilityState::Unknown,
    }
}

fn positive_or_unknown(result: &Result<bool>) -> CapabilityState {
    match result {
        Ok(true) => CapabilityState::Supported,
        Ok(false) | Err(_) => CapabilityState::Unknown,
    }
}

async fn custom_vision_probe(
    base: &str,
    headers: &std::collections::BTreeMap<String, String>,
    api_key: Option<&str>,
    protocol: &str,
    model: &str,
    nonce: &str,
) -> Result<bool> {
    // Hidden challenge: render nonce into a tiny PNG so the text prompt
    // never contains the expected token. A text-only model that echoes the
    // prompt will not see the challenge.
    let challenge = format!("VISION-{nonce}");
    let png_base64 = render_probe_png_base64(&challenge);
    // Prompt must not contain challenge; ask model to read image.
    let prompt = "Read the code visible in the attached image and reply exactly with that code. No other text.";
    let (suffix, body) = if protocol == "openai_responses" {
        (
            "/responses",
            serde_json::json!({
                "model": model,
                "input": [{"role":"user","content":[
                    {"type":"input_text","text":prompt},
                    {"type":"input_image","image_url":format!("data:image/png;base64,{png_base64}")}
                ]}],
                "stream": false
            }),
        )
    } else {
        (
            "/chat/completions",
            serde_json::json!({
                "model": model,
                "messages": [{"role":"user","content":[
                    {"type":"text","text":prompt},
                    {"type":"image_url","image_url":{"url":format!("data:image/png;base64,{png_base64}")}}
                ]}],
                "stream": false
            }),
        )
    };
    let value = send_custom_probe(base, suffix, headers, api_key, body).await?;
    let output = if protocol == "openai_responses" {
        extract_output_text(&value)
    } else {
        extract_chat_content(&value)
    };
    Ok(output.is_some_and(|text| text.contains(&challenge)))
}

/// Minimal 5x7 bitmap font rasterizer for probe images. No external
/// font dependency; uses a compact table for A-Z,0-9,'-'.
fn render_probe_png_base64(challenge: &str) -> String {
    use base64::engine::general_purpose::STANDARD as B64;
    use image::{ImageBuffer, Rgb};
    // Layout: each char 6px wide (5 + 1 gap), 7px tall, with margins.
    let char_w: u32 = 6;
    let char_h: u32 = 9;
    let margin: u32 = 4;
    let width = (challenge.len() as u32) * char_w + margin * 2;
    let height = char_h + margin * 2;
    let mut img: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(width, height, Rgb([255, 255, 255]));
    for (idx, ch) in challenge.chars().enumerate() {
        let bitmap = font_bitmap(ch);
        let ox = margin + (idx as u32) * char_w;
        let oy = margin + 1;
        for row in 0..7u32 {
            for col in 0..5u32 {
                if (bitmap[row as usize] >> (4 - col)) & 1 == 1 {
                    img.put_pixel(ox + col, oy + row, Rgb([0, 0, 0]));
                }
            }
        }
    }
    let mut buf = Vec::new();
    {
        let mut cursor = std::io::Cursor::new(&mut buf);
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .unwrap_or_default();
    }
    if buf.is_empty() {
        // Fallback 1x1 transparent
        return "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=".into();
    }
    B64.encode(&buf)
}

fn font_bitmap(ch: char) -> [u8; 7] {
    let c = ch.to_ascii_uppercase();
    match c {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01110, 0b10001, 0b10000, 0b01110, 0b00001, 0b10001, 0b01110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b01010, 0b01010, 0b00100, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b01110, 0b00000, 0b00000, 0b00000,
        ],
        _ => [
            0b00000, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110, 0b00000,
        ],
    }
}

async fn custom_file_input_probe(
    base: &str,
    headers: &std::collections::BTreeMap<String, String>,
    api_key: Option<&str>,
    protocol: &str,
    model: &str,
    nonce: &str,
) -> Result<bool> {
    // Chat Completions has no portable first-class file-input contract. Keep
    // this Unknown rather than manufacturing an Unsupported result.
    if protocol != "openai_responses" {
        return Err(anyhow!(
            "portable file-input probe unavailable for this protocol"
        ));
    }
    let challenge = format!("FILE-{nonce}");
    let data = STANDARD.encode(&challenge);
    let body = serde_json::json!({
        "model": model,
        "input": [{"role":"user","content":[
            {"type":"input_text","text":"Read the attached file and reply exactly with the challenge stored in the file. No other text."},
            {"type":"input_file","filename":"xiao-capability.txt","file_data":format!("data:text/plain;base64,{data}")}
        ]}],
        "stream": false
    });
    let value = send_custom_probe(base, "/responses", headers, api_key, body).await?;
    Ok(extract_output_text(&value).is_some_and(|text| text.contains(&challenge)))
}

async fn custom_native_probe(
    base: &str,
    headers: &std::collections::BTreeMap<String, String>,
    api_key: Option<&str>,
    protocol: &str,
    model: &str,
    nonce: &str,
) -> Result<bool> {
    let tool = serde_json::json!({
        "name":"xiao_capability_probe",
        "description":"Return the supplied nonce to prove function-call protocol support.",
        "parameters":{
            "type":"object",
            "additionalProperties":false,
            "required":["nonce"],
            "properties":{"nonce":{"type":"string"}}
        }
    });
    let (suffix, body) = if protocol == "openai_responses" {
        (
            "/responses",
            serde_json::json!({
                "model":model,
                "input":[{"role":"user","content":[{"type":"input_text","text":format!("Call xiao_capability_probe with nonce {nonce}. Do not answer in text.")}]}],
                "tools":[{"type":"function","name":tool["name"],"description":tool["description"],"parameters":tool["parameters"],"strict":true}],
                "tool_choice":{"type":"function","name":"xiao_capability_probe"},
                "stream":false,
            }),
        )
    } else {
        (
            "/chat/completions",
            serde_json::json!({
                "model":model,
                "messages":[{"role":"user","content":format!("Call xiao_capability_probe with nonce {nonce}. Do not answer in text.")}],
                "tools":[{"type":"function","function":tool}],
                "tool_choice":{"type":"function","function":{"name":"xiao_capability_probe"}},
                "stream":false,
            }),
        )
    };
    let value = send_custom_probe(base, suffix, headers, api_key, body).await?;
    let calls = if protocol == "openai_responses" {
        value
            .get("output")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| {
                item.get("type").and_then(serde_json::Value::as_str) == Some("function_call")
            })
            .map(|item| {
                let name = item.get("name").and_then(serde_json::Value::as_str);
                let arguments = match item.get("arguments") {
                    Some(serde_json::Value::String(raw)) => serde_json::from_str(raw).ok(),
                    Some(value @ serde_json::Value::Object(_)) => Some(value.clone()),
                    _ => None,
                };
                (name, arguments)
            })
            .collect::<Vec<_>>()
    } else {
        value
            .pointer("/choices/0/message/tool_calls")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .map(|item| {
                let function = item.get("function");
                let name = function
                    .and_then(|value| value.get("name"))
                    .and_then(serde_json::Value::as_str);
                let arguments = function
                    .and_then(|value| value.get("arguments"))
                    .and_then(serde_json::Value::as_str)
                    .and_then(|raw| serde_json::from_str(raw).ok());
                (name, arguments)
            })
            .collect::<Vec<_>>()
    };
    Ok(calls.into_iter().any(|(name, arguments)| {
        name == Some("xiao_capability_probe")
            && arguments
                .as_ref()
                .and_then(|value| value.get("nonce"))
                .and_then(serde_json::Value::as_str)
                == Some(nonce)
    }))
}

async fn custom_structured_probe(
    base: &str,
    headers: &std::collections::BTreeMap<String, String>,
    api_key: Option<&str>,
    protocol: &str,
    model: &str,
    nonce: &str,
) -> Result<bool> {
    let expected = format!("xiao-capability-probe:{nonce}");
    let instruction = format!(
        "Return exactly this JSON object and nothing else: {{\"kind\":\"final\",\"text\":\"{expected}\"}}"
    );
    let (suffix, body) = if protocol == "openai_responses" {
        (
            "/responses",
            serde_json::json!({"model":model,"input":[{"role":"user","content":[{"type":"input_text","text":instruction}]}],"stream":false}),
        )
    } else {
        (
            "/chat/completions",
            serde_json::json!({"model":model,"messages":[{"role":"user","content":instruction}],"stream":false}),
        )
    };
    let value = send_custom_probe(base, suffix, headers, api_key, body).await?;
    let output = if protocol == "openai_responses" {
        extract_output_text(&value)
    } else {
        extract_chat_content(&value)
    }
    .ok_or_else(|| anyhow!("structured capability probe returned no text"))?;
    Ok(matches!(
        serde_json::from_str::<StructuredAgentOutput>(output.trim()),
        Ok(StructuredAgentOutput::Final { text }) if text == expected
    ))
}

async fn send_custom_probe(
    base: &str,
    suffix: &str,
    headers: &std::collections::BTreeMap<String, String>,
    api_key: Option<&str>,
    body: serde_json::Value,
) -> Result<serde_json::Value> {
    const MAX_PROBE_BYTES: usize = 512 * 1024;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut request = client.post(endpoint_with_suffix(base, suffix));
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }
    if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
        request = request.bearer_auth(key);
    }
    let response =
        ensure_success(request.json(&body).send().await?, "Custom capability probe").await?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_PROBE_BYTES as u64)
    {
        return Err(anyhow!("custom capability response exceeded 512 KiB"));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len().saturating_add(chunk.len()) > MAX_PROBE_BYTES {
            return Err(anyhow!("custom capability response exceeded 512 KiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
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

fn is_explicit_streaming_unsupported(status: u16, body: &str) -> bool {
    let value = body.to_ascii_lowercase();
    (status == 400 || status == 415 || status == 422)
        && value.contains("stream")
        && (value.contains("unsupported")
            || value.contains("not supported")
            || value.contains("disabled")
            || value.contains("not enabled")
            || value.contains("does not support")
            || value.contains("only non-streaming"))
}

fn explicit_image_unsupported(error: &str) -> bool {
    let value = error.to_ascii_lowercase();
    value.contains("http 400")
        && [
            "image input is not supported",
            "image_url is not supported",
            "unsupported content type: image",
            "does not support images",
        ]
        .iter()
        .any(|needle| value.contains(needle))
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

struct StreamedChat {
    text: String,
    tool_calls: Vec<ToolCall>,
    continuation: Option<serde_json::Value>,
}

async fn consume_custom_chat_sse(
    response: reqwest::Response,
    progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    mut messages: Vec<serde_json::Value>,
    visible_text: bool,
) -> Result<StreamedChat> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut text = String::new();
    let mut calls: HashMap<usize, (String, String, String)> = HashMap::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        emit(
            &progress,
            AgentEvent::StreamChunk {
                provider: "custom".into(),
                bytes: chunk.len(),
            },
        );
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim_end_matches('\r').to_owned();
            buffer.drain(..=pos);
            let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                continue;
            };
            if data == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            let Some(delta) = value.pointer("/choices/0/delta") else {
                continue;
            };
            if let Some(part) = delta.get("content").and_then(serde_json::Value::as_str) {
                text.push_str(part);
                if visible_text {
                    emit(&progress, AgentEvent::TextDelta(part.to_owned()));
                }
            }
            for call in delta
                .get("tool_calls")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                let index = call
                    .get("index")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as usize;
                let entry = calls.entry(index).or_default();
                if let Some(id) = call.get("id").and_then(serde_json::Value::as_str) {
                    entry.0.push_str(id);
                }
                if let Some(name) = call
                    .pointer("/function/name")
                    .and_then(serde_json::Value::as_str)
                {
                    entry.1.push_str(name);
                }
                if let Some(args) = call
                    .pointer("/function/arguments")
                    .and_then(serde_json::Value::as_str)
                {
                    entry.2.push_str(args);
                }
            }
        }
    }
    let mut ordered = calls.into_iter().collect::<Vec<_>>();
    ordered.sort_by_key(|(index, _)| *index);
    let mut tool_calls = Vec::with_capacity(ordered.len());
    let mut wire_calls = Vec::with_capacity(ordered.len());
    for (_, (call_id, name, raw)) in ordered {
        let arguments = serde_json::from_str(&raw)
            .map_err(|_| anyhow!("custom chat tool call arguments are malformed JSON"))?;
        wire_calls.push(serde_json::json!({"id":call_id,"type":"function","function":{"name":name,"arguments":raw}}));
        tool_calls.push(ToolCall {
            call_id,
            name,
            arguments,
        });
    }
    let continuation = if tool_calls.is_empty() {
        None
    } else {
        messages.push(serde_json::json!({"role":"assistant","tool_calls":wire_calls}));
        Some(serde_json::json!({ "messages": messages }))
    };
    if text.is_empty() && tool_calls.is_empty() {
        return Err(anyhow!(
            "custom chat stream contained no assistant text or tool call"
        ));
    }
    Ok(StreamedChat {
        text,
        tool_calls,
        continuation,
    })
}

fn responses_tool_progress_event(value: &serde_json::Value) -> Option<AgentEvent> {
    let event_type = value.get("type")?.as_str()?;
    let tool = if event_type.contains("web_search_call") {
        "web_search"
    } else if event_type.contains("file_search_call") {
        "file_search"
    } else if event_type.contains("code_interpreter_call") {
        "code_interpreter"
    } else if event_type.contains("image_generation_call") {
        "image_generation"
    } else if event_type.contains("mcp_call") {
        value
            .get("name")
            .or_else(|| value.pointer("/item/name"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("mcp_tool")
    } else {
        return None;
    };
    if event_type.ends_with(".completed") {
        Some(AgentEvent::ToolCompleted {
            tool: tool.into(),
            summary: "completed".into(),
        })
    } else if event_type.ends_with(".failed") {
        Some(AgentEvent::ToolCompleted {
            tool: tool.into(),
            summary: "failed".into(),
        })
    } else if event_type.ends_with(".in_progress")
        || event_type.ends_with(".searching")
        || event_type.ends_with(".interpreting")
        || event_type.ends_with(".generating")
    {
        Some(AgentEvent::ToolStarted(tool.into()))
    } else {
        None
    }
}

async fn consume_responses_sse(
    response: reqwest::Response,
    provider: &str,
    progress: Option<mpsc::UnboundedSender<AgentEvent>>,
    visible_text: bool,
) -> Result<StreamedResponses> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut items = Vec::new();
    let mut argument_deltas = HashMap::<String, String>::new();
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

            let event_type = value.get("type").and_then(|value| value.as_str());
            if let Some(event) = responses_tool_progress_event(&value) {
                emit(&progress, event);
            }
            match event_type {
                Some("response.output_text.delta") => {
                    if let Some(delta) = value.get("delta").and_then(|value| value.as_str()) {
                        text.push_str(delta);
                        if visible_text {
                            emit(&progress, AgentEvent::TextDelta(delta.to_owned()));
                        }
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
                    let key = item
                        .get("item_id")
                        .or_else(|| item.get("id"))
                        .or_else(|| item.get("call_id"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(call_id);
                    let arguments = argument_deltas
                        .remove(key)
                        .or_else(|| {
                            item.get("arguments")
                                .and_then(|value| value.as_str())
                                .map(str::to_owned)
                        })
                        .as_deref()
                        .and_then(|value| serde_json::from_str(value).ok())
                        .unwrap_or_else(|| serde_json::json!({}));
                    calls.push(ToolCall {
                        call_id: call_id.into(),
                        name: name.into(),
                        arguments,
                    });
                    items.push(item.clone());
                }
                Some("response.function_call_arguments.delta") => {
                    if let (Some(key), Some(delta)) = (
                        value
                            .get("item_id")
                            .or_else(|| value.get("call_id"))
                            .and_then(serde_json::Value::as_str),
                        value.get("delta").and_then(serde_json::Value::as_str),
                    ) {
                        argument_deltas
                            .entry(key.into())
                            .or_default()
                            .push_str(delta);
                    }
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

struct AntigravityStream {
    text: String,
    tool_calls: Vec<ToolCall>,
    function_parts: Vec<serde_json::Value>,
}

async fn consume_antigravity_sse(
    response: reqwest::Response,
    progress: Option<mpsc::UnboundedSender<AgentEvent>>,
) -> Result<AntigravityStream> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut output = String::new();
    let mut calls = Vec::new();
    let mut function_parts = Vec::new();

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
                    if let Some(function) = part.get("functionCall") {
                        let Some(name) = function.get("name").and_then(serde_json::Value::as_str)
                        else {
                            continue;
                        };
                        let arguments = function
                            .get("args")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({}));
                        let call_id = function
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("agy-{name}-{}", calls.len() + 1));
                        calls.push(ToolCall {
                            call_id,
                            name: name.to_owned(),
                            arguments,
                        });
                        function_parts.push(part.clone());
                    }
                }
            }
        }
    }

    Ok(AntigravityStream {
        text: output,
        tool_calls: calls,
        function_parts,
    })
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
    use crate::storage::{ProviderProfileInput, ProviderProfileModelRecord, Storage};
    use axum::{http::HeaderMap, http::StatusCode, routing::post, Json, Router};

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
            tools: vec![],
            images: vec![],
            files: vec![],
            streaming: true,
        }
    }

    #[test]
    fn provider_translates_canonical_tool_specs_without_owning_policy() {
        let wire = responses_tool_specs(&[ToolSpec {
            name: "context_stats".into(),
            description: "Describe bounded context".into(),
            parameters: serde_json::json!({"type":"object"}),
            risk: crate::tools::ToolRisk::ReadOnly,
            origin: crate::tools::ToolOrigin::Builtin,
            effect: crate::tools::ToolEffect::None,
            required_capabilities: vec!["xiao.tool_registry".into()],
            timeout_ms: 5_000,
        }]);
        assert_eq!(wire[0]["type"], "function");
        assert_eq!(wire[0]["name"], "context_stats");
        assert!(wire[0].get("risk").is_none());
        assert!(wire[0].get("timeout_ms").is_none());
    }

    fn test_tool() -> ToolSpec {
        ToolSpec {
            name: "artifact_write".into(),
            description: "Create a bounded artifact".into(),
            parameters: serde_json::json!({
                "type":"object",
                "properties":{"path":{"type":"string"}},
                "required":["path"],
                "additionalProperties":false
            }),
            risk: crate::tools::ToolRisk::SideEffect,
            origin: crate::tools::ToolOrigin::Builtin,
            effect: crate::tools::ToolEffect::Idempotent,
            required_capabilities: vec![],
            timeout_ms: 5_000,
        }
    }

    fn named_tool(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: format!("Run {name}"),
            parameters: serde_json::json!({
                "type":"object",
                "properties":{"value":{"type":"string"}},
                "additionalProperties":false
            }),
            risk: crate::tools::ToolRisk::ReadOnly,
            origin: crate::tools::ToolOrigin::Builtin,
            effect: crate::tools::ToolEffect::None,
            required_capabilities: vec![],
            timeout_ms: 5_000,
        }
    }

    fn profile_model(
        profile_id: &str,
        model_id: &str,
        tool_protocol: ToolProtocol,
    ) -> ProviderProfileModelRecord {
        ProviderProfileModelRecord {
            profile_id: profile_id.into(),
            model_id: model_id.into(),
            text_capable: true,
            vision_capable: false,
            file_input_capable: false,
            native_tools: tool_protocol == ToolProtocol::Native,
            structured_output: tool_protocol != ToolProtocol::ChatOnly,
            continuation: tool_protocol != ToolProtocol::ChatOnly,
            native_tools_state: if tool_protocol == ToolProtocol::Native {
                "supported"
            } else {
                "unsupported"
            }
            .into(),
            structured_output_state: if tool_protocol != ToolProtocol::ChatOnly {
                "supported"
            } else {
                "unsupported"
            }
            .into(),
            continuation_state: if tool_protocol != ToolProtocol::ChatOnly {
                "supported"
            } else {
                "unsupported"
            }
            .into(),
            vision_state: "unknown".into(),
            file_input_state: "unknown".into(),
            model_discovery: true,
            tool_protocol: tool_protocol.as_str().into(),
            evidence: "deterministic production-path test".into(),
            probe_status: "completed".into(),
            probe_version: 1,
            probed_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    #[tokio::test]
    async fn custom_profile_without_key_never_inherits_another_profiles_secret_or_header() {
        let (captured_tx, mut captured_rx) = mpsc::unbounded_channel();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(
                move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                    let captured_tx = captured_tx.clone();
                    async move {
                        captured_tx.send((headers, body)).unwrap();
                        Json(serde_json::json!({
                            "choices":[{"message":{"role":"assistant","content":"profile-b-ok"}}]
                        }))
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (auth, _directory) = test_auth();
        let owner = "owner:telegram:7";
        let profiles = ProviderProfileStore::new(auth.storage());
        let credential = auth
            .create_api_key_credential("custom", "profile-a", "SECRET_A")
            .unwrap();
        let profile_a = profiles
            .create(ProviderProfileInput {
                profile_id: None,
                owner_id: owner.into(),
                alias: "profile-a".into(),
                endpoint: "https://a.example/v1".into(),
                protocol: "openai_chat_completions".into(),
                credential_ref: Some(credential.id),
                api_key_ref: None,
                safe_headers_json: r#"{"X-Profile-A":"HEADER_A"}"#.into(),
                secret_headers_ref: None,
            })
            .unwrap();
        profiles
            .replace_models(
                owner,
                &profile_a.profile_id,
                &[profile_model(
                    &profile_a.profile_id,
                    "m",
                    ToolProtocol::ChatOnly,
                )],
            )
            .unwrap();
        let profile_b = profiles
            .create(ProviderProfileInput {
                profile_id: None,
                owner_id: owner.into(),
                alias: "profile-b".into(),
                endpoint: format!("http://{address}/v1"),
                protocol: "openai_chat_completions".into(),
                credential_ref: None,
                api_key_ref: None,
                safe_headers_json: "{}".into(),
                secret_headers_ref: None,
            })
            .unwrap();
        profiles
            .replace_models(
                owner,
                &profile_b.profile_id,
                &[profile_model(
                    &profile_b.profile_id,
                    "m",
                    ToolProtocol::ChatOnly,
                )],
            )
            .unwrap();

        let provider = CustomProvider::new(
            CustomProviderConfig {
                // A selected profile is independently usable even when the
                // singleton compatibility config is disabled.
                enabled: false,
                base_url: Some("https://legacy.example/v1".into()),
                protocol: "openai_chat_completions".into(),
                models: vec!["m".into()],
                ..Default::default()
            },
            auth,
        );
        assert!(provider.enabled());
        let mut req = request(vec![message("user", "hello")]);
        req.account_id = Some(profile_b.profile_id);
        req.model = "m".into();
        let answer = provider.run(req, None).await.unwrap();
        assert_eq!(answer.final_answer, "profile-b-ok");
        let (headers, body) = captured_rx.recv().await.unwrap();
        assert!(headers.get("authorization").is_none());
        assert!(headers.get("x-profile-a").is_none());
        let wire = serde_json::to_string(&body).unwrap();
        assert!(!wire.contains("SECRET_A"));
        assert!(!wire.contains("HEADER_A"));
        server.abort();
    }

    #[tokio::test]
    async fn production_custom_structured_fallback_retains_tool_a_and_b_results_until_final() {
        let (captured_tx, mut captured_rx) = mpsc::unbounded_channel();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let app = Router::new().route(
            "/v1/chat/completions",
            post({
                let calls = calls.clone();
                move |Json(body): Json<serde_json::Value>| {
                    let captured_tx = captured_tx.clone();
                    let calls = calls.clone();
                    async move {
                        captured_tx.send(body).unwrap();
                        let content = match calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                            0 => r#"{"kind":"tool_calls","calls":[{"call_id":"a","name":"tool_a","arguments":{"value":"first"}}]}"#,
                            1 => r#"{"kind":"tool_calls","calls":[{"call_id":"b","name":"tool_b","arguments":{"value":"second"}}]}"#,
                            _ => r#"{"kind":"final","text":"verified final"}"#,
                        };
                        Json(serde_json::json!({
                            "choices":[{"message":{"role":"assistant","content":content}}]
                        }))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (auth, _directory) = test_auth();
        let owner = "owner:telegram:8";
        let profiles = ProviderProfileStore::new(auth.storage());
        let profile = profiles
            .create(ProviderProfileInput {
                profile_id: None,
                owner_id: owner.into(),
                alias: "structured".into(),
                endpoint: format!("http://{address}/v1"),
                protocol: "openai_chat_completions".into(),
                credential_ref: None,
                api_key_ref: None,
                safe_headers_json: "{}".into(),
                secret_headers_ref: None,
            })
            .unwrap();
        profiles
            .replace_models(
                owner,
                &profile.profile_id,
                &[profile_model(
                    &profile.profile_id,
                    "m",
                    ToolProtocol::StructuredJsonFallback,
                )],
            )
            .unwrap();
        let provider = CustomProvider::new(
            CustomProviderConfig {
                enabled: true,
                base_url: Some("https://legacy.example/v1".into()),
                protocol: "openai_chat_completions".into(),
                models: vec!["m".into()],
                ..Default::default()
            },
            auth,
        );
        let mut req = request(vec![message("user", "run two steps")]);
        req.account_id = Some(profile.profile_id);
        req.model = "m".into();
        req.tools = vec![named_tool("tool_a"), named_tool("tool_b")];

        let first = provider
            .run_turn(req.clone(), None, vec![], None)
            .await
            .unwrap();
        assert!(
            matches!(first.step, ProviderStep::ToolCalls(ref calls) if calls[0].name == "tool_a")
        );
        let second = provider
            .run_turn(
                req.clone(),
                first.continuation,
                vec![ToolResult {
                    call_id: "a".into(),
                    name: "tool_a".into(),
                    output: "RESULT_A".into(),
                    is_error: false,
                }],
                None,
            )
            .await
            .unwrap();
        assert!(
            matches!(second.step, ProviderStep::ToolCalls(ref calls) if calls[0].name == "tool_b")
        );
        let third = provider
            .run_turn(
                req,
                second.continuation,
                vec![ToolResult {
                    call_id: "b".into(),
                    name: "tool_b".into(),
                    output: "RESULT_B".into(),
                    is_error: false,
                }],
                None,
            )
            .await
            .unwrap();
        assert!(matches!(third.step, ProviderStep::Final(ref text) if text == "verified final"));
        let _first_wire = captured_rx.recv().await.unwrap();
        let second_wire = serde_json::to_string(&captured_rx.recv().await.unwrap()).unwrap();
        let third_wire = serde_json::to_string(&captured_rx.recv().await.unwrap()).unwrap();
        assert!(second_wire.contains("RESULT_A"));
        assert!(third_wire.contains("RESULT_A"));
        assert!(third_wire.contains("RESULT_B"));
        server.abort();
    }

    #[tokio::test]
    async fn production_custom_vision_serializes_supported_and_unknown_images() {
        let (captured_tx, mut captured_rx) = mpsc::unbounded_channel();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(body): Json<serde_json::Value>| {
                let captured_tx = captured_tx.clone();
                async move {
                    captured_tx.send(body).unwrap();
                    Json(serde_json::json!({
                        "choices":[{"message":{"role":"assistant","content":"visible"}}]
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (auth, _directory) = test_auth();
        let owner = "owner:telegram:9";
        let profiles = ProviderProfileStore::new(auth.storage());
        let profile = profiles
            .create(ProviderProfileInput {
                profile_id: None,
                owner_id: owner.into(),
                alias: "vision".into(),
                endpoint: format!("http://{address}/v1"),
                protocol: "openai_chat_completions".into(),
                credential_ref: None,
                api_key_ref: None,
                safe_headers_json: "{}".into(),
                secret_headers_ref: None,
            })
            .unwrap();
        let mut vision = profile_model(&profile.profile_id, "vision-m", ToolProtocol::ChatOnly);
        vision.vision_capable = true;
        vision.vision_state = "supported".into();
        let no_vision = profile_model(&profile.profile_id, "text-m", ToolProtocol::ChatOnly);
        profiles
            .replace_models(owner, &profile.profile_id, &[vision, no_vision])
            .unwrap();
        let provider = CustomProvider::new(
            CustomProviderConfig {
                enabled: true,
                base_url: Some("https://legacy.example/v1".into()),
                protocol: "openai_chat_completions".into(),
                models: vec!["vision-m".into(), "text-m".into()],
                ..Default::default()
            },
            auth,
        );
        let png = STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
            .unwrap();
        let image = NormalizedImage {
            attachment_id: "image-a".into(),
            mime_type: "image/png".into(),
            bytes: png,
            width: 1,
            height: 1,
            caption: "What is visible?".into(),
        };
        let mut req = request(vec![message("user", "inspect this image")]);
        req.account_id = Some(profile.profile_id.clone());
        req.model = "vision-m".into();
        req.images = vec![image.clone()];
        assert_eq!(
            provider.run(req.clone(), None).await.unwrap().final_answer,
            "visible"
        );
        let body = captured_rx.recv().await.unwrap();
        let serialized = serde_json::to_string(&body).unwrap();
        assert!(serialized.contains("image_url"));
        assert!(serialized.contains("data:image/png;base64,"));
        assert!(serialized.contains("What is visible?"));

        req.model = "text-m".into();
        assert_eq!(
            provider.run(req, None).await.unwrap().final_answer,
            "visible"
        );
        let optimistic = captured_rx.recv().await.unwrap();
        assert!(serde_json::to_string(&optimistic)
            .unwrap()
            .contains("image_url"));
        server.abort();
    }

    #[test]
    fn provider_capabilities_are_explicit_and_antigravity_translates_tools() {
        let (auth, _directory) = test_auth();
        let codex = CodexProvider::new(true, None, None, auth.clone());
        let antigravity = AntigravityProvider::new(
            true,
            "https://example.invalid".into(),
            None,
            "test".into(),
            "test".into(),
            auth,
        );
        assert_eq!(codex.capabilities("m").tool_protocol, ToolProtocol::Native);
        assert!(!codex.capabilities("m").vision);
        assert!(codex.capabilities("gpt-5.6-sol").vision);
        assert_eq!(
            antigravity.capabilities("m").tool_protocol,
            ToolProtocol::Native
        );
        assert!(!antigravity.capabilities("claude-sonnet-4-6").vision);
        assert!(antigravity.capabilities("gemini-pro-agent").vision);
        let wire = antigravity_tool_specs(&[test_tool()]);
        assert_eq!(wire[0]["name"], "artifact_write");
        assert_eq!(wire[0]["parameters"]["required"][0], "path");
        assert!(wire[0].get("risk").is_none());
    }

    #[test]
    fn unprobed_custom_model_is_explicitly_chat_only() {
        let (auth, _directory) = test_auth();
        let provider = CustomProvider::new(
            crate::config::CustomProviderConfig {
                enabled: true,
                base_url: Some("https://custom.example/v1".into()),
                models: vec!["unprobed".into()],
                default_model: Some("unprobed".into()),
                tool_protocol: "auto".into(),
                ..crate::config::CustomProviderConfig::default()
            },
            auth,
        );
        let capability = provider.capabilities("unprobed");
        assert_eq!(capability.tool_protocol, ToolProtocol::ChatOnly);
        assert!(!capability.is_agent_capable());
        assert!(capability.evidence.contains("has not been probed"));
    }

    #[tokio::test]
    async fn custom_chat_native_tools_continue_with_normalized_results() {
        let (captured_tx, mut captured_rx) = mpsc::unbounded_channel();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let app = Router::new().route(
            "/v1/chat/completions",
            post({
                let calls = calls.clone();
                move |Json(body): Json<serde_json::Value>| {
                    let captured_tx = captured_tx.clone();
                    let calls = calls.clone();
                    async move {
                        captured_tx.send(body).unwrap();
                        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                            Json(serde_json::json!({
                                "choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{
                                    "id":"call-a","type":"function","function":{"name":"artifact_write","arguments":"{\"path\":\"result.txt\"}"}
                                }]}}]
                            }))
                        } else {
                            Json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":"verified"}}]}))
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (auth, _directory) = test_auth();
        auth.storage()
            .upsert_provider_capability(&crate::storage::ProviderCapabilityRecord {
                provider: "custom".into(),
                model: "m".into(),
                tool_protocol: "native".into(),
                native_tool_calls: true,
                structured_output: true,
                continuation: true,
                probe_status: "completed".into(),
                probe_version: 1,
                probed_at: chrono::Utc::now().to_rfc3339(),
                evidence: "deterministic exact-model probe fixture".into(),
            })
            .unwrap();
        let provider = CustomProvider::new(
            CustomProviderConfig {
                enabled: true,
                base_url: Some(format!("http://{address}/v1")),
                protocol: "openai_chat_completions".into(),
                tool_protocol: "native".into(),
                models: vec!["m".into()],
                ..Default::default()
            },
            auth,
        );
        let mut req = request(vec![message("user", "create result")]);
        req.model = "m".into();
        req.tools = vec![test_tool()];
        let first = provider
            .run_turn(req.clone(), None, vec![], None)
            .await
            .unwrap();
        let ProviderStep::ToolCalls(tool_calls) = first.step else {
            panic!("tool call expected")
        };
        assert_eq!(tool_calls[0].name, "artifact_write");
        let second = provider
            .run_turn(
                req,
                first.continuation,
                vec![ToolResult {
                    call_id: "call-a".into(),
                    name: "artifact_write".into(),
                    output: "created".into(),
                    is_error: false,
                }],
                None,
            )
            .await
            .unwrap();
        assert!(matches!(second.step, ProviderStep::Final(ref text) if text == "verified"));
        let first_wire = captured_rx.recv().await.unwrap();
        let second_wire = captured_rx.recv().await.unwrap();
        assert_eq!(first_wire["tools"][0]["function"]["name"], "artifact_write");
        assert!(second_wire["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool" && message["tool_call_id"] == "call-a"));
        server.abort();
    }

    #[tokio::test]
    async fn custom_capability_probe_prefers_native_then_validated_structured_fallback() {
        let native = Router::new().route(
            "/v1/chat/completions",
            post(|Json(body): Json<serde_json::Value>| async move {
                let content = body["messages"][0]["content"].as_str().unwrap();
                let nonce = content
                    .split("nonce ")
                    .nth(1)
                    .unwrap()
                    .split('.')
                    .next()
                    .unwrap();
                Json(serde_json::json!({
                    "choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{
                        "id":"probe","type":"function","function":{
                            "name":"xiao_capability_probe",
                            "arguments":serde_json::json!({"nonce":nonce}).to_string()
                        }
                    }]}}]
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, native).await.unwrap() });
        let capability = probe_custom_tool_capability(
            &format!("http://{address}/v1"),
            &Default::default(),
            None,
            "openai_chat_completions",
            "m",
        )
        .await;
        assert_eq!(capability.tool_protocol, ToolProtocol::Native);
        assert!(capability.continuation);
        server.abort();

        let structured = Router::new().route(
            "/v1/chat/completions",
            post(|Json(body): Json<serde_json::Value>| async move {
                if body.get("tools").is_some() {
                    return Json(serde_json::json!({
                        "choices":[{"message":{"role":"assistant","content":"native tools unsupported"}}]
                    }));
                }
                let instruction = body["messages"][0]["content"].as_str().unwrap();
                let exact = instruction
                    .strip_prefix("Return exactly this JSON object and nothing else: ")
                    .unwrap();
                Json(serde_json::json!({
                    "choices":[{"message":{"role":"assistant","content":exact}}]
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, structured).await.unwrap() });
        let capability = probe_custom_tool_capability(
            &format!("http://{address}/v1"),
            &Default::default(),
            None,
            "openai_chat_completions",
            "m",
        )
        .await;
        assert_eq!(
            capability.tool_protocol,
            ToolProtocol::StructuredJsonFallback
        );
        assert!(capability.structured_output);
        server.abort();
    }

    #[test]
    fn structured_fallback_is_strict_and_cannot_call_undeclared_tools() {
        let tools = vec![test_tool()];
        assert!(matches!(
            parse_structured_agent_output(
                r#"{"kind":"tool_calls","calls":[{"call_id":"1","name":"artifact_write","arguments":{"path":"a"}}]}"#,
                &tools
            )
            .unwrap(),
            ProviderStep::ToolCalls(_)
        ));
        assert!(parse_structured_agent_output(
            r#"{"kind":"tool_calls","calls":[{"call_id":"1","name":"shell","arguments":{"cmd":"id"}}]}"#,
            &tools
        )
        .is_err());
        assert!(parse_structured_agent_output("```json\n{}\n```", &tools).is_err());
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

    #[test]
    fn responses_server_tools_emit_semantic_progress_events() {
        assert_eq!(
            responses_tool_progress_event(&serde_json::json!({
                "type":"response.web_search_call.searching"
            })),
            Some(AgentEvent::ToolStarted("web_search".into()))
        );
        assert_eq!(
            responses_tool_progress_event(&serde_json::json!({
                "type":"response.web_search_call.completed"
            })),
            Some(AgentEvent::ToolCompleted {
                tool: "web_search".into(),
                summary: "completed".into(),
            })
        );
        assert_eq!(
            responses_tool_progress_event(&serde_json::json!({
                "type":"response.mcp_call.in_progress",
                "name":"web_fetch"
            })),
            Some(AgentEvent::ToolStarted("web_fetch".into()))
        );
    }

    #[test]
    fn unknown_optional_inputs_are_optimistically_routable() {
        let record = crate::storage::ProviderProfileModelRecord {
            profile_id: "profile-a".into(),
            model_id: "model-a".into(),
            text_capable: true,
            vision_capable: false,
            file_input_capable: false,
            native_tools: true,
            structured_output: true,
            continuation: true,
            native_tools_state: "supported".into(),
            structured_output_state: "supported".into(),
            continuation_state: "supported".into(),
            vision_state: "unknown".into(),
            file_input_state: "unknown".into(),
            model_discovery: true,
            tool_protocol: "native".into(),
            evidence: "probe inconclusive".into(),
            probe_status: "completed".into(),
            probe_version: 1,
            probed_at: "now".into(),
        };
        let capabilities = profile_capabilities_from_record(record);
        assert!(capabilities.vision);
        assert!(capabilities.file_input);
    }

    #[test]
    fn explicit_unsupported_optional_inputs_are_not_routable() {
        let mut record = crate::storage::ProviderProfileModelRecord {
            profile_id: "profile-a".into(),
            model_id: "model-a".into(),
            text_capable: true,
            vision_capable: false,
            file_input_capable: false,
            native_tools: true,
            structured_output: true,
            continuation: true,
            native_tools_state: "supported".into(),
            structured_output_state: "supported".into(),
            continuation_state: "supported".into(),
            vision_state: "unsupported".into(),
            file_input_state: "unsupported".into(),
            model_discovery: true,
            tool_protocol: "native".into(),
            evidence: "explicit provider rejection".into(),
            probe_status: "completed".into(),
            probe_version: 1,
            probed_at: "now".into(),
        };
        let capabilities = profile_capabilities_from_record(record.clone());
        assert!(!capabilities.vision);
        assert!(!capabilities.file_input);
        record.vision_state = "supported".into();
        assert!(profile_capabilities_from_record(record).vision);
    }

    #[tokio::test]
    async fn custom_chat_non_streaming_when_requested_emits_no_deltas_and_returns_final_turn() {
        let (captured_stream_flag_tx, mut captured_stream_flag_rx) = mpsc::unbounded_channel();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(body): Json<serde_json::Value>| {
                let captured_tx = captured_stream_flag_tx.clone();
                async move {
                    let is_stream = body
                        .get("stream")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    captured_tx.send(is_stream).unwrap();
                    Json(serde_json::json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "chat non-stream result"
                            }
                        }]
                    }))
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
            protocol: "openai_chat_completions".into(),
            ..Default::default()
        };
        let provider = CustomProvider::new(config, auth);
        let mut req = request(vec![message("user", "hello non-streaming")]);
        req.streaming = false;

        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let turn = provider
            .run_turn(req, None, vec![], Some(progress_tx))
            .await
            .unwrap();

        match turn.step {
            ProviderStep::Final(answer) => assert_eq!(answer, "chat non-stream result"),
            _ => panic!("expected final answer"),
        }
        let stream_flag = captured_stream_flag_rx.recv().await.unwrap();
        assert!(!stream_flag);

        let mut events = Vec::new();
        while let Ok(event) = progress_rx.try_recv() {
            events.push(event);
        }
        assert!(!events.iter().any(|e| matches!(e, AgentEvent::TextDelta(_))));
        server.abort();
    }

    #[tokio::test]
    async fn custom_responses_non_streaming_when_requested_emits_no_deltas_and_returns_final_turn()
    {
        let (captured_stream_flag_tx, mut captured_stream_flag_rx) = mpsc::unbounded_channel();
        let app = Router::new().route(
            "/v1/responses",
            post(move |Json(body): Json<serde_json::Value>| {
                let captured_tx = captured_stream_flag_tx.clone();
                async move {
                    let is_stream = body
                        .get("stream")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    captured_tx.send(is_stream).unwrap();
                    Json(serde_json::json!({
                        "output_text": "responses non-stream result"
                    }))
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
        let mut req = request(vec![message("user", "hello responses non-streaming")]);
        req.streaming = false;

        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let turn = provider
            .run_turn(req, None, vec![], Some(progress_tx))
            .await
            .unwrap();

        match turn.step {
            ProviderStep::Final(answer) => assert_eq!(answer, "responses non-stream result"),
            _ => panic!("expected final answer"),
        }
        let stream_flag = captured_stream_flag_rx.recv().await.unwrap();
        assert!(!stream_flag);

        let mut events = Vec::new();
        while let Ok(event) = progress_rx.try_recv() {
            events.push(event);
        }
        assert!(!events.iter().any(|e| matches!(e, AgentEvent::TextDelta(_))));
        server.abort();
    }

    #[tokio::test]
    async fn custom_chat_streaming_fallback_retries_once_on_explicit_unsupported_error() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let attempt = Arc::new(AtomicUsize::new(0));
        let attempt_clone = attempt.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(body): Json<serde_json::Value>| {
                let attempt = attempt_clone.clone();
                async move {
                    let current = attempt.fetch_add(1, Ordering::SeqCst);
                    if current == 0 {
                        assert_eq!(body["stream"], true);
                        (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({
                                "error": { "message": "streaming is not supported on this model endpoint" }
                            })),
                        )
                    } else {
                        assert_eq!(body["stream"], false);
                        (
                            StatusCode::OK,
                            Json(serde_json::json!({
                                "choices": [{
                                    "message": {
                                        "role": "assistant",
                                        "content": "fallback non-stream works"
                                    }
                                }]
                            })),
                        )
                    }
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
            protocol: "openai_chat_completions".into(),
            ..Default::default()
        };
        let provider = CustomProvider::new(config, auth);
        let req = request(vec![message("user", "hello with auto streaming")]);

        let turn = provider.run_turn(req, None, vec![], None).await.unwrap();
        match turn.step {
            ProviderStep::Final(answer) => assert_eq!(answer, "fallback non-stream works"),
            _ => panic!("expected final answer"),
        }
        assert_eq!(attempt.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn custom_chat_streaming_success_records_streaming_supported_capability() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|Json(body): Json<serde_json::Value>| async move {
                assert_eq!(body["stream"], true);
                let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"streamed success\"}}]}\n\ndata: [DONE]\n\n";
                axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(axum::body::Body::from(sse_body))
                    .unwrap()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (auth, _directory) = test_auth();
        let storage = auth.storage();
        let profiles = ProviderProfileStore::new(storage.clone());
        let owner = "owner:stream";
        let profile = profiles
            .create(ProviderProfileInput {
                profile_id: None,
                owner_id: owner.into(),
                alias: "streamer".into(),
                endpoint: format!("http://{address}/v1"),
                protocol: "openai_chat_completions".into(),
                credential_ref: None,
                api_key_ref: None,
                safe_headers_json: "{}".into(),
                secret_headers_ref: None,
            })
            .unwrap();
        profiles
            .replace_models(
                owner,
                &profile.profile_id,
                &[profile_model(
                    &profile.profile_id,
                    "stream-model",
                    ToolProtocol::Native,
                )],
            )
            .unwrap();

        let provider = CustomProvider::new(
            CustomProviderConfig {
                enabled: true,
                base_url: Some(format!("http://{address}/v1")),
                protocol: "openai_chat_completions".into(),
                models: vec!["stream-model".into()],
                ..Default::default()
            },
            auth,
        );
        let mut req = request(vec![message("user", "hello streaming capability test")]);
        req.model = "stream-model".into();
        req.account_id = Some(profile.profile_id.clone());

        let turn = provider.run_turn(req, None, vec![], None).await.unwrap();
        match turn.step {
            ProviderStep::Final(answer) => assert_eq!(answer, "streamed success"),
            _ => panic!("expected final answer"),
        }

        let evidence = storage
            .get_capability_evidence(
                &profile.profile_id,
                "stream-model",
                "openai_chat_completions",
                "streaming",
            )
            .unwrap()
            .unwrap();
        assert_eq!(evidence.state, "supported");
        assert_eq!(evidence.source, "runtime_success");

        server.abort();
    }
}
