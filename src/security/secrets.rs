use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SecretStore {
    root: PathBuf,
}
impl SecretStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    fn path(&self, key: &str) -> PathBuf {
        let sanitized: String = key
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.root.join(format!("{sanitized}.secret"))
    }
    pub fn put(&self, key: &str, value: &str) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let path = self.path(key);
        let tmp = path.with_extension("secret.tmp");
        fs::write(&tmp, value.as_bytes()).with_context(|| format!("write {}", tmp.display()))?;
        set_0600(&tmp)?;
        fs::rename(&tmp, &path).with_context(|| format!("replace {}", path.display()))?;
        set_0600(&path)?;
        Ok(())
    }

    /// Store a value under a fresh immutable reference. Callers update their
    /// authoritative SQLite row to this returned ref in the same logical
    /// control-plane operation; old refs can then be garbage-collected after
    /// commit without overwriting a live secret in place.
    pub fn put_versioned(&self, namespace: &str, value: &str) -> Result<String> {
        let reference = format!("{namespace}:{}", Uuid::new_v4().simple());
        self.put(&reference, value)?;
        Ok(reference)
    }
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        let path = self.path(key);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(fs::read_to_string(path)?))
    }
    pub fn remove(&self, key: &str) -> Result<()> {
        let p = self.path(key);
        if p.exists() {
            fs::remove_file(p)?;
        }
        Ok(())
    }
    pub fn exists(&self, key: &str) -> Result<bool> {
        Ok(self.path(key).exists())
    }
    pub fn staged_key(key: &str) -> String {
        format!("{key}:staged")
    }
    pub fn put_staged(&self, key: &str, value: &str) -> Result<()> {
        self.put(&Self::staged_key(key), value)
    }
    pub fn commit_staged(&self, key: &str) -> Result<()> {
        let staged = self.path(&Self::staged_key(key));
        if staged.exists() {
            let dest = self.path(key);
            fs::create_dir_all(&self.root)?;
            fs::rename(&staged, &dest)
                .with_context(|| format!("commit staged {}", dest.display()))?;
            set_0600(&dest)?;
        }
        Ok(())
    }
    pub fn rollback_staged(&self, key: &str) -> Result<()> {
        let staged = self.path(&Self::staged_key(key));
        if staged.exists() {
            fs::remove_file(staged)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn set_0600(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut p = fs::metadata(path)?.permissions();
    p.set_mode(0o600);
    fs::set_permissions(path, p)?;
    Ok(())
}
#[cfg(not(unix))]
fn set_0600(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_secret_store_put_get_remove() {
        let dir = tempdir().unwrap();
        let store = SecretStore::new(dir.path());
        assert!(!store.exists("test_key").unwrap());
        store.put("test_key", "super_secret").unwrap();
        assert!(store.exists("test_key").unwrap());
        assert_eq!(
            store.get("test_key").unwrap(),
            Some("super_secret".to_string())
        );
        store.remove("test_key").unwrap();
        assert!(!store.exists("test_key").unwrap());
    }

    #[test]
    fn test_secret_store_path_sanitization() {
        let dir = tempdir().unwrap();
        let store = SecretStore::new(dir.path());
        store.put("../traversal/key", "val").unwrap();
        assert!(dir.path().join("___traversal_key.secret").exists());
        assert_eq!(
            store.get("../traversal/key").unwrap(),
            Some("val".to_string())
        );
    }
}
