//! Session state machine — explicit states, explicit transitions only.

use std::time::Instant;

use crate::types::{FailureReason, Outcome, PermissionRequest, Plan, ToolCall, TurnId};

/// Coarse session state. Transitions happen only through the agent loop —
/// never implicitly.
#[derive(Debug)]
pub enum SessionState {
    /// Nothing in flight; ready for a command.
    Idle,
    /// Plan mode: read-only tools until a [`Plan`] is emitted and approved.
    Planning {
        /// The plan under construction (filled on `exit_plan`).
        plan: Option<Plan>,
    },
    /// A model request is in flight.
    AwaitingModel,
    /// A tool call is executing.
    ExecutingTool {
        /// The call being run.
        call: ToolCall,
        /// When execution started (for stall detection).
        started: Instant,
    },
    /// The verifier is re-running checks after a claimed completion.
    Verifying {
        /// Which verification attempt (1-based).
        attempt: u8,
    },
    /// Blocked on a permission decision.
    AwaitingPermission {
        /// The pending request.
        request: PermissionRequest,
    },
    /// Blocked on a user answer.
    AwaitingUser,
    /// History was compacted at the given turn.
    Compacted {
        /// Turn the compaction happened on.
        at_turn: TurnId,
    },
    /// Session finished successfully.
    Done {
        /// How it ended.
        outcome: Outcome,
    },
    /// Session failed terminally.
    Failed {
        /// Why.
        reason: FailureReason,
    },
    /// Session was cancelled.
    Cancelled,
}

impl SessionState {
    /// Whether the session can accept a new user message right now.
    pub fn accepts_input(&self) -> bool {
        matches!(
            self,
            Self::Idle | Self::Done { .. } | Self::Compacted { .. }
        )
    }
}
