use std::time::Instant;

use crate::types::{FailureReason, Outcome, PermissionRequest, Plan, ToolCall, TurnId};

#[derive(Debug)]
pub enum SessionState {
    Idle,
    Planning { plan: Option<Plan> },
    AwaitingModel,
    ExecutingTool { call: ToolCall, started: Instant },
    Verifying { attempt: u8 },
    AwaitingPermission { request: PermissionRequest },
    AwaitingUser,
    Compacted { at_turn: TurnId },
    Done { outcome: Outcome },
    Failed { reason: FailureReason },
    Cancelled,
}

impl SessionState {
    pub fn accepts_input(&self) -> bool {
        matches!(
            self,
            Self::Idle | Self::Done { .. } | Self::Compacted { .. }
        )
    }
}
