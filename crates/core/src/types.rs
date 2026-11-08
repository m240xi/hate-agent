//! Identifier and value types shared across the agent core.
//!
//! These are the vocabulary every other crate speaks: session/turn ids,
//! tool calls, token usage, plans, and outcomes. They are pure data —
//! no behavior lives here.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for an agent session (one workspace + one conversation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(#[serde(with = "uuid::serde::simple")] pub Uuid);

impl SessionId {
    /// Mint a fresh session id.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

/// Monotonic turn counter within a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TurnId(pub u32);

impl TurnId {
    /// The first turn of a session.
    pub const FIRST: Self = Self(1);

    /// Advance to the next turn.
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// Identifier for a filesystem checkpoint snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CheckpointId(pub String);

/// Identifier for a single model request within a turn (used to correlate
/// usage records when a turn is retried).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(pub String);

/// A tool's registered name. Newtype so tool names can't be confused with
/// arbitrary strings at call sites.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolName(pub String);

impl ToolName {
    /// Construct from a static name.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A single tool invocation requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned call id, echoed back in `tool_result`.
    pub id: String,
    /// Which tool to run.
    pub name: ToolName,
    /// Parsed arguments object.
    pub args: serde_json::Value,
}

/// Token accounting for one model request or one tool result.
///
/// `full_tokens` vs `sent_tokens` on tool output is what makes the token
/// savings legible — both are recorded, never just the sent count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Total input tokens including cached reads.
    pub input: u64,
    /// Portion of `input` served from provider prompt cache.
    pub cached_input: u64,
    /// Output tokens (including thinking where the provider bills them).
    pub output: u64,
    /// Reasoning/thinking tokens, when reported separately.
    pub thinking: u64,
}

impl TokenUsage {
    /// Total billable-ish token count for display purposes.
    pub fn total(&self) -> u64 {
        self.input + self.output
    }
}

/// Why a compaction ran — surfaced in the UI so the user can tell an
/// intentional shrink from a bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactionReason {
    /// Context crossed the soft threshold (0.72 of window).
    Threshold,
    /// Context crossed the hard stop (0.92 of window).
    HardStop,
    /// User requested compaction explicitly.
    Manual,
    /// Model switch forced re-evaluation.
    ModelSwitch,
}

/// The structured plan a model must emit before `exit_plan` in plan mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// Ordered steps the agent intends to take.
    pub steps: Vec<Step>,
}

/// One step in a [`Plan`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    /// What this step does, in imperative voice.
    pub description: String,
    /// Tool names this step is expected to use (advisory, not enforced).
    #[serde(default)]
    pub tools: Vec<ToolName>,
}

/// How a task ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// Task completed and verified.
    Completed,
    /// Task ended because the user cancelled.
    Cancelled,
    /// Task ended awaiting user input that never resolved into completion.
    AwaitingUser,
}

/// Why a session or task failed — always carries enough detail to act on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureReason {
    /// A configured hard limit was hit.
    LimitExceeded {
        /// Which limit (e.g. `max_turns`, `max_wall_clock`).
        limit: String,
        /// Observed value when the limit tripped.
        value: u64,
    },
    /// The provider returned an unrecoverable error.
    Provider {
        /// Provider error detail, already redacted.
        message: String,
    },
    /// A tool failed in a way the loop could not recover from.
    Tool {
        /// Tool name.
        name: String,
        /// Error detail.
        message: String,
    },
    /// The user rejected a permission prompt with "deny".
    PermissionDenied {
        /// Tool that was denied.
        tool: String,
    },
    /// Internal invariant violation.
    Internal {
        /// What invariant broke.
        message: String,
    },
}

/// Result of a finished turn, emitted on `Event::TurnComplete`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnOutcome {
    /// How the turn ended.
    pub outcome: Outcome,
    /// Token usage for the whole turn.
    pub usage: TokenUsage,
    /// Number of tool calls executed during the turn.
    pub tool_calls: u32,
    /// Wall-clock duration of the turn.
    pub elapsed_ms: u64,
    /// Cost in USD computed from the price table, when known.
    pub cost_usd: Option<f64>,
}

/// A permission decision requested by the policy engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    /// Correlates the request with the tool call that triggered it.
    pub id: String,
    /// The tool call awaiting approval.
    pub call: ToolCall,
    /// Human-readable reason the policy engine gated this call
    /// (e.g. "destructive command", "network access").
    pub reason: String,
    /// For `bash`, the classified command and its bucket, for display.
    #[serde(default)]
    pub detail: Option<String>,
}

/// User's answer to a [`PermissionRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionDecision {
    /// Run once.
    Allow,
    /// Run and remember for this session (session-scoped allowlist).
    AllowSession,
    /// Deny; the tool call fails and the model sees the denial.
    Deny,
}

impl PermissionDecision {
    /// Whether the call may execute at all.
    pub fn is_allowed(self) -> bool {
        matches!(self, Self::Allow | Self::AllowSession)
    }
}
