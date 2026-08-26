use std::{collections::BTreeMap, sync::Arc};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::{
    auth::AuthManager,
    config::CustomProviderConfig,
    security::{redact::redact_text, secrets::SecretStore},
    storage::{ProviderProfileInput, ProviderProfileModelRecord, ProviderProfileRecord, Storage},
};

#[derive(Clone)]
pub struct ProviderProfileStore {
    storage: Arc<Storage>,
}

impl ProviderProfileStore {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn set_capability_override(
        &self,
        owner_id: &str,
        profile_id: &str,
        model_id: &str,
        capability: &str,
        owner_override: &str,
    ) -> Result<()> {
        if !matches!(capability, "vision" | "file_input") {
            return Err(anyhow!("capability must be vision or file_input"));
        }
        if !matches!(
            owner_override,
            "auto" | "force_supported" | "force_unsupported"
        ) {
            return Err(anyhow!("invalid capability override"));
        }
        let profile = self
            .get(owner_id, profile_id)?
            .ok_or_else(|| anyhow!("Custom profile not found"))?;
        let model = self
            .model(profile_id, model_id)?
            .ok_or_else(|| anyhow!("Custom model not found"))?;
        let state = if capability == "vision" {
            &model.vision_state
        } else {
            &model.file_input_state
        };
        let now = Utc::now().to_rfc3339();
        self.storage.with_conn(|connection| {
            connection.execute(
                "INSERT INTO provider_capability_evidence(profile_id,model_id,protocol,capability,state,owner_override,source,observed_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(profile_id,model_id,protocol,capability) DO UPDATE SET owner_override=excluded.owner_override,source=excluded.source,observed_at=excluded.observed_at",
                params![profile_id, model_id, profile.protocol, capability, state, owner_override, "owner_override", now],
            )?;
            Ok(())
        })
    }

    pub fn capability_override(
        &self,
        profile_id: &str,
        model_id: &str,
        protocol: &str,
        capability: &str,
    ) -> Result<String> {
        self.storage.with_conn(|connection| {
            Ok(connection.query_row(
                "SELECT owner_override FROM provider_capability_evidence WHERE profile_id=? AND model_id=? AND protocol=? AND capability=?",
                params![profile_id, model_id, protocol, capability], |row| row.get(0),
            ).optional()?.unwrap_or_else(|| "auto".into()))
        })
    }

    pub fn record_runtime_capability(
        &self,
        profile_id: &str,
        model_id: &str,
        protocol: &str,
        capability: &str,
        state: &str,
        source: &str,
    ) -> Result<()> {
        if !matches!(
            (capability, state),
            (
                "vision" | "file_input" | "streaming",
                "supported" | "unsupported"
            )
        ) {
            return Err(anyhow!("invalid runtime capability evidence"));
        }
        let now = Utc::now().to_rfc3339();
        self.storage.with_conn(|connection| {
            let transaction=connection.transaction()?;
            transaction.execute("INSERT INTO provider_capability_evidence(profile_id,model_id,protocol,capability,state,owner_override,source,observed_at) VALUES(?,?,?,?,?,'auto',?,?) ON CONFLICT(profile_id,model_id,protocol,capability) DO UPDATE SET state=excluded.state,source=excluded.source,observed_at=excluded.observed_at", params![profile_id,model_id,protocol,capability,state,source,now])?;
            if capability=="vision" { transaction.execute("UPDATE provider_profile_models SET vision_state=?,vision_capable=? WHERE profile_id=? AND model_id=?",params![state,(state=="supported") as i32,profile_id,model_id])?; }
            if capability=="file_input" { transaction.execute("UPDATE provider_profile_models SET file_input_state=?,file_input_capable=? WHERE profile_id=? AND model_id=?",params![state,(state=="supported") as i32,profile_id,model_id])?; }
            transaction.commit()?; Ok(())
        })
    }

    pub fn create(&self, mut input: ProviderProfileInput) -> Result<ProviderProfileRecord> {
        input.alias = canonical_alias(&input.alias)?;
        input.endpoint = validate_endpoint(&input.endpoint)?;
        validate_protocol(&input.protocol)?;
        let headers = parse_safe_headers(&input.safe_headers_json)?;
        input.safe_headers_json = serde_json::to_string(&headers)?;
        let profile_id = input
            .profile_id
            .take()
            .unwrap_or_else(|| format!("custom:{}", Uuid::new_v4().simple()));
        let now = Utc::now().to_rfc3339();
        self.storage.with_conn(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT OR IGNORE INTO owners(owner_id,telegram_user_id,created_at,updated_at) VALUES(?,NULL,?,?)",
                params![input.owner_id, now, now],
            )?;
            transaction.execute(
                "INSERT INTO provider_profiles(profile_id,owner_id,provider_kind,alias,endpoint,protocol,credential_ref,api_key_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at) VALUES(?,?,'custom',?,?,?,?,?,?,?,1,'unknown',?,?)",
                params![profile_id, input.owner_id, input.alias, input.endpoint, input.protocol, input.credential_ref, input.api_key_ref, input.safe_headers_json, input.secret_headers_ref, now, now],
            )?;
            transaction.commit()?;
            Ok(())
        })?;
        self.get(&input.owner_id, &profile_id)?
            .ok_or_else(|| anyhow!("created Custom profile is missing"))
    }

    /// Commit a newly discovered Custom profile, its exact model catalog and
    /// one session selection as a single SQLite transaction.  Telegram's
    /// setup wizard prepares its immutable credential before this call; no
    /// profile or active-session split can survive a failed catalog/audit
    /// write.
    pub(crate) fn create_with_models_and_activate_session(
        &self,
        mut input: ProviderProfileInput,
        models: &[ProviderProfileModelRecord],
        session_id: &str,
        selected_model: &str,
    ) -> Result<ProviderProfileRecord> {
        input.alias = canonical_alias(&input.alias)?;
        input.endpoint = validate_endpoint(&input.endpoint)?;
        validate_protocol(&input.protocol)?;
        let headers = parse_safe_headers(&input.safe_headers_json)?;
        input.safe_headers_json = serde_json::to_string(&headers)?;
        if models.len() > 2_000 {
            return Err(anyhow!("Custom profile model catalog is too large"));
        }
        let selected_model = selected_model.trim();
        if selected_model.is_empty() || selected_model.chars().count() > 512 {
            return Err(anyhow!("selected Custom model is empty or too long"));
        }
        if !models.iter().any(|model| model.model_id == selected_model) {
            return Err(anyhow!(
                "selected Custom model is absent from the discovered catalog"
            ));
        }
        for model in models {
            validate_tool_protocol(&model.tool_protocol)?;
            if model.model_id.trim().is_empty() || model.model_id.chars().count() > 512 {
                return Err(anyhow!("Custom model id is empty or too long"));
            }
        }
        let profile_id = input
            .profile_id
            .take()
            .unwrap_or_else(|| format!("custom:{}", Uuid::new_v4().simple()));
        let models = models
            .iter()
            .cloned()
            .map(|mut model| {
                model.profile_id = profile_id.clone();
                model
            })
            .collect::<Vec<_>>();
        let now = Utc::now().to_rfc3339();
        self.storage.with_conn(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT OR IGNORE INTO owners(owner_id,telegram_user_id,created_at,updated_at) VALUES(?,NULL,?,?)",
                params![input.owner_id, now, now],
            )?;
            if let Some(credential_ref) = input.credential_ref.as_deref() {
                let credential = transaction
                    .query_row(
                        "SELECT owner_id,provider FROM provider_accounts WHERE id=?",
                        params![credential_ref],
                        |row| {
                            Ok((
                                row.get::<_, Option<String>>(0)?,
                                row.get::<_, String>(1)?,
                            ))
                        },
                    )
                    .optional()?
                    .ok_or_else(|| anyhow!("prepared Custom profile credential is missing"))?;
                if credential.0.as_deref() != Some(input.owner_id.as_str()) {
                    return Err(anyhow!(
                        "prepared Custom profile credential belongs to another owner"
                    ));
                }
                if credential.1 != "custom" {
                    return Err(anyhow!(
                        "prepared Custom profile credential is not a Custom credential"
                    ));
                }
            }
            transaction.execute(
                "INSERT INTO provider_profiles(profile_id,owner_id,provider_kind,alias,endpoint,protocol,credential_ref,api_key_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at) VALUES(?,?,'custom',?,?,?,?,?,?,?,1,'unknown',?,?)",
                params![profile_id, input.owner_id, input.alias, input.endpoint, input.protocol, input.credential_ref, input.api_key_ref, input.safe_headers_json, input.secret_headers_ref, now, now],
            )?;
            for model in &models {
                transaction.execute(
                    "INSERT INTO provider_profile_models(profile_id,model_id,text_capable,vision_capable,file_input_capable,native_tools,structured_output,continuation,native_tools_state,structured_output_state,continuation_state,vision_state,file_input_state,model_discovery,tool_protocol,evidence,probe_status,probe_version,probed_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    params![profile_id, model.model_id, model.text_capable as i32, model.vision_capable as i32, model.file_input_capable as i32, model.native_tools as i32, model.structured_output as i32, model.continuation as i32, model.native_tools_state, model.structured_output_state, model.continuation_state, model.vision_state, model.file_input_state, model.model_discovery as i32, model.tool_protocol, model.evidence, model.probe_status, model.probe_version, model.probed_at],
                )?;
            }
            transaction.execute(
                "UPDATE provider_profiles SET reachability='reachable',last_probe_at=?,updated_at=? WHERE profile_id=?",
                params![now, now, profile_id],
            )?;
            if transaction.execute(
                "UPDATE sessions SET provider='custom',account_id=?,model=?,last_active_at=? WHERE id=? AND owner_principal=? AND archived=0",
                params![profile_id, selected_model, now, session_id, input.owner_id],
            )? != 1 {
                return Err(anyhow!("session not found for owner or is archived"));
            }
            transaction.execute(
                "INSERT INTO audit_events(principal,action,detail,created_at) VALUES(?,?,?,?)",
                params![input.owner_id, "custom_provider_configured", format!("session_id={session_id};profile_id={profile_id};model={selected_model}"), now],
            )?;
            transaction.commit()?;
            Ok(())
        })?;
        self.get(&input.owner_id, &profile_id)?
            .ok_or_else(|| anyhow!("created Custom profile is missing"))
    }

    pub fn list(&self, owner_id: &str) -> Result<Vec<ProviderProfileRecord>> {
        self.storage.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT profile_id,owner_id,alias,endpoint,protocol,credential_ref,api_key_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at,last_probe_at FROM provider_profiles WHERE owner_id=? ORDER BY updated_at DESC,alias",
            )?;
            let rows = statement.query_map(params![owner_id], row_profile)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn get(&self, owner_id: &str, profile_id: &str) -> Result<Option<ProviderProfileRecord>> {
        self.storage.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT profile_id,owner_id,alias,endpoint,protocol,credential_ref,api_key_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at,last_probe_at FROM provider_profiles WHERE owner_id=? AND profile_id=?",
                    params![owner_id, profile_id],
                    row_profile,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn get_by_id(&self, profile_id: &str) -> Result<Option<ProviderProfileRecord>> {
        self.storage.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT profile_id,owner_id,alias,endpoint,protocol,credential_ref,api_key_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at,last_probe_at FROM provider_profiles WHERE profile_id=?",
                    params![profile_id],
                    row_profile,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn get_by_alias(
        &self,
        owner_id: &str,
        alias: &str,
    ) -> Result<Option<ProviderProfileRecord>> {
        let alias = canonical_alias(alias)?;
        self.storage.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT profile_id,owner_id,alias,endpoint,protocol,credential_ref,api_key_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at,last_probe_at FROM provider_profiles WHERE owner_id=? AND alias=?",
                    params![owner_id, alias],
                    row_profile,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn set_reachability(
        &self,
        owner_id: &str,
        profile_id: &str,
        reachability: &str,
    ) -> Result<()> {
        if !matches!(reachability, "unknown" | "reachable" | "unreachable") {
            return Err(anyhow!("invalid Custom profile reachability"));
        }
        self.update_exact(
            owner_id,
            profile_id,
            "UPDATE provider_profiles SET reachability=?,last_probe_at=?,updated_at=? WHERE owner_id=? AND profile_id=?",
            params![
                reachability,
                Utc::now().to_rfc3339(),
                Utc::now().to_rfc3339(),
                owner_id,
                profile_id
            ],
        )
    }

    pub fn set_credential(
        &self,
        owner_id: &str,
        profile_id: &str,
        credential_ref: Option<&str>,
    ) -> Result<()> {
        self.update_exact(
            owner_id,
            profile_id,
            "UPDATE provider_profiles SET credential_ref=?,updated_at=? WHERE owner_id=? AND profile_id=?",
            params![credential_ref, Utc::now().to_rfc3339(), owner_id, profile_id],
        )
    }

    pub fn set_direct_api_key(
        &self,
        secrets: &SecretStore,
        owner_id: &str,
        profile_id: &str,
        api_key: &str,
    ) -> Result<String> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(anyhow!("API key cannot be empty"));
        }
        let profile = self
            .get(owner_id, profile_id)?
            .ok_or_else(|| anyhow!("Custom profile not found for owner"))?;
        let ref_id = format!(
            "custom-api-key:{}:{}",
            profile.profile_id.replace(':', "_"),
            Uuid::new_v4().simple()
        );
        secrets.put(&ref_id, api_key)?;
        self.update_exact(
            owner_id,
            profile_id,
            "UPDATE provider_profiles SET api_key_ref=?,updated_at=? WHERE owner_id=? AND profile_id=?",
            params![Some(&ref_id), Utc::now().to_rfc3339(), owner_id, profile_id],
        )?;
        Ok(ref_id)
    }

    pub fn resolve_api_key(
        &self,
        secrets: &SecretStore,
        profile: &ProviderProfileRecord,
    ) -> Result<Option<String>> {
        if let Some(key_ref) = &profile.api_key_ref {
            if let Some(key) = secrets.get(key_ref)? {
                return Ok(Some(key));
            }
        }
        if let Some(cred_ref) = &profile.credential_ref {
            if let Some(cred_str) = secrets.get(&format!("account-{cred_ref}"))? {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cred_str) {
                    if let Some(key) = val.get("api_key").and_then(|k| k.as_str()) {
                        return Ok(Some(key.to_string()));
                    }
                }
            }
        }
        Ok(None)
    }

    pub fn migrate_legacy_credentials(
        &self,
        secrets: &SecretStore,
        auth: &AuthManager,
    ) -> Result<usize> {
        let unmigrated: Vec<(String, String, String)> = self.storage.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT owner_id, profile_id, credential_ref FROM provider_profiles WHERE api_key_ref IS NULL AND credential_ref IS NOT NULL",
            )?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })?;

        let mut count = 0;
        for (owner_id, profile_id, cred_ref) in unmigrated {
            if let Ok(Some(cred)) = auth.credential(&cred_ref) {
                if let Some(key) = cred.api_key {
                    if !key.trim().is_empty() {
                        let _ = self.set_direct_api_key(secrets, &owner_id, &profile_id, &key)?;
                        count += 1;
                    }
                }
            }
        }
        Ok(count)
    }

    pub fn edit_metadata(
        &self,
        owner_id: &str,
        profile_id: &str,
        alias: Option<&str>,
        protocol: Option<&str>,
        headers: Option<&BTreeMap<String, String>>,
    ) -> Result<()> {
        let alias = alias.map(canonical_alias).transpose()?;
        if let Some(protocol) = protocol {
            validate_protocol(protocol)?;
        }
        let headers_json = headers.map(serde_json::to_string).transpose()?;
        if let Some(raw) = headers_json.as_deref() {
            let _ = parse_safe_headers(raw)?;
        }
        self.storage.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let current = transaction
                .query_row(
                    "SELECT alias,protocol,safe_headers_json FROM provider_profiles WHERE owner_id=? AND profile_id=?",
                    params![owner_id, profile_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
                )
                .optional()?
                .ok_or_else(|| anyhow!("Custom profile not found for owner"))?;
            let next_alias = alias.as_deref().unwrap_or(&current.0);
            let next_protocol = protocol.unwrap_or(&current.1);
            let next_headers = headers_json.as_deref().unwrap_or(&current.2);
            transaction.execute(
                "UPDATE provider_profiles SET alias=?,protocol=?,safe_headers_json=?,reachability='unknown',last_probe_at=NULL,updated_at=? WHERE owner_id=? AND profile_id=?",
                params![next_alias, next_protocol, next_headers, Utc::now().to_rfc3339(), owner_id, profile_id],
            )?;
            if protocol.is_some_and(|value| value != current.1) {
                transaction.execute(
                    "DELETE FROM provider_profile_models WHERE profile_id=?",
                    params![profile_id],
                )?;
                transaction.execute(
                    "UPDATE provider_capability_evidence SET state='unknown',source='invalidated_on_endpoint_change',observed_at=? WHERE profile_id=? AND source != 'owner_override'",
                    params![Utc::now().to_rfc3339(), profile_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    /// Endpoint changes cross a trust boundary. Credential and all headers
    /// are cleared by default; retaining them requires a separate explicit
    /// owner flow that v0.2.6 intentionally does not make implicit.
    pub fn change_endpoint(&self, owner_id: &str, profile_id: &str, endpoint: &str) -> Result<()> {
        let endpoint = validate_endpoint(endpoint)?;
        self.storage.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let changed = transaction.execute(
                "UPDATE provider_profiles SET endpoint=?,credential_ref=NULL,api_key_ref=NULL,safe_headers_json='{}',secret_headers_ref=NULL,reachability='unknown',last_probe_at=NULL,updated_at=? WHERE owner_id=? AND profile_id=?",
                params![endpoint, Utc::now().to_rfc3339(), owner_id, profile_id],
            )?;
            if changed != 1 {
                return Err(anyhow!("Custom profile not found for owner"));
            }
            transaction.execute(
                "DELETE FROM provider_profile_models WHERE profile_id=?",
                params![profile_id],
            )?;
            transaction.execute(
                "UPDATE provider_capability_evidence SET state='unknown',source='invalidated_on_endpoint_change',observed_at=? WHERE profile_id=? AND source != 'owner_override'",
                params![Utc::now().to_rfc3339(), profile_id],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    /// Trust-boundary endpoint change that also removes write-only secret headers
    /// from SecretStore. Used by CustomProfileService and IPC edit_endpoint.
    pub fn change_endpoint_with_secrets(
        &self,
        owner_id: &str,
        profile_id: &str,
        endpoint: &str,
        secrets: &SecretStore,
    ) -> Result<()> {
        let endpoint = validate_endpoint(endpoint)?;
        let secret_ref = secret_headers_ref_for(profile_id);
        let old_secret_ref: Option<String> = self.storage.with_conn(|connection| {
            Ok(connection
                .query_row(
                    "SELECT secret_headers_ref FROM provider_profiles WHERE owner_id=? AND profile_id=?",
                    params![owner_id, profile_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten())
        })?;
        self.storage.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let changed = transaction.execute(
                "UPDATE provider_profiles SET endpoint=?,credential_ref=NULL,api_key_ref=NULL,safe_headers_json='{}',secret_headers_ref=NULL,reachability='unknown',last_probe_at=NULL,updated_at=? WHERE owner_id=? AND profile_id=?",
                params![endpoint, Utc::now().to_rfc3339(), owner_id, profile_id],
            )?;
            if changed != 1 {
                return Err(anyhow!("Custom profile not found for owner"));
            }
            transaction.execute(
                "DELETE FROM provider_profile_models WHERE profile_id=?",
                params![profile_id],
            )?;
            transaction.execute(
                "UPDATE provider_capability_evidence SET state='unknown',source='invalidated_on_endpoint_change',observed_at=? WHERE profile_id=? AND source != 'owner_override'",
                params![Utc::now().to_rfc3339(), profile_id],
            )?;
            transaction.commit()?;
            Ok(())
        })?;
        // Best-effort secret cleanup after DB commit; redacted on failure.
        if let Some(reference) = old_secret_ref {
            let _ = secrets.remove(&reference);
        }
        let _ = secrets.remove(&secret_ref);
        let _ = secrets.rollback_staged(&secret_ref);
        Ok(())
    }

    pub fn replace_models(
        &self,
        owner_id: &str,
        profile_id: &str,
        models: &[ProviderProfileModelRecord],
    ) -> Result<()> {
        if models.len() > 2_000 {
            return Err(anyhow!("Custom profile model catalog is too large"));
        }
        self.storage.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let owns: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_profiles WHERE owner_id=? AND profile_id=?)",
                params![owner_id, profile_id],
                |row| row.get(0),
            )?;
            if !owns {
                return Err(anyhow!("Custom profile not found for owner"));
            }
            transaction.execute(
                "DELETE FROM provider_profile_models WHERE profile_id=?",
                params![profile_id],
            )?;
            for model in models {
                validate_tool_protocol(&model.tool_protocol)?;
                if model.model_id.trim().is_empty() || model.model_id.chars().count() > 512 {
                    return Err(anyhow!("Custom model id is empty or too long"));
                }
                transaction.execute(
                    "INSERT INTO provider_profile_models(profile_id,model_id,text_capable,vision_capable,file_input_capable,native_tools,structured_output,continuation,native_tools_state,structured_output_state,continuation_state,vision_state,file_input_state,model_discovery,tool_protocol,evidence,probe_status,probe_version,probed_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    params![profile_id, model.model_id, model.text_capable as i32, model.vision_capable as i32, model.file_input_capable as i32, model.native_tools as i32, model.structured_output as i32, model.continuation as i32, model.native_tools_state, model.structured_output_state, model.continuation_state, model.vision_state, model.file_input_state, model.model_discovery as i32, model.tool_protocol, model.evidence, model.probe_status, model.probe_version, model.probed_at],
                )?;
            }
            transaction.execute(
                "UPDATE provider_profiles SET reachability='reachable',last_probe_at=?,updated_at=? WHERE profile_id=?",
                params![Utc::now().to_rfc3339(), Utc::now().to_rfc3339(), profile_id],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn models(&self, profile_id: &str) -> Result<Vec<ProviderProfileModelRecord>> {
        self.storage.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT profile_id,model_id,text_capable,vision_capable,file_input_capable,native_tools,structured_output,continuation,native_tools_state,structured_output_state,continuation_state,vision_state,file_input_state,model_discovery,tool_protocol,evidence,probe_status,probe_version,probed_at FROM provider_profile_models WHERE profile_id=? ORDER BY model_id",
            )?;
            let rows = statement.query_map(params![profile_id], row_profile_model)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn all_models(&self) -> Result<Vec<String>> {
        self.storage.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT DISTINCT model_id FROM provider_profile_models ORDER BY model_id",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn model(
        &self,
        profile_id: &str,
        model_id: &str,
    ) -> Result<Option<ProviderProfileModelRecord>> {
        self.storage.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT profile_id,model_id,text_capable,vision_capable,file_input_capable,native_tools,structured_output,continuation,native_tools_state,structured_output_state,continuation_state,vision_state,file_input_state,model_discovery,tool_protocol,evidence,probe_status,probe_version,probed_at FROM provider_profile_models WHERE profile_id=? AND model_id=?",
                    params![profile_id, model_id],
                    row_profile_model,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn delete(&self, owner_id: &str, profile_id: &str) -> Result<Option<String>> {
        self.storage.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let credential = transaction
                .query_row(
                    "SELECT credential_ref FROM provider_profiles WHERE owner_id=? AND profile_id=?",
                    params![owner_id, profile_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
            let in_use: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE provider='custom' AND account_id=? AND archived=0)",
                params![profile_id],
                |row| row.get(0),
            )?;
            if in_use {
                return Err(anyhow!("Custom profile is selected by an active session"));
            }
            let changed = transaction.execute(
                "DELETE FROM provider_profiles WHERE owner_id=? AND profile_id=?",
                params![owner_id, profile_id],
            )?;
            if changed != 1 {
                return Err(anyhow!("Custom profile not found for owner"));
            }
            transaction.commit()?;
            Ok(credential)
        })
    }

    pub fn delete_with_secrets(
        &self,
        owner_id: &str,
        profile_id: &str,
        secrets: &SecretStore,
    ) -> Result<Option<String>> {
        let profile = self.get(owner_id, profile_id)?;
        let credential = self.delete(owner_id, profile_id)?;
        let secret_ref = secret_headers_ref_for(profile_id);
        let _ = secrets.remove(&secret_ref);
        let _ = secrets.rollback_staged(&secret_ref);
        if let Some(p) = profile {
            if let Some(key_ref) = p.api_key_ref {
                let _ = secrets.remove(&key_ref);
            }
        }
        Ok(credential)
    }

    pub fn migrate_singleton(
        &self,
        owner_id: &str,
        config: &CustomProviderConfig,
        credential_ref: Option<String>,
    ) -> Result<Option<ProviderProfileRecord>> {
        if !self.list(owner_id)?.is_empty() {
            return Ok(None);
        }
        let Some(endpoint) = config.base_url.as_deref() else {
            return Ok(None);
        };
        let alias = config
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("custom");
        let profile = self.create(ProviderProfileInput {
            profile_id: Some(format!("custom:migrated:{}", short_owner(owner_id))),
            owner_id: owner_id.into(),
            alias: alias.into(),
            endpoint: endpoint.into(),
            protocol: config.protocol.clone(),
            credential_ref,
            api_key_ref: None,
            safe_headers_json: serde_json::to_string(&config.headers)?,
            secret_headers_ref: None,
        })?;
        let capabilities = config
            .models
            .iter()
            .map(|model| ProviderProfileModelRecord {
                profile_id: profile.profile_id.clone(),
                model_id: model.clone(),
                text_capable: true,
                vision_capable: false,
                file_input_capable: false,
                native_tools: config.tool_protocol == "native",
                structured_output: matches!(
                    config.tool_protocol.as_str(),
                    "native" | "structured_json"
                ),
                continuation: matches!(config.tool_protocol.as_str(), "native" | "structured_json"),
                native_tools_state: match config.tool_protocol.as_str() {
                    "native" => "supported",
                    "structured_json" | "chat_only" => "unsupported",
                    _ => "unknown",
                }
                .into(),
                structured_output_state: match config.tool_protocol.as_str() {
                    "native" | "structured_json" => "supported",
                    "chat_only" => "unsupported",
                    _ => "unknown",
                }
                .into(),
                continuation_state: match config.tool_protocol.as_str() {
                    "native" | "structured_json" => "supported",
                    "chat_only" => "unsupported",
                    _ => "unknown",
                }
                .into(),
                vision_state: "unknown".into(),
                file_input_state: "unknown".into(),
                model_discovery: true,
                tool_protocol: match config.tool_protocol.as_str() {
                    "native" => "native",
                    "structured_json" => "structured_json_fallback",
                    _ => "chat_only",
                }
                .into(),
                evidence: "migrated v0.2.5 Custom capability; re-probe recommended".into(),
                probe_status: "unprobed".into(),
                probe_version: 1,
                probed_at: Utc::now().to_rfc3339(),
            })
            .collect::<Vec<_>>();
        self.replace_models(owner_id, &profile.profile_id, &capabilities)?;
        self.storage.with_conn(|connection| {
            connection.execute(
                "UPDATE sessions SET account_id=? WHERE owner_principal=? AND provider='custom'",
                params![profile.profile_id, owner_id],
            )?;
            Ok(())
        })?;
        Ok(Some(profile))
    }

    fn update_exact<P: rusqlite::Params>(
        &self,
        _owner_id: &str,
        _profile_id: &str,
        sql: &str,
        params: P,
    ) -> Result<()> {
        self.storage.with_conn(|connection| {
            if connection.execute(sql, params)? != 1 {
                return Err(anyhow!("Custom profile not found for owner"));
            }
            Ok(())
        })
    }
}

impl ProviderProfileRecord {
    pub fn safe_headers(&self) -> Result<BTreeMap<String, String>> {
        parse_safe_headers(&self.safe_headers_json)
    }

    pub fn secret_headers(&self, secrets: &SecretStore) -> Result<BTreeMap<String, String>> {
        load_secret_headers(secrets, self.secret_headers_ref.as_deref())
    }

    pub fn secret_header_names(&self, secrets: &SecretStore) -> Result<Vec<String>> {
        Ok(self.secret_headers(secrets)?.keys().cloned().collect())
    }

    pub fn all_header_names(&self, secrets: &SecretStore) -> Result<Vec<String>> {
        let mut names: Vec<String> = self.safe_headers()?.keys().cloned().collect();
        names.extend(self.secret_headers(secrets)?.keys().cloned());
        names.sort();
        names.dedup();
        Ok(names)
    }

    pub fn merged_headers(&self, secrets: &SecretStore) -> Result<BTreeMap<String, String>> {
        let mut merged = self.safe_headers()?;
        merged.extend(self.secret_headers(secrets)?);
        Ok(merged)
    }
}

/// P1-5 atomic edit service: validate -> stage secret -> DB txn -> commit -> delete old secret -> rollback on fail -> invalidate models on endpoint change.
#[derive(Debug, Clone, Default)]
pub struct CustomProfileEdit {
    pub alias: Option<String>,
    pub endpoint: Option<String>,
    pub protocol: Option<String>,
    pub safe_headers: Option<BTreeMap<String, String>>,
    pub secret_headers: Option<BTreeMap<String, String>>,
    pub clear_secret_headers: bool,
    pub keep_credential_on_endpoint_change: bool,
    pub keep_safe_headers_on_endpoint_change: bool,
    pub keep_secret_headers_on_endpoint_change: bool,
    pub api_key: Option<String>,
    pub remove_api_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomProfileEditResult {
    pub profile: ProviderProfileRecord,
    pub cleanup_warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomProfileDeleteResult {
    pub credential_ref: Option<String>,
    pub cleanup_warnings: Vec<String>,
}

pub struct CustomProfileService {
    storage: Arc<Storage>,
    secrets: SecretStore,
    auth: Option<Arc<AuthManager>>,
}

impl CustomProfileService {
    pub fn new(storage: Arc<Storage>, secrets: SecretStore) -> Self {
        Self {
            storage,
            secrets,
            auth: None,
        }
    }

    pub fn with_auth(storage: Arc<Storage>, secrets: SecretStore, auth: Arc<AuthManager>) -> Self {
        Self {
            storage,
            secrets,
            auth: Some(auth),
        }
    }

    /// Create a profile and all of its write-only credentials as one logical
    /// application operation. Secret material is durable before the profile
    /// row can reference it; a failed DB transaction removes only the newly
    /// staged refs and credential.
    #[allow(clippy::too_many_arguments)]
    pub fn create_profile(
        &self,
        owner_id: &str,
        alias: &str,
        endpoint: &str,
        protocol: &str,
        safe_headers: BTreeMap<String, String>,
        secret_headers: BTreeMap<String, String>,
        api_key: Option<&str>,
    ) -> Result<CustomProfileEditResult> {
        self.create_profile_with_credential_ref(
            owner_id,
            alias,
            endpoint,
            protocol,
            safe_headers,
            secret_headers,
            None,
            api_key,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_profile_with_credential_ref(
        &self,
        owner_id: &str,
        alias: &str,
        endpoint: &str,
        protocol: &str,
        safe_headers: BTreeMap<String, String>,
        secret_headers: BTreeMap<String, String>,
        existing_credential_ref: Option<&str>,
        api_key: Option<&str>,
    ) -> Result<CustomProfileEditResult> {
        if existing_credential_ref.is_some() && api_key.is_some() {
            return Err(anyhow!(
                "cannot attach an existing credential and create a replacement API key"
            ));
        }
        let alias = canonical_alias(alias)?;
        let endpoint = validate_endpoint(endpoint)?;
        validate_protocol(protocol)?;
        let safe_headers_json =
            serde_json::to_string(&parse_safe_headers(&serde_json::to_string(&safe_headers)?)?)?;
        validate_secret_headers(&secret_headers)?;
        let profile_id = format!("custom:{}", Uuid::new_v4().simple());
        let new_secret_ref = if secret_headers.is_empty() {
            None
        } else {
            Some(self.secrets.put_versioned(
                &format!("custom-secret-headers-{profile_id}"),
                &serde_json::to_string(&secret_headers)?,
            )?)
        };
        let replacement_key = api_key.map(str::trim).filter(|value| !value.is_empty());
        let new_credential = if let Some(key) = replacement_key {
            let Some(auth) = self.auth.as_ref() else {
                if let Some(reference) = new_secret_ref.as_deref() {
                    let _ = self.secrets.remove(reference);
                }
                return Err(anyhow!(
                    "Custom profile API-key edits require the auth service"
                ));
            };
            match auth.create_api_key_credential("custom", &alias, key) {
                Ok(record) => Some(record),
                Err(error) => {
                    if let Some(reference) = new_secret_ref.as_deref() {
                        let _ = self.secrets.remove(reference);
                    }
                    return Err(anyhow!(redact_text(&error.to_string())));
                }
            }
        } else {
            None
        };
        let credential_ref = new_credential
            .as_ref()
            .map(|record| record.id.as_str())
            .or(existing_credential_ref);
        let now = Utc::now().to_rfc3339();
        let db_result = self.storage.with_conn(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT OR IGNORE INTO owners(owner_id,telegram_user_id,created_at,updated_at) VALUES(?,NULL,?,?)",
                params![owner_id, now, now],
            )?;
            if let Some(credential_ref) = existing_credential_ref {
                let credential = transaction
                    .query_row(
                        "SELECT owner_id,provider FROM provider_accounts WHERE id=?",
                        params![credential_ref],
                        |row| {
                            Ok((
                                row.get::<_, Option<String>>(0)?,
                                row.get::<_, String>(1)?,
                            ))
                        },
                    )
                    .optional()?
                    .ok_or_else(|| anyhow!("prepared Custom profile credential is missing"))?;
                if credential.0.as_deref() != Some(owner_id) {
                    return Err(anyhow!(
                        "prepared Custom profile credential belongs to another owner"
                    ));
                }
                if credential.1 != "custom" {
                    return Err(anyhow!(
                        "prepared Custom profile credential is not a Custom credential"
                    ));
                }
            }
            if let Some(credential_ref) = new_credential.as_ref().map(|record| record.id.as_str())
            {
                if transaction.execute(
                    "UPDATE provider_accounts SET owner_id=? WHERE id=? AND (owner_id IS NULL OR owner_id=?)",
                    params![owner_id, credential_ref, owner_id],
                )? != 1 {
                    return Err(anyhow!("prepared Custom profile credential is missing"));
                }
            }
            transaction.execute(
                "INSERT INTO provider_profiles(profile_id,owner_id,provider_kind,alias,endpoint,protocol,credential_ref,api_key_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at) VALUES(?,?,'custom',?,?,?,?,NULL,?,?,1,'unknown',?,?)",
                params![profile_id, owner_id, alias, endpoint, protocol, credential_ref, safe_headers_json, new_secret_ref, now, now],
            )?;
            transaction.commit()?;
            Ok(())
        });
        if let Err(error) = db_result {
            if let Some(reference) = new_secret_ref.as_deref() {
                let _ = self.secrets.remove(reference);
            }
            if let (Some(auth), Some(credential)) = (self.auth.as_ref(), new_credential.as_ref()) {
                let _ = auth.logout(&credential.id);
            }
            return Err(anyhow!(redact_text(&error.to_string())));
        }
        let profile = self
            .storage
            .with_conn(|connection| {
                connection
                    .query_row(
                        "SELECT profile_id,owner_id,alias,endpoint,protocol,credential_ref,api_key_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at,last_probe_at FROM provider_profiles WHERE owner_id=? AND profile_id=?",
                        params![owner_id, profile_id],
                        row_profile,
                    )
                    .optional()
                    .map_err(Into::into)
            })?
            .ok_or_else(|| anyhow!("created Custom profile is missing"))?;
        Ok(CustomProfileEditResult {
            profile,
            cleanup_warnings: Vec::new(),
        })
    }

    /// Atomically publish a Custom login wizard's prepared credential,
    /// discovered catalog and selected session model.  The credential itself
    /// is immutable and was stored before this call; SQLite is the sole
    /// authority for the profile/catalog/session/audit switch.
    #[allow(clippy::too_many_arguments)]
    pub fn create_profile_with_models_and_activate_session_with_credential_ref(
        &self,
        owner_id: &str,
        alias: &str,
        endpoint: &str,
        protocol: &str,
        safe_headers: BTreeMap<String, String>,
        existing_credential_ref: Option<&str>,
        models: &[ProviderProfileModelRecord],
        session_id: &str,
        selected_model: &str,
    ) -> Result<CustomProfileEditResult> {
        let profile = ProviderProfileStore::new(self.storage.clone())
            .create_with_models_and_activate_session(
                ProviderProfileInput {
                    profile_id: None,
                    owner_id: owner_id.into(),
                    alias: alias.into(),
                    endpoint: endpoint.into(),
                    protocol: protocol.into(),
                    credential_ref: existing_credential_ref.map(str::to_owned),
                    api_key_ref: None,
                    safe_headers_json: serde_json::to_string(&safe_headers)?,
                    secret_headers_ref: None,
                },
                models,
                session_id,
                selected_model,
            )?;
        Ok(CustomProfileEditResult {
            profile,
            cleanup_warnings: Vec::new(),
        })
    }

    pub fn edit(
        &self,
        owner_id: &str,
        profile_id: &str,
        edit: CustomProfileEdit,
    ) -> Result<ProviderProfileRecord> {
        Ok(self.edit_with_warnings(owner_id, profile_id, edit)?.profile)
    }

    pub fn edit_with_warnings(
        &self,
        owner_id: &str,
        profile_id: &str,
        edit: CustomProfileEdit,
    ) -> Result<CustomProfileEditResult> {
        if edit.api_key.is_some() && edit.remove_api_key {
            return Err(anyhow!("cannot set and remove an API key in one edit"));
        }

        // Validate every non-secret field before preparing any replacement.
        let alias = edit
            .alias
            .as_deref()
            .map(canonical_alias)
            .transpose()
            .map_err(|error| anyhow!(redact_text(&error.to_string())))?;
        let endpoint = edit
            .endpoint
            .as_deref()
            .map(validate_endpoint)
            .transpose()
            .map_err(|error| anyhow!(redact_text(&error.to_string())))?;
        if let Some(protocol) = edit.protocol.as_deref() {
            validate_protocol(protocol)
                .map_err(|error| anyhow!(redact_text(&error.to_string())))?;
        }
        let safe_headers_json = edit
            .safe_headers
            .as_ref()
            .map(|headers| {
                let normalized = parse_safe_headers(&serde_json::to_string(headers)?)?;
                Ok::<_, anyhow::Error>(serde_json::to_string(&normalized)?)
            })
            .transpose()
            .map_err(|error| anyhow!(redact_text(&error.to_string())))?;
        let secret_headers_json = edit
            .secret_headers
            .as_ref()
            .map(|headers| {
                validate_secret_headers(headers)?;
                Ok::<_, anyhow::Error>(serde_json::to_string(headers)?)
            })
            .transpose()
            .map_err(|error| anyhow!(redact_text(&error.to_string())))?;
        if edit.secret_headers.is_some() && edit.clear_secret_headers {
            return Err(anyhow!("cannot set and clear secret headers in one edit"));
        }

        let current = self
            .storage
            .with_conn(|connection| {
                connection
                    .query_row(
                        "SELECT profile_id,owner_id,alias,endpoint,protocol,credential_ref,api_key_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at,last_probe_at FROM provider_profiles WHERE owner_id=? AND profile_id=?",
                        params![owner_id, profile_id],
                        row_profile,
                    )
                    .optional()
                    .map_err(Into::into)
            })?
            .ok_or_else(|| anyhow!("Custom profile not found for owner"))?;

        let endpoint_changed = endpoint
            .as_deref()
            .is_some_and(|value| value != current.endpoint);
        let protocol_changed = edit
            .protocol
            .as_deref()
            .is_some_and(|value| value != current.protocol);
        let replacement_key = edit
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());

        // Prepare immutable references before the authoritative transaction.
        // If the transaction fails, these fresh values are garbage-collected;
        // the old profile remains untouched.
        let new_secret_ref = if let Some(json) = secret_headers_json.as_deref() {
            Some(
                self.secrets
                    .put_versioned(&format!("custom-secret-headers-{profile_id}"), json)
                    .map_err(|error| anyhow!(redact_text(&error.to_string())))?,
            )
        } else {
            None
        };
        let new_credential = if let Some(key) = replacement_key {
            let auth = self
                .auth
                .as_ref()
                .ok_or_else(|| anyhow!("Custom profile API-key edits require the auth service"));
            match auth {
                Ok(auth) => match auth.create_api_key_credential(
                    "custom",
                    alias.as_deref().unwrap_or(&current.alias),
                    key,
                ) {
                    Ok(record) => Some(record),
                    Err(error) => {
                        if let Some(reference) = new_secret_ref.as_deref() {
                            let _ = self.secrets.remove(reference);
                        }
                        return Err(anyhow!(redact_text(&error.to_string())));
                    }
                },
                Err(error) => {
                    if let Some(reference) = new_secret_ref.as_deref() {
                        let _ = self.secrets.remove(reference);
                    }
                    return Err(error);
                }
            }
        } else {
            None
        };
        let new_credential_ref = new_credential.as_ref().map(|record| record.id.clone());

        let next_safe_headers = if let Some(headers) = safe_headers_json.as_deref() {
            headers.to_owned()
        } else if endpoint_changed && !edit.keep_safe_headers_on_endpoint_change {
            "{}".into()
        } else {
            current.safe_headers_json.clone()
        };
        let next_secret_ref = if edit.clear_secret_headers {
            None
        } else if let Some(reference) = new_secret_ref.as_deref() {
            Some(reference.to_owned())
        } else if endpoint_changed && !edit.keep_secret_headers_on_endpoint_change {
            None
        } else {
            current.secret_headers_ref.clone()
        };
        let next_credential_ref = if edit.remove_api_key {
            None
        } else if let Some(reference) = new_credential_ref.as_deref() {
            Some(reference.to_owned())
        } else if endpoint_changed && !edit.keep_credential_on_endpoint_change {
            None
        } else {
            current.credential_ref.clone()
        };

        let db_result: Result<()> = self.storage.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_profiles WHERE owner_id=? AND profile_id=?)",
                params![owner_id, profile_id],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(anyhow!("Custom profile not found for owner"));
            }
            if let Some(credential_ref) = new_credential_ref.as_deref() {
                let assigned = transaction.execute(
                    "UPDATE provider_accounts SET owner_id=? WHERE id=?",
                    params![owner_id, credential_ref],
                )?;
                if assigned != 1 {
                    return Err(anyhow!("prepared Custom profile credential is missing"));
                }
            }
            let next_alias = alias.as_deref().unwrap_or(&current.alias);
            let next_endpoint = endpoint.as_deref().unwrap_or(&current.endpoint);
            let next_protocol = edit.protocol.as_deref().unwrap_or(&current.protocol);
            transaction.execute(
                "UPDATE provider_profiles SET alias=?,endpoint=?,protocol=?,safe_headers_json=?,secret_headers_ref=?,credential_ref=?,reachability='unknown',last_probe_at=NULL,updated_at=? WHERE owner_id=? AND profile_id=?",
                params![next_alias, next_endpoint, next_protocol, next_safe_headers, next_secret_ref, next_credential_ref, Utc::now().to_rfc3339(), owner_id, profile_id],
            )?;
            if endpoint_changed || protocol_changed {
                transaction.execute(
                    "DELETE FROM provider_profile_models WHERE profile_id=?",
                    params![profile_id],
                )?;
                transaction.execute(
                    "UPDATE provider_capability_evidence SET state='unknown',source='invalidated_on_endpoint_change',observed_at=? WHERE profile_id=? AND source != 'owner_override'",
                    params![Utc::now().to_rfc3339(), profile_id],
                )?;
            }
            transaction.commit()?;
            Ok(())
        });

        if let Err(error) = db_result {
            if let Some(reference) = new_secret_ref.as_deref() {
                let _ = self.secrets.remove(reference);
            }
            if let (Some(auth), Some(credential)) = (self.auth.as_ref(), new_credential.as_ref()) {
                let _ = auth.logout(&credential.id);
            }
            return Err(anyhow!(redact_text(&error.to_string())));
        }

        let mut cleanup_warnings = Vec::new();
        if current.secret_headers_ref.as_deref() != next_secret_ref.as_deref() {
            if let Some(reference) = current.secret_headers_ref.as_deref() {
                let injected_cleanup_failure = std::env::var("XIAO_INJECT_PROFILE_FAILURE")
                    .ok()
                    .is_some_and(|value| value == "secret_gc");
                if injected_cleanup_failure {
                    cleanup_warnings.push(
                        "obsolete secret-header reference cleanup deferred: injected failure"
                            .into(),
                    );
                } else if let Err(error) = self.secrets.remove(reference) {
                    cleanup_warnings.push(format!(
                        "obsolete secret-header reference cleanup deferred: {}",
                        redact_text(&error.to_string())
                    ));
                }
            }
        }
        if current.credential_ref.as_deref() != next_credential_ref.as_deref() {
            if let Some(reference) = current.credential_ref.as_deref() {
                if let Some(auth) = self.auth.as_ref() {
                    let injected_cleanup_failure = std::env::var("XIAO_INJECT_PROFILE_FAILURE")
                        .ok()
                        .is_some_and(|value| value == "credential_gc");
                    if injected_cleanup_failure {
                        cleanup_warnings
                            .push("obsolete credential cleanup deferred: injected failure".into());
                    } else if let Err(error) = auth.logout(reference) {
                        cleanup_warnings.push(format!(
                            "obsolete credential cleanup deferred: {}",
                            redact_text(&error.to_string())
                        ));
                    }
                } else {
                    cleanup_warnings
                        .push("obsolete credential cleanup requires the auth service".into());
                }
            }
        }
        if endpoint_changed
            && (edit.keep_credential_on_endpoint_change
                || edit.keep_safe_headers_on_endpoint_change
                || edit.keep_secret_headers_on_endpoint_change)
        {
            if let Err(error) = self.storage.audit(
                owner_id,
                "custom_profile_endpoint_credentials_retained",
                &format!(
                    "profile_id={profile_id};credential={};safe_headers={};secret_headers={}",
                    edit.keep_credential_on_endpoint_change,
                    edit.keep_safe_headers_on_endpoint_change,
                    edit.keep_secret_headers_on_endpoint_change
                ),
            ) {
                cleanup_warnings.push(format!(
                    "retention audit deferred: {}",
                    redact_text(&error.to_string())
                ));
            }
        }
        if !cleanup_warnings.is_empty() {
            let audit = cleanup_warnings.join("; ");
            if let Err(error) = self.storage.audit(
                owner_id,
                "custom_profile_cleanup_warning",
                &format!("profile_id={profile_id};{audit}"),
            ) {
                cleanup_warnings.push(format!(
                    "cleanup warning audit deferred: {}",
                    redact_text(&error.to_string())
                ));
            }
        }

        let profile = self
            .storage
            .with_conn(|connection| {
                connection
                    .query_row(
                        "SELECT profile_id,owner_id,alias,endpoint,protocol,credential_ref,api_key_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at,last_probe_at FROM provider_profiles WHERE owner_id=? AND profile_id=?",
                        params![owner_id, profile_id],
                        row_profile,
                    )
                    .optional()
                    .map_err(Into::into)
            })?
            .ok_or_else(|| anyhow!("edited Custom profile is missing"))?;
        Ok(CustomProfileEditResult {
            profile,
            cleanup_warnings,
        })
    }

    /// Delete a profile through the same application service as create/edit.
    /// The profile row is authoritative once its transaction commits; secret
    /// and credential cleanup is deliberately post-commit and therefore
    /// returns bounded warnings instead of manufacturing a rollback/error.
    pub fn delete_with_warnings(
        &self,
        owner_id: &str,
        profile_id: &str,
    ) -> Result<CustomProfileDeleteResult> {
        let current = ProviderProfileStore::new(self.storage.clone())
            .get(owner_id, profile_id)?
            .ok_or_else(|| anyhow!("Custom profile not found for owner"))?;
        let credential_ref =
            ProviderProfileStore::new(self.storage.clone()).delete(owner_id, profile_id)?;
        let mut cleanup_warnings = Vec::new();

        if let Some(reference) = current.secret_headers_ref.as_deref() {
            if let Err(error) = self.secrets.remove(reference) {
                cleanup_warnings.push(format!(
                    "obsolete secret-header reference cleanup deferred: {}",
                    redact_text(&error.to_string())
                ));
            }
        }
        if let Some(reference) = credential_ref.as_deref() {
            if let Some(auth) = self.auth.as_ref() {
                if let Err(error) = auth.logout(reference) {
                    cleanup_warnings.push(format!(
                        "obsolete credential cleanup deferred: {}",
                        redact_text(&error.to_string())
                    ));
                }
            } else {
                cleanup_warnings
                    .push("obsolete credential cleanup requires the auth service".into());
            }
        }
        if !cleanup_warnings.is_empty() {
            let audit = cleanup_warnings.join("; ");
            if let Err(error) = self.storage.audit(
                owner_id,
                "custom_profile_cleanup_warning",
                &format!("profile_id={profile_id};{audit}"),
            ) {
                cleanup_warnings.push(format!(
                    "cleanup warning audit deferred: {}",
                    redact_text(&error.to_string())
                ));
            }
        }
        Ok(CustomProfileDeleteResult {
            credential_ref,
            cleanup_warnings,
        })
    }
}

fn row_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderProfileRecord> {
    Ok(ProviderProfileRecord {
        profile_id: row.get(0)?,
        owner_id: row.get(1)?,
        alias: row.get(2)?,
        endpoint: row.get(3)?,
        protocol: row.get(4)?,
        credential_ref: row.get(5)?,
        api_key_ref: row.get(6)?,
        safe_headers_json: row.get(7)?,
        secret_headers_ref: row.get(8)?,
        enabled: row.get::<_, i64>(9)? != 0,
        reachability: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        last_probe_at: row.get(13)?,
    })
}

fn row_profile_model(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderProfileModelRecord> {
    Ok(ProviderProfileModelRecord {
        profile_id: row.get(0)?,
        model_id: row.get(1)?,
        text_capable: row.get::<_, i64>(2)? != 0,
        vision_capable: row.get::<_, i64>(3)? != 0,
        file_input_capable: row.get::<_, i64>(4)? != 0,
        native_tools: row.get::<_, i64>(5)? != 0,
        structured_output: row.get::<_, i64>(6)? != 0,
        continuation: row.get::<_, i64>(7)? != 0,
        native_tools_state: row.get(8)?,
        structured_output_state: row.get(9)?,
        continuation_state: row.get(10)?,
        vision_state: row.get(11)?,
        file_input_state: row.get(12)?,
        model_discovery: row.get::<_, i64>(13)? != 0,
        tool_protocol: row.get(14)?,
        evidence: row.get(15)?,
        probe_status: row.get(16)?,
        probe_version: row.get::<_, i64>(17)? as u32,
        probed_at: row.get(18)?,
    })
}

fn canonical_alias(value: &str) -> Result<String> {
    let value = value.trim().to_ascii_lowercase().replace(' ', "-");
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        });
    valid
        .then_some(value)
        .ok_or_else(|| anyhow!("Custom profile alias must use lowercase letters, digits, - or _"))
}

fn validate_endpoint(value: &str) -> Result<String> {
    let mut parsed = Url::parse(value.trim()).context("invalid Custom profile endpoint")?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(anyhow!(
            "Custom profile endpoint must be HTTP(S), include a host, and contain no credentials"
        ));
    }
    parsed.set_fragment(None);
    Ok(parsed.to_string().trim_end_matches('/').to_owned())
}

fn validate_protocol(value: &str) -> Result<()> {
    if matches!(value, "openai_chat_completions" | "openai_responses") {
        Ok(())
    } else {
        Err(anyhow!("unsupported Custom profile protocol"))
    }
}

fn validate_tool_protocol(value: &str) -> Result<()> {
    if matches!(value, "native" | "structured_json_fallback" | "chat_only") {
        Ok(())
    } else {
        Err(anyhow!("invalid profile tool protocol"))
    }
}

fn parse_safe_headers(value: &str) -> Result<BTreeMap<String, String>> {
    let headers = serde_json::from_str::<BTreeMap<String, String>>(value)
        .context("Custom profile safe headers must be a JSON object")?;
    for (name, value) in &headers {
        let lower = name.trim().to_ascii_lowercase();
        if lower.is_empty()
            || lower.chars().count() > 128
            || value.chars().count() > 4_096
            || [
                "authorization",
                "cookie",
                "proxy-authorization",
                "x-api-key",
            ]
            .contains(&lower.as_str())
            || lower.contains("token")
            || lower.contains("secret")
        {
            return Err(anyhow!(
                "Custom profile contains a forbidden or secret-bearing plain header"
            ));
        }
    }
    Ok(headers)
}

fn validate_secret_headers(headers: &BTreeMap<String, String>) -> Result<()> {
    for (name, value) in headers {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 128 || value.chars().count() > 4_096 {
            return Err(anyhow!("Custom secret header name or value is invalid"));
        }
    }
    Ok(())
}

fn parse_secret_headers(value: &str) -> Result<BTreeMap<String, String>> {
    let headers = serde_json::from_str::<BTreeMap<String, String>>(value)
        .context("Custom secret headers must be a JSON object")?;
    validate_secret_headers(&headers)?;
    Ok(headers)
}

fn load_secret_headers(
    secrets: &SecretStore,
    secret_ref: Option<&str>,
) -> Result<BTreeMap<String, String>> {
    let Some(reference) = secret_ref else {
        return Ok(BTreeMap::new());
    };
    let Some(json) = secrets.get(reference)? else {
        return Ok(BTreeMap::new());
    };
    parse_secret_headers(&json)
}

pub fn secret_headers_ref_for(profile_id: &str) -> String {
    format!("custom_secret_headers:{profile_id}")
}

fn short_owner(owner_id: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(owner_id.as_bytes()))[..12].into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    fn fixture() -> (
        Arc<Storage>,
        Arc<AuthManager>,
        SecretStore,
        tempfile::TempDir,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(Storage::open_memory().unwrap());
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::AppConfig::default()));
        let auth = Arc::new(AuthManager::with_config(
            storage.clone(),
            directory.path().join("secrets"),
            config,
        ));
        let secrets = auth.secrets().clone();
        (storage, auth, secrets, directory)
    }

    fn probed_model(profile_id: &str, model_id: &str) -> ProviderProfileModelRecord {
        ProviderProfileModelRecord {
            profile_id: profile_id.into(),
            model_id: model_id.into(),
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
            evidence: "deterministic exact-model probe".into(),
            probe_status: "completed".into(),
            probe_version: 1,
            probed_at: Utc::now().to_rfc3339(),
        }
    }

    fn secret_filenames(directory: &tempfile::TempDir) -> Vec<String> {
        let mut names = std::fs::read_dir(directory.path().join("secrets"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    static PROFILE_ENV_LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();

    #[test]
    fn endpoint_edit_clears_credentials_and_headers() {
        let storage = Arc::new(Storage::open_memory().unwrap());
        let store = ProviderProfileStore::new(storage);
        let profile = store
            .create(ProviderProfileInput {
                profile_id: None,
                owner_id: "owner:test".into(),
                alias: "a".into(),
                endpoint: "https://a.example/v1".into(),
                protocol: "openai_chat_completions".into(),
                credential_ref: Some("credential-a".into()),
                api_key_ref: None,
                safe_headers_json: r#"{"X-Workspace":"A"}"#.into(),
                secret_headers_ref: None,
            })
            .unwrap();
        store
            .change_endpoint("owner:test", &profile.profile_id, "https://b.example/v1")
            .unwrap();
        let changed = store
            .get("owner:test", &profile.profile_id)
            .unwrap()
            .unwrap();
        assert_eq!(changed.endpoint, "https://b.example/v1");
        assert!(changed.credential_ref.is_none());
        assert!(changed.safe_headers().unwrap().is_empty());
    }

    #[test]
    fn custom_profile_a_secrets_never_reach_profile_b() {
        let (storage, auth, secrets, _directory) = fixture();
        let service = CustomProfileService::with_auth(storage, secrets.clone(), auth.clone());
        let profile_a = service
            .create_profile(
                "owner:test",
                "profile-a",
                "https://a.example/v1",
                "openai_chat_completions",
                [("X-Profile-A", "safe-a")]
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into()))
                    .collect(),
                [("X-Secret-A", "secret-a")]
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into()))
                    .collect(),
                Some("API_KEY_A"),
            )
            .unwrap()
            .profile;
        let profile_b = service
            .create_profile(
                "owner:test",
                "profile-b",
                "https://b.example/v1",
                "openai_chat_completions",
                BTreeMap::new(),
                BTreeMap::new(),
                None,
            )
            .unwrap()
            .profile;

        assert_eq!(
            profile_a
                .secret_headers(&secrets)
                .unwrap()
                .get("X-Secret-A")
                .map(String::as_str),
            Some("secret-a")
        );
        assert!(profile_b.merged_headers(&secrets).unwrap().is_empty());
        assert!(profile_b.credential_ref.is_none());
        assert!(profile_a
            .credential_ref
            .as_deref()
            .and_then(|id| auth.credential(id).unwrap())
            .is_some_and(|credential| credential.api_key.as_deref() == Some("API_KEY_A")));
    }

    #[test]
    fn existing_credential_ref_must_be_same_owner_and_custom_provider() {
        let (storage, auth, secrets, _directory) = fixture();
        let service = CustomProfileService::with_auth(storage.clone(), secrets, auth.clone());
        let credential = auth
            .create_api_key_credential("custom", "owner-a", "API_KEY_A")
            .unwrap();
        storage
            .set_account_owner("owner:a", &credential.id)
            .unwrap();

        let cross_owner = service
            .create_profile_with_credential_ref(
                "owner:b",
                "cross-owner",
                "https://b.example/v1",
                "openai_chat_completions",
                BTreeMap::new(),
                BTreeMap::new(),
                Some(&credential.id),
                None,
            )
            .unwrap_err()
            .to_string();
        assert!(cross_owner.contains("another owner"));
        assert!(ProviderProfileStore::new(storage.clone())
            .list("owner:b")
            .unwrap()
            .is_empty());

        let other_provider = auth
            .create_api_key_credential("codex", "codex", "OAUTH_MATERIAL")
            .unwrap();
        storage
            .set_account_owner("owner:a", &other_provider.id)
            .unwrap();
        let wrong_provider = service
            .create_profile_with_credential_ref(
                "owner:a",
                "wrong-provider",
                "https://a.example/v1",
                "openai_chat_completions",
                BTreeMap::new(),
                BTreeMap::new(),
                Some(&other_provider.id),
                None,
            )
            .unwrap_err()
            .to_string();
        assert!(wrong_provider.contains("not a Custom credential"));
    }

    #[test]
    fn endpoint_replacement_swaps_all_profile_scoped_secrets_in_one_patch() {
        let (storage, auth, secrets, _directory) = fixture();
        let service =
            CustomProfileService::with_auth(storage.clone(), secrets.clone(), auth.clone());
        let old = service
            .create_profile(
                "owner:test",
                "replace-me",
                "https://old.example/v1",
                "openai_chat_completions",
                [("X-Old-Safe", "safe-old")]
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into()))
                    .collect(),
                [("X-Old-Secret", "secret-old")]
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into()))
                    .collect(),
                Some("API_KEY_OLD"),
            )
            .unwrap()
            .profile;
        let old_credential = old.credential_ref.clone().unwrap();
        let old_secret_ref = old.secret_headers_ref.clone().unwrap();
        ProviderProfileStore::new(storage.clone())
            .replace_models(
                "owner:test",
                &old.profile_id,
                &[probed_model(&old.profile_id, "old-model")],
            )
            .unwrap();

        let edited = service
            .edit_with_warnings(
                "owner:test",
                &old.profile_id,
                CustomProfileEdit {
                    endpoint: Some("https://new.example/v1".into()),
                    api_key: Some("API_KEY_NEW".into()),
                    safe_headers: Some(
                        [("X-New-Safe", "safe-new")]
                            .into_iter()
                            .map(|(key, value)| (key.into(), value.into()))
                            .collect(),
                    ),
                    secret_headers: Some(
                        [("X-New-Secret", "secret-new")]
                            .into_iter()
                            .map(|(key, value)| (key.into(), value.into()))
                            .collect(),
                    ),
                    ..Default::default()
                },
            )
            .unwrap();
        let current = edited.profile;
        assert_eq!(current.endpoint, "https://new.example/v1");
        assert_eq!(
            current
                .safe_headers()
                .unwrap()
                .get("X-New-Safe")
                .map(String::as_str),
            Some("safe-new")
        );
        assert_eq!(
            current
                .secret_headers(&secrets)
                .unwrap()
                .get("X-New-Secret")
                .map(String::as_str),
            Some("secret-new")
        );
        assert_ne!(
            current.credential_ref.as_deref(),
            Some(old_credential.as_str())
        );
        assert_ne!(
            current.secret_headers_ref.as_deref(),
            Some(old_secret_ref.as_str())
        );
        assert!(auth.credential(&old_credential).unwrap().is_none());
        assert!(secrets.get(&old_secret_ref).unwrap().is_none());
        assert!(current
            .credential_ref
            .as_deref()
            .and_then(|id| auth.credential(id).unwrap())
            .is_some_and(|credential| credential.api_key.as_deref() == Some("API_KEY_NEW")));
        assert!(ProviderProfileStore::new(storage.clone())
            .models(&current.profile_id)
            .unwrap()
            .is_empty());
        assert_eq!(current.reachability, "unknown");
        assert!(current.last_probe_at.is_none());
    }

    #[test]
    fn profile_db_failure_after_secret_staging_leaves_old_state_and_no_new_refs() {
        let (storage, auth, secrets, directory) = fixture();
        let service =
            CustomProfileService::with_auth(storage.clone(), secrets.clone(), auth.clone());
        let old = service
            .create_profile(
                "owner:test",
                "stable",
                "https://stable.example/v1",
                "openai_chat_completions",
                BTreeMap::new(),
                [("X-Stable", "stable-secret")]
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into()))
                    .collect(),
                Some("STABLE_KEY"),
            )
            .unwrap()
            .profile;
        let before_files = secret_filenames(&directory);
        storage
            .with_conn(|connection| {
                connection.execute_batch(
                    "CREATE TRIGGER reject_profile_edit BEFORE UPDATE ON provider_profiles
                     WHEN NEW.alias='reject'
                     BEGIN SELECT RAISE(FAIL,'synthetic profile DB failure'); END;",
                )?;
                Ok(())
            })
            .unwrap();
        let error = service
            .edit_with_warnings(
                "owner:test",
                &old.profile_id,
                CustomProfileEdit {
                    alias: Some("reject".into()),
                    api_key: Some("NEW_KEY_MUST_NOT_COMMIT".into()),
                    secret_headers: Some(
                        [("X-New", "new-secret")]
                            .into_iter()
                            .map(|(key, value)| (key.into(), value.into()))
                            .collect(),
                    ),
                    ..Default::default()
                },
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("synthetic profile DB failure"));
        let current = ProviderProfileStore::new(storage.clone())
            .get("owner:test", &old.profile_id)
            .unwrap()
            .unwrap();
        assert_eq!(current.alias, "stable");
        assert_eq!(current.credential_ref, old.credential_ref);
        assert_eq!(current.secret_headers_ref, old.secret_headers_ref);
        assert_eq!(secret_filenames(&directory), before_files);
        assert!(!auth
            .accounts(Some("custom"))
            .unwrap()
            .iter()
            .any(|account| auth.credential(&account.id).unwrap().is_some_and(
                |credential| credential.api_key.as_deref() == Some("NEW_KEY_MUST_NOT_COMMIT")
            )));
        storage
            .with_conn(|connection| {
                connection.execute_batch("DROP TRIGGER reject_profile_edit;")?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn post_commit_secret_gc_failure_is_success_with_bounded_warning() {
        let _guard = PROFILE_ENV_LOCK
            .get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap();
        let (storage, auth, secrets, _directory) = fixture();
        let service = CustomProfileService::with_auth(storage, secrets.clone(), auth);
        let old = service
            .create_profile(
                "owner:test",
                "gc",
                "https://old.example/v1",
                "openai_chat_completions",
                BTreeMap::new(),
                [("X-Old", "old-secret")]
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into()))
                    .collect(),
                None,
            )
            .unwrap()
            .profile;
        let old_ref = old.secret_headers_ref.clone().unwrap();
        std::env::set_var("XIAO_INJECT_PROFILE_FAILURE", "secret_gc");
        let result = service
            .edit_with_warnings(
                "owner:test",
                &old.profile_id,
                CustomProfileEdit {
                    endpoint: Some("https://new.example/v1".into()),
                    secret_headers: Some(
                        [("X-New", "new-secret")]
                            .into_iter()
                            .map(|(key, value)| (key.into(), value.into()))
                            .collect(),
                    ),
                    ..Default::default()
                },
            )
            .unwrap();
        std::env::remove_var("XIAO_INJECT_PROFILE_FAILURE");
        assert_eq!(result.profile.endpoint, "https://new.example/v1");
        assert!(result
            .cleanup_warnings
            .iter()
            .any(|warning| warning.contains("secret-header") && warning.contains("deferred")));
        assert!(secrets.get(&old_ref).unwrap().is_some());
        assert_ne!(
            result.profile.secret_headers_ref.as_deref(),
            Some(old_ref.as_str())
        );
    }

    #[test]
    fn profile_delete_commits_then_collects_versioned_secret_and_credential() {
        let (storage, auth, secrets, _directory) = fixture();
        let service =
            CustomProfileService::with_auth(storage.clone(), secrets.clone(), auth.clone());
        let profile = service
            .create_profile(
                "owner:test",
                "delete-me",
                "https://delete.example/v1",
                "openai_chat_completions",
                BTreeMap::new(),
                [("X-Secret", "delete-secret")]
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into()))
                    .collect(),
                Some("DELETE_KEY"),
            )
            .unwrap()
            .profile;
        let secret_ref = profile.secret_headers_ref.clone().unwrap();
        let credential_ref = profile.credential_ref.clone().unwrap();
        let result = service
            .delete_with_warnings("owner:test", &profile.profile_id)
            .unwrap();
        assert!(result.cleanup_warnings.is_empty());
        assert_eq!(
            result.credential_ref.as_deref(),
            Some(credential_ref.as_str())
        );
        assert!(ProviderProfileStore::new(storage.clone())
            .get("owner:test", &profile.profile_id)
            .unwrap()
            .is_none());
        assert!(secrets.get(&secret_ref).unwrap().is_none());
        assert!(auth.credential(&credential_ref).unwrap().is_none());
    }
}
