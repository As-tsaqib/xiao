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
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let config: Self =
            toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
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
            return Err(anyhow!("ipc.bind must be loopback-only in v0.1.0"));
        }
        if self.telegram.transport != "long_polling" {
            return Err(anyhow!("telegram.transport must be long_polling in v0.1.0"));
        }
        if self.telegram.ui.menu_ttl_seconds == 0 {
            return Err(anyhow!("telegram.ui.menu_ttl_seconds must be > 0"));
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
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,
    #[serde(default)]
    pub allowed_user_ids: Vec<i64>,
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
}
