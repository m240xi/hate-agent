//! The typed event stream every consumer (CLI, desktop UI, bench) shares.
//!
//! Events are the *only* way information leaves the agent core. The UI is a
//! pure consumer: it renders events and sends commands, never the reverse.

use serde::Serialize;

use crate::types::{
    CheckpointId, CompactionReason, FailureReason, Outcome, PermissionRequest, TokenUsage,
    ToolCall, TurnId, TurnOutcome,
};

/// Errors surfaced on the event stream. Serialized so the UI can render
/// them without a parallel error channel.
#[derive(Debug, Clone, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "message")]
pub enum AgentError {
    /// Provider request failed after retries.
    #[error("provider error: {0}")]
    Provider(String),
    /// Provider rejected our credentials.
    #[error("authentication failed: {0}")]
    Auth(String),
    /// A tool implementation returned an error.
    #[error("tool {tool} failed: {message}")]
    Tool {
        /// Tool name.
        tool: String,
        /// Error detail.
        message: String,
    },
    /// The session hit a configured hard limit.
    #[error("limit exceeded: {limit} = {value}")]
    LimitExceeded {
        /// Which limit.
        limit: String,
        /// Observed value.
        value: u64,
    },
    /// The user cancelled the turn or session.
    #[error("cancelled")]
    Cancelled,
    /// Anything else — always carries the underlying message (spec §28.14:
    /// provider errors are never silently swallowed).
    #[error("internal error: {0}")]
    Internal(String),
}

/// Everything the core emits. Serialized for Tauri IPC and CLI rendering.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Streamed assistant text delta.
    Delta {
        /// New text fragment.
        text: String,
    },
    /// Streamed reasoning/thinking delta (UI renders dimmed, collapsed).
    ThinkingDelta {
        /// New thinking fragment.
        text: String,
    },
    /// A tool call was requested by the model and accepted for execution.
    ToolStarted {
        /// The call.
        call: ToolCall,
    },
    /// Shaped/truncated preview of a tool result, for live UI display.
    ToolOutput {
        /// Correlates to `ToolCall::id`.
        call_id: String,
        /// Truncated preview text.
        preview: String,
        /// Tokens the preview cost.
        tokens: u32,
    },
    /// Tool execution finished; carries the savings-legibility counters.
    ToolFinished {
        /// Correlates to `ToolCall::id`.
        call_id: String,
        /// Whether the tool succeeded.
        ok: bool,
        /// Tokens the full untruncated output would have cost.
        full_tokens: u32,
        /// Tokens actually sent to the model after shaping.
        sent_tokens: u32,
    },
    /// Policy engine requires user approval before a tool call runs.
    PermissionRequested {
        /// The pending request; respond via the `RespondPermission` command.
        request: PermissionRequest,
    },
    /// Token usage for a completed model request.
    TokensCounted {
        /// Usage record.
        usage: TokenUsage,
        /// Turn this usage belongs to.
        turn: TurnId,
    },
    /// Context compaction began.
    CompactionStarted {
        /// Why it ran.
        reason: CompactionReason,
        /// Tokens before compaction.
        before: u64,
        /// Tokens after compaction (filled when finished; 0 while running).
        after: u64,
    },
    /// A filesystem checkpoint was created.
    CheckpointCreated {
        /// The checkpoint id.
        id: CheckpointId,
    },
    /// A learned skill was created or updated (auto-reflection or the
    /// `skill_save` tool).
    SkillSaved {
        /// Skill slug.
        name: String,
        /// Human title.
        title: String,
        /// Body size in bytes.
        bytes: u32,
    },
    /// The current turn finished.
    TurnComplete {
        /// Outcome details.
        outcome: TurnOutcome,
    },
    /// The session failed terminally.
    Failed {
        /// Why.
        reason: FailureReason,
    },
    /// Non-fatal error worth surfacing (e.g. retry notice, 429 reset time).
    Error {
        /// What happened.
        error: AgentError,
    },
    /// Assistant is waiting for a user answer (`ask_user` tool or plan mode).
    AwaitingUser {
        /// The question being asked.
        prompt: String,
    },
    /// Session reached a terminal successful state.
    Done {
        /// Final outcome.
        outcome: Outcome,
    },
}
