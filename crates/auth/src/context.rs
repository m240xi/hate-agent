use std::sync::Arc;

use hate_core::provider::CredentialSource;

use crate::claude::{ClaudeAuth, ClaudeCredentialSource};
use crate::nous::{NousCredentialSource, PortalAuth};
use crate::source::{ApiKeySource, EnvKeySource};
use crate::store::{AuthError, AuthStore, Config, CredentialBlob, ProviderConfig};

#[derive(Debug, Clone)]
pub struct CredStatus {
    pub kind: &'static str,
    pub expires_at: Option<u64>,
}

pub struct AuthContext {
    pub store: Arc<dyn AuthStore>,
    pub config: Config,
}

impl AuthContext {
    pub fn load() -> Result<Self, AuthError> {
        Ok(Self {
            store: Arc::new(crate::CompositeStore::new()?),
            config: Config::load()?,
        })
    }

    pub fn with_store(store: Arc<dyn AuthStore>, config: Config) -> Self {
        Self { store, config }
    }

    pub fn resolve(&self, name: Option<&str>) -> Option<(&str, &ProviderConfig)> {
        self.config.resolve_provider(name)
    }

    pub fn credential(&self, name: &str) -> Result<Arc<dyn CredentialSource>, AuthError> {
        match self.store.get(name)? {
            Some(CredentialBlob::ApiKey { key }) => {
                return Ok(Arc::new(ApiKeySource::new(key)));
            }
            Some(CredentialBlob::OAuth { .. }) => match name {
                "nous" => {
                    let auth = Arc::new(PortalAuth::new(Arc::clone(&self.store)));
                    return Ok(Arc::new(NousCredentialSource::new(auth)));
                }
                "claude" | "anthropic" => {
                    let auth = Arc::new(ClaudeAuth::for_provider(Arc::clone(&self.store), name));
                    return Ok(Arc::new(ClaudeCredentialSource::new(auth)));
                }
                _ => {
                    return Err(AuthError::Config(format!(
                        "provider {name} has OAuth credentials but no OAuth flow is implemented for it"
                    )));
                }
            },
            None => {}
        }

        if name == "nous" && !std::env::var("NOUS_API_KEY").is_ok_and(|v| !v.trim().is_empty()) {
            return Err(AuthError::NeedsLogin);
        }

        let vars = env_vars_for(name);
        if vars.is_empty() {
            Ok(Arc::new(ApiKeySource::new("unused")))
        } else {
            Ok(Arc::new(EnvKeySource::new(vars)))
        }
    }

    pub fn set_api_key(&self, name: &str, key: &str) -> Result<(), AuthError> {
        self.store.set(
            name,
            &CredentialBlob::ApiKey {
                key: key.to_string(),
            },
        )
    }

    pub fn logout(&self, name: &str) -> Result<(), AuthError> {
        self.store.delete(name)
    }

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

fn env_vars_for(name: &str) -> &'static [&'static str] {
    match name {
        "openai" => &["OPENAI_API_KEY"],
        "anthropic" | "claude" => &["ANTHROPIC_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "gemini" | "google" => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        "xai" | "grok" => &["XAI_API_KEY"],
        "groq" => &["GROQ_API_KEY"],
        "mistral" => &["MISTRAL_API_KEY"],
        "nous" => &["NOUS_API_KEY"],
        _ => &[],
    }
}
