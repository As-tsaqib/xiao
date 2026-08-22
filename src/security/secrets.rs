use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct SecretStore {
    root: PathBuf,
}
impl SecretStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    fn path(&self, key: &str) -> PathBuf {
        self.root.join(format!("{key}.secret"))
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
