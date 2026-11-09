//! Non-secret config and the credential blob stored per provider.
//!
//! `config.toml` lives at `dirs::config_dir()/hate-agent/` and holds only
//! non-secret settings (provider kind, base URL, model). Secrets never
//! touch this file — they go through [`AuthStore`] into the OS keychain.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Everything that can go wrong in auth land.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// Keychain/secret-store failure.
    #[error("credential store: {0}")]
    Store(String),
    /// OAuth endpoint or protocol failure.
    #[error("oauth: {0}")]
    OAuth(String),
    /// The user must (re-)authenticate — no credential or refresh rejected.
    #[error("not authenticated — run `hate auth login`")]
    NeedsLogin,
    /// Config file problems.
    #[error("config: {0}")]
    Config(String),
}

/// One provider's non-secret settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Adapter family: `openai_compat` or `anthropic`.
    pub kind: String,
    /// API base URL.
    pub base_url: String,
    /// Default model id.
    pub model: String,
}

/// `~/.config/hate-agent/config.toml` (or platform equivalent).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Name of the provider used when none is specified.
    #[serde(default)]
    pub default_provider: Option<String>,
    /// Named provider configs.
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    /// Folder the code session operates in (picked in the UI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
}

impl Config {
    /// Path to the config file.
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("hate-agent").join("config.toml"))
    }

    /// Load config; a missing file yields [`Config::default`], a malformed
    /// one is an error (never silently reset user settings).
    ///
    /// # Errors
    /// [`AuthError::Config`] on read/parse failure.
    pub fn load() -> Result<Self, AuthError> {
        let Some(path) = Self::path() else {
            return Ok(Self::default());
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| AuthError::Config(format!("read {}: {e}", path.display())))?;
        toml::from_str(&text)
            .map_err(|e| AuthError::Config(format!("parse {}: {e}", path.display())))
    }

    /// Persist config (creates parent dirs; best-effort 0600 on unix).
    ///
    /// # Errors
    /// [`AuthError::Config`] on write failure.
    pub fn save(&self) -> Result<(), AuthError> {
        let Some(path) = Self::path() else {
            return Err(AuthError::Config("no config dir".to_string()));
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AuthError::Config(format!("mkdir {}: {e}", parent.display())))?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| AuthError::Config(format!("serialize: {e}")))?;
        std::fs::write(&path, text)
            .map_err(|e| AuthError::Config(format!("write {}: {e}", path.display())))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    /// Insert the built-in Nous provider when nothing is configured, so
    /// a fresh install has a working `hate run` target. Also repairs
    /// configs written before the Portal catalog change — the original
    /// `hermes-4-405b` default 404s on the current inference API.
    pub fn ensure_default_provider(&mut self) {
        /// Model shipped as the Nous default; the Portal retired it.
        const LEGACY_DEFAULT_MODEL: &str = "hermes-4-405b";
        /// Current default — verified against the live `/v1/models` catalog.
        const DEFAULT_MODEL: &str = "moonshotai/kimi-k3";

        if self.providers.is_empty() {
            self.providers.insert(
                "nous".to_string(),
                ProviderConfig {
                    kind: "openai_compat".to_string(),
                    base_url: crate::nous::DEFAULT_INFERENCE_URL.to_string(),
                    model: DEFAULT_MODEL.to_string(),
                },
            );
            self.default_provider = Some("nous".to_string());
        }
        if let Some(nous) = self.providers.get_mut("nous")
            && nous.model == LEGACY_DEFAULT_MODEL
        {
            nous.model = DEFAULT_MODEL.to_string();
        }
    }

    /// Resolve the provider to use: explicit name, else default, else the
    /// first configured one.
    pub fn resolve_provider(&self, name: Option<&str>) -> Option<(&str, &ProviderConfig)> {
        let key = name
            .map(str::to_string)
            .or_else(|| self.default_provider.clone())
            .or_else(|| self.providers.keys().next().cloned())?;
        self.providers
            .get_key_value(&key)
            .map(|(k, v)| (k.as_str(), v))
    }
}

/// OAuth token material as returned by the Portal token endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    /// Bearer access token (`inference:invoke` scoped for Nous).
    pub access_token: String,
    /// Single-use refresh token — MUST be persisted on every rotation.
    pub refresh_token: String,
    /// Access-token expiry, unix seconds. `None` = treat as long-lived.
    #[serde(default)]
    pub expires_at: Option<u64>,
    /// Portal-returned inference endpoint override (validated before use).
    #[serde(default)]
    pub inference_base_url: Option<String>,
    /// Short-lived inference agent key, when the Portal mints one.
    #[serde(default)]
    pub agent_key: Option<String>,
    /// Agent-key expiry, unix seconds.
    #[serde(default)]
    pub agent_key_expires_at: Option<u64>,
    /// Granted OAuth scope.
    #[serde(default)]
    pub scope: Option<String>,
}

/// A stored credential for one provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialBlob {
    /// Plain API key (`OpenAI`, Anthropic, `OpenRouter`, Ollama-none-needed…).
    ApiKey {
        /// The secret key.
        key: String,
    },
    /// OAuth tokens with refresh material.
    OAuth {
        /// Token bundle.
        tokens: OAuthTokens,
    },
}

impl CredentialBlob {
    /// Whether this blob carries OAuth material needing refresh logic.
    pub fn is_oauth(&self) -> bool {
        matches!(self, Self::OAuth { .. })
    }
}

/// Where secrets live. One implementation per platform backend; the
/// interface stays sync because keychain calls are fast and local.
pub trait AuthStore: Send + Sync {
    /// Read the blob for `provider`; `None` when absent.
    ///
    /// # Errors
    /// Store-level failure (not "missing" — that's `Ok(None)`).
    fn get(&self, provider: &str) -> Result<Option<CredentialBlob>, AuthError>;
    /// Write (or overwrite) the blob for `provider`.
    ///
    /// # Errors
    /// Store-level failure.
    fn set(&self, provider: &str, blob: &CredentialBlob) -> Result<(), AuthError>;
    /// Remove the blob for `provider`; missing is not an error.
    ///
    /// # Errors
    /// Store-level failure.
    fn delete(&self, provider: &str) -> Result<(), AuthError>;
}

/// In-memory store for tests and `--dry-run` paths.
#[derive(Default)]
pub struct MemoryStore {
    inner: std::sync::Mutex<BTreeMap<String, CredentialBlob>>,
}

impl MemoryStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl AuthStore for MemoryStore {
    fn get(&self, provider: &str) -> Result<Option<CredentialBlob>, AuthError> {
        Ok(self
            .inner
            .lock()
            .expect("memory store poisoned")
            .get(provider)
            .cloned())
    }

    fn set(&self, provider: &str, blob: &CredentialBlob) -> Result<(), AuthError> {
        self.inner
            .lock()
            .expect("memory store poisoned")
            .insert(provider.to_string(), blob.clone());
        Ok(())
    }

    fn delete(&self, provider: &str) -> Result<(), AuthError> {
        self.inner
            .lock()
            .expect("memory store poisoned")
            .remove(provider);
        Ok(())
    }
}
