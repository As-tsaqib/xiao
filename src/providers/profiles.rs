use std::{collections::BTreeMap, sync::Arc};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use url::Url;
use uuid::Uuid;

use crate::{
    config::CustomProviderConfig,
    security::secrets::SecretStore,
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
                "INSERT INTO provider_profiles(profile_id,owner_id,provider_kind,alias,endpoint,protocol,credential_ref,safe_headers_json,enabled,reachability,created_at,updated_at) VALUES(?,?,'custom',?,?,?,?,?,1,'unknown',?,?)",
                params![profile_id, input.owner_id, input.alias, input.endpoint, input.protocol, input.credential_ref, input.safe_headers_json, now, now],
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
                "SELECT profile_id,owner_id,alias,endpoint,protocol,credential_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at,last_probe_at FROM provider_profiles WHERE owner_id=? ORDER BY updated_at DESC,alias",
            )?;
            let rows = statement.query_map(params![owner_id], row_profile)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn get(&self, owner_id: &str, profile_id: &str) -> Result<Option<ProviderProfileRecord>> {
        self.storage.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT profile_id,owner_id,alias,endpoint,protocol,credential_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at,last_probe_at FROM provider_profiles WHERE owner_id=? AND profile_id=?",
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
                    "SELECT profile_id,owner_id,alias,endpoint,protocol,credential_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at,last_probe_at FROM provider_profiles WHERE profile_id=?",
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
                    "SELECT profile_id,owner_id,alias,endpoint,protocol,credential_ref,safe_headers_json,secret_headers_ref,enabled,reachability,created_at,updated_at,last_probe_at FROM provider_profiles WHERE owner_id=? AND alias=?",
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
                "UPDATE provider_profiles SET endpoint=?,credential_ref=NULL,safe_headers_json='{}',secret_headers_ref=NULL,reachability='unknown',last_probe_at=NULL,updated_at=? WHERE owner_id=? AND profile_id=?",
                params![endpoint, Utc::now().to_rfc3339(), owner_id, profile_id],
            )?;
            if changed != 1 {
                return Err(anyhow!("Custom profile not found for owner"));
            }
            transaction.execute(
                "DELETE FROM provider_profile_models WHERE profile_id=?",
                params![profile_id],
            )?;
            transaction.commit()?;
            Ok(())
        })
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
                    "INSERT INTO provider_profile_models(profile_id,model_id,text_capable,vision_capable,file_input_capable,native_tools,structured_output,continuation,native_tools_state,structured_output_state,continuation_state,vision_state,file_input_state,model_discovery,tool_protocol,evidence,probed_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    params![profile_id, model.model_id, model.text_capable as i32, model.vision_capable as i32, model.file_input_capable as i32, model.native_tools as i32, model.structured_output as i32, model.continuation as i32, model.native_tools_state, model.structured_output_state, model.continuation_state, model.vision_state, model.file_input_state, model.model_discovery as i32, model.tool_protocol, model.evidence, model.probed_at],
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
                "SELECT profile_id,model_id,text_capable,vision_capable,file_input_capable,native_tools,structured_output,continuation,native_tools_state,structured_output_state,continuation_state,vision_state,file_input_state,model_discovery,tool_protocol,evidence,probed_at FROM provider_profile_models WHERE profile_id=? ORDER BY model_id",
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
                    "SELECT profile_id,model_id,text_capable,vision_capable,file_input_capable,native_tools,structured_output,continuation,native_tools_state,structured_output_state,continuation_state,vision_state,file_input_state,model_discovery,tool_protocol,evidence,probed_at FROM provider_profile_models WHERE profile_id=? AND model_id=?",
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
            safe_headers_json: serde_json::to_string(&config.headers)?,
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
}

fn row_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderProfileRecord> {
    Ok(ProviderProfileRecord {
        profile_id: row.get(0)?,
        owner_id: row.get(1)?,
        alias: row.get(2)?,
        endpoint: row.get(3)?,
        protocol: row.get(4)?,
        credential_ref: row.get(5)?,
        safe_headers_json: row.get(6)?,
        secret_headers_ref: row.get(7)?,
        enabled: row.get::<_, i64>(8)? != 0,
        reachability: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        last_probe_at: row.get(12)?,
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
        probed_at: row.get(16)?,
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

fn short_owner(owner_id: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(owner_id.as_bytes()))[..12].into()
}

#[cfg(test)]
mod tests {
    use super::*;

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
                safe_headers_json: r#"{"X-Workspace":"A"}"#.into(),
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
}
