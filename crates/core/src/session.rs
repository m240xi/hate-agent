//! The session actor: owns conversation state, runs turns, emits events.
//!
//! Architecture (spec §5/§8): one actor task per session. Commands arrive
//! on an mpsc channel; a user message spawns a *turn task* that streams
//! from the provider while the actor stays responsive to `Cancel`.
//! Cancellation is `CancellationToken`-based so an in-flight HTTP stream
//! dies within the 250ms budget — the actor never awaits the stream
//! inline.
//!
//! M2+: the turn task owns the tool-call loop — read tools run freely,
//! mutations and shell are policy-gated through [`PermissionHub`], which
//! routes user decisions from the actor's command channel back into the
//! waiting tool loop.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use crate::event::{AgentError, Event};
use crate::mcp::{MCP_PREFIX, McpHub};
use crate::persist::{PersistCtx, PersistedSession};
use crate::provider::{ChatRequest, Delta, Message, Provider, ProviderError, ThinkingLevel};
use crate::state::SessionState;
use crate::tools::{ToolOutput, WorkspaceTools};
use crate::types::{
    CheckpointId, CompactionReason, FailureReason, Outcome, PermissionDecision, PermissionRequest,
    SessionId, TokenUsage, ToolCall, TurnId, TurnOutcome,
};
use hate_context::O200kCounter;
use hate_ledger::{Ledger, RecordRequest};

/// Event channel capacity. Deltas can arrive at ~50/s; 2048 gives the
/// Tauri forwarding task ~40s of slack before a slow consumer drops events.
const EVENT_BUFFER: usize = 2048;

/// Hard limits for one task (spec §8). All configurable, sane defaults.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Max user-message turns per session before `Failed`.
    pub max_turns: u32,
    /// Max wall-clock for a whole task (first message → last reply).
    pub max_wall_clock: Duration,
    /// Max USD spend per task; `None` = unlimited.
    pub max_cost_usd: Option<f64>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_turns: 40,
            max_wall_clock: Duration::from_mins(20),
            max_cost_usd: None,
        }
    }
}

/// Everything needed to construct a session.
pub struct SessionConfig {
    /// The model provider (adapter chosen by the caller).
    pub provider: Arc<dyn Provider>,
    /// Model id passed to the provider.
    pub model: String,
    /// Provider adapter name for ledger records (`openai_compat`, …).
    pub provider_name: String,
    /// Reasoning effort applied to every turn until changed via
    /// [`Command::SetOptions`].
    pub thinking: ThinkingLevel,
    /// Hard limits.
    pub limits: Limits,
    /// Token/cost ledger; `None` disables accounting (tests).
    pub ledger: Option<Arc<Ledger>>,
    /// Session persistence; `None` = ephemeral (CLI one-shots, tests).
    pub persist: Option<PersistCtx>,
    /// Workspace tool surface (M2 read tools, M3 mutations + bash).
    /// `None` = no filesystem tools — chat sessions and sessions
    /// without a workspace stay workspace-tool-free.
    pub tools: Option<Arc<WorkspaceTools>>,
    /// MCP servers' tool surface. Independent of `tools` — MCP tools
    /// aren't workspace-bound, so chat sessions get them too when
    /// servers are configured. Live `configure()` reloads are visible
    /// from the next turn on.
    pub mcp: Option<Arc<McpHub>>,
    /// Permission decision router shared with the in-flight turn task.
    pub permissions: Arc<PermissionHub>,
    /// Model context window in tokens; `0` = unknown → compaction
    /// disabled (the provider errors honestly on overflow instead).
    pub context_window: u64,
}

/// Commands the outside world can send a session.
#[derive(Debug)]
pub enum Command {
    /// A new user message — starts a turn.
    UserMessage(String),
    /// Cancel the in-flight turn (no-op when idle).
    Cancel,
    /// Answer a pending `ask_user` prompt (M2+; accepted now, ignored).
    AnswerPrompt(String),
    /// Resolve a permission request (M3+; accepted now, ignored).
    RespondPermission {
        /// Correlates to the pending request id.
        request_id: String,
        /// Serialized decision.
        decision: String,
    },
    /// Change per-turn options mid-session: model id and/or thinking
    /// level. `None` fields keep their current value. Takes effect on
    /// the *next* turn — an in-flight turn is never mutated.
    SetOptions {
        /// New model id, when switching models.
        model: Option<String>,
        /// New thinking level, when changing effort.
        thinking: Option<ThinkingLevel>,
        /// New context window when the model's catalog metadata is
        /// known — keeps compaction thresholds honest after a switch.
        context_window: Option<u64>,
    },
    /// `/compact` — force a context compaction now. Only handled while
    /// idle; a turn in flight owns its convo clone and can't be touched.
    Compact,
}

/// Point-in-time session status for UI queries.
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    /// Current state-machine state (debug form).
    pub state: String,
    /// Current turn number.
    pub turn: u32,
    /// Accumulated usage across the session.
    pub usage: TokenUsage,
    /// Whether a turn is in flight.
    pub running: bool,
}

/// Errors from [`SessionHandle::command`].
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The actor task is gone (session shut down).
    #[error("session closed")]
    Closed,
    /// Command channel full — caller should retry or drop.
    #[error("session busy")]
    Busy,
}

/// Cloneable handle to a running session. Cheap to share across Tauri
/// commands and CLI code.
#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::Sender<Command>,
    events: broadcast::Sender<Event>,
    snapshot: Arc<std::sync::RwLock<SessionSnapshot>>,
    session_id: SessionId,
}

impl SessionHandle {
    /// Send a command to the actor. Never blocks: the channel is bounded
    /// and a full channel means the session is over capacity, not stuck.
    ///
    /// # Errors
    /// [`SessionError::Closed`] when the actor is gone,
    /// [`SessionError::Busy`] when the queue is full.
    pub fn command(&self, cmd: Command) -> Result<(), SessionError> {
        self.tx.try_send(cmd).map_err(|e| match e {
            mpsc::error::TrySendError::Closed(_) => SessionError::Closed,
            mpsc::error::TrySendError::Full(_) => SessionError::Busy,
        })
    }

    /// Subscribe to the session's event stream. Each subscriber gets every
    /// event from subscription time onward; a lagged subscriber misses
    /// events (the Tauri forwarder keeps up easily at this volume).
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Current status snapshot.
    pub fn snapshot(&self) -> SessionSnapshot {
        self.snapshot.read().expect("snapshot poisoned").clone()
    }

    /// This session's id (also the ledger `session_id`).
    pub fn session_id(&self) -> SessionId {
        self.session_id
    }
}

/// What a finished turn reports back to the actor. `convo` is the
/// complete post-turn conversation — user message, assistant rounds,
/// tool results, and any compaction already applied — so the actor can
/// adopt it verbatim as the new history.
enum TurnReport {
    /// Turn finished (model stopped calling tools, or hit a cap).
    Completed {
        /// Full post-turn conversation (empty only for dead-task fallback).
        convo: Vec<Message>,
        /// Summed usage across all provider rounds.
        usage: TokenUsage,
        /// Tool calls executed during the turn.
        tool_calls: u32,
        /// Request latency, ms.
        latency_ms: u64,
        /// USD cost summed across priced rounds, when priced.
        cost_usd: Option<f64>,
    },
    /// Provider/stream failure; `convo` preserves completed tool rounds.
    Failed {
        /// The provider error.
        error: ProviderError,
        /// Full post-turn conversation.
        convo: Vec<Message>,
    },
    /// Cancellation token fired; `convo` preserves completed rounds.
    Cancelled {
        /// Full post-turn conversation.
        convo: Vec<Message>,
    },
}

/// Spawn a session actor on the current tokio runtime.
///
/// # Panics
/// Panics if called outside a tokio runtime — callers (Tauri setup, CLI
/// `#[tokio::main]`) always have one.
pub fn spawn_session(cfg: SessionConfig) -> SessionHandle {
    let (tx, rx) = mpsc::channel::<Command>(64);
    let (events, _) = broadcast::channel::<Event>(EVENT_BUFFER);
    let session_id = SessionId::new();
    // A restored session seeds history/counters from the journal; the
    // wall-clock budget restarts fresh (Instant can't round-trip).
    let (history, turn, usage, cost_usd) =
        match cfg.persist.as_ref().and_then(|p| p.restore.as_ref()) {
            Some(r) => (r.messages.clone(), TurnId(r.turn), r.usage, r.cost_usd),
            None => (Vec::new(), TurnId(0), TokenUsage::default(), 0.0),
        };
    let snapshot = Arc::new(std::sync::RwLock::new(SessionSnapshot {
        state: "idle".to_string(),
        turn: turn.0,
        usage,
        running: false,
    }));

    let actor = Actor {
        cfg,
        rx,
        events: events.clone(),
        session_id,
        history,
        pending_user: None,
        state: SessionState::Idle,
        turn,
        task_started: Instant::now(),
        turn_started: Instant::now(),
        cancel: CancellationToken::new(),
        turn_done: None,
        cost_usd,
        snapshot: snapshot.clone(),
    };
    tokio::spawn(actor.run());

    SessionHandle {
        tx,
        events,
        snapshot,
        session_id,
    }
}

/// Internal actor state.
struct Actor {
    cfg: SessionConfig,
    rx: mpsc::Receiver<Command>,
    events: broadcast::Sender<Event>,
    session_id: SessionId,
    /// Completed exchanges only — a turn's user message lands here when
    /// the turn reports back (see `pending_user`).
    history: Vec<Message>,
    /// User text of the in-flight turn, promoted into `history` on report.
    pending_user: Option<String>,
    state: SessionState,
    turn: TurnId,
    /// First-message timestamp — the task wall-clock limit measures from here.
    task_started: Instant,
    /// Current turn's start — per-turn elapsed and cancel reporting.
    turn_started: Instant,
    /// Cancel token for the in-flight turn; replaced each turn.
    cancel: CancellationToken,
    /// Receives the turn task's single report; `Some` while a turn runs.
    turn_done: Option<mpsc::Receiver<TurnReport>>,
    /// Accumulated USD spend this task (from priced ledger records).
    cost_usd: f64,
    snapshot: Arc<std::sync::RwLock<SessionSnapshot>>,
}

impl Actor {
    #[instrument(skip_all, fields(session = %self.session_id.0))]
    async fn run(mut self) {
        info!("session started");
        loop {
            // Wait for a command or the in-flight turn's report, whichever
            // comes first. With no turn running there is nothing to select.
            let report = match self.turn_done.as_mut() {
                Some(done) => {
                    tokio::select! {
                        cmd = self.rx.recv() => match cmd {
                            Some(c) => {
                                self.handle_command(c);
                                None
                            }
                            None => break, // every handle dropped
                        },
                        rep = done.recv() => {
                            self.turn_done = None;
                            // A closed channel means the turn task died
                            // without reporting — surface it, never hang.
                            Some(rep.unwrap_or_else(|| TurnReport::Failed {
                                error: ProviderError::Stream(
                                    "turn task terminated without report".to_string(),
                                ),
                                convo: Vec::new(),
                            }))
                        }
                    }
                }
                None => match self.rx.recv().await {
                    Some(c) => {
                        self.handle_command(c);
                        None
                    }
                    None => break,
                },
            };

            if let Some(rep) = report {
                self.finish_turn(rep);
            }
        }
        info!("session ended");
    }

    fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::UserMessage(text) => self.begin_turn(text),
            Command::Cancel => {
                if self.turn_done.is_some() {
                    debug!("cancelling in-flight turn");
                    self.cancel.cancel();
                }
            }
            // M2 ask_user prompt; still accepted-and-ignored.
            Command::AnswerPrompt(_) => {
                warn!("prompt response with nothing pending");
            }
            Command::RespondPermission {
                request_id,
                decision,
            } => {
                let d = match decision.as_str() {
                    "allow" => PermissionDecision::Allow,
                    "allow_session" => PermissionDecision::AllowSession,
                    _ => PermissionDecision::Deny,
                };
                self.cfg.permissions.resolve(&request_id, d);
            }
            Command::SetOptions {
                model,
                thinking,
                context_window,
            } => {
                if let Some(m) = model {
                    debug!(model = %m, "session model switched");
                    self.cfg.model = m;
                }
                if let Some(t) = thinking {
                    debug!(thinking = ?t, "session thinking level switched");
                    self.cfg.thinking = t;
                }
                if let Some(w) = context_window {
                    self.cfg.context_window = w;
                }
                // Option changes are session state — persist even when no
                // turn ever runs so a model pick survives a restart.
                self.persist_now();
            }
            Command::Compact => self.compact_now(),
        }
    }

    /// `/compact` — shrink history now, outside any turn. Same safety
    /// rules as the automatic path; reports honest before/after
    /// estimates so the UI can show real numbers.
    fn compact_now(&mut self) {
        if self.turn_done.is_some() {
            self.emit(Event::Error {
                error: AgentError::Internal("can't compact while a turn is running".to_string()),
            });
            return;
        }
        let system = self.system_prompt();
        let mut specs = self
            .cfg
            .tools
            .as_ref()
            .map(|t| t.specs())
            .unwrap_or_default();
        if let Some(hub) = self.cfg.mcp.as_ref() {
            specs.extend(hub.specs());
        }
        match force_compact(&mut self.history, &system, &specs) {
            Some((before, after)) => {
                self.emit(Event::CompactionStarted {
                    reason: CompactionReason::Manual,
                    before,
                    after,
                });
                self.persist_now();
            }
            None => self.emit(Event::Error {
                error: AgentError::Internal("nothing to compact yet".to_string()),
            }),
        }
    }

    /// S0 + S3 — the system string `begin_turn` sends, so compaction
    /// estimates measure what the provider actually receives.
    fn system_prompt(&self) -> String {
        // S0 system prompt + S3 project instructions when the workspace
        // declares them (AGENTS.md etc.). Stable per session, so prefix
        // caching still hits.
        match self.cfg.tools.as_ref().and_then(|t| t.project_context()) {
            Some(ctx) => format!(
                "{}\n\nProject instructions (from the workspace):\n{}",
                hate_prompt::SYSTEM_PROMPT,
                ctx
            ),
            None => hate_prompt::SYSTEM_PROMPT.to_string(),
        }
    }

    #[instrument(skip_all, fields(turn = self.turn.0 + 1))]
    fn begin_turn(&mut self, text: String) {
        // Hard limits first — a tripped limit is a Failed, never a silent stop.
        if let Err(reason) = self.check_limits() {
            self.emit(Event::Failed { reason });
            return;
        }
        if self.turn_done.is_some() {
            self.emit(Event::Error {
                error: AgentError::Internal("turn already in flight".to_string()),
            });
            return;
        }

        self.turn = self.turn.next();
        self.state = SessionState::AwaitingModel;
        self.cancel = CancellationToken::new();
        self.turn_started = Instant::now();
        self.pending_user = Some(text.clone());
        self.set_snapshot();

        let mut messages = self.history.clone();
        messages.push(Message::User { content: text });

        let (done_tx, done_rx) = mpsc::channel(1);
        self.turn_done = Some(done_rx);

        let provider = Arc::clone(&self.cfg.provider);
        let cancel = self.cancel.clone();
        let events = self.events.clone();
        let system = Some(self.system_prompt());
        let model = self.cfg.model.clone();
        let provider_name = self.cfg.provider_name.clone();
        let thinking = self.cfg.thinking;
        let tools = self.cfg.tools.clone();
        let mcp = self.cfg.mcp.clone();
        let permissions = self.cfg.permissions.clone();
        let ledger = self.cfg.ledger.clone();
        let context_window = self.cfg.context_window;
        let session_id = self.session_id;
        let turn_id = self.turn;
        let started = self.turn_started;

        tokio::spawn(async move {
            run_turn(
                provider,
                messages,
                system,
                model,
                provider_name,
                thinking,
                tools,
                mcp,
                permissions,
                cancel,
                events,
                done_tx,
                ledger,
                session_id,
                turn_id,
                started,
                context_window,
            )
            .await;
        });
    }

    fn check_limits(&self) -> Result<(), FailureReason> {
        if self.turn.0 >= self.cfg.limits.max_turns {
            return Err(FailureReason::LimitExceeded {
                limit: "max_turns".to_string(),
                value: u64::from(self.turn.0),
            });
        }
        if self.task_started.elapsed() > self.cfg.limits.max_wall_clock {
            return Err(FailureReason::LimitExceeded {
                limit: "max_wall_clock".to_string(),
                value: self.task_started.elapsed().as_secs(),
            });
        }
        if let Some(cap) = self.cfg.limits.max_cost_usd
            && self.cost_usd >= cap
        {
            return Err(FailureReason::LimitExceeded {
                limit: "max_cost_usd".to_string(),
                value: (self.cost_usd * 1_000_000.0) as u64,
            });
        }
        Ok(())
    }

    /// Adopt the turn's reported conversation as history. Reports from
    /// `run_turn` carry the complete post-turn convo — user message
    /// included, compaction already applied. An empty `convo` means the
    /// turn task died without reporting; keep prior history and record
    /// just the user message.
    fn adopt_convo(&mut self, convo: Vec<Message>, user_msg: Option<Message>) {
        if convo.is_empty() {
            if let Some(u) = user_msg {
                self.history.push(u);
            }
        } else {
            self.history = convo;
        }
    }

    fn finish_turn(&mut self, rep: TurnReport) {
        // Whatever the outcome, the user message becomes part of history —
        // the model should see what the user actually said even if the
        // reply was cancelled or failed. `adopt_convo` dedupes: run_turn
        // reports already contain it.
        let user_msg = self
            .pending_user
            .take()
            .map(|t| Message::User { content: t });

        match rep {
            TurnReport::Completed {
                convo,
                usage,
                tool_calls,
                latency_ms,
                cost_usd,
            } => {
                self.adopt_convo(convo, user_msg);
                if let Some(cost) = cost_usd {
                    self.cost_usd += cost;
                }
                self.snapshot_usage(usage);
                let outcome = TurnOutcome {
                    outcome: Outcome::Completed,
                    usage,
                    tool_calls,
                    elapsed_ms: latency_ms,
                    cost_usd,
                };
                self.emit(Event::TurnComplete { outcome });
            }
            TurnReport::Failed { error, convo } => {
                // Completed tool rounds stay in history — the model did
                // real work before the failure and should see it.
                self.adopt_convo(convo, user_msg);
                self.emit(Event::Failed {
                    reason: error.into(),
                });
            }
            TurnReport::Cancelled { convo } => {
                self.adopt_convo(convo, user_msg);
                let outcome = TurnOutcome {
                    outcome: Outcome::Cancelled,
                    usage: TokenUsage::default(),
                    tool_calls: 0,
                    elapsed_ms: self.turn_started.elapsed().as_millis() as u64,
                    cost_usd: None,
                };
                self.emit(Event::TurnComplete { outcome });
            }
        }
        self.state = SessionState::Idle;
        self.set_snapshot();
        // Mutating turns leave a restorable checkpoint before the
        // session-state write — every outcome, not just success.
        self.persist_checkpoint();
        // Every turn outcome (completed/failed/cancelled) changes history
        // or counters — this is the "context always survives" write.
        self.persist_now();
    }

    /// Persist this turn's captured file originals as a checkpoint and
    /// announce it. No-op when nothing mutated or checkpoints are off.
    /// Write failures degrade silently — the turn already happened.
    fn persist_checkpoint(&self) {
        let Some(tools) = self.cfg.tools.as_ref() else {
            return;
        };
        let Some(bundle) = tools.drain_checkpoint() else {
            return;
        };
        match crate::checkpoint::write_checkpoint(&bundle) {
            Ok(()) => self.emit(Event::CheckpointCreated {
                id: CheckpointId(bundle.id),
            }),
            Err(e) => warn!(error = %e, "checkpoint write failed"),
        }
    }

    /// Snapshot actor state into the journal. Synchronous and cheap —
    /// one SQLite upsert of a few hundred KB at most, once per turn.
    fn persist_now(&self) {
        let Some(p) = self.cfg.persist.as_ref() else {
            return;
        };
        let title = self
            .history
            .iter()
            .find_map(|m| match m {
                Message::User { content } => {
                    let line = content.lines().next().unwrap_or("").trim();
                    (!line.is_empty()).then(|| line.chars().take(80).collect::<String>())
                }
                _ => None,
            })
            .unwrap_or_else(|| "New session".to_string());
        let usage = self.snapshot.read().expect("snapshot poisoned").usage;
        p.journal.save(&PersistedSession {
            key: p.key.clone(),
            mode: p.mode.clone(),
            provider: self.cfg.provider_name.clone(),
            model: self.cfg.model.clone(),
            thinking: self.cfg.thinking,
            workspace: p.workspace.clone(),
            title,
            messages: self.history.clone(),
            turn: self.turn.0,
            usage,
            cost_usd: self.cost_usd,
        });
    }

    /// Fold one request's usage into the session snapshot totals.
    fn snapshot_usage(&self, u: TokenUsage) {
        let mut s = self.snapshot.write().expect("snapshot poisoned");
        s.usage.input += u.input;
        s.usage.cached_input += u.cached_input;
        s.usage.output += u.output;
        s.usage.thinking += u.thinking;
    }

    fn emit(&self, ev: Event) {
        // Broadcast never blocks meaningfully; a fully-lagged subscriber
        // set is not a reason to kill the turn.
        let _ = self.events.send(ev);
    }

    fn set_snapshot(&self) {
        let mut s = self.snapshot.write().expect("snapshot poisoned");
        s.state = format!("{:?}", self.state);
        s.turn = self.turn.0;
        s.running = self.turn_done.is_some();
    }
}

/// Routes permission decisions from the actor's command channel to the
/// in-flight tool loop waiting on them, and holds the session-scoped
/// allowlist behind "Run & remember".
#[derive(Default)]
pub struct PermissionHub {
    /// `request_id` → (decision sender, allowlist key).
    pending: Mutex<HashMap<String, (oneshot::Sender<PermissionDecision>, String)>>,
    /// Allowlist keys approved with `AllowSession` this session.
    allowed: Mutex<HashSet<String>>,
}

/// What the policy gate resolved to for one call.
enum Gate {
    /// Approved (or auto-allowed) — execute.
    Run,
    /// User denied — feed the denial back as a tool error.
    Denied(String),
    /// Turn cancelled while waiting — abort the whole turn.
    Cancelled,
}

impl PermissionHub {
    /// The allowlist key for a call: bash remembers the exact command,
    /// file tools remember the tool name.
    fn key_for(call: &ToolCall) -> String {
        if call.name.as_str() == "bash" {
            let cmd = call
                .args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("bash:{cmd}")
        } else {
            call.name.as_str().to_string()
        }
    }

    /// Whether this call's key was session-allowed already.
    fn is_allowed(&self, call: &ToolCall) -> bool {
        self.allowed
            .lock()
            .expect("permission hub poisoned")
            .contains(&Self::key_for(call))
    }

    /// Register a waiter for `id`; the returned receiver resolves when
    /// the actor answers (or drops on cancel/teardown → deny).
    fn insert(&self, id: String, tx: oneshot::Sender<PermissionDecision>, key: String) {
        self.pending
            .lock()
            .expect("permission hub poisoned")
            .insert(id, (tx, key));
    }

    /// Remove a pending request without answering (cancel path).
    fn remove(&self, id: &str) {
        self.pending
            .lock()
            .expect("permission hub poisoned")
            .remove(id);
    }

    /// Actor side: deliver the user's decision. `AllowSession` also
    /// records the call's key so matching future calls skip the prompt.
    pub fn resolve(&self, id: &str, decision: PermissionDecision) {
        let entry = self
            .pending
            .lock()
            .expect("permission hub poisoned")
            .remove(id);
        let Some((tx, key)) = entry else {
            return;
        };
        if decision == PermissionDecision::AllowSession {
            self.allowed
                .lock()
                .expect("permission hub poisoned")
                .insert(key);
        }
        let _ = tx.send(decision);
    }
}

/// Ask the user (via the actor) whether `call` may run. Auto-allows
/// session-approved keys; otherwise emits `PermissionRequested` and
/// waits — racing the turn's cancel token.
async fn gate_tool(
    hub: &Arc<PermissionHub>,
    cancel: &CancellationToken,
    events: &broadcast::Sender<Event>,
    call: &ToolCall,
    reason: String,
    detail: Option<String>,
) -> Gate {
    if hub.is_allowed(call) {
        return Gate::Run;
    }
    let id = Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    hub.insert(id.clone(), tx, PermissionHub::key_for(call));
    let _ = events.send(Event::PermissionRequested {
        request: PermissionRequest {
            id: id.clone(),
            call: call.clone(),
            reason,
            detail,
        },
    });
    let decision = tokio::select! {
        () = cancel.cancelled() => None,
        d = rx => Some(d.unwrap_or(PermissionDecision::Deny)),
    };
    match decision {
        None => {
            hub.remove(&id);
            Gate::Cancelled
        }
        Some(d) if d.is_allowed() => Gate::Run,
        Some(_) => Gate::Denied(format!(
            "permission denied by user for `{}`",
            call.name.as_str()
        )),
    }
}

/// Which tool surface a call routes to. MCP names are prefixed
/// `mcp__` so routing is a string check, never ambiguous.
enum ToolBackend<'a> {
    /// Filesystem tools bound to the session workspace.
    Ws(&'a Arc<WorkspaceTools>),
    /// Tools advertised by configured MCP servers.
    Mcp(&'a Arc<McpHub>),
}

impl ToolBackend<'_> {
    /// Policy gate — `Some` means the call needs user approval.
    fn gate_reason(&self, call: &ToolCall) -> Option<(String, Option<String>)> {
        match self {
            Self::Ws(t) => t.gate_reason(call),
            Self::Mcp(h) => h.gate_reason(call),
        }
    }

    /// Run the call. `cancel` only reaches the MCP side — workspace
    /// tools manage their own timeouts.
    async fn execute(&self, call: &ToolCall, cancel: &CancellationToken) -> ToolOutput {
        match self {
            Self::Ws(t) => t.execute(call).await,
            Self::Mcp(h) => h.execute(call, cancel).await,
        }
    }

    /// Error-shaped output for policy denials.
    fn tool_error(&self, msg: String) -> ToolOutput {
        match self {
            Self::Ws(t) => t.tool_error(msg),
            Self::Mcp(h) => h.tool_error(msg),
        }
    }
}

/// Borrow whichever backend exists purely to shape an error output —
/// used when a call arrives for a backend this session doesn't have.
fn error_backend<'a>(
    mcp: Option<&'a Arc<McpHub>>,
    tools: Option<&'a Arc<WorkspaceTools>>,
) -> ToolBackend<'a> {
    mcp.map(ToolBackend::Mcp)
        .or_else(|| tools.map(ToolBackend::Ws))
        .expect("error_backend requires at least one tool backend")
}

/// The per-turn driver. Owns the whole tool-call loop: stream one
/// provider round, execute any requested tools, feed the results back,
/// and keep going until the model answers with no calls. Sends exactly
/// one [`TurnReport`] when done; every round is a billed request, so
/// usage/cost/tool counts accumulate across the whole turn.
#[allow(clippy::too_many_arguments)]
async fn run_turn(
    provider: Arc<dyn Provider>,
    mut convo: Vec<Message>,
    system: Option<String>,
    model: String,
    provider_name: String,
    thinking: ThinkingLevel,
    tools: Option<Arc<WorkspaceTools>>,
    mcp: Option<Arc<McpHub>>,
    permissions: Arc<PermissionHub>,
    cancel: CancellationToken,
    events: broadcast::Sender<Event>,
    done_tx: mpsc::Sender<TurnReport>,
    ledger: Option<Arc<Ledger>>,
    session_id: SessionId,
    turn_id: TurnId,
    started: Instant,
    context_window: u64,
) {
    // A model that never stops calling tools must not loop forever.
    const MAX_TOOL_ROUNDS: u32 = 12;
    const MAX_TOOL_CALLS: u32 = 48;

    let mut specs = tools.as_ref().map(|t| t.specs()).unwrap_or_default();
    if let Some(hub) = mcp.as_ref() {
        specs.extend(hub.specs());
    }
    let mut total_usage = TokenUsage::default();
    let mut total_cost: Option<f64> = None;
    let mut tool_calls_done = 0u32;
    let mut round = 0u32;

    loop {
        if cancel.is_cancelled() {
            let _ = done_tx
                .send(TurnReport::Cancelled {
                    convo: convo.clone(),
                })
                .await;
            return;
        }
        if round >= MAX_TOOL_ROUNDS {
            let _ = done_tx
                .send(TurnReport::Failed {
                    error: ProviderError::Stream(format!(
                        "tool loop exceeded {MAX_TOOL_ROUNDS} provider rounds"
                    )),
                    convo: convo.clone(),
                })
                .await;
            return;
        }
        round += 1;

        // Context-pressure check before every billed round: the tool
        // loop can grow `convo` far past one turn's worth of messages.
        if let Some((before, after, reason)) = maybe_compact(
            &mut convo,
            system.as_deref().unwrap_or(""),
            &specs,
            context_window,
        ) {
            let _ = events.send(Event::CompactionStarted {
                reason,
                before,
                after,
            });
        }

        // Provider-bound copy: canonical `convo` keeps full tool output
        // for the journal; only the wire view decays stale results.
        let req = ChatRequest {
            model: model.clone(),
            system: system.clone(),
            messages: decayed_view(&convo),
            tools: specs.clone(),
            max_output_tokens: 8192,
            temperature: None,
            thinking,
        };

        let round_started = Instant::now();
        let request_id = Uuid::new_v4().to_string();
        let stream = provider.stream(req, cancel.clone());
        tokio::pin!(stream);

        let mut text = String::new();
        let mut calls: Vec<ToolCall> = Vec::new();
        let mut usage = TokenUsage::default();

        let emit = |ev: Event| {
            let _ = events.send(ev);
        };

        loop {
            // Prioritise the cancel check: `select!` picks randomly among
            // ready branches, so a saturated stream could starve it.
            if cancel.is_cancelled() {
                let _ = done_tx
                    .send(TurnReport::Cancelled {
                        convo: convo.clone(),
                    })
                    .await;
                return;
            }
            tokio::select! {
                () = cancel.cancelled() => {
                    let _ = done_tx
                        .send(TurnReport::Cancelled {
                            convo: convo.clone(),
                        })
                        .await;
                    return;
                }
                item = stream.next() => {
                    match item {
                        Some(Ok(Delta::Text(t))) => {
                            text.push_str(&t);
                            emit(Event::Delta { text: t });
                        }
                        Some(Ok(Delta::Thinking(t))) => {
                            emit(Event::ThinkingDelta { text: t });
                        }
                        Some(Ok(Delta::ToolCall(call))) => {
                            calls.push(call);
                        }
                        Some(Ok(Delta::Usage(u))) => {
                            usage = u;
                            emit(Event::TokensCounted { usage, turn: turn_id });
                        }
                        Some(Ok(Delta::Done)) => break,
                        // Channel closed without Done: cancelled streams
                        // end this way, but so do truncated ones.
                        None => {
                            if cancel.is_cancelled() {
                                let _ = done_tx
                                    .send(TurnReport::Cancelled {
                                        convo: convo.clone(),
                                    })
                                    .await;
                            } else {
                                let _ = done_tx
                                    .send(TurnReport::Failed {
                                        error: ProviderError::Stream(
                                            "provider stream ended without Delta::Done".to_string(),
                                        ),
                                        convo: convo.clone(),
                                    })
                                    .await;
                            }
                            return;
                        }
                        Some(Err(e)) => {
                            let _ = done_tx
                                .send(TurnReport::Failed {
                                    error: e,
                                    convo: convo.clone(),
                                })
                                .await;
                            return;
                        }
                    }
                }
            }
        }

        // ── round complete: account for it, then decide ────────────────
        let round_latency = round_started.elapsed().as_millis() as u64;

        // Ledger write is best-effort: accounting failure must not fail
        // a turn. One record per provider request (per round).
        if let Some(ledger) = &ledger {
            let rec = RecordRequest {
                session_id: session_id.0.to_string(),
                turn_id: turn_id.0,
                request_id,
                model: model.clone(),
                provider: provider_name.clone(),
                input_tokens: usage.input,
                cached_input_tokens: usage.cached_input,
                output_tokens: usage.output,
                thinking_tokens: usage.thinking,
                latency_ms: round_latency,
            };
            match ledger.record_request(&rec) {
                Ok(Some(cost)) => total_cost = Some(total_cost.unwrap_or(0.0) + cost),
                Ok(None) => {}
                Err(e) => warn!(error = %e, "ledger record failed"),
            }
        }
        total_usage.input += usage.input;
        total_usage.cached_input += usage.cached_input;
        total_usage.output += usage.output;
        total_usage.thinking += usage.thinking;
        tool_calls_done += calls.len() as u32;

        // Record the assistant round BEFORE the no-calls exit — the
        // final reply must land in history too, not just tool rounds.
        convo.push(Message::Assistant {
            content: text,
            tool_calls: calls.clone(),
        });

        if calls.is_empty() {
            let _ = done_tx
                .send(TurnReport::Completed {
                    convo: convo.clone(),
                    usage: total_usage,
                    tool_calls: tool_calls_done,
                    latency_ms: started.elapsed().as_millis() as u64,
                    cost_usd: total_cost,
                })
                .await;
            return;
        }

        if tools.is_none() && mcp.is_none() {
            let _ = done_tx
                .send(TurnReport::Failed {
                    error: ProviderError::Stream(
                        "model requested tool call(s) but this session has no tools".to_string(),
                    ),
                    convo: convo.clone(),
                })
                .await;
            return;
        }

        if tool_calls_done > MAX_TOOL_CALLS {
            let _ = done_tx
                .send(TurnReport::Failed {
                    error: ProviderError::Stream(format!(
                        "tool call limit exceeded ({MAX_TOOL_CALLS})"
                    )),
                    convo: convo.clone(),
                })
                .await;
            return;
        }

        // The assistant round is already recorded above; run every call
        // so the batch stays atomic: history never holds a call without
        // its result, even if the user cancels immediately after.
        for call in calls {
            emit(Event::ToolStarted { call: call.clone() });
            // `mcp__…` routes to the MCP hub; everything else to the
            // workspace tools. A call for a backend this session lacks
            // becomes an honest tool error, not a turn failure.
            let backend = if call.name.as_str().starts_with(MCP_PREFIX) {
                mcp.as_ref().map(ToolBackend::Mcp)
            } else {
                tools.as_ref().map(ToolBackend::Ws)
            };
            let out = match backend {
                Some(b) => match b.gate_reason(&call) {
                    Some((reason, detail)) => {
                        match gate_tool(&permissions, &cancel, &events, &call, reason, detail).await
                        {
                            Gate::Run => b.execute(&call, &cancel).await,
                            Gate::Denied(msg) => b.tool_error(msg),
                            Gate::Cancelled => {
                                let _ = done_tx
                                    .send(TurnReport::Cancelled {
                                        convo: convo.clone(),
                                    })
                                    .await;
                                return;
                            }
                        }
                    }
                    None => b.execute(&call, &cancel).await,
                },
                None => error_backend(mcp.as_ref(), tools.as_ref()).tool_error(format!(
                    "unknown tool `{}` for this session",
                    call.name.as_str()
                )),
            };
            emit(Event::ToolOutput {
                call_id: call.id.clone(),
                preview: out.preview,
                tokens: out.sent_tokens,
            });
            emit(Event::ToolFinished {
                call_id: call.id.clone(),
                ok: out.ok,
                full_tokens: out.full_tokens,
                sent_tokens: out.sent_tokens,
            });
            convo.push(Message::Tool {
                call_id: call.id.clone(),
                name: call.name.as_str().to_string(),
                content: out.content,
            });
        }
        // Next iteration streams the follow-up request with results in.
    }
}

// ── context compaction ────────────────────────────────────────────────────

/// Compact proactively at 72% of the window; hard-stop at 92%.
const COMPACT_SOFT: f64 = 0.72;
const COMPACT_HARD: f64 = 0.92;
/// Messages kept verbatim at the tail.
const COMPACT_TAIL: usize = 12;
/// Caps for the generated compaction note.
const NOTE_FILES_MAX: usize = 15;
const NOTE_SNIPPET: usize = 600;

/// Rough input-token estimate for trigger checks — o200k over the
/// serialized prompt. Provider `count_tokens` would cost an HTTP call
/// per round on Anthropic; the 0.72/0.92 thresholds leave enough
/// margin for BPE drift. `0` = tokenizer unavailable → never compact
/// (the provider then errors honestly on real overflow).
fn estimate_tokens(system: &str, tools: &[crate::provider::ToolSpec], messages: &[Message]) -> u64 {
    let Ok(counter) = O200kCounter::new() else {
        return 0;
    };
    let mut text = String::with_capacity(64 * 1024);
    text.push_str(system);
    if let Ok(specs) = serde_json::to_string(tools) {
        text.push_str(&specs);
    }
    for m in messages {
        push_message_text(m, &mut text);
    }
    counter.count_sync(&text)
}

fn push_message_text(m: &Message, out: &mut String) {
    match m {
        Message::User { content } => {
            out.push_str("\nuser\n");
            out.push_str(content);
        }
        Message::Assistant {
            content,
            tool_calls,
        } => {
            out.push_str("\nassistant\n");
            out.push_str(content);
            for c in tool_calls {
                out.push('\n');
                out.push_str(c.name.as_str());
                out.push_str(&serde_json::to_string(&c.args).unwrap_or_default());
            }
        }
        Message::Tool { name, content, .. } => {
            out.push_str("\ntool\n");
            out.push_str(name);
            out.push('\n');
            out.push_str(content);
        }
    }
}

/// Compact `convo` in place when it approaches `window`. Keeps the
/// opening user message (the task anchor) and a verbatim tail; the
/// dropped middle becomes a structured note preserving the files the
/// agent touched. Returns `(before, after, reason)` when it ran.
fn maybe_compact(
    convo: &mut Vec<Message>,
    system: &str,
    tools: &[crate::provider::ToolSpec],
    window: u64,
) -> Option<(u64, u64, CompactionReason)> {
    if window == 0 || convo.len() < COMPACT_TAIL + 4 {
        return None;
    }
    // Estimate what actually goes on the wire: stale tool outputs are
    // decayed in the request view, so counting raw `convo` would
    // overstate pressure and compact earlier than needed.
    let before = estimate_tokens(system, tools, &decayed_view(convo));
    if before == 0 {
        return None;
    }
    let ratio = before as f64 / window as f64;
    let reason = if ratio >= COMPACT_HARD {
        CompactionReason::HardStop
    } else if ratio >= COMPACT_SOFT {
        CompactionReason::Threshold
    } else {
        return None;
    };

    if !compact_core(convo) {
        return None;
    }
    let after = estimate_tokens(system, tools, &decayed_view(convo));
    Some((before, after, reason))
}

/// User-invoked compaction (`/compact`): same anchor/tail/note safety
/// rules as the automatic path, but no window threshold — the user
/// asked, so we shrink whenever there's anything meaningful to drop.
/// Returns `(before, after)` token estimates like [`maybe_compact`].
fn force_compact(
    convo: &mut Vec<Message>,
    system: &str,
    tools: &[crate::provider::ToolSpec],
) -> Option<(u64, u64)> {
    if convo.len() < COMPACT_TAIL + 4 {
        return None;
    }
    let before = estimate_tokens(system, tools, &decayed_view(convo));
    if before == 0 || !compact_core(convo) {
        return None;
    }
    let after = estimate_tokens(system, tools, &decayed_view(convo));
    Some((before, after))
}

/// The actual rewrite shared by auto and manual compaction: keep the
/// task anchor + a tool-safe verbatim tail, replace the dropped middle
/// with a structured note. Returns false when there's nothing safe to
/// drop (convo too small or tail would swallow everything).
fn compact_core(convo: &mut Vec<Message>) -> bool {
    let n = convo.len();
    // Task anchor: the first message when it's the user's opening ask.
    let anchor = usize::from(matches!(convo.first(), Some(Message::User { .. })));
    // Tail must never begin on a `Tool` result — its parent assistant
    // call would be dropped, orphaning the result for the provider.
    let mut tail = n.saturating_sub(COMPACT_TAIL).max(anchor);
    while tail < n && matches!(convo[tail], Message::Tool { .. }) {
        tail += 1;
    }
    if tail <= anchor + 2 {
        return false; // nothing meaningful to drop
    }

    let dropped: Vec<Message> = convo[anchor..tail].to_vec();
    let note = compaction_note(&dropped, anchor > 0);
    let mut next = Vec::with_capacity(anchor + 1 + (n - tail));
    next.extend(convo[..anchor].iter().cloned());
    next.push(Message::User { content: note });
    next.extend(convo[tail..].iter().cloned());
    *convo = next;
    true
}

/// Structured note standing in for the dropped messages — preserves
/// the task text (when no anchor survives) and every file path the
/// agent touched, the highest-value memory for continuing work.
fn compaction_note(dropped: &[Message], has_anchor: bool) -> String {
    use std::fmt::Write as _;
    let mut note = String::from(
        "[Context compacted]\nEarlier turns were compacted to fit the model's context window. \
         The visible transcript still shows the full history.\n",
    );
    if !has_anchor
        && let Some(first) = dropped.iter().find_map(|m| match m {
            Message::User { content } if !content.starts_with('[') => Some(content.as_str()),
            _ => None,
        })
    {
        note.push_str("\nOriginal request:\n");
        note.push_str(&truncate(first, NOTE_SNIPPET));
        note.push('\n');
    }

    let mut files: Vec<&str> = Vec::new();
    for m in dropped {
        let Message::Assistant { tool_calls, .. } = m else {
            continue;
        };
        for c in tool_calls {
            for key in ["path", "file_path", "notebook_path"] {
                if let Some(p) = c.args.get(key).and_then(|v| v.as_str())
                    && !files.contains(&p)
                {
                    files.push(p);
                    if files.len() >= NOTE_FILES_MAX {
                        break;
                    }
                }
            }
            if files.len() >= NOTE_FILES_MAX {
                break;
            }
        }
        if files.len() >= NOTE_FILES_MAX {
            break;
        }
    }
    if !files.is_empty() {
        note.push_str("\nFiles touched:\n");
        for f in &files {
            note.push_str("- ");
            note.push_str(f);
            note.push('\n');
        }
    }
    let _ = write!(note, "\n({} earlier messages compacted)\n", dropped.len());
    note
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

// ── stale tool-output decay ───────────────────────────────────────────────

/// Byte cap for tool outputs carried into a NEW turn as history.
const STALE_TOOL_CAP: usize = 8 * 1024;
const STALE_HEAD: usize = 6 * 1024;
const STALE_TAIL: usize = 2 * 1024;
/// Fallback keep-tail when no user message marks the turn boundary.
const DECAY_KEEP_TAIL: usize = 16;
const ELISION_MARKER: &str = "[tool output elided";

/// Clone `convo` with oversized tool results from *prior* turns shrunk
/// to head+tail stubs. The boundary is this turn's user message — the
/// last `User` role (compaction notes are `User` too but sit at the
/// front, never last). Within-turn results stay full: the model needs
/// them now, and re-running the tool is the honest path back to full
/// fidelity once they go stale. Applied to the provider-bound copy
/// only — canonical history (and the journal) keeps the real bytes,
/// and the marker can never nest because every view rebuilds from
/// full text.
fn decayed_view(convo: &[Message]) -> Vec<Message> {
    let mut view = convo.to_vec();
    let boundary = convo
        .iter()
        .rposition(|m| matches!(m, Message::User { .. }))
        .unwrap_or_else(|| convo.len().saturating_sub(DECAY_KEEP_TAIL));
    for m in &mut view[..boundary] {
        if let Message::Tool { content, .. } = m {
            elide_stale_tool_output(content);
        }
    }
    view
}

/// Shrink an oversized historical tool result to head+tail with an
/// honest marker — the model keeps the shape of what it saw and can
/// re-run the tool (args stay in history) for the full text.
fn elide_stale_tool_output(content: &mut String) {
    if content.len() <= STALE_TOOL_CAP || content.starts_with(ELISION_MARKER) {
        return;
    }
    let orig = content.len();
    let mut head_end = STALE_HEAD.min(orig);
    while head_end > 0 && !content.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = orig.saturating_sub(STALE_TAIL);
    while tail_start < orig && !content.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    *content = format!(
        "[tool output elided — was {orig} bytes; re-run the tool for full output]\n{}\n…\n{}",
        &content[..head_end],
        &content[tail_start..],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolName;

    fn user(text: &str) -> Message {
        Message::User {
            content: text.to_string(),
        }
    }

    fn assistant(text: &str) -> Message {
        Message::Assistant {
            content: text.to_string(),
            tool_calls: Vec::new(),
        }
    }

    /// 20 alternating messages with enough text to blow a tiny window.
    fn fat_convo() -> Vec<Message> {
        let filler = "lorem ipsum dolor sit amet ".repeat(80);
        let mut v = vec![user("THE TASK: refactor the parser")];
        for i in 0..10 {
            v.push(assistant(&format!("{filler} step {i} a")));
            v.push(user(&format!("{filler} step {i} u")));
        }
        v.push(assistant("final answer pending"));
        v
    }

    #[test]
    fn under_threshold_is_noop() {
        let mut convo = fat_convo();
        let n = convo.len();
        // Huge window → ratio far below soft threshold.
        assert!(maybe_compact(&mut convo, "sys", &[], u64::MAX).is_none());
        assert_eq!(convo.len(), n);
    }

    #[test]
    fn zero_window_disables_compaction() {
        let mut convo = fat_convo();
        assert!(maybe_compact(&mut convo, "sys", &[], 0).is_none());
    }

    #[test]
    fn compacts_middle_and_preserves_anchor_and_tail() {
        let mut convo = fat_convo();
        // Window small enough that ~21 fat messages exceed 72%.
        let r = maybe_compact(&mut convo, "sys", &[], 2_000).expect("should compact");
        assert!(matches!(
            r.2,
            CompactionReason::Threshold | CompactionReason::HardStop
        ));
        assert!(r.0 > r.1, "before={} after={}", r.0, r.1);

        // Anchor + note + tail.
        assert_eq!(convo.len(), 1 + 1 + COMPACT_TAIL);
        assert!(matches!(&convo[0], Message::User { content } if content.contains("THE TASK")));
        assert!(
            matches!(&convo[1], Message::User { content } if content.contains("[Context compacted]"))
        );
        assert!(
            matches!(&convo[2], Message::User { .. })
                || matches!(&convo[2], Message::Assistant { .. })
        );
        // Tail is verbatim — last message is the original final answer.
        assert!(matches!(
            convo.last(),
            Some(Message::Assistant { content, .. }) if content == "final answer pending"
        ));
        // Note records the dropped count.
        if let Message::User { content } = &convo[1] {
            assert!(content.contains("earlier messages compacted"));
        }
    }

    #[test]
    fn force_compact_ignores_threshold() {
        let mut convo = fat_convo();
        let n = convo.len();
        // Huge window → the automatic path would never fire; the
        // user-invoked one still compacts whenever there's something
        // meaningful to drop.
        let (before, after) = force_compact(&mut convo, "sys", &[]).expect("manual compacts");
        assert!(before > after, "before={before} after={after}");
        assert!(convo.len() < n);
        assert!(
            matches!(&convo[1], Message::User { content } if content.contains("[Context compacted]"))
        );
    }

    #[test]
    fn force_compact_tiny_convo_is_noop() {
        let mut convo = vec![user("hi"), assistant("hello")];
        assert!(force_compact(&mut convo, "sys", &[]).is_none());
        assert_eq!(convo.len(), 2);
    }

    #[test]
    fn tail_never_starts_on_tool_result() {
        // Craft a convo where n - COMPACT_TAIL lands inside a run of
        // Tool messages: the boundary must advance past them.
        let mut convo = vec![user("task")];
        for i in 0..8 {
            convo.push(Message::Assistant {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: format!("c{i}"),
                    name: ToolName::new("read_file"),
                    args: serde_json::json!({"path": format!("f{i}.rs")}),
                }],
            });
            convo.push(Message::Tool {
                call_id: format!("c{i}"),
                name: "read_file".to_string(),
                content: "x".repeat(400),
            });
        }
        // Pad to just over the minimum so the naive boundary sits inside
        // the tool-result run.
        while convo.len() < COMPACT_TAIL + 5 {
            convo.push(assistant("pad"));
        }
        let r = maybe_compact(&mut convo, "sys", &[], 500);
        assert!(r.is_some());
        // Whatever follows the note must not be an orphaned Tool result.
        let note_idx = convo
            .iter()
            .position(|m| matches!(m, Message::User { content } if content.contains("[Context compacted]")))
            .expect("note present");
        assert!(
            !matches!(convo[note_idx + 1], Message::Tool { .. }),
            "tail must not begin on a tool result"
        );
    }
}

#[cfg(test)]
mod decay_tests {
    use super::*;

    #[test]
    fn small_outputs_untouched() {
        let mut s = "short".to_string();
        elide_stale_tool_output(&mut s);
        assert_eq!(s, "short");
    }

    #[test]
    fn big_outputs_elided_with_marker() {
        // >8KB with a multibyte char straddling the head boundary.
        let mut s = "a".repeat(7000) + "é" + &"b".repeat(3000);
        let orig = s.len();
        elide_stale_tool_output(&mut s);
        // Head + tail + marker ≈ 8.3KB regardless of original size.
        assert!(s.len() < orig);
        assert!(s.len() <= STALE_HEAD + STALE_TAIL + 512);
        assert!(s.contains("tool output elided"));
        assert!(s.contains(&format!("was {orig} bytes")));
        assert!(s.starts_with("[tool output elided"));
        assert!(s.ends_with('b'));
    }

    #[test]
    fn multibyte_safe() {
        // All-emoji content: every char is 4 bytes — boundaries must hold.
        let mut s = "🦀".repeat(3000);
        elide_stale_tool_output(&mut s);
        assert!(s.is_char_boundary(s.len()));
        assert!(s.contains("elided"));
    }
}
