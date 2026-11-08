//! Provider construction: config → `Arc<dyn Provider>`.
//!
//! One entry point so the CLI and the desktop app can't drift apart on
//! which adapter a provider name maps to. Unknown kinds fall back to
//! OpenAI-compatible with a warning — that's the spec's "route by
//! capability, warn on unknown" rule (§14).

use std::sync::Arc;

use tracing::warn;

use hate_core::provider::Provider;

use crate::anthropic::AnthropicProvider;
use crate::openai_compat::OpenAiCompatProvider;
use crate::retry::RetryingProvider;
use crate::sse::SharedCredential;

/// Which adapter family a configured provider uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// Generic `OpenAI` `/chat/completions` dialect.
    OpenAiCompat,
    /// Anthropic `/v1/messages`.
    Anthropic,
}

impl ProviderKind {
    /// Parse a config string. Unknown → `OpenAiCompat` (the universal
    /// dialect) — callers should warn, which `build` does.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "anthropic" | "claude" => Self::Anthropic,
            _ => Self::OpenAiCompat,
        }
    }

    /// Stable name for ledger records.
    pub fn name(self) -> &'static str {
        match self {
            Self::OpenAiCompat => "openai_compat",
            Self::Anthropic => "anthropic",
        }
    }
}

/// Everything needed to construct a provider.
pub struct ProviderSpec {
    /// Adapter family.
    pub kind: ProviderKind,
    /// API base URL (`https://api.anthropic.com`,
    /// `https://inference-api.nousresearch.com/v1`, `http://localhost:11434/v1`, …).
    pub base_url: String,
    /// Credential source — resolved per request so OAuth tokens refresh.
    pub credential: SharedCredential,
}

/// Build a ready-to-stream provider. Every returned provider is wrapped
/// in [`RetryingProvider`] — retry policy is not optional (spec §14).
///
/// Panics never; construction is infallible because adapters defer all
/// I/O to request time.
pub fn build(spec: ProviderSpec) -> Arc<dyn Provider> {
    let inner: Arc<dyn Provider> = match spec.kind {
        ProviderKind::OpenAiCompat => {
            Arc::new(OpenAiCompatProvider::new(spec.base_url, spec.credential))
        }
        ProviderKind::Anthropic => Arc::new(AnthropicProvider::new(spec.base_url, spec.credential)),
    };
    Arc::new(RetryingProvider::new(inner))
}

/// Warn (once, at startup) when a config string didn't name a known kind.
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
