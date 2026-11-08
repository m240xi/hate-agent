//! [`AuthContext`]: config + credential store together — the one object
//! CLI and app code use to answer "who am I calling as?".

use std::sync::Arc;

use hate_core::provider::CredentialSource;

use crate::nous::{NousCredentialSource, PortalAuth};
use crate::source::{ApiKeySource, EnvKeySource};
use crate::store::{AuthError, AuthStore, Config, CredentialBlob, ProviderConfig};

/// Credential state for `hate auth status`.
#[derive(Debug, Clone)]
pub struct CredStatus {
    /// `none` | `api_key` | `oauth`
    pub kind: &'static str,
    /// Access-token expiry for OAuth, unix seconds.
    pub expires_at: Option<u64>,
}

/// Loaded auth world: where secrets live + what providers are configured.
pub struct AuthContext {
    /// Secret store (keychain in production, memory in tests).
    pub store: Arc<dyn AuthStore>,
    /// Non-secret config (`config.toml`).
    pub config: Config,
}

impl AuthContext {
    /// Production context: composite secret store + user config file.
    ///
    /// # Errors
    /// [`AuthError::Config`] on malformed config,
    /// [`AuthError::Store`] when the secrets dir is unresolvable.
    pub fn load() -> Result<Self, AuthError> {
        Ok(Self {
            store: Arc::new(crate::CompositeStore::new()?),
            config: Config::load()?,
        })
    }

    /// Test/custom context.
    pub fn with_store(store: Arc<dyn AuthStore>, config: Config) -> Self {
        Self { store, config }
    }

    /// Resolve `(name, ProviderConfig)` for `name` or the configured
    /// default — same rule the session factory uses.
    pub fn resolve(&self, name: Option<&str>) -> Option<(&str, &ProviderConfig)> {
        self.config.resolve_provider(name)
    }

    /// Credential source for provider `name`.
    ///
    /// Order: keychain blob (API key or OAuth) → conventional env var →
    /// no-auth for local providers (Ollama & friends ignore bearer).
    ///
    /// # Errors
    /// [`AuthError::Store`] on keychain failure; [`AuthError::Config`]
    /// never — missing credentials surface at request time via the
    /// source's own error so `hate run` can still print a useful message.
    pub fn credential(&self, name: &str) -> Result<Arc<dyn CredentialSource>, AuthError> {
        match self.store.get(name)? {
            Some(CredentialBlob::ApiKey { key }) => {
                return Ok(Arc::new(ApiKeySource::new(key)));
            }
            Some(CredentialBlob::OAuth { .. }) => {
                if name == "nous" {
                    let auth = Arc::new(PortalAuth::new(Arc::clone(&self.store)));
                    return Ok(Arc::new(NousCredentialSource::new(auth)));
                }
                return Err(AuthError::Config(format!(
                    "provider {name} has OAuth credentials but no OAuth flow is implemented for it"
                )));
            }
            None => {}
        }

        // Nous with no stored credential and no env key should say
        // "log in", not "no API key found" — the fix is a different
        // command entirely.
        if name == "nous" && !std::env::var("NOUS_API_KEY").is_ok_and(|v| !v.trim().is_empty()) {
            return Err(AuthError::NeedsLogin);
        }

        let vars = env_vars_for(name);
        if vars.is_empty() {
            // Local servers (ollama, lmstudio, vllm…) don't check auth;
            // a constant bearer keeps adapters that require the header
            // happy without inventing a secret.
            Ok(Arc::new(ApiKeySource::new("unused")))
        } else {
            Ok(Arc::new(EnvKeySource::new(vars)))
        }
    }

    /// Store an API key for `name` in the keychain.
    ///
    /// # Errors
    /// [`AuthError::Store`] on keychain failure.
    pub fn set_api_key(&self, name: &str, key: &str) -> Result<(), AuthError> {
        self.store.set(
            name,
            &CredentialBlob::ApiKey {
                key: key.to_string(),
            },
        )
    }

    /// Remove all credentials for `name`.
    ///
    /// # Errors
    /// [`AuthError::Store`] on keychain failure.
    pub fn logout(&self, name: &str) -> Result<(), AuthError> {
        self.store.delete(name)
    }

    /// What credentials exist for `name` (never returns secrets).
    ///
    /// # Errors
    /// [`AuthError::Store`] on keychain failure.
    pub fn status(&self, name: &str) -> Result<CredStatus, AuthError> {
        Ok(match self.store.get(name)? {
            Some(CredentialBlob::ApiKey { .. }) => CredStatus {
                kind: "api_key",
                expires_at: None,
            },
            Some(CredentialBlob::OAuth { tokens }) => CredStatus {
                kind: "oauth",
                expires_at: tokens.expires_at,
            },
            None => CredStatus {
                kind: "none",
                expires_at: None,
            },
        })
    }

    /// Base URL override for OAuth providers whose endpoint the server
    /// chooses (Nous `inference_base_url`). `None` → use config value.
    ///
    /// # Errors
    /// [`AuthError::OAuth`] if the stored URL fails the host allowlist.
    pub fn base_url_override(&self, name: &str) -> Result<Option<String>, AuthError> {
        if name != "nous" {
            return Ok(None);
        }
        match self.store.get(name)? {
            Some(CredentialBlob::OAuth { .. }) => {
                let auth = PortalAuth::new(Arc::clone(&self.store));
                auth.inference_base_url().map(Some)
            }
            _ => Ok(None),
        }
    }
}

/// Conventional env vars per provider name, checked in order.
fn env_vars_for(name: &str) -> &'static [&'static str] {
    match name {
        "openai" => &["OPENAI_API_KEY"],
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "gemini" | "google" => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        "xai" | "grok" => &["XAI_API_KEY"],
        "nous" => &["NOUS_API_KEY"],
        _ => &[],
    }
}
