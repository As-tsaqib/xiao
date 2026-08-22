use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{broadcast, RwLock};

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{distributions::Alphanumeric, Rng};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

use crate::{
    config::AppConfig,
    security::secrets::SecretStore,
    storage::{AccountRecord, Storage},
};

const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CODEX_DEVICE_USER_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const CODEX_DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const CODEX_DEVICE_VERIFY_URL: &str = "https://auth.openai.com/codex/device";
const CODEX_OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthChallenge {
    BrowserUrl {
        url: String,
        transaction_id: String,
    },
    DeviceCode {
        verification_url: String,
        user_code: String,
        transaction_id: String,
        interval_seconds: u64,
    },
    ApiKey {
        provider: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub provider: String,
    pub account_id: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expires_at_unix: Option<i64>,
    pub account_native_id: Option<String>,
    pub project_id: Option<String>,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone)]
enum TxnKind {
    CodexDevice {
        device_auth_id: String,
        user_code: String,
    },
    AntigravityPkce {
        verifier: String,
        state: String,
        redirect_uri: String,
    },
}

#[derive(Debug, Clone)]
struct AuthTxn {
    provider: String,
    kind: TxnKind,
}

#[derive(Debug, Clone)]
pub enum AuthEvent {
    Completed {
        transaction_id: String,
        account: AccountRecord,
    },
    Failed {
        transaction_id: String,
        provider: String,
        error: String,
    },
}

pub struct AuthManager {
    storage: Arc<Storage>,
    secrets: SecretStore,
    txns: Mutex<HashMap<String, AuthTxn>>,
    client: Client,
    events: broadcast::Sender<AuthEvent>,
    antigravity_redirect_uri: String,
    config: Arc<RwLock<AppConfig>>,
    refresh_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl AuthManager {
    pub fn new(storage: Arc<Storage>, secrets_dir: std::path::PathBuf) -> Self {
        Self::with_config(
            storage,
            secrets_dir,
            "http://127.0.0.1:37921/v1/auth/antigravity/browser-callback".into(),
            Arc::new(RwLock::new(AppConfig::default())),
        )
    }

    pub fn with_redirect_uri(
        storage: Arc<Storage>,
        secrets_dir: std::path::PathBuf,
        antigravity_redirect_uri: String,
    ) -> Self {
        Self::with_config(
            storage,
            secrets_dir,
            antigravity_redirect_uri,
            Arc::new(RwLock::new(AppConfig::default())),
        )
    }

    pub fn with_config(
        storage: Arc<Storage>,
        secrets_dir: std::path::PathBuf,
        antigravity_redirect_uri: String,
        config: Arc<RwLock<AppConfig>>,
    ) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            storage,
            secrets: SecretStore::new(secrets_dir),
            txns: Mutex::new(HashMap::new()),
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("http client"),
            events,
            antigravity_redirect_uri,
            config,
            refresh_locks: Mutex::new(HashMap::new()),
        }
    }

    pub async fn begin_login(&self, provider: &str) -> Result<AuthChallenge> {
        match provider {
            "codex" => self.begin_codex_device().await,
            "antigravity" | "agy" => self.begin_antigravity().await,
            "custom" => Ok(AuthChallenge::ApiKey {
                provider: "custom".into(),
            }),
            _ => Err(anyhow!("unknown provider")),
        }
    }

    async fn begin_codex_device(&self) -> Result<AuthChallenge> {
        #[derive(Serialize)]
        struct Req<'a> {
            client_id: &'a str,
        }
        #[derive(Deserialize)]
        struct Resp {
            device_auth_id: String,
            #[serde(alias = "usercode")]
            user_code: String,
            interval: serde_json::Value,
        }
        let r = self
            .client
            .post(CODEX_DEVICE_USER_CODE_URL)
            .json(&Req {
                client_id: CODEX_CLIENT_ID,
            })
            .send()
            .await?
            .error_for_status()?;
        let body: Resp = r.json().await?;
        let interval = parse_interval(&body.interval).unwrap_or(5);
        let txid = Uuid::new_v4().to_string();
        self.txns.lock().unwrap().insert(
            txid.clone(),
            AuthTxn {
                provider: "codex".into(),
                kind: TxnKind::CodexDevice {
                    device_auth_id: body.device_auth_id,
                    user_code: body.user_code.clone(),
                },
            },
        );
        Ok(AuthChallenge::DeviceCode {
            verification_url: CODEX_DEVICE_VERIFY_URL.into(),
            user_code: body.user_code,
            transaction_id: txid,
            interval_seconds: interval,
        })
    }

    async fn begin_antigravity(&self) -> Result<AuthChallenge> {
        let agy = self.config.read().await.providers.antigravity.clone();
        let client_id=agy.oauth_client_id.as_deref().map(str::trim).filter(|x|!x.is_empty()).ok_or_else(||anyhow!("Antigravity OAuth Client ID is not configured; configure your Desktop OAuth client in KernelSU WebUI"))?.to_owned();
        let redirect_uri = self.antigravity_redirect_uri.clone();
        let scopes = agy.oauth_scopes.join(" ");
        let verifier = random_urlsafe(64);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = random_urlsafe(32);
        let mut url = Url::parse(&agy.auth_url)?;
        url.query_pairs_mut()
            .append_pair("client_id", &client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("scope", &scopes)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent");
        let txid = Uuid::new_v4().to_string();
        self.txns.lock().unwrap().insert(
            txid.clone(),
            AuthTxn {
                provider: "antigravity".into(),
                kind: TxnKind::AntigravityPkce {
                    verifier,
                    state,
                    redirect_uri,
                },
            },
        );
        Ok(AuthChallenge::BrowserUrl {
            url: url.to_string(),
            transaction_id: txid,
        })
    }

    pub async fn poll_codex(&self, transaction_id: &str) -> Result<Option<AccountRecord>> {
        let txn = self
            .txns
            .lock()
            .unwrap()
            .get(transaction_id)
            .cloned()
            .ok_or_else(|| anyhow!("auth transaction not found"))?;
        let TxnKind::CodexDevice {
            device_auth_id,
            user_code,
        } = txn.kind
        else {
            return Err(anyhow!("not a Codex device transaction"));
        };
        #[derive(Serialize)]
        struct Poll<'a> {
            device_auth_id: &'a str,
            user_code: &'a str,
        }
        let resp = self
            .client
            .post(CODEX_DEVICE_TOKEN_URL)
            .json(&Poll {
                device_auth_id: &device_auth_id,
                user_code: &user_code,
            })
            .send()
            .await?;
        if resp.status().as_u16() == 403 || resp.status().as_u16() == 404 {
            return Ok(None);
        }
        let resp = resp.error_for_status()?;
        #[derive(Deserialize)]
        struct DeviceToken {
            authorization_code: String,
            code_verifier: String,
            code_challenge: String,
        }
        let d: DeviceToken = resp.json().await?;
        let _ = &d.code_challenge;
        let form = [
            ("grant_type", "authorization_code"),
            ("client_id", CODEX_CLIENT_ID),
            ("code", d.authorization_code.as_str()),
            ("redirect_uri", CODEX_DEVICE_REDIRECT_URI),
            ("code_verifier", d.code_verifier.as_str()),
        ];
        let token = self
            .client
            .post(CODEX_OAUTH_TOKEN_URL)
            .form(&form)
            .send()
            .await?
            .error_for_status()?;
        let body: OAuthToken = token.json().await?;
        let claims = body.id_token.as_deref().and_then(jwt_claims);
        let email = claims
            .as_ref()
            .and_then(|v| v.get("email"))
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let native = claims.as_ref().and_then(chatgpt_account_id).or_else(|| {
            jwt_claims(body.access_token.as_str()).and_then(|v| chatgpt_account_id(&v))
        });
        let account_id = Uuid::new_v4().to_string();
        let cred = Credential {
            provider: "codex".into(),
            account_id: account_id.clone(),
            access_token: Some(body.access_token),
            refresh_token: body.refresh_token,
            id_token: body.id_token,
            expires_at_unix: body.expires_in.map(|s| chrono::Utc::now().timestamp() + s),
            account_native_id: native.clone(),
            project_id: None,
            api_key: None,
        };
        let rec = self.persist_credential(
            cred,
            email.clone(),
            serde_json::json!({"chatgpt_account_id":native}).to_string(),
        )?;
        self.txns.lock().unwrap().remove(transaction_id);
        let _ = self.events.send(AuthEvent::Completed {
            transaction_id: transaction_id.to_owned(),
            account: rec.clone(),
        });
        Ok(Some(rec))
    }

    pub async fn complete_antigravity(
        &self,
        transaction_id: &str,
        code: &str,
        returned_state: &str,
    ) -> Result<AccountRecord> {
        let txn = self
            .txns
            .lock()
            .unwrap()
            .get(transaction_id)
            .cloned()
            .ok_or_else(|| anyhow!("auth transaction not found"))?;
        let TxnKind::AntigravityPkce {
            verifier,
            state,
            redirect_uri,
        } = txn.kind
        else {
            return Err(anyhow!("not an Antigravity transaction"));
        };
        if state != returned_state {
            return Err(anyhow!("OAuth state mismatch"));
        }
        let agy = self.config.read().await.providers.antigravity.clone();
        let client_id = agy
            .oauth_client_id
            .as_deref()
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .ok_or_else(|| anyhow!("Antigravity OAuth Client ID is not configured"))?
            .to_owned();
        let client_secret = self
            .secrets
            .get("antigravity-oauth-client-secret")?
            .unwrap_or_default();
        let mut form = vec![
            ("client_id", client_id),
            ("code", code.to_owned()),
            ("grant_type", "authorization_code".into()),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
        ];
        if !client_secret.is_empty() {
            form.push(("client_secret", client_secret));
        }
        let token = self
            .client
            .post(&agy.token_url)
            .form(&form)
            .send()
            .await?
            .error_for_status()?;
        let body: OAuthToken = token.json().await?;
        let user = self
            .client
            .get(&agy.userinfo_url)
            .bearer_auth(&body.access_token)
            .send()
            .await?
            .error_for_status()?;
        let user: serde_json::Value = user.json().await?;
        let email = user
            .get("email")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let project_id = Some(self.fetch_antigravity_project(&body.access_token).await?);
        let account_id = Uuid::new_v4().to_string();
        let cred = Credential {
            provider: "antigravity".into(),
            account_id: account_id.clone(),
            access_token: Some(body.access_token),
            refresh_token: body.refresh_token,
            id_token: body.id_token,
            expires_at_unix: body.expires_in.map(|s| chrono::Utc::now().timestamp() + s),
            account_native_id: None,
            project_id: project_id.clone(),
            api_key: None,
        };
        let rec = self.persist_credential(
            cred,
            email,
            serde_json::json!({"project_id":project_id}).to_string(),
        )?;
        self.txns.lock().unwrap().remove(transaction_id);
        let _ = self.events.send(AuthEvent::Completed {
            transaction_id: transaction_id.to_owned(),
            account: rec.clone(),
        });
        Ok(rec)
    }

    pub async fn complete_antigravity_by_state(
        &self,
        code: &str,
        returned_state: &str,
    ) -> Result<(String, AccountRecord)> {
        let transaction_id = {
            let txns = self.txns.lock().unwrap();
            txns.iter().find_map(|(id, txn)| match &txn.kind {
                TxnKind::AntigravityPkce { state, .. } if state == returned_state => {
                    Some(id.clone())
                }
                _ => None,
            })
        }
        .ok_or_else(|| anyhow!("Antigravity OAuth state is unknown or expired"))?;
        match self
            .complete_antigravity(&transaction_id, code, returned_state)
            .await
        {
            Ok(account) => Ok((transaction_id, account)),
            Err(error) => {
                let message = error.to_string();
                self.fail_transaction(&transaction_id, message.clone());
                Err(anyhow!(message))
            }
        }
    }

    async fn fetch_antigravity_project(&self, access: &str) -> Result<String> {
        let agy = self.config.read().await.providers.antigravity.clone();
        let load_base = agy.codeassist_base;
        let daily_base = agy.daily_base;
        let metadata = serde_json::json!({"ideType":"ANTIGRAVITY"});
        let user_agent = agy
            .user_agent
            .unwrap_or_else(|| format!("xiao/{}", crate::VERSION));
        let load = self
            .client
            .post(format!("{load_base}/v1internal:loadCodeAssist"))
            .bearer_auth(access)
            .header("Accept", "*/*")
            .header("User-Agent", &user_agent)
            .json(&serde_json::json!({"metadata":metadata}))
            .send()
            .await?
            .error_for_status()?;
        let value: serde_json::Value = load.json().await?;
        if let Some(project) = value
            .get("cloudaicompanionProject")
            .and_then(project_id_from_value)
            .or_else(|| value.get("projectId").and_then(project_id_from_value))
            .or_else(|| value.get("project").and_then(project_id_from_value))
        {
            return Ok(project);
        }

        let tier = value
            .get("allowedTiers")
            .and_then(|v| v.as_array())
            .and_then(|tiers| {
                tiers
                    .iter()
                    .find(|t| t.get("isDefault").and_then(|v| v.as_bool()) == Some(true))
            })
            .and_then(|t| t.get("id"))
            .and_then(|v| v.as_str())
            .or_else(|| value.pointer("/currentTier/id").and_then(|v| v.as_str()))
            .unwrap_or("free-tier")
            .to_owned();
        let x_goog = agy.x_goog_api_client;
        // Bounded onboarding. A later request can retry project bootstrap if the
        // control plane was temporarily unavailable.
        for attempt in 0..3 {
            let response = self.client.post(format!("{daily_base}/v1internal:onboardUser"))
                .bearer_auth(access)
                .header("Accept", "*/*")
                .header("User-Agent", &user_agent)
                .header("X-Goog-Api-Client", &x_goog)
                .json(&serde_json::json!({"tier_id":tier,"metadata":{"ide_type":"ANTIGRAVITY","ide_name":"antigravity"}}))
                .send().await?;
            if response.status().is_success() {
                let result: serde_json::Value = response.json().await?;
                if result.get("done").and_then(|v| v.as_bool()) == Some(true) {
                    if let Some(project) = result
                        .pointer("/response/cloudaicompanionProject")
                        .and_then(project_id_from_value)
                        .or_else(|| {
                            result
                                .pointer("/response/projectId")
                                .and_then(project_id_from_value)
                        })
                        .or_else(|| {
                            result
                                .pointer("/response/project")
                                .and_then(project_id_from_value)
                        })
                    {
                        return Ok(project);
                    }
                    return Err(anyhow!(
                        "Antigravity onboarding completed without a project id"
                    ));
                }
            }
            if attempt < 2 {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
        Err(anyhow!(
            "Antigravity project discovery/onboarding did not complete"
        ))
    }

    pub fn configure_api_key(
        &self,
        provider: &str,
        label: &str,
        key: &str,
    ) -> Result<AccountRecord> {
        let id = Uuid::new_v4().to_string();
        let cred = Credential {
            provider: provider.into(),
            account_id: id.clone(),
            access_token: None,
            refresh_token: None,
            id_token: None,
            expires_at_unix: None,
            account_native_id: None,
            project_id: None,
            api_key: Some(key.into()),
        };
        self.persist_credential(
            cred,
            None,
            serde_json::json!({"kind":"api_key"}).to_string(),
        )
        .map(|mut r| {
            r.label = label.into();
            let _ = self.storage.upsert_account(&r);
            r
        })
    }

    fn persist_credential(
        &self,
        cred: Credential,
        email: Option<String>,
        metadata_json: String,
    ) -> Result<AccountRecord> {
        let account_id = cred.account_id.clone();
        let provider = cred.provider.clone();
        self.secrets.put(
            &format!("account-{account_id}"),
            &serde_json::to_string(&cred)?,
        )?;
        let rec = AccountRecord {
            id: account_id,
            provider: provider.clone(),
            label: email.clone().unwrap_or_else(|| provider.clone()),
            email,
            status: "connected".into(),
            access_expires_at: cred
                .expires_at_unix
                .and_then(|x| chrono::DateTime::from_timestamp(x, 0))
                .map(|d| d.to_rfc3339()),
            metadata_json,
        };
        self.storage.upsert_account(&rec)?;
        Ok(rec)
    }

    pub fn cancel_transaction(&self, transaction_id: &str) -> bool {
        self.txns.lock().unwrap().remove(transaction_id).is_some()
    }
    pub fn fail_transaction(&self, transaction_id: &str, error: impl Into<String>) {
        if let Some(txn) = self.txns.lock().unwrap().remove(transaction_id) {
            let _ = self.events.send(AuthEvent::Failed {
                transaction_id: transaction_id.to_owned(),
                provider: txn.provider,
                error: error.into(),
            });
        }
    }
    pub fn subscribe(&self) -> broadcast::Receiver<AuthEvent> {
        self.events.subscribe()
    }
    pub fn transaction_provider(&self, id: &str) -> Option<String> {
        self.txns
            .lock()
            .unwrap()
            .get(id)
            .map(|t| t.provider.clone())
    }

    pub fn credential(&self, account_id: &str) -> Result<Option<Credential>> {
        self.secrets
            .get(&format!("account-{account_id}"))?
            .map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
    }
    pub async fn credential_for_use(&self, account_id: &str) -> Result<Option<Credential>> {
        let Some(initial) = self.credential(account_id)? else {
            return Ok(None);
        };
        if !credential_needs_refresh(&initial) {
            return Ok(Some(initial));
        }
        let lock = {
            let mut locks = self.refresh_locks.lock().unwrap();
            locks
                .entry(account_id.to_owned())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;
        let Some(current) = self.credential(account_id)? else {
            return Ok(None);
        };
        if !credential_needs_refresh(&current) {
            return Ok(Some(current));
        }
        let refreshed = match current.provider.as_str() {
            "codex" => self.refresh_codex(&current).await?,
            "antigravity" => self.refresh_antigravity(&current).await?,
            _ => current,
        };
        self.persist_refreshed(&refreshed)?;
        Ok(Some(refreshed))
    }

    async fn refresh_codex(&self, current: &Credential) -> Result<Credential> {
        let refresh = current.refresh_token.as_deref().ok_or_else(|| {
            anyhow!("Codex access expired and no refresh token is available; sign in again")
        })?;
        let form = [
            ("grant_type", "refresh_token"),
            ("client_id", CODEX_CLIENT_ID),
            ("refresh_token", refresh),
        ];
        let response = self
            .client
            .post(CODEX_OAUTH_TOKEN_URL)
            .form(&form)
            .send()
            .await?
            .error_for_status()
            .context(
                "Codex token refresh failed; sign in again if the refresh token was revoked",
            )?;
        let body: OAuthToken = response.json().await?;
        let mut next = current.clone();
        next.access_token = Some(body.access_token);
        if body.refresh_token.is_some() {
            next.refresh_token = body.refresh_token
        }
        if body.id_token.is_some() {
            next.id_token = body.id_token
        }
        next.expires_at_unix = body.expires_in.map(|s| chrono::Utc::now().timestamp() + s);
        let claims = next
            .id_token
            .as_deref()
            .and_then(jwt_claims)
            .or_else(|| next.access_token.as_deref().and_then(jwt_claims));
        if let Some(native) = claims.as_ref().and_then(chatgpt_account_id) {
            next.account_native_id = Some(native)
        }
        Ok(next)
    }

    async fn refresh_antigravity(&self, current: &Credential) -> Result<Credential> {
        let refresh = current.refresh_token.as_deref().ok_or_else(|| {
            anyhow!("Antigravity access expired and no refresh token is available; sign in again")
        })?;
        let agy = self.config.read().await.providers.antigravity.clone();
        let client_id = agy
            .oauth_client_id
            .ok_or_else(|| anyhow!("Antigravity OAuth Client ID is required to refresh access"))?;
        let client_secret = self
            .secrets
            .get("antigravity-oauth-client-secret")?
            .unwrap_or_default();
        let mut form = vec![
            ("grant_type", "refresh_token".to_owned()),
            ("client_id", client_id),
            ("refresh_token", refresh.to_owned()),
        ];
        if !client_secret.is_empty() {
            form.push(("client_secret", client_secret));
        }
        let response = self
            .client
            .post(&agy.token_url)
            .form(&form)
            .send()
            .await?
            .error_for_status()
            .context("Antigravity token refresh failed; sign in again if access was revoked")?;
        let body: OAuthToken = response.json().await?;
        let mut next = current.clone();
        next.access_token = Some(body.access_token);
        if body.refresh_token.is_some() {
            next.refresh_token = body.refresh_token
        }
        if body.id_token.is_some() {
            next.id_token = body.id_token
        }
        next.expires_at_unix = body.expires_in.map(|s| chrono::Utc::now().timestamp() + s);
        Ok(next)
    }

    fn persist_refreshed(&self, cred: &Credential) -> Result<()> {
        self.secrets.put(
            &format!("account-{}", cred.account_id),
            &serde_json::to_string(cred)?,
        )?;
        if let Some(mut account) = self.storage.account(&cred.account_id)? {
            account.status = "connected".into();
            account.access_expires_at = cred
                .expires_at_unix
                .and_then(|x| chrono::DateTime::from_timestamp(x, 0))
                .map(|d| d.to_rfc3339());
            self.storage.upsert_account(&account)?;
        }
        Ok(())
    }
    pub fn accounts(&self, provider: Option<&str>) -> Result<Vec<AccountRecord>> {
        self.storage.accounts(provider)
    }
    pub fn logout(&self, account_id: &str) -> Result<()> {
        self.secrets.remove(&format!("account-{account_id}"))?;
        self.storage.delete_account(account_id)
    }
    pub fn set_antigravity_client_secret(&self, value: Option<&str>) -> Result<()> {
        match value.map(str::trim).filter(|x| !x.is_empty()) {
            Some(v) => self.secrets.put("antigravity-oauth-client-secret", v),
            None => Ok(()),
        }
    }
    pub fn antigravity_client_secret_configured(&self) -> bool {
        self.secrets
            .get("antigravity-oauth-client-secret")
            .ok()
            .flatten()
            .is_some()
    }
}

fn project_id_from_value(value: &serde_json::Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        let s = s.trim();
        if !s.is_empty() {
            return Some(s.to_owned());
        }
    }
    value
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
fn credential_needs_refresh(credential: &Credential) -> bool {
    credential
        .expires_at_unix
        .map(|expiry| expiry <= chrono::Utc::now().timestamp() + 120)
        .unwrap_or(false)
}

#[derive(Debug, Deserialize)]
struct OAuthToken {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
}
fn parse_interval(v: &serde_json::Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}
fn random_urlsafe(n: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(n)
        .map(char::from)
        .collect()
}
fn jwt_claims(token: &str) -> Option<serde_json::Value> {
    let part = token.split('.').nth(1)?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).ok()?).ok()
}
fn chatgpt_account_id(v: &serde_json::Value) -> Option<String> {
    v.get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .map(str::to_owned)
}
