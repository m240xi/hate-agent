//! The provider contract: one `Delta` stream regardless of vendor.
//!
//! This module owns the *vocabulary* of model I/O — requests, messages,
//! deltas, capabilities — plus the `Provider` trait itself. Adapters live
//! in `hate-providers`; the session loop in this crate only ever sees
//! these types, so swapping vendors is a config change, not a code change.

use std::pin::Pin;

use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::event::AgentError;
use crate::types::{FailureReason, TokenUsage, ToolCall, ToolName};

/// Boxed future shorthand for object-safe async trait methods.
pub type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Why a provider call failed. Every variant carries enough detail to act
/// on — spec §28.14 forbids silently swallowing provider errors.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ProviderError {
    /// HTTP error status with a trimmed body snippet.
    #[error("provider returned HTTP {status}: {message}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// First ~200 chars of the response body.
        message: String,
        /// Server-requested delay from a `Retry-After` header, when present.
        retry_after: Option<std::time::Duration>,
    },
    /// Connection-level failure (DNS, TLS, reset) before/during streaming.
    #[error("network error: {0}")]
    Network(String),
    /// The stream broke mid-response or produced undecodable frames.
    #[error("stream error: {0}")]
    Stream(String),
    /// Response body could not be decoded into the expected shape.
    #[error("decode error: {0}")]
    Decode(String),
    /// Credentials were rejected (401/403) or are absent.
    #[error("auth error: {0}")]
    Auth(String),
    /// The caller's [`CancellationToken`] fired.
    #[error("cancelled")]
    Cancelled,
}

impl From<ProviderError> for AgentError {
    fn from(e: ProviderError) -> Self {
        match e {
            ProviderError::Auth(m) => AgentError::Auth(m),
            ProviderError::Cancelled => AgentError::Cancelled,
            other => AgentError::Provider(other.to_string()),
        }
    }
}

impl From<ProviderError> for FailureReason {
    fn from(e: ProviderError) -> Self {
        // `Display` already carries the "auth error:"/"HTTP 500:" prefix,
        // so the message stays actionable without a parallel taxonomy.
        Self::Provider {
            message: e.to_string(),
        }
    }
}

/// One normalized streaming item from any provider.
#[derive(Debug, Clone)]
pub enum Delta {
    /// Assistant-visible text fragment.
    Text(String),
    /// Reasoning/thinking fragment (rendered dimmed, collapsed in UI).
    Thinking(String),
    /// A complete tool call — adapters accumulate fragments internally so
    /// the session only ever sees parsed, ready-to-run calls.
    ToolCall(ToolCall),
    /// Token accounting for the request (usually arrives once, at the end).
    Usage(TokenUsage),
    /// The provider signalled end of response.
    Done,
}

/// A chat message in provider-neutral form. System prompt is *not* a
/// message — it travels in [`ChatRequest::system`] so each adapter can
/// place it where its vendor expects (`OpenAI`: leading system message;
/// Anthropic: top-level `system` field, which keeps it out of the
/// message array and improves cache prefix stability).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    /// End-user text.
    User {
        /// Message text.
        content: String,
    },
    /// Assistant text plus any tool calls it emitted.
    Assistant {
        /// Visible response text (may be empty when only tool calls ran).
        content: String,
        /// Tool calls the assistant requested.
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
    },
    /// Result of a completed tool call, fed back to the model.
    Tool {
        /// Correlates to the originating [`ToolCall::id`].
        call_id: String,
        /// Tool name (some vendors require it alongside the id).
        name: String,
        /// Shaped tool output text.
        content: String,
    },
}

/// A tool schema advertised to the model. Serialized per-vendor by the
/// adapter; the session never sees wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Registered tool name.
    pub name: ToolName,
    /// One-line description (≤120 chars per spec §10).
    pub description: String,
    /// JSON Schema for the arguments object.
    pub input_schema: serde_json::Value,
}

/// Thinking/reasoning effort hint. Vendors map this onto their own
/// controls (Anthropic `thinking.budget_tokens`, `OpenAI` `reasoning_effort`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThinkingLevel {
    /// No extended reasoning.
    #[default]
    Off,
    /// Light reasoning.
    Low,
    /// Full reasoning budget.
    High,
}

/// Everything a provider needs for one streamed request.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// Model identifier as the provider knows it (`hermes-4-405b`,
    /// `claude-sonnet-4-5`, …). Routing is by capability, not vendor.
    pub model: String,
    /// S0 system prompt — stable, byte-identical across requests.
    pub system: Option<String>,
    /// Ordered conversation (user/assistant/tool turns only).
    pub messages: Vec<Message>,
    /// Tool schemas to advertise (empty in M1).
    pub tools: Vec<ToolSpec>,
    /// Upper bound on generated tokens.
    pub max_output_tokens: u32,
    /// Sampling temperature; `None` = provider default.
    pub temperature: Option<f32>,
    /// Reasoning effort hint.
    pub thinking: ThinkingLevel,
}

/// What a provider/model can do — used for routing and UI badges, never
/// for correctness-critical branching.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    /// Accepts image/document content parts.
    pub vision: bool,
    /// Supports tool calling.
    pub tools: bool,
    /// Prompt-cache control style.
    pub cache: CacheMode,
    /// Supports extended thinking/reasoning output.
    pub thinking: bool,
}

/// How a provider handles prompt caching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// No prompt caching — prefix stability still helps latency.
    None,
    /// Automatic prefix caching (`OpenAI`, `Gemini`, most compat endpoints).
    Automatic,
    /// Explicit cache breakpoints required (Anthropic `cache_control`).
    Explicit,
}

/// A model listing entry from `Provider::list_models`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Provider-side model id.
    pub id: String,
    /// Human-readable name when the provider supplies one.
    pub name: Option<String>,
}

/// Where a provider gets its bearer credential. Implemented by `hate-auth`
/// (keyring, OAuth refresh, env fallback); the provider calls [`bearer`]
/// once per request so short-lived tokens (Nous Portal) stay fresh without
/// the adapter knowing anything about OAuth.
///
/// [`bearer`]: CredentialSource::bearer
pub trait CredentialSource: Send + Sync {
    /// A currently-valid bearer token (access token or API key).
    fn bearer(&self) -> BoxFuture<'_, Result<String, ProviderError>>;
}

/// The one trait every adapter implements. Object-safe: the session holds
/// `Arc<dyn Provider>` and never knows the vendor.
///
/// Cancellation is *not* a method — callers pass a
/// [`CancellationToken`] into [`Provider::stream`], which must abort the
/// in-flight HTTP stream within 250ms (spec §8, §27).
pub trait Provider: Send + Sync {
    /// Stream one chat completion as normalized [`Delta`]s.
    ///
    /// The returned stream ends with [`Delta::Done`] on success or a
    /// single [`ProviderError`] on failure. Implementations must honor
    /// `cancel` promptly even mid-body.
    fn stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> BoxStream<'static, Result<Delta, ProviderError>>;

    /// Count the input tokens `req` will occupy, using the provider's real
    /// counter where one exists (Anthropic `count_tokens` endpoint) or the
    /// sanctioned BPE approximation (o200k for OpenAI-compatible/local).
    /// Never `chars / 4`.
    fn count_tokens<'a>(
        &'a self,
        req: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<u64, ProviderError>>;

    /// List models the credential can access.
    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>>;

    /// Static capability report for this provider+model pairing.
    fn capabilities(&self) -> Capabilities;
}
