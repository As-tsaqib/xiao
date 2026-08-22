use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{broadcast, oneshot, RwLock};

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Html,
    routing::get,
    Router,
};
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
const CODEX_OAUTH_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const CODEX_OAUTH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub const CODEX_OAUTH_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";

const ANTIGRAVITY_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const ANTIGRAVITY_CLIENT_SECRET: &str = "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";
pub const ANTIGRAVITY_OAUTH_REDIRECT_URI: &str = "http://localhost:51121/oauth-callback";
const ANTIGRAVITY_USER_AGENT: &str = "antigravity/hub/2.2.1 darwin/arm64";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthChallenge {
    BrowserUrl {
        provider: String,
        url: String,
        transaction_id: String,
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
    CodexPkce {
        verifier: String,
        state: String,
        redirect_uri: String,
    },
    AntigravityOAuth {
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
    config: Arc<RwLock<AppConfig>>,
    refresh_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

#[derive(Clone)]
struct CallbackState {
    auth: Arc<AuthManager>,
    provider: String,
    transaction_id: String,
    done: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

impl AuthManager {
    pub fn new(storage: Arc<Storage>, secrets_dir: std::path::PathBuf) -> Self {
        Self::with_config(
            storage,
            secrets_dir,
            Arc::new(RwLock::new(AppConfig::default())),
        )
    }

    pub fn with_config(
        storage: Arc<Storage>,
        secrets_dir: std::path::PathBuf,
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
            config,
            refresh_locks: Mutex::new(HashMap::new()),
        }
    }

    pub async fn begin_login(self: &Arc<Self>, provider: &str) -> Result<AuthChallenge> {
        match provider {
            "codex" => self.begin_codex().await,
            "antigravity" | "agy" => self.begin_antigravity().await,
            "custom" => Ok(AuthChallenge::ApiKey {
                provider: "custom".into(),
            }),
            _ => Err(anyhow!("unknown provider")),
        }
    }

    async fn begin_codex(self: &Arc<Self>) -> Result<AuthChallenge> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:1455")
            .await
            .context("Codex OAuth callback port 1455 is already in use")?;
        let verifier = random_urlsafe(64);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = random_urlsafe(32);
        let url = codex_authorization_url(&state, &challenge)?;
        let txid = Uuid::new_v4().to_string();
        self.txns.lock().unwrap().insert(
            txid.clone(),
            AuthTxn {
                provider: "codex".into(),
                kind: TxnKind::CodexPkce {
                    verifier,
                    state,
                    redirect_uri: CODEX_OAUTH_REDIRECT_URI.into(),
                },
            },
        );
        self.spawn_callback_listener(listener, "codex", &txid);
        Ok(AuthChallenge::BrowserUrl {
            provider: "codex".into(),
            url,
            transaction_id: txid,
        })
    }

    async fn begin_antigravity(self: &Arc<Self>) -> Result<AuthChallenge> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:51121")
            .await
            .context("Antigravity OAuth callback port 51121 is already in use")?;
        let agy = self.config.read().await.providers.antigravity.clone();
        let client_id = antigravity_client_id(&agy);
        let redirect_uri = ANTIGRAVITY_OAUTH_REDIRECT_URI.to_owned();
        let state = random_urlsafe(32);
        let url = antigravity_authorization_url(&agy, &client_id, &state)?;
        let txid = Uuid::new_v4().to_string();
        self.txns.lock().unwrap().insert(
            txid.clone(),
            AuthTxn {
                provider: "antigravity".into(),
                kind: TxnKind::AntigravityOAuth {
                    state,
                    redirect_uri,
                },
            },
        );
        self.spawn_callback_listener(listener, "antigravity", &txid);
        Ok(AuthChallenge::BrowserUrl {
            provider: "antigravity".into(),
            url,
            transaction_id: txid,
        })
    }

    fn spawn_callback_listener(
        self: &Arc<Self>,
        listener: tokio::net::TcpListener,
        provider: &str,
        transaction_id: &str,
    ) {
        let (done_tx, done_rx) = oneshot::channel();
        let state = CallbackState {
            auth: self.clone(),
            provider: provider.to_owned(),
            transaction_id: transaction_id.to_owned(),
            done: Arc::new(Mutex::new(Some(done_tx))),
        };
        let router = Router::new()
            .route("/auth/callback", get(oauth_browser_callback))
            .route("/oauth-callback", get(oauth_browser_callback))
            .with_state(state);
        let auth = self.clone();
        let transaction_id = transaction_id.to_owned();
        let provider = provider.to_owned();
        tokio::spawn(async move {
            let shutdown_auth = auth.clone();
            let shutdown_txid = transaction_id.clone();
            let shutdown = async move {
                tokio::select! {
                    _ = done_rx => {}
                    _ = async {
                        for _ in 0..300 {
                            if shutdown_auth.transaction_provider(&shutdown_txid).is_none() {
                                return;
                            }
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    } => {}
                }
            };
            if let Err(error) = axum::serve(listener, router)
                .with_graceful_shutdown(shutdown)
                .await
            {
                auth.fail_transaction(
                    &transaction_id,
                    format!("OAuth callback server failed: {error}"),
                );
                return;
            }
            if auth.transaction_provider(&transaction_id).is_some() {
                auth.fail_transaction(
                    &transaction_id,
                    format!("{provider} OAuth callback timed out"),
                );
            }
        });
    }

    pub async fn complete_codex(
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
        let TxnKind::CodexPkce {
            verifier,
            state,
            redirect_uri,
        } = txn.kind
        else {
            return Err(anyhow!("not a Codex OAuth transaction"));
        };
        if state != returned_state {
            return Err(anyhow!("OAuth state mismatch"));
        }
        let form = [
            ("grant_type", "authorization_code"),
            ("client_id", CODEX_CLIENT_ID),
            ("code", code),
            ("redirect_uri", redirect_uri.as_str()),
            ("code_verifier", verifier.as_str()),
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
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("Codex token response is missing the account email"))?;
        let native = claims
            .as_ref()
            .and_then(chatgpt_account_id)
            .or_else(|| jwt_claims(body.access_token.as_str()).and_then(|v| chatgpt_account_id(&v)))
            .ok_or_else(|| anyhow!("Codex token response is missing the ChatGPT account id"))?;
        let plan_type = claims.as_ref().and_then(chatgpt_plan_type);
        let account_id = Uuid::new_v4().to_string();
        let cred = Credential {
            provider: "codex".into(),
            account_id: account_id.clone(),
            access_token: Some(body.access_token),
            refresh_token: body.refresh_token,
            id_token: body.id_token,
            expires_at_unix: body.expires_in.map(|s| chrono::Utc::now().timestamp() + s),
            account_native_id: Some(native.clone()),
            project_id: None,
            api_key: None,
        };
        let rec = self.persist_credential(
            cred,
            Some(email),
            serde_json::json!({"chatgpt_account_id":native,"plan_type":plan_type}).to_string(),
        )?;
        self.txns.lock().unwrap().remove(transaction_id);
        let _ = self.events.send(AuthEvent::Completed {
            transaction_id: transaction_id.to_owned(),
            account: rec.clone(),
        });
        Ok(rec)
    }

    pub async fn complete_codex_by_state(
        &self,
        code: &str,
        returned_state: &str,
    ) -> Result<(String, AccountRecord)> {
        let transaction_id = self.transaction_id_by_state("codex", returned_state)?;
        match self
            .complete_codex(&transaction_id, code, returned_state)
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
        let TxnKind::AntigravityOAuth {
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
        let client_id = antigravity_client_id(&agy);
        let client_secret = self.antigravity_client_secret(&agy)?;
        let mut form = vec![
            ("client_id", client_id),
            ("code", code.to_owned()),
            ("grant_type", "authorization_code".into()),
            ("redirect_uri", redirect_uri),
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
            .header("User-Agent", antigravity_user_agent(&agy))
            .send()
            .await?
            .error_for_status()?;
        let user: serde_json::Value = user.json().await?;
        let email = user
            .get("email")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("Antigravity userinfo response is missing the account email"))?;
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
            Some(email),
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
        let transaction_id = self.transaction_id_by_state("antigravity", returned_state)?;
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
        let user_agent = antigravity_user_agent(&agy).to_owned();
        let load_base = agy.codeassist_base;
        let daily_base = agy.daily_base;
        let metadata = serde_json::json!({"ideType":"ANTIGRAVITY"});
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
        let onboard_user_agent = format!("{} google-api-nodejs-client/10.3.0", user_agent);
        for attempt in 0..5 {
            let response = self
                .client
                .post(format!("{daily_base}/v1internal:onboardUser"))
                .bearer_auth(access)
                .header("Accept", "*/*")
                .header("User-Agent", &onboard_user_agent)
                .header("X-Goog-Api-Client", &x_goog)
                .json(&serde_json::json!({
                    "tier_id": tier,
                    "metadata": {
                        "ide_type": "ANTIGRAVITY",
                        "ide_version": "2.2.1",
                        "ide_name": "antigravity"
                    }
                }))
                .send()
                .await?;
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
            if attempt < 4 {
                tokio::time::sleep(Duration::from_secs(2)).await;
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
        let id = self
            .accounts(Some(provider))?
            .into_iter()
            .next()
            .map(|account| account.id)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
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
        let mut record = self.persist_credential(
            cred,
            None,
            serde_json::json!({"kind":"api_key"}).to_string(),
        )?;
        record.label = label.into();
        self.storage.upsert_account(&record)?;
        Ok(record)
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
    pub fn fail_transaction_by_state(&self, provider: &str, state: &str, error: impl Into<String>) {
        if let Ok(transaction_id) = self.transaction_id_by_state(provider, state) {
            self.fail_transaction(&transaction_id, error);
        }
    }

    fn transaction_id_by_state(&self, provider: &str, returned_state: &str) -> Result<String> {
        let transaction_id = self.txns.lock().unwrap().iter().find_map(|(id, txn)| {
            if txn.provider != provider {
                return None;
            }
            let state = match &txn.kind {
                TxnKind::CodexPkce { state, .. } | TxnKind::AntigravityOAuth { state, .. } => state,
            };
            (state == returned_state).then(|| id.clone())
        });
        transaction_id.ok_or_else(|| anyhow!("{provider} OAuth state is unknown or expired"))
    }

    fn antigravity_client_secret(
        &self,
        config: &crate::config::AntigravityProviderConfig,
    ) -> Result<String> {
        if config
            .oauth_client_id
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
        {
            return Ok(self
                .secrets
                .get("antigravity-oauth-client-secret")?
                .unwrap_or_default());
        }
        Ok(ANTIGRAVITY_CLIENT_SECRET.to_owned())
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
            ("scope", "openid profile email"),
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
        let client_id = antigravity_client_id(&agy);
        let client_secret = self.antigravity_client_secret(&agy)?;
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
    pub(crate) fn provider_api_key(&self, provider: &str) -> Result<Option<String>> {
        for account in self.accounts(Some(provider))? {
            if account.status != "connected" {
                continue;
            }
            if let Some(key) = self
                .credential(&account.id)?
                .and_then(|credential| credential.api_key)
                .map(|key| key.trim().to_owned())
                .filter(|key| !key.is_empty())
            {
                return Ok(Some(key));
            }
        }
        Ok(None)
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

async fn oauth_browser_callback(
    State(state): State<CallbackState>,
    Query(query): Query<CallbackQuery>,
) -> (StatusCode, Html<String>) {
    let result = if let Some(error) = query.error.as_deref() {
        let detail = query.error_description.as_deref().unwrap_or(error);
        if let Some(returned_state) = query.state.as_deref() {
            state.auth.fail_transaction_by_state(
                &state.provider,
                returned_state,
                detail.to_owned(),
            );
        }
        Err(anyhow!(detail.to_owned()))
    } else {
        match (query.code.as_deref(), query.state.as_deref()) {
            (Some(code), Some(returned_state))
                if !code.is_empty() && !returned_state.is_empty() =>
            {
                if state.provider == "codex" {
                    state
                        .auth
                        .complete_codex_by_state(code, returned_state)
                        .await
                        .map(|(_, account)| account)
                } else {
                    state
                        .auth
                        .complete_antigravity_by_state(code, returned_state)
                        .await
                        .map(|(_, account)| account)
                }
            }
            _ => Err(anyhow!("missing OAuth code or state")),
        }
    };

    if let Err(error) = &result {
        state
            .auth
            .fail_transaction(&state.transaction_id, error.to_string());
    }
    if let Some(done) = state.done.lock().unwrap().take() {
        let _ = done.send(());
    }

    match result {
        Ok(account) => (
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
                html_escape(&error.to_string())
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
fn codex_authorization_url(state: &str, challenge: &str) -> Result<String> {
    let mut url = Url::parse(CODEX_OAUTH_AUTHORIZE_URL)?;
    url.query_pairs_mut()
        .append_pair("client_id", CODEX_CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", CODEX_OAUTH_REDIRECT_URI)
        .append_pair("scope", "openid email profile offline_access")
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("prompt", "login")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true");
    Ok(url.to_string())
}

fn antigravity_authorization_url(
    config: &crate::config::AntigravityProviderConfig,
    client_id: &str,
    state: &str,
) -> Result<String> {
    let mut url = Url::parse(&config.auth_url)?;
    url.query_pairs_mut()
        .append_pair("access_type", "offline")
        .append_pair("client_id", client_id)
        .append_pair("prompt", "consent")
        .append_pair("redirect_uri", ANTIGRAVITY_OAUTH_REDIRECT_URI)
        .append_pair("response_type", "code")
        .append_pair("scope", &config.oauth_scopes.join(" "))
        .append_pair("state", state);
    Ok(url.to_string())
}

fn antigravity_client_id(config: &crate::config::AntigravityProviderConfig) -> String {
    config
        .oauth_client_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(ANTIGRAVITY_CLIENT_ID)
        .to_owned()
}

pub(crate) fn antigravity_user_agent(config: &crate::config::AntigravityProviderConfig) -> &str {
    config
        .user_agent
        .as_deref()
        .unwrap_or(ANTIGRAVITY_USER_AGENT)
}

fn credential_needs_refresh(credential: &Credential) -> bool {
    let lead_seconds = match credential.provider.as_str() {
        "codex" => 5 * 24 * 60 * 60,
        "antigravity" => 5 * 60,
        _ => 120,
    };
    credential
        .expires_at_unix
        .map(|expiry| expiry <= chrono::Utc::now().timestamp() + lead_seconds)
        .unwrap_or(false)
}

#[derive(Debug, Deserialize)]
struct OAuthToken {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
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

fn chatgpt_plan_type(v: &serde_json::Value) -> Option<String> {
    v.get("https://api.openai.com/auth")?
        .get("chatgpt_plan_type")?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(url: &str) -> HashMap<String, String> {
        Url::parse(url)
            .unwrap()
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }

    #[test]
    fn codex_url_matches_cliproxyapi_browser_pkce_contract() {
        let params = query(&codex_authorization_url("state-value", "pkce-value").unwrap());
        assert_eq!(params["client_id"], CODEX_CLIENT_ID);
        assert_eq!(params["redirect_uri"], CODEX_OAUTH_REDIRECT_URI);
        assert_eq!(params["scope"], "openid email profile offline_access");
        assert_eq!(params["state"], "state-value");
        assert_eq!(params["code_challenge"], "pkce-value");
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["prompt"], "login");
        assert_eq!(params["id_token_add_organizations"], "true");
        assert_eq!(params["codex_cli_simplified_flow"], "true");
    }

    #[test]
    fn antigravity_url_uses_cliproxyapi_defaults_without_operator_config() {
        let config = crate::config::AntigravityProviderConfig::default();
        let client_id = antigravity_client_id(&config);
        let url = antigravity_authorization_url(&config, &client_id, "state-value").unwrap();
        let params = query(&url);
        assert_eq!(params["client_id"], ANTIGRAVITY_CLIENT_ID);
        assert_eq!(params["redirect_uri"], ANTIGRAVITY_OAUTH_REDIRECT_URI);
        assert_eq!(params["state"], "state-value");
        assert_eq!(params["access_type"], "offline");
        assert_eq!(params["prompt"], "consent");
        assert!(params["scope"].contains("cloud-platform"));
        assert!(!params.contains_key("code_challenge"));
        assert_eq!(antigravity_user_agent(&config), ANTIGRAVITY_USER_AGENT);
    }
}
