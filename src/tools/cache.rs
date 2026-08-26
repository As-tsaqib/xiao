use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::security::redact::contains_secret_material;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedPlan {
    pub steps: serde_json::Value,
    pub schema_version: u32,
    pub environment_fingerprint: String,
}

impl CachedPlan {
    pub fn key(&self) -> Result<String> {
        let normalized = serde_json::to_vec(self)?;
        if contains_secret_material(&String::from_utf8_lossy(&normalized)) {
            return Err(anyhow!("secret-bearing plan cannot be cached"));
        }
        Ok(format!("{:x}", Sha256::digest(normalized)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedScript {
    pub path: PathBuf,
    pub interpreter: PathBuf,
    pub sha256: String,
    pub source: String,
}

impl CachedScript {
    pub fn verify(&self) -> Result<()> {
        if !self.path.is_file() || !self.interpreter.is_absolute() || self.source.trim().is_empty()
        {
            return Err(anyhow!(
                "cached script manifest is not file-backed and auditable"
            ));
        }
        let bytes = std::fs::read(&self.path)?;
        if contains_secret_material(&String::from_utf8_lossy(&bytes)) {
            return Err(anyhow!("secret-bearing script cannot be cached"));
        }
        let actual = format!("{:x}", Sha256::digest(bytes));
        if actual != self.sha256 {
            return Err(anyhow!("cached script content hash changed"));
        }
        Ok(())
    }
}

pub fn dynamic_observation_is_cacheable(tool: &str) -> bool {
    !matches!(
        tool,
        "termux_terminal" | "termux_job" | "context_stats" | "session_search" | "memory_search"
    )
}

pub fn script_hash(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(std::fs::read(path)?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plan_keys_are_stable_secret_free_and_environment_scoped() {
        let plan = CachedPlan {
            steps: json!([{"program":"ps","args":["-A"]}]),
            schema_version: 1,
            environment_fingerprint: "termux-v1".into(),
        };
        assert_eq!(plan.key().unwrap(), plan.clone().key().unwrap());
        let mut changed = plan.clone();
        changed.environment_fingerprint = "termux-v2".into();
        assert_ne!(plan.key().unwrap(), changed.key().unwrap());
        let secret = CachedPlan {
            steps: json!({"authorization":"Bearer sk-secret123456789"}),
            ..plan
        };
        assert!(secret.key().is_err());
        assert!(!dynamic_observation_is_cacheable("termux_job"));
    }

    #[test]
    fn file_backed_script_hash_is_checked_before_execution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inspect.sh");
        std::fs::write(&path, "printf ok").unwrap();
        let cached = CachedScript {
            path: path.clone(),
            interpreter: "/bin/sh".into(),
            sha256: script_hash(&path).unwrap(),
            source: "builtin:test".into(),
        };
        cached.verify().unwrap();
        std::fs::write(path, "printf changed").unwrap();
        assert!(cached.verify().is_err());
    }
}
