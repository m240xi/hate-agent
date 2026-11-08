//! Ready-made [`CredentialSource`]s: static API keys and env-var lookup.
//!
//! These cover every API-key provider (`OpenAI`, Anthropic, `OpenRouter`,
//! `DeepSeek`, Gemini-compat, Ollama-none). OAuth providers get their own
//! source in `nous.rs` — the trait is the same either way.

use std::sync::Arc;

use hate_core::provider::{BoxFuture, CredentialSource, ProviderError};

/// A fixed API key, resolved once at construction.
pub struct ApiKeySource {
    key: Arc<str>,
}

impl ApiKeySource {
    /// Wrap a key string.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: Arc::from(key.into()),
        }
    }
}

impl CredentialSource for ApiKeySource {
    fn bearer(&self) -> BoxFuture<'_, Result<String, ProviderError>> {
        let key = self.key.to_string();
        Box::pin(async move { Ok(key) })
    }
}

/// Env-var credential lookup, checked on every request so `export
/// OPENAI_API_KEY=…` mid-session works without restart. Vars are tried
/// in order; the first non-empty wins.
pub struct EnvKeySource {
    vars: Vec<String>,
}

impl EnvKeySource {
    /// Watch these env vars, in priority order.
    pub fn new(vars: &[&str]) -> Self {
        Self {
            vars: vars.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

impl CredentialSource for EnvKeySource {
    fn bearer(&self) -> BoxFuture<'_, Result<String, ProviderError>> {
        let vars = self.vars.clone();
        Box::pin(async move {
            for var in &vars {
                if let Ok(v) = std::env::var(var)
                    && !v.trim().is_empty()
                {
                    return Ok(v);
                }
            }
            Err(ProviderError::Auth(format!(
                "no API key found (checked: {})",
                vars.join(", ")
            )))
        })
    }
}
