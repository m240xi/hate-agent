//! hate-core: session state machine, turn assembly, and event types.
//!
//! This crate is the agent's brain and contains zero Tauri dependency. The
//! desktop app and the headless CLI are both thin adapters over the session
//! handle (lands in M1).

#![deny(unsafe_code)]

mod event;
pub mod provider;
mod session;
mod state;
mod types;

pub use event::{AgentError, Event};
pub use provider::{
    BoxFuture, CacheMode, Capabilities, ChatRequest, CredentialSource, Delta, Message, ModelInfo,
    ModelMeta, Provider, ProviderError, ThinkingLevel, ToolSpec,
};
pub use session::{
    Command, Limits, SessionConfig, SessionError, SessionHandle, SessionSnapshot, spawn_session,
};
pub use state::SessionState;
pub use types::{
    CheckpointId, CompactionReason, FailureReason, Outcome, PermissionDecision, PermissionRequest,
    Plan, RequestId, SessionId, Step, TokenUsage, ToolCall, ToolName, TurnId, TurnOutcome,
};
