//! The session actor: owns conversation state, runs turns, emits events.
//!
//! Architecture (spec §5/§8): one actor task per session. Commands arrive
//! on an mpsc channel; a user message spawns a *turn task* that streams
//! from the provider while the actor stays responsive to `Cancel`.
//! Cancellation is `CancellationToken`-based so an in-flight HTTP stream
//! dies within the 250ms budget — the actor never awaits the stream
//! inline.
//!
//! M1 scope: text-only turns, no tool execution (tools land in M2). The
//! loop is already shaped for tool calls — [`Delta::ToolCall`]s are
//! collected into the turn report and surfaced as an error until the
//! tool registry exists.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use crate::event::{AgentError, Event};
use crate::persist::{PersistCtx, PersistedSession};
use crate::provider::{ChatRequest, Delta, Message, Provider, ProviderError, ThinkingLevel};
use crate::state::SessionState;
use crate::tools::WorkspaceTools;
use crate::types::{FailureReason, Outcome, SessionId, TokenUsage, ToolCall, TurnId, TurnOutcome};
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
    /// Workspace tool surface (M2 read tools). `None` = text-only turns —
    /// chat sessions and sessions without a workspace stay tool-free.
    pub tools: Option<Arc<WorkspaceTools>>,
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
    },
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

/// What a finished turn reports back to the actor. `tail` holds every
/// message the turn produced *after* the user message — assistant rounds
/// plus tool results — so history stays faithful even when a turn ran
/// several provider requests or ended early.
enum TurnReport {
    /// Turn finished (model stopped calling tools, or hit a cap).
    Completed {
        /// Messages appended after the user message.
        tail: Vec<Message>,
        /// Summed usage across all provider rounds.
        usage: TokenUsage,
        /// Tool calls executed during the turn.
        tool_calls: u32,
        /// Request latency, ms.
        latency_ms: u64,
        /// USD cost summed across priced rounds, when priced.
        cost_usd: Option<f64>,
    },
    /// Provider/stream failure; `tail` preserves completed tool rounds.
    Failed {
        /// The provider error.
        error: ProviderError,
        /// Completed-round messages worth keeping in history.
        tail: Vec<Message>,
    },
    /// Cancellation token fired; `tail` preserves completed rounds.
    Cancelled {
        /// Completed-round messages worth keeping in history.
        tail: Vec<Message>,
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
                                tail: Vec::new(),
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
            // M2/M3 fill these in; accepting-and-ignoring beats erroring
            // so older frontends don't break.
            Command::AnswerPrompt(_) | Command::RespondPermission { .. } => {
                warn!("prompt/permission response with nothing pending");
            }
            Command::SetOptions { model, thinking } => {
                if let Some(m) = model {
                    debug!(model = %m, "session model switched");
                    self.cfg.model = m;
                }
                if let Some(t) = thinking {
                    debug!(thinking = ?t, "session thinking level switched");
                    self.cfg.thinking = t;
                }
                // Option changes are session state — persist even when no
                // turn ever runs so a model pick survives a restart.
                self.persist_now();
            }
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
        let system = Some(hate_prompt::SYSTEM_PROMPT.to_string());
        let model = self.cfg.model.clone();
        let provider_name = self.cfg.provider_name.clone();
        let thinking = self.cfg.thinking;
        let tools = self.cfg.tools.clone();
        let ledger = self.cfg.ledger.clone();
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
                cancel,
                events,
                done_tx,
                ledger,
                session_id,
                turn_id,
                started,
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

    fn finish_turn(&mut self, rep: TurnReport) {
        // Whatever the outcome, the user message becomes part of history —
        // the model should see what the user actually said even if the
        // reply was cancelled or failed.
        if let Some(user_text) = self.pending_user.take() {
            self.history.push(Message::User { content: user_text });
        }

        match rep {
            TurnReport::Completed {
                tail,
                usage,
                tool_calls,
                latency_ms,
                cost_usd,
            } => {
                self.history.extend(tail);
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
            TurnReport::Failed { error, tail } => {
                // Completed tool rounds stay in history — the model did
                // real work before the failure and should see it.
                self.history.extend(tail);
                self.emit(Event::Failed {
                    reason: error.into(),
                });
            }
            TurnReport::Cancelled { tail } => {
                self.history.extend(tail);
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
        // Every turn outcome (completed/failed/cancelled) changes history
        // or counters — this is the "context always survives" write.
        self.persist_now();
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
    cancel: CancellationToken,
    events: broadcast::Sender<Event>,
    done_tx: mpsc::Sender<TurnReport>,
    ledger: Option<Arc<Ledger>>,
    session_id: SessionId,
    turn_id: TurnId,
    started: Instant,
) {
    // A model that never stops calling tools must not loop forever.
    const MAX_TOOL_ROUNDS: u32 = 12;
    const MAX_TOOL_CALLS: u32 = 48;

    // Messages this turn appends start after the user message.
    let base = convo.len();
    let specs = tools.as_ref().map(|t| t.specs()).unwrap_or_default();
    let mut total_usage = TokenUsage::default();
    let mut total_cost: Option<f64> = None;
    let mut tool_calls_done = 0u32;
    let mut round = 0u32;

    loop {
        if cancel.is_cancelled() {
            let _ = done_tx
                .send(TurnReport::Cancelled {
                    tail: convo[base..].to_vec(),
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
                    tail: convo[base..].to_vec(),
                })
                .await;
            return;
        }
        round += 1;

        let req = ChatRequest {
            model: model.clone(),
            system: system.clone(),
            messages: convo.clone(),
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
                        tail: convo[base..].to_vec(),
                    })
                    .await;
                return;
            }
            tokio::select! {
                () = cancel.cancelled() => {
                    let _ = done_tx
                        .send(TurnReport::Cancelled {
                            tail: convo[base..].to_vec(),
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
                                        tail: convo[base..].to_vec(),
                                    })
                                    .await;
                            } else {
                                let _ = done_tx
                                    .send(TurnReport::Failed {
                                        error: ProviderError::Stream(
                                            "provider stream ended without Delta::Done".to_string(),
                                        ),
                                        tail: convo[base..].to_vec(),
                                    })
                                    .await;
                            }
                            return;
                        }
                        Some(Err(e)) => {
                            let _ = done_tx
                                .send(TurnReport::Failed {
                                    error: e,
                                    tail: convo[base..].to_vec(),
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

        if calls.is_empty() {
            let _ = done_tx
                .send(TurnReport::Completed {
                    tail: convo[base..].to_vec(),
                    usage: total_usage,
                    tool_calls: tool_calls_done,
                    latency_ms: started.elapsed().as_millis() as u64,
                    cost_usd: total_cost,
                })
                .await;
            return;
        }

        let Some(tools) = tools.clone() else {
            let _ = done_tx
                .send(TurnReport::Failed {
                    error: ProviderError::Stream(format!(
                        "model requested {} tool call(s) but this session has no tools",
                        calls.len()
                    )),
                    tail: convo[base..].to_vec(),
                })
                .await;
            return;
        };

        if tool_calls_done > MAX_TOOL_CALLS {
            let _ = done_tx
                .send(TurnReport::Failed {
                    error: ProviderError::Stream(format!(
                        "tool call limit exceeded ({MAX_TOOL_CALLS})"
                    )),
                    tail: convo[base..].to_vec(),
                })
                .await;
            return;
        }

        // Record the assistant round, then run every call so the batch
        // stays atomic: history never holds a call without its result,
        // even if the user cancels immediately after.
        convo.push(Message::Assistant {
            content: text,
            tool_calls: calls.clone(),
        });
        for call in calls {
            emit(Event::ToolStarted { call: call.clone() });
            let out = tools.execute(&call).await;
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
