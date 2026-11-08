//! hate-core: session state machine, turn assembly, and event types.
//!
//! This crate is the agent's brain and contains zero Tauri dependency. The
//! desktop app and the headless CLI are both thin adapters over the session
//! handle (lands in M1).

#![deny(unsafe_code)]

mod event;
mod state;
mod types;

pub use event::{AgentError, Event};
pub use state::SessionState;
pub use types::{
    CheckpointId, CompactionReason, FailureReason, Outcome, PermissionDecision, PermissionRequest,
    Plan, RequestId, SessionId, Step, TokenUsage, ToolCall, ToolName, TurnId, TurnOutcome,
};
