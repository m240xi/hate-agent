//! Session persistence boundary. The actor owns live conversation state;
//! a [`SessionJournal`] is the sink it reports finished turns to, and the
//! source it restores from on spawn. Storage lives in `hate-app`
//! (SQLite); core only sees this trait so the CLI and tests can run
//! journal-free.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::provider::{Message, ThinkingLevel};
use crate::types::TokenUsage;

/// Everything needed to rebuild a session's *model-visible* context after
/// a restart: the provider message history plus the counters that drive
/// limits and the UI's session stats. UI chrome (transcript blocks,
/// drafts, pane state) is the frontend's concern and travels separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSession {
    /// App-level session id (uuid string) — distinct from the actor's
    /// ledger [`crate::SessionId`].
    pub key: String,
    /// `chat` | `code` — which surface owns this session.
    pub mode: String,
    /// Provider config name the session runs against.
    pub provider: String,
    /// Model id active at last save.
    pub model: String,
    /// Thinking level active at last save.
    pub thinking: ThinkingLevel,
    /// Workspace folder for code sessions.
    pub workspace: Option<String>,
    /// Derived from the first user message; "New session" while empty.
    pub title: String,
    /// Completed exchanges — the exact `Vec<Message>` sent to providers.
    pub messages: Vec<Message>,
    /// Turn counter (restored so `max_turns` still applies).
    pub turn: u32,
    /// Cumulative token usage across the session.
    pub usage: TokenUsage,
    /// Accumulated USD spend (priced ledger records only).
    pub cost_usd: f64,
}

/// List-row shape for the session browser — no message payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// App-level session id.
    pub id: String,
    /// `chat` | `code`.
    pub mode: String,
    /// Derived title.
    pub title: String,
    /// Provider config name.
    pub provider: String,
    /// Model id.
    pub model: String,
    /// Workspace folder for code sessions.
    pub workspace: Option<String>,
    /// Last save time, unix seconds.
    pub updated_at: u32,
}

/// Persistence wiring for one spawned session. `None` on [`crate::SessionConfig`]
/// means ephemeral (CLI one-shots, tests).
pub struct PersistCtx {
    /// Row key in the journal.
    pub key: String,
    /// `chat` | `code`.
    pub mode: String,
    /// Workspace captured at session creation.
    pub workspace: Option<String>,
    /// Storage sink.
    pub journal: Arc<dyn SessionJournal>,
    /// State to seed the actor with (messages already in the DB).
    pub restore: Option<PersistedSession>,
}

/// Storage contract. Implementations must be `Send + Sync` and cheap —
/// the actor calls [`SessionJournal::save`] synchronously at turn end.
pub trait SessionJournal: Send + Sync {
    /// Load a persisted session by key; `None` when absent/corrupt.
    fn load(&self, key: &str) -> Option<PersistedSession>;
    /// Upsert a snapshot. Called at every turn end and option change.
    fn save(&self, snap: &PersistedSession);
    /// Remove a session permanently.
    fn delete(&self, key: &str);
    /// List sessions, optionally filtered by mode, newest first.
    fn list(&self, mode: Option<&str>) -> Vec<SessionMeta>;
}
