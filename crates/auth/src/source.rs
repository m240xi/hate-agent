use std::sync::Arc;

use hate_core::provider::{BoxFuture, CredentialSource, ProviderError};

pub struct ApiKeySource {
    key: Arc<str>,
}

impl ApiKeySource {
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

pub struct EnvKeySource {
    vars: Vec<String>,
}

impl EnvKeySource {
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
