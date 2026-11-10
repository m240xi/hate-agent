use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("credential store: {0}")]
    Store(String),
    #[error("oauth: {0}")]
    OAuth(String),
    #[error("not authenticated — run `hate auth login`")]
    NeedsLogin,
    #[error("config: {0}")]
    Config(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: String,
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub default_provider: Option<String>,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    #[serde(default = "default_true")]
    pub auto_skills: bool,
    #[serde(default = "default_true")]
    pub discord_presence: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_provider: None,
            providers: BTreeMap::new(),
            workspace_path: None,
            auto_skills: true,
            discord_presence: true,
        }
    }
}

fn default_true() -> bool {
    true
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("hate-agent").join("config.toml"))
    }

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

    pub fn ensure_default_provider(&mut self) {
        const LEGACY_DEFAULT_MODEL: &str = "hermes-4-405b";
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub inference_base_url: Option<String>,
    #[serde(default)]
    pub agent_key: Option<String>,
    #[serde(default)]
    pub agent_key_expires_at: Option<u64>,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialBlob {
    ApiKey { key: String },
    OAuth { tokens: OAuthTokens },
}

impl CredentialBlob {
    pub fn is_oauth(&self) -> bool {
        matches!(self, Self::OAuth { .. })
    }
}

pub trait AuthStore: Send + Sync {
    fn get(&self, provider: &str) -> Result<Option<CredentialBlob>, AuthError>;
    fn set(&self, provider: &str, blob: &CredentialBlob) -> Result<(), AuthError>;
    fn delete(&self, provider: &str) -> Result<(), AuthError>;
}

#[derive(Default)]
pub struct MemoryStore {
    inner: std::sync::Mutex<BTreeMap<String, CredentialBlob>>,
}

impl MemoryStore {
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
