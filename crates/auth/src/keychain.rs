//! OS keychain backend for [`AuthStore`] via the `keyring` crate.
//!
//! Service name is `hate-agent`; the keychain "user" is the provider
//! name (`nous`, `anthropic`, `openai`, …). Blobs are stored as JSON.

use keyring::Entry;

use crate::store::{AuthError, AuthStore, CredentialBlob};

/// Keychain-backed credential store.
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    /// Store under the `hate-agent` service name.
    pub fn new() -> Self {
        Self {
            service: "hate-agent".to_string(),
        }
    }

    fn entry(&self, provider: &str) -> Result<Entry, AuthError> {
        Entry::new(&self.service, provider).map_err(|e| AuthError::Store(e.to_string()))
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthStore for KeyringStore {
    fn get(&self, provider: &str) -> Result<Option<CredentialBlob>, AuthError> {
        match self.entry(provider)?.get_password() {
            Ok(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|e| AuthError::Store(format!("corrupt credential for {provider}: {e}"))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AuthError::Store(e.to_string())),
        }
    }

    fn set(&self, provider: &str, blob: &CredentialBlob) -> Result<(), AuthError> {
        let json =
            serde_json::to_string(blob).map_err(|e| AuthError::Store(format!("serialize: {e}")))?;
        self.entry(provider)?
            .set_password(&json)
            .map_err(|e| AuthError::Store(e.to_string()))
    }

    fn delete(&self, provider: &str) -> Result<(), AuthError> {
        match self.entry(provider)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(AuthError::Store(e.to_string())),
        }
    }
}
