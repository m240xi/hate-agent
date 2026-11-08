//! File-backed secret store for blobs too large for the OS keychain.
//!
//! Windows Credential Manager caps generic-credential blobs at ~2560
//! bytes; a Nous OAuth bundle (two JWTs + metadata) exceeds that, so
//! `auth login` literally cannot complete there. OAuth material
//! therefore lives in `%APPDATA%/hate-agent/secrets/<provider>.json` —
//! inside the per-user profile, same protection model as Hermes'
//! `~/.hermes/auth.json` and Claude Code's credentials file. Unix gets
//! an explicit 0600; Windows relies on the profile's user-only ACL.

use std::path::PathBuf;

use crate::store::{AuthError, AuthStore, CredentialBlob};

/// Directory holding per-provider secret files.
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    /// Store under `<data_dir>/hate-agent/secrets/`.
    ///
    /// # Errors
    /// [`AuthError::Store`] when no data dir exists.
    pub fn new() -> Result<Self, AuthError> {
        let dir = dirs::data_dir()
            .ok_or_else(|| AuthError::Store("no data dir".to_string()))?
            .join("hate-agent")
            .join("secrets");
        Ok(Self { dir })
    }

    /// Store rooted at an explicit directory (tests).
    #[cfg(test)]
    pub(crate) fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, provider: &str) -> PathBuf {
        // Provider names are config keys; keep filenames safe regardless.
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
        // Atomic-ish replace: rename over the old file where the OS
        // allows; fall back to remove-then-rename on Windows.
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
