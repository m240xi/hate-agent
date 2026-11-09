use std::path::PathBuf;

use crate::store::{AuthError, AuthStore, CredentialBlob};

pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    pub fn new() -> Result<Self, AuthError> {
        let dir = dirs::data_dir()
            .ok_or_else(|| AuthError::Store("no data dir".to_string()))?
            .join("hate-agent")
            .join("secrets");
        Ok(Self { dir })
    }

    #[cfg(test)]
    pub(crate) fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, provider: &str) -> PathBuf {
        let safe: String = provider
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir.join(format!("{safe}.json"))
    }
}

impl AuthStore for FileStore {
    fn get(&self, provider: &str) -> Result<Option<CredentialBlob>, AuthError> {
        let path = self.path(provider);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| AuthError::Store(format!("read {}: {e}", path.display())))?;
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| AuthError::Store(format!("corrupt secret for {provider}: {e}")))
    }

    fn set(&self, provider: &str, blob: &CredentialBlob) -> Result<(), AuthError> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| AuthError::Store(format!("mkdir {}: {e}", self.dir.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700));
        }
        let path = self.path(provider);
        let tmp = path.with_extension("tmp");
        let text = serde_json::to_string_pretty(blob)
            .map_err(|e| AuthError::Store(format!("serialize: {e}")))?;
        std::fs::write(&tmp, text)
            .map_err(|e| AuthError::Store(format!("write {}: {e}", tmp.display())))?;
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&path);
            std::fs::rename(&tmp, &path)
                .map_err(|e| AuthError::Store(format!("rename {}: {e}", path.display())))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    fn delete(&self, provider: &str) -> Result<(), AuthError> {
        let path = self.path(provider);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AuthError::Store(format!("delete {}: {e}", path.display()))),
        }
    }
}
