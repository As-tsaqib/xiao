use std::{
    collections::BTreeSet,
    fs,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub ipc: IpcConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub paths: PathsConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub attachments: AttachmentConfig,
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let mut config: Self =
            toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
        // v0.2.7 config migration: a single legacy allowed_user_ids entry is
        // unambiguous and becomes the canonical owner. Multiple entries are
        // intentionally preserved as a resolution-required state; Xiao must
        // never silently choose one.
        config.telegram.access.migrate_legacy_owner();
        config.validate()?;
        Ok(config)
    }

    pub fn save_atomic(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, toml::to_string_pretty(self)?)?;
        set_private_file(&tmp)?;
        fs::rename(&tmp, path)?;
        set_private_file(path)?;
        Ok(())
    }

    pub fn standalone(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        let mut config = Self::default();
        config.storage.database = data_dir.join("data/xiao.db");
        config.paths.logs_dir = data_dir.join("logs");
        config.paths.secrets_dir = data_dir.join("secrets");
        config.paths.data_dir = data_dir;
        config.telegram.enabled = false;
        config
    }

    pub fn validate(&self) -> Result<()> {
        let addr: SocketAddr = self.ipc.bind.parse().context("invalid ipc.bind")?;
        if !addr.ip().is_loopback() {
            return Err(anyhow!("ipc.bind must be loopback-only"));
        }
        if self.telegram.transport != "long_polling" {
            return Err(anyhow!("telegram.transport must be long_polling"));
        }
        if self.telegram.access.owner_user_id.is_some_and(|id| id <= 0) {
            return Err(anyhow!("telegram.access.owner_user_id must be positive"));
        }
        if self.telegram.access.allowed_chat_ids.contains(&0) {
            return Err(anyhow!(
                "telegram.access.allowed_chat_ids cannot contain zero"
            ));
        }
        if self.telegram.ui.menu_ttl_seconds == 0 {
            return Err(anyhow!("telegram.ui.menu_ttl_seconds must be > 0"));
        }
        if !(2..=32).contains(&self.agent.max_turns) {
            return Err(anyhow!("agent.max_turns must be between 2 and 32"));
        }
        if !(1..=256).contains(&self.agent.max_tool_calls) {
            return Err(anyhow!("agent.max_tool_calls must be between 1 and 256"));
        }
        if !(1..=8).contains(&self.agent.max_no_progress_repeats) {
            return Err(anyhow!(
                "agent.max_no_progress_repeats must be between 1 and 8"
            ));
        }
        if !(10..=3_600).contains(&self.agent.max_runtime_seconds) {
            return Err(anyhow!(
                "agent.max_runtime_seconds must be between 10 and 3600"
            ));
        }
        if !(4_096..=1_000_000).contains(&self.agent.context_max_chars) {
            return Err(anyhow!(
                "agent.context_max_chars must be between 4096 and 1000000"
            ));
        }
        if self.agent.summary_threshold_chars < 1_024
            || self.agent.summary_threshold_chars > self.agent.context_max_chars
        {
            return Err(anyhow!(
                "agent.summary_threshold_chars must be at least 1024 and not exceed agent.context_max_chars"
            ));
        }
        if !(64 * 1024..=100 * 1024 * 1024).contains(&self.attachments.max_image_bytes)
            || !(64 * 1024..=200 * 1024 * 1024).contains(&self.attachments.max_document_bytes)
            || self.attachments.max_session_bytes < self.attachments.max_document_bytes
            || self.attachments.max_session_bytes > 1024 * 1024 * 1024
            || self.attachments.max_owner_bytes < self.attachments.max_session_bytes
            || self.attachments.max_global_bytes < self.attachments.max_owner_bytes
            || self.attachments.max_global_bytes > 16 * 1024 * 1024 * 1024
        {
            return Err(anyhow!("attachment byte limits are invalid"));
        }
        if !(1_000_000..=100_000_000).contains(&self.attachments.max_image_pixels)
            || !(512..=16_384).contains(&self.attachments.chunk_chars)
            || !(1..=20).contains(&self.attachments.retrieval_chunks)
            || !(1..=120).contains(&self.attachments.processing_timeout_seconds)
            || !(1..=200).contains(&self.attachments.max_pdf_pages)
            || !(1_000_000..=100_000_000).contains(&self.attachments.max_pdf_page_pixels)
            || !(1..=120).contains(&self.attachments.ocr_page_timeout_seconds)
            || !(1..=3650).contains(&self.attachments.retention_days)
        {
            return Err(anyhow!("attachment processing limits are invalid"));
        }
        if !(1_024..=65_536).contains(&self.agent.tool_output_max_chars) {
            return Err(anyhow!(
                "agent.tool_output_max_chars must be between 1024 and 65536"
            ));
        }
        if !matches!(
            self.telegram.ui.progress_detail.as_str(),
            "minimal" | "normal" | "detailed"
        ) {
            return Err(anyhow!(
                "telegram.ui.progress_detail must be minimal, normal, or detailed"
            ));
        }
        if !matches!(
            self.telegram.ui.menu_close_behavior.as_str(),
            "keep_summary" | "remove_keyboard" | "delete_message"
        ) {
            return Err(anyhow!("telegram.ui.menu_close_behavior is invalid"));
        }
        if !matches!(
            self.providers.custom.protocol.as_str(),
            "openai_responses" | "openai_chat_completions"
        ) {
            return Err(anyhow!("unsupported custom provider protocol"));
        }
        if !matches!(
            self.providers.custom.tool_protocol.as_str(),
            "auto" | "native" | "structured_json" | "chat_only"
        ) {
            return Err(anyhow!(
                "providers.custom.tool_protocol must be auto, native, structured_json, or chat_only"
            ));
        }
        if self.providers.custom.enabled
            && self
                .providers
                .custom
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .is_none()
        {
            return Err(anyhow!(
                "providers.custom.base_url is required when the custom provider is enabled"
            ));
        }
        if self.providers.custom.enabled
            && self.providers.custom.models.is_empty()
            && self
                .providers
                .custom
                .default_model
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .is_none()
        {
            return Err(anyhow!(
                "configure at least one custom model or a custom default_model"
            ));
        }
        if let Some(base) = self.providers.custom.base_url.as_deref() {
            let parsed = url::Url::parse(base).context("invalid custom provider base_url")?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(anyhow!("custom provider base_url must use http/https"));
            }
        }
        for (name, value) in &self.providers.custom.headers {
            if name.trim().is_empty()
                || name.contains('\r')
                || name.contains('\n')
                || value.contains('\r')
                || value.contains('\n')
            {
                return Err(anyhow!("invalid custom provider header"));
            }
            if matches!(
                name.trim().to_ascii_lowercase().as_str(),
                "authorization" | "proxy-authorization" | "x-api-key" | "api-key"
            ) {
                return Err(anyhow!("secret-bearing custom provider headers must use the API Key field/secret storage"));
            }
        }
        for (label, value) in [
            ("antigravity.auth_url", &self.providers.antigravity.auth_url),
            (
                "antigravity.token_url",
                &self.providers.antigravity.token_url,
            ),
            (
                "antigravity.userinfo_url",
                &self.providers.antigravity.userinfo_url,
            ),
            (
                "antigravity.codeassist_base",
                &self.providers.antigravity.codeassist_base,
            ),
            (
                "antigravity.daily_base",
                &self.providers.antigravity.daily_base,
            ),
        ] {
            let u = url::Url::parse(value).with_context(|| format!("invalid {label}"))?;
            if u.scheme() != "https" {
                return Err(anyhow!("{label} must use https"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentConfig {
    #[serde(default = "default_attachment_image_bytes")]
    pub max_image_bytes: u64,
    #[serde(default = "default_attachment_document_bytes")]
    pub max_document_bytes: u64,
    #[serde(default = "default_attachment_session_bytes")]
    pub max_session_bytes: u64,
    #[serde(default = "default_attachment_owner_bytes")]
    pub max_owner_bytes: u64,
    #[serde(default = "default_attachment_global_bytes")]
    pub max_global_bytes: u64,
    #[serde(default = "default_attachment_retention_days")]
    pub retention_days: u64,
    #[serde(default)]
    pub retain_failed: bool,
    #[serde(default = "default_attachment_pdf_pages")]
    pub max_pdf_pages: usize,
    #[serde(default = "default_attachment_pdf_page_pixels")]
    pub max_pdf_page_pixels: u64,
    #[serde(default = "default_attachment_ocr_timeout")]
    pub ocr_page_timeout_seconds: u64,
    #[serde(default = "default_attachment_image_pixels")]
    pub max_image_pixels: u64,
    #[serde(default = "default_attachment_text_chars")]
    pub max_extracted_text_chars: usize,
    #[serde(default = "default_attachment_chunk_chars")]
    pub chunk_chars: usize,
    #[serde(default = "default_attachment_retrieval_chunks")]
    pub retrieval_chunks: usize,
    #[serde(default = "default_attachment_processing_timeout")]
    pub processing_timeout_seconds: u64,
}

impl Default for AttachmentConfig {
    fn default() -> Self {
        Self {
            max_image_bytes: default_attachment_image_bytes(),
            max_document_bytes: default_attachment_document_bytes(),
            max_session_bytes: default_attachment_session_bytes(),
            max_owner_bytes: default_attachment_owner_bytes(),
            max_global_bytes: default_attachment_global_bytes(),
            retention_days: default_attachment_retention_days(),
            retain_failed: false,
            max_pdf_pages: default_attachment_pdf_pages(),
            max_pdf_page_pixels: default_attachment_pdf_page_pixels(),
            ocr_page_timeout_seconds: default_attachment_ocr_timeout(),
            max_image_pixels: default_attachment_image_pixels(),
            max_extracted_text_chars: default_attachment_text_chars(),
            chunk_chars: default_attachment_chunk_chars(),
            retrieval_chunks: default_attachment_retrieval_chunks(),
            processing_timeout_seconds: default_attachment_processing_timeout(),
        }
    }
}

/// Bounded runtime controls. Defaults are deliberately conservative and are
/// applied when upgrading a v0.1 configuration that has no `[agent]` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentConfig {
    #[serde(default = "default_agent_max_turns")]
    pub max_turns: usize,
    #[serde(default = "default_agent_max_tool_calls")]
    pub max_tool_calls: usize,
    #[serde(default = "default_agent_no_progress_repeats")]
    pub max_no_progress_repeats: usize,
    #[serde(default = "default_agent_runtime_seconds")]
    pub max_runtime_seconds: u64,
    #[serde(default = "default_context_max_chars")]
    pub context_max_chars: usize,
    #[serde(default = "default_summary_threshold_chars")]
    pub summary_threshold_chars: usize,
    #[serde(default = "default_tool_output_max_chars")]
    pub tool_output_max_chars: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: default_agent_max_turns(),
            max_tool_calls: default_agent_max_tool_calls(),
            max_no_progress_repeats: default_agent_no_progress_repeats(),
            max_runtime_seconds: default_agent_runtime_seconds(),
            context_max_chars: default_context_max_chars(),
            summary_threshold_chars: default_summary_threshold_chars(),
            tool_output_max_chars: default_tool_output_max_chars(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub auto_restart: bool,
}
impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_restart: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "default_log")]
    pub log_level: String,
}
impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            log_level: default_log(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_transport")]
    pub transport: String,
    #[serde(default)]
    pub access: TelegramAccess,
    #[serde(default)]
    pub ui: TelegramUi,
}
impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            transport: default_transport(),
            access: TelegramAccess::default(),
            ui: TelegramUi::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TelegramAccess {
    /// Canonical Xiao owner. Telegram authorization is always bound to this
    /// user id; chat ids only constrain where this owner may interact.
    #[serde(default)]
    pub owner_user_id: Option<i64>,
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,
    /// Legacy v0.2.6 migration input. Never emit it again. Zero means setup is
    /// required, one is migrated automatically, and multiple require explicit
    /// owner resolution via the shared setup service.
    #[serde(default, skip_serializing)]
    pub allowed_user_ids: Vec<i64>,
}

impl TelegramAccess {
    pub fn migrate_legacy_owner(&mut self) {
        self.allowed_chat_ids.sort_unstable();
        self.allowed_chat_ids.dedup();
        self.allowed_user_ids.sort_unstable();
        self.allowed_user_ids.dedup();
        if self.owner_user_id.is_none() && self.allowed_user_ids.len() == 1 {
            self.owner_user_id = self.allowed_user_ids.first().copied();
            self.allowed_user_ids.clear();
        } else if self.owner_user_id.is_some() {
            // An explicit canonical owner resolves any stale legacy list.
            self.allowed_user_ids.clear();
        }
    }

    pub fn owner_resolution_required(&self) -> bool {
        self.owner_user_id.is_none() && self.allowed_user_ids.len() > 1
    }

    pub fn setup_required(&self) -> bool {
        self.owner_user_id.is_none() && self.allowed_user_ids.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramUi {
    #[serde(default = "default_menu_ttl")]
    pub menu_ttl_seconds: u64,
    #[serde(default = "default_progress_detail")]
    pub progress_detail: String,
    #[serde(default = "default_close_behavior")]
    pub menu_close_behavior: String,
}
impl Default for TelegramUi {
    fn default() -> Self {
        Self {
            menu_ttl_seconds: default_menu_ttl(),
            progress_detail: default_progress_detail(),
            menu_close_behavior: default_close_behavior(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
}
impl Default for IpcConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
        }
    }
}
impl IpcConfig {
    pub fn socket_addr(&self) -> Result<SocketAddr> {
        Ok(self.bind.parse()?)
    }
    pub fn ip(&self) -> Result<IpAddr> {
        Ok(self.socket_addr()?.ip())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_database")]
    pub database: PathBuf,
}
impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            database: default_database(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default = "default_logs_dir")]
    pub logs_dir: PathBuf,
    #[serde(default = "default_secrets_dir")]
    pub secrets_dir: PathBuf,
}
impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            logs_dir: default_logs_dir(),
            secrets_dir: default_secrets_dir(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProvidersConfig {
    #[serde(default)]
    pub codex: ProviderEndpointConfig,
    #[serde(default)]
    pub antigravity: AntigravityProviderConfig,
    #[serde(default)]
    pub custom: CustomProviderConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityProviderConfig {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    /// Optional Desktop OAuth client override. When unset, xiao uses the same
    /// installed-app client as CLIProxyAPI's Antigravity authenticator.
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    #[serde(default = "default_agy_scopes")]
    pub oauth_scopes: Vec<String>,
    #[serde(default = "default_google_auth_url")]
    pub auth_url: String,
    #[serde(default = "default_google_token_url")]
    pub token_url: String,
    #[serde(default = "default_google_userinfo_url")]
    pub userinfo_url: String,
    #[serde(default = "default_agy_codeassist_base")]
    pub codeassist_base: String,
    #[serde(default = "default_agy_daily_base")]
    pub daily_base: String,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default = "default_agy_x_goog")]
    pub x_goog_api_client: String,
}
impl Default for AntigravityProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_model: None,
            base_url: None,
            oauth_client_id: None,
            oauth_scopes: default_agy_scopes(),
            auth_url: default_google_auth_url(),
            token_url: default_google_token_url(),
            userinfo_url: default_google_userinfo_url(),
            codeassist_base: default_agy_codeassist_base(),
            daily_base: default_agy_daily_base(),
            user_agent: None,
            x_goog_api_client: default_agy_x_goog(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderEndpointConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}
impl Default for ProviderEndpointConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_model: None,
            base_url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProviderConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default = "default_custom_protocol")]
    pub protocol: String,
    /// Agent protocol selected by a capability probe. `auto` prefers the
    /// provider's native OpenAI-compatible function/tool protocol.
    #[serde(default = "default_custom_tool_protocol")]
    pub tool_protocol: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
}
impl Default for CustomProviderConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            name: Some("Custom".into()),
            base_url: None,
            protocol: default_custom_protocol(),
            tool_protocol: default_custom_tool_protocol(),
            models: vec![],
            default_model: None,
            headers: std::collections::BTreeMap::new(),
        }
    }
}

pub fn parse_id_list(input: &str) -> Result<Vec<i64>> {
    let mut out = BTreeSet::new();
    for raw in input.split(',') {
        let s = raw.trim();
        if s.is_empty() {
            continue;
        }
        let id = s
            .parse::<i64>()
            .with_context(|| format!("invalid signed Telegram ID: {s}"))?;
        if id == 0 {
            return Err(anyhow!("Telegram IDs must be non-zero"));
        }
        out.insert(id);
    }
    Ok(out.into_iter().collect())
}

fn yes() -> bool {
    true
}
fn default_log() -> String {
    "info".into()
}
fn default_transport() -> String {
    "long_polling".into()
}
fn default_menu_ttl() -> u64 {
    900
}
fn default_progress_detail() -> String {
    "normal".into()
}
fn default_close_behavior() -> String {
    "keep_summary".into()
}
fn default_bind() -> String {
    "127.0.0.1:37921".into()
}
fn default_database() -> PathBuf {
    PathBuf::from("/data/adb/xiao/data/xiao.db")
}
fn default_data_dir() -> PathBuf {
    PathBuf::from("/data/adb/xiao")
}
fn default_logs_dir() -> PathBuf {
    PathBuf::from("/data/adb/xiao/logs")
}
fn default_secrets_dir() -> PathBuf {
    PathBuf::from("/data/adb/xiao/secrets")
}
fn default_custom_protocol() -> String {
    "openai_chat_completions".into()
}
fn default_custom_tool_protocol() -> String {
    "auto".into()
}
fn default_agent_max_turns() -> usize {
    8
}
fn default_agent_max_tool_calls() -> usize {
    32
}
fn default_agent_no_progress_repeats() -> usize {
    2
}
fn default_agent_runtime_seconds() -> u64 {
    600
}
fn default_context_max_chars() -> usize {
    32_000
}
fn default_summary_threshold_chars() -> usize {
    24_000
}
fn default_tool_output_max_chars() -> usize {
    4_096
}
fn default_attachment_image_bytes() -> u64 {
    20 * 1024 * 1024
}
fn default_attachment_document_bytes() -> u64 {
    50 * 1024 * 1024
}
fn default_attachment_session_bytes() -> u64 {
    200 * 1024 * 1024
}
fn default_attachment_owner_bytes() -> u64 {
    1024 * 1024 * 1024
}
fn default_attachment_global_bytes() -> u64 {
    2 * 1024 * 1024 * 1024
}
fn default_attachment_retention_days() -> u64 {
    30
}
fn default_attachment_pdf_pages() -> usize {
    40
}
fn default_attachment_pdf_page_pixels() -> u64 {
    20_000_000
}
fn default_attachment_ocr_timeout() -> u64 {
    20
}
fn default_attachment_image_pixels() -> u64 {
    40_000_000
}
fn default_attachment_text_chars() -> usize {
    2_000_000
}
fn default_attachment_chunk_chars() -> usize {
    4_000
}
fn default_attachment_retrieval_chunks() -> usize {
    6
}
fn default_attachment_processing_timeout() -> u64 {
    30
}
fn default_google_auth_url() -> String {
    "https://accounts.google.com/o/oauth2/v2/auth".into()
}
fn default_google_token_url() -> String {
    "https://oauth2.googleapis.com/token".into()
}
fn default_google_userinfo_url() -> String {
    "https://www.googleapis.com/oauth2/v2/userinfo?alt=json".into()
}
fn default_agy_codeassist_base() -> String {
    "https://cloudcode-pa.googleapis.com".into()
}
fn default_agy_daily_base() -> String {
    "https://daily-cloudcode-pa.googleapis.com".into()
}
fn default_agy_x_goog() -> String {
    "gl-node/22.21.1".into()
}
fn default_agy_scopes() -> Vec<String> {
    vec![
        "https://www.googleapis.com/auth/cloud-platform".into(),
        "https://www.googleapis.com/auth/userinfo.email".into(),
        "https://www.googleapis.com/auth/userinfo.profile".into(),
        "https://www.googleapis.com/auth/cclog".into(),
        "https://www.googleapis.com/auth/experimentsandconfigs".into(),
    ]
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_signed_deduped_ids() {
        assert_eq!(
            parse_id_list("123, -1005, 123, 9").unwrap(),
            vec![-1005, 9, 123]
        );
    }
    #[test]
    fn rejects_non_loopback() {
        let mut c = AppConfig::default();
        c.ipc.bind = "0.0.0.0:37921".into();
        assert!(c.validate().is_err());
    }
    #[test]
    fn enabled_custom_requires_endpoint_and_model() {
        let mut c = AppConfig::default();
        c.providers.custom.enabled = true;
        assert!(c.validate().is_err());
        c.providers.custom.base_url = Some("http://127.0.0.1:9000/v1".into());
        assert!(c.validate().is_err());
        c.providers.custom.models.push("local-model".into());
        assert!(c.validate().is_ok());
    }
    #[test]
    fn custom_secret_headers_are_rejected_from_plain_config() {
        let mut c = AppConfig::default();
        c.providers
            .custom
            .headers
            .insert("Authorization".into(), "Bearer do-not-store-here".into());
        assert!(c.validate().is_err());
    }

    #[test]
    fn standalone_config_keeps_all_mutable_state_under_its_data_dir() {
        let c = AppConfig::standalone("/tmp/xiao-user");
        assert_eq!(c.paths.data_dir, PathBuf::from("/tmp/xiao-user"));
        assert_eq!(c.paths.logs_dir, PathBuf::from("/tmp/xiao-user/logs"));
        assert_eq!(c.paths.secrets_dir, PathBuf::from("/tmp/xiao-user/secrets"));
        assert_eq!(
            c.storage.database,
            PathBuf::from("/tmp/xiao-user/data/xiao.db")
        );
        assert!(!c.telegram.enabled);
    }

    #[test]
    fn legacy_owner_migration_requires_exactly_one_or_explicit_resolution() {
        let mut zero = TelegramAccess::default();
        zero.migrate_legacy_owner();
        assert!(zero.setup_required());
        assert!(!zero.owner_resolution_required());

        let mut one = TelegramAccess {
            owner_user_id: None,
            allowed_user_ids: vec![42],
            allowed_chat_ids: vec![-100],
        };
        one.migrate_legacy_owner();
        assert_eq!(one.owner_user_id, Some(42));
        assert!(one.allowed_user_ids.is_empty());
        assert_eq!(one.allowed_chat_ids, vec![-100]);

        let mut many = TelegramAccess {
            owner_user_id: None,
            allowed_user_ids: vec![42, 43],
            allowed_chat_ids: vec![-100],
        };
        many.migrate_legacy_owner();
        assert_eq!(many.owner_user_id, None);
        assert!(many.owner_resolution_required());
        assert!(!many.setup_required());
        assert_eq!(many.allowed_user_ids, vec![42, 43]);
    }

    #[test]
    fn explicit_owner_clears_legacy_owner_candidates() {
        let mut access = TelegramAccess {
            owner_user_id: Some(99),
            allowed_user_ids: vec![42, 43],
            allowed_chat_ids: vec![-100],
        };
        access.migrate_legacy_owner();
        assert_eq!(access.owner_user_id, Some(99));
        assert!(access.allowed_user_ids.is_empty());
        assert!(!access.owner_resolution_required());
    }

    #[test]
    fn legacy_configuration_gets_bounded_agent_defaults() {
        let config: AppConfig = toml::from_str("[ipc]\nbind='127.0.0.1:37921'\n").unwrap();
        assert_eq!(config.agent, AgentConfig::default());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_unbounded_agent_runtime_configuration() {
        let mut config = AppConfig::default();
        config.agent.max_turns = 0;
        assert!(config.validate().is_err());
        config.agent = AgentConfig::default();
        config.agent.summary_threshold_chars = config.agent.context_max_chars + 1;
        assert!(config.validate().is_err());
    }
}
