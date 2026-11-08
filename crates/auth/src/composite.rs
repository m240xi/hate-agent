//! [`CompositeStore`]: routes each blob to the backend that fits it.
//!
//! API keys are small → OS keychain (`keyring`). OAuth bundles are
//! JWTs that overflow Windows Credential Manager → per-user file store.
//! `set` clears the *other* backend so a provider never has ambiguous
//! credentials in two places.

use std::sync::Arc;

use crate::AuthError;
use crate::file_store::FileStore;
use crate::keychain::KeyringStore;
use crate::store::{AuthStore, CredentialBlob};

/// Keychain for small secrets, file store for large OAuth blobs.
pub struct CompositeStore {
    /// Backend for `ApiKey` blobs.
    small: Arc<dyn AuthStore>,
    /// Backend for `OAuth` blobs.
    large: Arc<dyn AuthStore>,
}

impl CompositeStore {
    /// Production store. The file half is mandatory — without a data
    /// dir there is nowhere safe to put OAuth blobs.
    ///
    /// # Errors
    /// [`AuthError::Store`] when the secrets directory can't be resolved.
    pub fn new() -> Result<Self, AuthError> {
        Ok(Self {
            small: Arc::new(KeyringStore::new()),
            large: Arc::new(FileStore::new()?),
        })
    }

    /// Test constructor with injected backends.
    #[cfg(test)]
    pub(crate) fn with_backends(small: Arc<dyn AuthStore>, large: Arc<dyn AuthStore>) -> Self {
        Self { small, large }
    }
}

impl AuthStore for CompositeStore {
    fn get(&self, provider: &str) -> Result<Option<CredentialBlob>, AuthError> {
        // OAuth blobs live only in the file store; API keys only in the
        // keychain — checking large first disambiguates without a marker.
        if let Some(blob) = self.large.get(provider)? {
            return Ok(Some(blob));
        }
        self.small.get(provider)
    }

    fn set(&self, provider: &str, blob: &CredentialBlob) -> Result<(), AuthError> {
        match blob {
            CredentialBlob::OAuth { .. } => {
                self.large.set(provider, blob)?;
                // Stale API-key entry would confuse `status`; remove it
                // so exactly one credential exists per provider.
                let _ = self.small.delete(provider);
            }
            CredentialBlob::ApiKey { .. } => {
                self.small.set(provider, blob)?;
                let _ = self.large.delete(provider);
            }
        }
        Ok(())
    }

    fn delete(&self, provider: &str) -> Result<(), AuthError> {
        self.large.delete(provider)?;
        self.small.delete(provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{MemoryStore, OAuthTokens};

    fn oauth_blob() -> CredentialBlob {
        CredentialBlob::OAuth {
            tokens: OAuthTokens {
                access_token: "a".repeat(4000), // bigger than keychain limit
                refresh_token: "r".repeat(500),
                expires_at: Some(1),
                inference_base_url: None,
                agent_key: Some("k".repeat(800)),
                agent_key_expires_at: None,
                scope: None,
            },
        }
    }

    #[test]
    fn oauth_blob_roundtrips_through_file_store() {
        let dir = std::env::temp_dir().join(format!("hate-test-{}", std::process::id()));
        let file = Arc::new(FileStore::at(dir.clone()));
        let store = CompositeStore::with_backends(Arc::new(MemoryStore::new()), file);

        store.set("nous", &oauth_blob()).expect("set");
        let got = store.get("nous").expect("get").expect("present");
        match got {
            CredentialBlob::OAuth { tokens } => {
                assert_eq!(tokens.access_token.len(), 4000);
                assert_eq!(tokens.refresh_token.len(), 500);
            }
            CredentialBlob::ApiKey { .. } => panic!("wrong blob kind"),
        }
        // File actually exists on disk (proves it didn't hit "keychain").
        assert!(dir.join("nous.json").exists());

        store.delete("nous").expect("delete");
        assert!(store.get("nous").expect("get").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn api_key_goes_to_small_store_and_clears_large() {
        let dir = std::env::temp_dir().join(format!("hate-test2-{}", std::process::id()));
        let file = Arc::new(FileStore::at(dir.clone()));
        let small = Arc::new(MemoryStore::new());
        let store = CompositeStore::with_backends(small.clone(), file);

        // OAuth first, then an API key for the same provider must evict it.
        store.set("p", &oauth_blob()).expect("set oauth");
        store
            .set(
                "p",
                &CredentialBlob::ApiKey {
                    key: "sk-test".to_string(),
                },
            )
            .expect("set key");
        match store.get("p").expect("get").expect("present") {
            CredentialBlob::ApiKey { key } => assert_eq!(key, "sk-test"),
            CredentialBlob::OAuth { .. } => panic!("api key should win after eviction"),
        }
        assert!(!dir.join("p.json").exists(), "oauth file evicted");
        let _ = small;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
