use std::sync::Arc;

use tracing::warn;

use hate_core::provider::Provider;

use crate::anthropic::AnthropicProvider;
use crate::openai_compat::OpenAiCompatProvider;
use crate::retry::RetryingProvider;
use crate::sse::SharedCredential;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAiCompat,
    Anthropic,
}

impl ProviderKind {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "anthropic" | "claude" => Self::Anthropic,
            _ => Self::OpenAiCompat,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::OpenAiCompat => "openai_compat",
            Self::Anthropic => "anthropic",
        }
    }
}

pub struct ProviderSpec {
    pub kind: ProviderKind,
    pub base_url: String,
    pub credential: SharedCredential,
}

pub fn build(spec: ProviderSpec) -> Arc<dyn Provider> {
    let inner: Arc<dyn Provider> = match spec.kind {
        ProviderKind::OpenAiCompat => {
            Arc::new(OpenAiCompatProvider::new(spec.base_url, spec.credential))
        }
        ProviderKind::Anthropic => Arc::new(AnthropicProvider::new(spec.base_url, spec.credential)),
    };
    Arc::new(RetryingProvider::new(inner))
}

pub fn warn_if_unknown(kind_str: &str) {
    if !matches!(
        kind_str.trim().to_ascii_lowercase().as_str(),
        "openai_compat" | "openai" | "anthropic" | "claude" | "nous"
    ) {
        warn!(
            provider = kind_str,
            "unknown provider kind; using openai-compatible adapter"
        );
    }
}
