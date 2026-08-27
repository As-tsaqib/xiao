use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::security::redact::contains_secret_material;

const TRUSTED_INTERPRETERS: &[&str] = &[
    "/bin/sh",
    "/bin/bash",
    "/usr/bin/sh",
    "/usr/bin/bash",
    "/system/bin/sh",
    "/data/data/com.termux/files/usr/bin/sh",
    "/data/data/com.termux/files/usr/bin/bash",
];

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

#[derive(Debug, Clone, Default)]
pub struct PlanCache {
    plans: Arc<RwLock<HashMap<String, CachedPlan>>>,
}

impl PlanCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, plan: CachedPlan) -> Result<String> {
        let key = plan.key()?;
        self.plans.write().unwrap().insert(key.clone(), plan);
        Ok(key)
    }

    pub fn get(&self, key: &str) -> Option<CachedPlan> {
        self.plans.read().unwrap().get(key).cloned()
    }

    pub fn clear(&self) {
        self.plans.write().unwrap().clear();
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
        let interp_str = self.interpreter.to_string_lossy();
        if !TRUSTED_INTERPRETERS
            .iter()
            .any(|allowed| interp_str == *allowed)
        {
            return Err(anyhow!("untrusted script interpreter: {}", interp_str));
        }
        let bytes = std::fs::read(&self.path)?;
        let content = String::from_utf8_lossy(&bytes);
        if contains_secret_material(&content) {
            return Err(anyhow!("secret-bearing script cannot be cached"));
        }
        let lower = content.to_ascii_lowercase();
        if lower.contains("su ")
            || lower.contains("tsu ")
            || lower.contains("sudo ")
            || lower.starts_with(
                "su
",
            )
            || lower.starts_with(
                "tsu
",
            )
        {
            return Err(anyhow!("script contains root escalation commands"));
        }
        let actual = format!("{:x}", Sha256::digest(bytes));
        if actual != self.sha256 {
            return Err(anyhow!("cached script content hash changed"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScriptCache {
    scripts: Arc<RwLock<HashMap<PathBuf, CachedScript>>>,
}

impl ScriptCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, script: CachedScript) -> Result<()> {
        script.verify()?;
        self.scripts
            .write()
            .unwrap()
            .insert(script.path.clone(), script);
        Ok(())
    }

    pub fn get(&self, path: &Path) -> Option<CachedScript> {
        self.scripts.read().unwrap().get(path).cloned()
    }

    pub fn verify_path(&self, path: &Path) -> Result<CachedScript> {
        let script = self
            .get(path)
            .ok_or_else(|| anyhow!("script not found in cache"))?;
        script.verify()?;
        Ok(script)
    }

    pub fn clear(&self) {
        self.scripts.write().unwrap().clear();
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

    #[test]
    fn script_cannot_become_root_escalation_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad_script.sh");
        std::fs::write(&path, "su -c 'id'").unwrap();
        let cached = CachedScript {
            path: path.clone(),
            interpreter: "/bin/sh".into(),
            sha256: script_hash(&path).unwrap(),
            source: "builtin:test".into(),
        };
        assert!(cached.verify().is_err());

        // Untrusted interpreter
        let safe_path = dir.path().join("safe.sh");
        std::fs::write(&safe_path, "echo hello").unwrap();
        let untrusted_interp = CachedScript {
            path: safe_path.clone(),
            interpreter: "/usr/local/bin/custom_sh".into(),
            sha256: script_hash(&safe_path).unwrap(),
            source: "builtin:test".into(),
        };
        assert!(untrusted_interp.verify().is_err());
    }
}
