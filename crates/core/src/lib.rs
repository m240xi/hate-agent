#![deny(unsafe_code)]

mod checkpoint;
mod event;
pub mod index;
pub mod mcp;
pub mod persist;
pub mod policy;
pub mod process_util;
pub mod provider;
mod session;
mod shape;
pub mod skills;
mod state;
pub mod subagent;
pub mod terminal_bridge;
pub mod tool_search;
mod tools;
mod types;
pub mod verify;
pub mod view;
pub mod web_fetch;

pub use checkpoint::{
    CheckpointBundle, checkpoint_diff, evict_old_checkpoints, restore_checkpoint, write_checkpoint,
};
pub use event::{AgentError, Event};
pub use index::{
    FileNode, NodeKind, SymKind, SymbolHit, WorkspaceIndex, build_repo_map,
    collect_workspace_symbols_all,
};
pub use mcp::{MCP_PREFIX, McpConfigFile, McpHub, McpServerConfig, McpServerStatus};
pub use persist::{ExternalRef, PersistCtx, PersistedSession, SessionJournal, SessionMeta};
pub use policy::{
    CommandClass, CommandClassification, RepoTrustReport, SecretRedactor, TrustStore,
    classify_bash, inspect_repo_trust, redact_secrets,
};
pub use provider::{
    BoxFuture, CacheMode, Capabilities, ChatRequest, CredentialSource, Delta, ImagePart, Message,
    ModelInfo, ModelMeta, Provider, ProviderError, ThinkingLevel, ToolSpec,
};
pub use session::{
    Command, ExternalSession, Limits, PermissionHub, SessionConfig, SessionError, SessionHandle,
    SessionSnapshot, spawn_session,
};
pub use skills::{Skill, SkillStore};
pub use state::SessionState;
pub use terminal_bridge::{TerminalBridge, TerminalTabSummary, TerminalToolkit};
pub use tools::{ToolOutput, WorkspaceRule, WorkspaceTools, inspect_instructions};
pub use types::{
    CheckpointId, CompactionReason, FailureReason, Outcome, PermissionDecision, PermissionRequest,
    Plan, RequestId, SessionId, Step, TokenUsage, ToolCall, ToolName, TurnId, TurnOutcome,
};
pub use view::{ChangedFile, FileDiff, FileView, MergeBackSummary, WorktreeStatus};
pub use web_fetch::fetch_url_text;
