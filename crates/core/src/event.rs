use serde::Serialize;

use crate::types::{
    CheckpointId, CompactionReason, FailureReason, Outcome, PermissionRequest, TokenUsage,
    ToolCall, TurnId, TurnOutcome,
};

#[derive(Debug, Clone, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "message")]
pub enum AgentError {
    #[error("provider error: {0}")]
    Provider(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("tool {tool} failed: {message}")]
    Tool { tool: String, message: String },
    #[error("limit exceeded: {limit} = {value}")]
    LimitExceeded { limit: String, value: u64 },
    #[error("cancelled")]
    Cancelled,
    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Delta {
        text: String,
    },
    ThinkingDelta {
        text: String,
    },
    ToolStarted {
        call: ToolCall,
    },
    ToolOutput {
        call_id: String,
        preview: String,
        tokens: u32,
    },
    ToolFinished {
        call_id: String,
        ok: bool,
        full_tokens: u32,
        sent_tokens: u32,
    },
    PermissionRequested {
        request: PermissionRequest,
    },
    TokensCounted {
        usage: TokenUsage,
        turn: TurnId,
        thinking_est: u32,
    },
    CompactionStarted {
        reason: CompactionReason,
        before: u64,
        after: u64,
    },
    CheckpointCreated {
        id: CheckpointId,
    },
    SkillSaved {
        name: String,
        title: String,
        bytes: u32,
    },
    TurnComplete {
        outcome: TurnOutcome,
    },
    Failed {
        reason: FailureReason,
    },
    Error {
        error: AgentError,
    },
    AwaitingUser {
        prompt: String,
    },
    Done {
        outcome: Outcome,
    },
}
