#![deny(unsafe_code)]

mod checkpoint;
mod event;
pub mod index;
pub mod mcp;
pub mod persist;
pub mod provider;
mod session;
mod shape;
pub mod skills;
mod state;
mod tools;
mod types;

pub use checkpoint::{CheckpointBundle, restore_checkpoint, write_checkpoint};
pub use event::{AgentError, Event};
pub use index::{FileNode, NodeKind, SymKind, SymbolHit, WorkspaceIndex};
pub use mcp::{MCP_PREFIX, McpConfigFile, McpHub, McpServerConfig, McpServerStatus};
pub use persist::{PersistCtx, PersistedSession, SessionJournal, SessionMeta};
pub use provider::{
    BoxFuture, CacheMode, Capabilities, ChatRequest, CredentialSource, Delta, Message, ModelInfo,
    ModelMeta, Provider, ProviderError, ThinkingLevel, ToolSpec,
};
pub use session::{
    Command, Limits, PermissionHub, SessionConfig, SessionError, SessionHandle, SessionSnapshot,
    spawn_session,
};
pub use skills::{Skill, SkillStore};
pub use state::SessionState;
pub use tools::{ToolOutput, WorkspaceTools};
pub use types::{
    CheckpointId, CompactionReason, FailureReason, Outcome, PermissionDecision, PermissionRequest,
    Plan, RequestId, SessionId, Step, TokenUsage, ToolCall, ToolName, TurnId, TurnOutcome,
};
