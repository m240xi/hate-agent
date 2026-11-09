use std::pin::Pin;

use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::event::AgentError;
use crate::types::{FailureReason, TokenUsage, ToolCall, ToolName};

pub type BoxFuture<'a, T> = Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum ProviderError {
    #[error("provider returned HTTP {status}: {message}")]
    Http {
        status: u16,
        message: String,
        retry_after: Option<std::time::Duration>,
    },
    #[error("network error: {0}")]
    Network(String),
    #[error("stream error: {0}")]
    Stream(String),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("auth error: {0}")]
    Auth(String),
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
        Self::Provider {
            message: e.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Delta {
    Text(String),
    Thinking(String),
    ToolCall(ToolCall),
    Usage(TokenUsage),
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User {
        content: String,
    },
    Assistant {
        content: String,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        call_id: String,
        name: String,
        content: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: ToolName,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThinkingLevel {
    #[default]
    Off,
    Low,
    Medium,
    High,
}

impl ThinkingLevel {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "" => Ok(Self::Off),
            "low" | "minimal" => Ok(Self::Low),
            "medium" | "med" => Ok(Self::Medium),
            "high" | "max" | "xhigh" => Ok(Self::High),
            other => Err(format!("unknown thinking level {other:?}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_output_tokens: u32,
    pub temperature: Option<f32>,
    pub thinking: ThinkingLevel,
}

#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    pub vision: bool,
    pub tools: bool,
    pub cache: CacheMode,
    pub thinking: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    None,
    Automatic,
    Explicit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: Option<String>,
    #[serde(default)]
    pub meta: Option<ModelMeta>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelMeta {
    pub context_tokens: Option<u32>,
    pub price_in_mtok: Option<f64>,
    pub price_out_mtok: Option<f64>,
    pub price_cache_mtok: Option<f64>,
    #[serde(default)]
    pub reasoning_efforts: Vec<String>,
    pub default_effort: Option<String>,
    pub supports_tools: Option<bool>,
    pub supports_vision: Option<bool>,
}

pub trait CredentialSource: Send + Sync {
    fn bearer(&self) -> BoxFuture<'_, Result<String, ProviderError>>;

    fn is_oauth(&self) -> bool {
        false
    }
}

pub trait Provider: Send + Sync {
    fn stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> BoxStream<'static, Result<Delta, ProviderError>>;

    fn count_tokens<'a>(
        &'a self,
        req: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<u64, ProviderError>>;

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>>;

    fn capabilities(&self) -> Capabilities;
}
