use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

use crate::event::{AgentError, Event};
use crate::index::build_repo_map;
use crate::mcp::{MCP_PREFIX, McpHub};
use crate::persist::{PersistCtx, PersistedSession};
use crate::provider::{
    ChatRequest, Delta, Message, Provider, ProviderError, ThinkingLevel, ToolSpec,
};
use crate::skills::{self, Skill, SkillStore, build_digest, parse_reflection, text_signals};
use crate::state::SessionState;
use crate::subagent::{review_large_diff, run_subagent};
use crate::terminal_bridge::TerminalToolkit;
use crate::tool_search::{ToolCandidate, search_tools};
use crate::tools::{ToolOutput, WorkspaceTools};
use crate::types::{
    CheckpointId, CompactionReason, FailureReason, Outcome, PermissionDecision, PermissionRequest,
    SessionId, TokenUsage, ToolCall, TurnId, TurnOutcome,
};
use crate::verify::{VerifyBaseline, capture_baseline, diff_new_failures, run_verify_checks};
use hate_context::O200kCounter;
use hate_ledger::{Ledger, RecordRequest};

const EVENT_BUFFER: usize = 2048;
const MAX_ACTIVE_TOOLS: usize = 16;
const MAX_DEFERRED_LOADS: u8 = 2;

#[derive(Debug, Clone, Default)]
struct DeferredState {
    loaded: Vec<ToolSpec>,
    loads: u8,
}

#[derive(Debug, Clone)]
pub struct Limits {
    /// Per-turn wall-clock budget; guards against a runaway turn, not the session.
    pub max_wall_clock: Duration,
    pub max_cost_usd: Option<f64>,
    pub max_tool_rounds: u32,
    pub max_tool_calls: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_wall_clock: Duration::from_hours(2),
            max_cost_usd: None,
            max_tool_rounds: 40,
            max_tool_calls: 120,
        }
    }
}

pub struct SessionConfig {
    pub provider: Arc<dyn Provider>,
    pub model: String,
    pub provider_name: String,
    pub thinking: ThinkingLevel,
    pub limits: Limits,
    pub ledger: Option<Arc<Ledger>>,
    pub persist: Option<PersistCtx>,
    pub tools: Option<Arc<WorkspaceTools>>,
    pub terminal: Option<Arc<TerminalToolkit>>,
    pub mcp: Option<Arc<McpHub>>,
    pub skills: Option<Arc<SkillStore>>,
    pub auto_skills: bool,
    pub permissions: Arc<PermissionHub>,
    pub context_window: u64,
}

pub enum Command {
    UserMessage {
        text: String,
        images: Vec<crate::ImagePart>,
    },
    Steer {
        text: String,
    },
    QueueUserMessage {
        text: String,
        images: Vec<crate::ImagePart>,
    },
    Cancel,
    AnswerPrompt(String),
    RespondPermission {
        request_id: String,
        decision: String,
    },
    SetOptions {
        model: Option<String>,
        thinking: Option<ThinkingLevel>,
        context_window: Option<u64>,
    },
    SetProvider {
        provider: Arc<dyn Provider>,
        provider_name: String,
        model: Option<String>,
    },
    Compact,
}

impl Command {
    pub fn user(text: impl Into<String>) -> Self {
        Self::UserMessage {
            text: text.into(),
            images: Vec::new(),
        }
    }

    pub fn user_with_images(text: impl Into<String>, images: Vec<crate::ImagePart>) -> Self {
        Self::UserMessage {
            text: text.into(),
            images,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub state: String,
    pub turn: u32,
    pub usage: TokenUsage,
    pub running: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session closed")]
    Closed,
    #[error("session busy")]
    Busy,
}

#[derive(Clone)]
pub struct SessionHandle {
    tx: mpsc::Sender<Command>,
    events: broadcast::Sender<Event>,
    snapshot: Arc<std::sync::RwLock<SessionSnapshot>>,
    session_id: SessionId,
}

impl SessionHandle {
    pub fn command(&self, cmd: Command) -> Result<(), SessionError> {
        self.tx.try_send(cmd).map_err(|e| match e {
            mpsc::error::TrySendError::Closed(_) => SessionError::Closed,
            mpsc::error::TrySendError::Full(_) => SessionError::Busy,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        self.snapshot.read().expect("snapshot poisoned").clone()
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn external() -> ExternalSession {
        let (tx, rx) = mpsc::channel::<Command>(64);
        let (events, _) = broadcast::channel::<Event>(EVENT_BUFFER);
        let snapshot = Arc::new(std::sync::RwLock::new(SessionSnapshot {
            state: "idle".to_string(),
            turn: 0,
            usage: TokenUsage::default(),
            running: false,
        }));
        let session_id = SessionId::new();
        ExternalSession {
            handle: SessionHandle {
                tx,
                events: events.clone(),
                snapshot: snapshot.clone(),
                session_id,
            },
            commands: rx,
            events,
            snapshot,
        }
    }
}

pub struct ExternalSession {
    pub handle: SessionHandle,
    pub commands: mpsc::Receiver<Command>,
    pub events: broadcast::Sender<Event>,
    pub snapshot: Arc<std::sync::RwLock<SessionSnapshot>>,
}

enum TurnReport {
    Completed {
        convo: Vec<Message>,
        usage: TokenUsage,
        tool_calls: u32,
        latency_ms: u64,
        cost_usd: Option<f64>,
        tool_full_tokens: u32,
        tool_sent_tokens: u32,
        cache_hit_ratio: f32,
        cache_break_reason: Option<String>,
    },
    Failed {
        error: ProviderError,
        convo: Vec<Message>,
    },
    Cancelled {
        convo: Vec<Message>,
    },
}

pub fn spawn_session(cfg: SessionConfig) -> SessionHandle {
    let (tx, rx) = mpsc::channel::<Command>(64);
    let (events, _) = broadcast::channel::<Event>(EVENT_BUFFER);
    let session_id = SessionId::new();
    let (history, turn, usage, cost_usd) =
        match cfg.persist.as_ref().and_then(|p| p.restore.as_ref()) {
            Some(r) => (r.messages.clone(), TurnId(r.turn), r.usage, r.cost_usd),
            None => (Vec::new(), TurnId(0), TokenUsage::default(), 0.0),
        };
    let mut history = history;
    for m in &mut history {
        if let Message::User { content, .. } = m
            && let Some(at) = content.find(LEGACY_PLAN_DIRECTIVE)
        {
            content.truncate(at);
        }
    }
    let snapshot = Arc::new(std::sync::RwLock::new(SessionSnapshot {
        state: "idle".to_string(),
        turn: turn.0,
        usage,
        running: false,
    }));

    let signals = cfg
        .tools
        .as_ref()
        .map_or_default(|t| skills::workspace_signals(t.root()));

    let repo_map_injected = !history.is_empty();
    let last_model = cfg.model.clone();
    let actor = Actor {
        cfg,
        rx,
        events: events.clone(),
        session_id,
        history,
        pending_user: None,
        state: SessionState::Idle,
        turn,
        turn_started: Instant::now(),
        cancel: CancellationToken::new(),
        turn_done: None,
        cost_usd,
        signals,
        reflection_attempted: false,
        plan_directive: None,
        plan_just_ended: false,
        repo_map_injected,
        last_model,
        steer_queue: Arc::new(Mutex::new(VecDeque::new())),
        queued_turns: VecDeque::new(),
        verify_baseline: Arc::new(Mutex::new(None)),
        deferred: Arc::new(Mutex::new(DeferredState::default())),
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

struct Actor {
    cfg: SessionConfig,
    rx: mpsc::Receiver<Command>,
    events: broadcast::Sender<Event>,
    session_id: SessionId,
    history: Vec<Message>,
    pending_user: Option<String>,
    state: SessionState,
    turn: TurnId,
    turn_started: Instant,
    cancel: CancellationToken,
    turn_done: Option<mpsc::Receiver<TurnReport>>,
    cost_usd: f64,
    signals: Vec<String>,
    reflection_attempted: bool,
    plan_directive: Option<&'static str>,
    plan_just_ended: bool,
    repo_map_injected: bool,
    last_model: String,
    steer_queue: Arc<Mutex<VecDeque<String>>>,
    queued_turns: VecDeque<(String, Vec<crate::ImagePart>)>,
    verify_baseline: Arc<Mutex<Option<VerifyBaseline>>>,
    deferred: Arc<Mutex<DeferredState>>,
    snapshot: Arc<std::sync::RwLock<SessionSnapshot>>,
}

impl Actor {
    #[instrument(skip_all, fields(session = %self.session_id.0))]
    async fn run(mut self) {
        info!("session started");
        loop {
            let report = match self.turn_done.as_mut() {
                Some(done) => {
                    tokio::select! {
                        cmd = self.rx.recv() => match cmd {
                            Some(c) => {
                                self.handle_command(c);
                                None
                            }
                            None => break,
                        },
                        rep = done.recv() => {
                            self.turn_done = None;
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
            Command::UserMessage { text, images } => self.begin_turn(text, images),
            Command::Steer { text } => {
                let trimmed = text.trim().to_string();
                if trimmed.is_empty() {
                    return;
                }
                if self.turn_done.is_some() {
                    self.steer_queue
                        .lock()
                        .expect("steer queue poisoned")
                        .push_back(trimmed);
                } else {
                    self.begin_turn(trimmed, Vec::new());
                }
            }
            Command::QueueUserMessage { text, images } => {
                if text.trim().is_empty() && images.is_empty() {
                    return;
                }
                if self.turn_done.is_some() {
                    self.queued_turns.push_back((text, images));
                } else {
                    self.begin_turn(text, images);
                }
            }
            Command::Cancel => {
                if self.turn_done.is_some() {
                    debug!("cancelling in-flight turn");
                    self.cancel.cancel();
                }
            }
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
                self.persist_now();
            }
            Command::SetProvider {
                provider,
                provider_name,
                model,
            } => {
                debug!(provider = %provider_name, "session provider switched");
                self.cfg.provider = provider;
                self.cfg.provider_name = provider_name;
                if let Some(m) = model {
                    self.cfg.model = m;
                }
                self.persist_now();
            }
            Command::Compact => self.compact_now(),
        }
    }

    fn compact_now(&mut self) {
        if self.turn_done.is_some() {
            self.emit(Event::Error {
                error: AgentError::Internal("can't compact while a turn is running".to_string()),
            });
            return;
        }
        let system = self.system_prompt();
        let deferred = self.deferred.lock().expect("deferred poisoned").clone();
        let specs = build_session_specs(
            self.cfg.tools.as_ref(),
            self.cfg.terminal.as_ref(),
            self.cfg.mcp.as_ref(),
            self.cfg.skills.as_ref(),
            &deferred,
        );
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

    fn system_prompt(&self) -> String {
        let mut out = hate_prompt::SYSTEM_PROMPT.to_string();
        if let Some(ctx) = self.cfg.tools.as_ref().and_then(|t| t.project_context()) {
            out.push_str("\n\nProject instructions (from the workspace):\n");
            out.push_str(&ctx);
        }
        if let Some(term) = self.cfg.terminal.as_ref()
            && term.has_open_terminals()
        {
            out.push_str("\n\n## Workspace Terminal\nInteractive workspace terminal tab(s) are open. Use `terminal_read` to inspect recent terminal output and `terminal_write` (user-confirmed) to send commands or input into an open tab.");
        }
        if let Some(store) = self.cfg.skills.as_ref() {
            out.push_str("\n\n");
            out.push_str(&store.render_prompt(&self.signals));
        }
        let _ = write!(
            out,
            "\n\n## Runtime\nYou are the model `{}`, served via the `{}` provider. If asked which \
             model you are, answer with exactly that. Earlier replies in this conversation may \
             have come from a different model — never repeat their self-descriptions.",
            self.cfg.model, self.cfg.provider_name
        );
        out
    }

    #[instrument(skip_all, fields(turn = self.turn.0 + 1))]
    fn begin_turn(&mut self, text: String, images: Vec<crate::ImagePart>) {
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
        let (text, plan_mode) = match parse_plan_command(&text) {
            Some(task) => {
                let directive = if self.cfg.tools.is_some() {
                    PLAN_DIRECTIVE_WORKSPACE
                } else {
                    PLAN_DIRECTIVE_CHAT
                };
                self.plan_directive = Some(directive);
                (format!("{task}\n\n{directive}"), true)
            }
            None if std::mem::take(&mut self.plan_just_ended) => {
                (format!("{text}\n\n{PLAN_OFF_NOTE}"), false)
            }
            None => (text, false),
        };
        let text = if self.repo_map_injected {
            text
        } else {
            self.repo_map_injected = true;
            if let Some(t) = self.cfg.tools.as_ref()
                && let Some(map) = build_repo_map(t.root(), 800)
            {
                format!("{map}\n\n{text}")
            } else {
                text
            }
        };
        self.pending_user = Some(text.clone());
        self.set_snapshot();

        let mut messages = self.history.clone();
        messages.push(Message::user_with_images(text, images));

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
        let terminal = self.cfg.terminal.clone();
        let mcp = self.cfg.mcp.clone();
        let skills = self.cfg.skills.clone();
        let permissions = self.cfg.permissions.clone();
        let ledger = self.cfg.ledger.clone();
        let context_window = self.cfg.context_window;
        let limits = self.cfg.limits.clone();
        let cost_before = self.cost_usd;
        let session_id = self.session_id;
        let turn_id = self.turn;
        let started = self.turn_started;
        let deferred = Arc::clone(&self.deferred);
        let verify_baseline = Arc::clone(&self.verify_baseline);
        let steer_queue = Arc::clone(&self.steer_queue);
        steer_queue.lock().expect("steer queue poisoned").clear();
        let model_changed = self.turn.0 > 1 && self.last_model != model;
        self.last_model.clone_from(&model);

        tokio::spawn(async move {
            run_turn(
                provider,
                messages,
                system,
                model,
                provider_name,
                thinking,
                tools,
                terminal,
                mcp,
                skills,
                permissions,
                cancel,
                events,
                done_tx,
                ledger,
                session_id,
                turn_id,
                started,
                context_window,
                limits,
                cost_before,
                plan_mode,
                model_changed,
                deferred,
                verify_baseline,
                steer_queue,
            )
            .await;
        });
    }

    fn check_limits(&self) -> Result<(), FailureReason> {
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
        let user_msg = self.pending_user.take().map(Message::user);
        let mut completed_cleanly = false;

        match rep {
            TurnReport::Completed {
                convo,
                usage,
                tool_calls,
                latency_ms,
                cost_usd,
                tool_full_tokens,
                tool_sent_tokens,
                cache_hit_ratio,
                cache_break_reason,
            } => {
                completed_cleanly = true;
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
                    tool_full_tokens,
                    tool_sent_tokens,
                    cache_hit_ratio,
                    cache_break_reason,
                };
                self.emit(Event::TurnComplete { outcome });
            }
            TurnReport::Failed { error, convo } => {
                self.queued_turns.clear();
                self.adopt_convo(convo, user_msg);
                self.emit(Event::Failed {
                    reason: error.into(),
                });
            }
            TurnReport::Cancelled { convo } => {
                self.queued_turns.clear();
                self.adopt_convo(convo, user_msg);
                let outcome = TurnOutcome {
                    outcome: Outcome::Cancelled,
                    usage: TokenUsage::default(),
                    tool_calls: 0,
                    elapsed_ms: self.turn_started.elapsed().as_millis() as u64,
                    cost_usd: None,
                    tool_full_tokens: 0,
                    tool_sent_tokens: 0,
                    cache_hit_ratio: 0.0,
                    cache_break_reason: None,
                };
                self.emit(Event::TurnComplete { outcome });
            }
        }
        if let Some(directive) = self.plan_directive.take() {
            strip_plan_directive(&mut self.history, directive);
            self.plan_just_ended = true;
        }
        self.state = SessionState::Idle;
        self.set_snapshot();
        self.persist_checkpoint();
        self.persist_now();
        self.maybe_reflect();
        if completed_cleanly && let Some((next_text, next_images)) = self.queued_turns.pop_front() {
            self.begin_turn(next_text, next_images);
        }
    }

    fn maybe_reflect(&mut self) {
        const MIN_TURNS: u32 = 3;
        if self.reflection_attempted || !self.cfg.auto_skills || self.turn.0 < MIN_TURNS {
            return;
        }
        let Some(store) = self.cfg.skills.clone() else {
            return;
        };
        let mut probe = self.signals.clone();
        for m in self.history.iter().rev().take(6) {
            if let Message::User { content, .. } = m {
                probe.extend(text_signals(content));
            }
        }
        if store.load().iter().any(|s| s.matches(&probe)) {
            return;
        }
        self.reflection_attempted = true;

        let digest = build_digest(&self.history, &self.signals);
        let provider = Arc::clone(&self.cfg.provider);
        let model = self.cfg.model.clone();
        let provider_name = self.cfg.provider_name.clone();
        let events = self.events.clone();
        let ledger = self.cfg.ledger.clone();
        let session_id = self.session_id;
        let turn_id = self.turn;
        tokio::spawn(async move {
            let req = ChatRequest {
                model: model.clone(),
                system: Some(skills::REFLECTION_SYSTEM.to_string()),
                messages: vec![Message::user(digest)],
                tools: Vec::new(),
                max_output_tokens: 2000,
                temperature: None,
                thinking: ThinkingLevel::Off,
            };
            let started = Instant::now();
            let mut stream = provider.stream(req, CancellationToken::new());
            let mut text = String::new();
            let mut usage = TokenUsage::default();
            while let Some(item) = stream.next().await {
                match item {
                    Ok(Delta::Text(t)) => text.push_str(&t),
                    Ok(Delta::Usage(u)) => usage = u,
                    Ok(Delta::Done) => break,
                    Err(e) => {
                        debug!(error = %e, "skill reflection call failed");
                        return;
                    }
                    _ => {}
                }
            }
            if let Some(ledger) = ledger {
                let rec = RecordRequest {
                    session_id: session_id.0.to_string(),
                    turn_id: turn_id.0,
                    request_id: Uuid::new_v4().to_string(),
                    model,
                    provider: provider_name,
                    input_tokens: usage.input,
                    cached_input_tokens: usage.cached_input,
                    output_tokens: usage.output,
                    thinking_tokens: usage.thinking,
                    latency_ms: started.elapsed().as_millis() as u64,
                };
                if let Err(e) = ledger.record_request(&rec) {
                    debug!(error = %e, "reflection ledger record failed");
                }
            }
            let Some((title, triggers, body)) = parse_reflection(&text) else {
                debug!("skill reflection produced no usable skill");
                return;
            };
            let skill = Skill {
                name: skills::slugify(&title),
                title,
                triggers,
                updated: skills::now_secs(),
                body,
            };
            if store.get(&skill.name).is_some() {
                debug!(name = %skill.name, "reflection skipped: skill exists");
                return;
            }
            match store.save(&skill) {
                Ok(saved) => {
                    let _ = events.send(Event::SkillSaved {
                        name: saved.name,
                        title: saved.title,
                        bytes: saved.body.len() as u32,
                    });
                }
                Err(e) => warn!(error = %e, "skill reflection save failed"),
            }
        });
    }

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

    fn persist_now(&self) {
        let Some(p) = self.cfg.persist.as_ref() else {
            return;
        };
        let title = self
            .history
            .iter()
            .find_map(|m| match m {
                Message::User { content, .. } => {
                    let line = content.lines().next().unwrap_or("").trim();
                    (!line.is_empty()).then(|| line.chars().take(80).collect::<String>())
                }
                _ => None,
            })
            .unwrap_or_else(|| "New chat".to_string());
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
            external: None,
        });
    }

    fn snapshot_usage(&self, u: TokenUsage) {
        let mut s = self.snapshot.write().expect("snapshot poisoned");
        s.usage.input += u.input;
        s.usage.cached_input += u.cached_input;
        s.usage.output += u.output;
        s.usage.thinking += u.thinking;
    }

    fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    fn set_snapshot(&self) {
        let mut s = self.snapshot.write().expect("snapshot poisoned");
        s.state = format!("{:?}", self.state);
        s.turn = self.turn.0;
        s.running = self.turn_done.is_some();
    }
}

struct PendingPermission {
    tx: oneshot::Sender<PermissionDecision>,
    key: String,
    rememberable: bool,
}

#[derive(Default)]
pub struct PermissionHub {
    pending: Mutex<HashMap<String, PendingPermission>>,
    allowed: Mutex<HashSet<String>>,
}

enum Gate {
    Run,
    Denied(String),
    Cancelled,
}

impl PermissionHub {
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

    fn is_rememberable(call: &ToolCall) -> bool {
        if call.name.as_str() == "bash"
            && let Some(cmd) = call.args.get("command").and_then(|v| v.as_str())
        {
            return crate::policy::classify_bash(cmd).class
                != crate::policy::CommandClass::Destructive;
        }
        true
    }

    fn is_allowed(&self, call: &ToolCall) -> bool {
        Self::is_rememberable(call)
            && self
                .allowed
                .lock()
                .expect("permission hub poisoned")
                .contains(&Self::key_for(call))
    }

    fn insert(
        &self,
        id: String,
        tx: oneshot::Sender<PermissionDecision>,
        key: String,
        rememberable: bool,
    ) {
        self.pending
            .lock()
            .expect("permission hub poisoned")
            .insert(
                id,
                PendingPermission {
                    tx,
                    key,
                    rememberable,
                },
            );
    }

    fn remove(&self, id: &str) {
        self.pending
            .lock()
            .expect("permission hub poisoned")
            .remove(id);
    }

    pub fn resolve(&self, id: &str, decision: PermissionDecision) {
        let entry = self
            .pending
            .lock()
            .expect("permission hub poisoned")
            .remove(id);
        let Some(PendingPermission {
            tx,
            key,
            rememberable,
        }) = entry
        else {
            return;
        };
        if decision == PermissionDecision::AllowSession && rememberable {
            self.allowed
                .lock()
                .expect("permission hub poisoned")
                .insert(key);
        }
        let _ = tx.send(decision);
    }
}

async fn gate_tool(
    hub: &Arc<PermissionHub>,
    cancel: &CancellationToken,
    events: &broadcast::Sender<Event>,
    call: &ToolCall,
    reason: String,
    detail: Option<String>,
    diff_preview: Option<String>,
) -> Gate {
    if hub.is_allowed(call) {
        return Gate::Run;
    }
    let id = Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    hub.insert(
        id.clone(),
        tx,
        PermissionHub::key_for(call),
        PermissionHub::is_rememberable(call),
    );
    let _ = events.send(Event::PermissionRequested {
        request: PermissionRequest {
            id: id.clone(),
            call: call.clone(),
            reason,
            detail,
            diff_preview,
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

enum ToolBackend<'a> {
    Ws(&'a Arc<WorkspaceTools>),
    Term(&'a Arc<TerminalToolkit>),
    Mcp(&'a Arc<McpHub>),
    Skill(&'a Arc<SkillStore>),
}

impl ToolBackend<'_> {
    fn gate_reason(&self, call: &ToolCall) -> Option<(String, Option<String>)> {
        match self {
            Self::Ws(t) => t.gate_reason(call),
            Self::Term(t) => t.gate_reason(call),
            Self::Mcp(h) => h.gate_reason(call),
            Self::Skill(_) => None,
        }
    }

    fn diff_preview(&self, call: &ToolCall) -> Option<String> {
        match self {
            Self::Ws(t) => t.diff_preview(call),
            _ => None,
        }
    }

    async fn execute(&self, call: &ToolCall, cancel: &CancellationToken) -> ToolOutput {
        match self {
            Self::Ws(t) => t.execute(call, cancel).await,
            Self::Term(t) => t.execute(call),
            Self::Mcp(h) => h.execute(call, cancel).await,
            Self::Skill(s) => s.execute(call),
        }
    }

    fn tool_error(&self, msg: String) -> ToolOutput {
        match self {
            Self::Ws(t) => t.tool_error(msg),
            Self::Term(t) => t.tool_error(msg),
            Self::Mcp(h) => h.tool_error(msg),
            Self::Skill(s) => s.tool_error(msg),
        }
    }
}

fn error_backend<'a>(
    mcp: Option<&'a Arc<McpHub>>,
    tools: Option<&'a Arc<WorkspaceTools>>,
    skills: Option<&'a Arc<SkillStore>>,
    terminal: Option<&'a Arc<TerminalToolkit>>,
) -> ToolBackend<'a> {
    mcp.map(ToolBackend::Mcp)
        .or_else(|| tools.map(ToolBackend::Ws))
        .or_else(|| skills.map(ToolBackend::Skill))
        .or_else(|| terminal.map(ToolBackend::Term))
        .expect("error_backend requires at least one tool backend")
}

fn build_session_specs(
    tools: Option<&Arc<WorkspaceTools>>,
    terminal: Option<&Arc<TerminalToolkit>>,
    mcp: Option<&Arc<McpHub>>,
    skills: Option<&Arc<SkillStore>>,
    deferred: &DeferredState,
) -> Vec<ToolSpec> {
    let mut specs = match tools {
        Some(t) => t.specs(),
        None if mcp.is_some_and(|h| !h.configs().is_empty() || !h.specs().is_empty()) => {
            vec![WorkspaceTools::tool_search_spec()]
        }
        None => Vec::new(),
    };
    if let Some(term) = terminal {
        specs.extend(term.specs());
    }
    if let Some(store) = skills {
        specs.extend(store.specs());
    }
    for s in &deferred.loaded {
        if !specs.iter().any(|e| e.name == s.name) {
            specs.push(s.clone());
        }
    }
    specs
}

#[allow(clippy::too_many_arguments)]
async fn run_turn(
    provider: Arc<dyn Provider>,
    mut convo: Vec<Message>,
    system: Option<String>,
    model: String,
    provider_name: String,
    thinking: ThinkingLevel,
    tools: Option<Arc<WorkspaceTools>>,
    terminal: Option<Arc<TerminalToolkit>>,
    mcp: Option<Arc<McpHub>>,
    skills: Option<Arc<SkillStore>>,
    permissions: Arc<PermissionHub>,
    cancel: CancellationToken,
    events: broadcast::Sender<Event>,
    done_tx: mpsc::Sender<TurnReport>,
    ledger: Option<Arc<Ledger>>,
    session_id: SessionId,
    turn_id: TurnId,
    started: Instant,
    context_window: u64,
    limits: Limits,
    cost_before: f64,
    plan_mode: bool,
    model_changed: bool,
    deferred_shared: Arc<Mutex<DeferredState>>,
    verify_baseline: Arc<Mutex<Option<VerifyBaseline>>>,
    steer_queue: Arc<Mutex<VecDeque<String>>>,
) {
    const MAX_VERIFY_ATTEMPTS: u8 = 3;
    const MAX_PARALLEL_SUBAGENTS: usize = 4;

    let max_tool_rounds = limits.max_tool_rounds;
    let max_tool_calls = limits.max_tool_calls;

    if let Some(t) = tools.as_ref() {
        t.reset_read_memo();
    }
    if let Some(hub) = mcp.as_ref() {
        hub.kick_refresh();
    }

    let mut deferred = deferred_shared.lock().expect("deferred poisoned").clone();
    let mut specs = build_session_specs(
        tools.as_ref(),
        terminal.as_ref(),
        mcp.as_ref(),
        skills.as_ref(),
        &deferred,
    );
    let mut total_usage = TokenUsage::default();
    let mut total_cost: Option<f64> = None;
    let mut tool_calls_done = 0u32;
    let mut tool_totals = (0u32, 0u32);
    let mut round = 0u32;
    let mut edited_this_turn = false;
    let mut verify_attempts = 0u8;
    let mut tool_prefix_changed = false;
    let mut last_cache_ratio = 0.0f32;
    let mut last_break_reason: Option<String> = None;

    loop {
        if cancel.is_cancelled() {
            let _ = done_tx
                .send(TurnReport::Cancelled {
                    convo: convo.clone(),
                })
                .await;
            return;
        }
        if round >= max_tool_rounds {
            let _ = done_tx
                .send(TurnReport::Failed {
                    error: ProviderError::Stream(format!(
                        "tool loop exceeded {max_tool_rounds} provider rounds"
                    )),
                    convo: convo.clone(),
                })
                .await;
            return;
        }
        if started.elapsed() > limits.max_wall_clock {
            let _ = done_tx
                .send(TurnReport::Failed {
                    error: ProviderError::Stream(format!(
                        "turn exceeded the {}s wall-clock limit",
                        limits.max_wall_clock.as_secs()
                    )),
                    convo: convo.clone(),
                })
                .await;
            return;
        }
        if let Some(cap) = limits.max_cost_usd
            && cost_before + total_cost.unwrap_or(0.0) >= cap
        {
            let _ = done_tx
                .send(TurnReport::Failed {
                    error: ProviderError::Stream(format!(
                        "limit exceeded: max_cost_usd = {cap:.2} (spent {:.2})",
                        cost_before + total_cost.unwrap_or(0.0)
                    )),
                    convo: convo.clone(),
                })
                .await;
            return;
        }
        round += 1;

        let mut compacted_this_round = false;
        if let Some((before, after, reason)) = maybe_compact(
            &mut convo,
            system.as_deref().unwrap_or(""),
            &specs,
            context_window,
        ) {
            compacted_this_round = true;
            if let Some(t) = tools.as_ref() {
                t.reset_read_memo();
            }
            let _ = events.send(Event::CompactionStarted {
                reason,
                before,
                after,
            });
        }

        {
            let mut sq = steer_queue.lock().expect("steer queue poisoned");
            while let Some(note) = sq.pop_front() {
                convo.push(Message::user(format!("[steering note from user]: {note}")));
            }
        }

        let mut req = ChatRequest {
            model: model.clone(),
            system: system.clone(),
            messages: decayed_view(&convo),
            tools: specs.clone(),
            max_output_tokens: 8192,
            temperature: None,
            thinking,
        };
        let redacted = crate::policy::redact_chat_request(
            &crate::policy::SecretRedactor::default(),
            &mut req.system,
            &mut req.messages,
        );
        if redacted > 0 {
            debug!(count = redacted, "redacted outbound secret(s)");
        }

        let round_started = Instant::now();
        let request_id = Uuid::new_v4().to_string();
        let stream = provider.stream(req, cancel.clone());
        tokio::pin!(stream);

        let mut text = String::new();
        let mut thinking_text = String::new();
        let mut calls: Vec<ToolCall> = Vec::new();
        let mut usage = TokenUsage::default();

        let emit = |ev: Event| {
            let _ = events.send(ev);
        };

        loop {
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
                            thinking_text.push_str(&t);
                            emit(Event::ThinkingDelta { text: t });
                        }
                        Some(Ok(Delta::ToolCall(call))) => {
                            calls.push(call);
                        }
                        Some(Ok(Delta::Usage(u))) => {
                            usage = u;
                            let thinking_est = if thinking_text.is_empty() {
                                0
                            } else {
                                O200kCounter::new()
                                    .map_or(0, |c| c.count_sync(&thinking_text) as u32)
                            };
                            let ratio = compute_cache_ratio(usage.input, usage.cached_input);
                            let break_reason = if (turn_id.0 > 1 || round > 1)
                                && usage.input >= 1024
                                && ratio < 0.5
                            {
                                if compacted_this_round {
                                    Some("compaction".to_string())
                                } else if tool_prefix_changed {
                                    Some("tool_prefix_changed".to_string())
                                } else if model_changed && round == 1 {
                                    Some("model_changed".to_string())
                                } else {
                                    Some("low_cache_hit".to_string())
                                }
                            } else {
                                None
                            };
                            last_cache_ratio = ratio;
                            if break_reason.is_some() {
                                last_break_reason.clone_from(&break_reason);
                            }
                            tool_prefix_changed = false;
                            emit(Event::TokensCounted {
                                usage,
                                turn: turn_id,
                                thinking_est,
                                cache_hit_ratio: ratio,
                                cache_break_reason: break_reason,
                            });
                        }
                        Some(Ok(Delta::Done)) => break,
                        None => {
                            if cancel.is_cancelled() {
                                let _ = done_tx
                                    .send(TurnReport::Cancelled {
                                        convo: convo.clone(),
                                    })
                                    .await;
                                return;
                            }
                            // A gateway that closed the socket after the last
                            // chunk (no `Done`) still gave us a usable answer.
                            // Only a stream that produced nothing is a failure.
                            if !text.is_empty()
                                || !thinking_text.is_empty()
                                || !calls.is_empty()
                            {
                                warn!("provider stream ended without Delta::Done");
                                break;
                            }
                            let _ = done_tx
                                .send(TurnReport::Failed {
                                    error: ProviderError::Stream(
                                        "provider stream ended without Delta::Done".to_string(),
                                    ),
                                    convo: convo.clone(),
                                })
                                .await;
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

        let round_latency = round_started.elapsed().as_millis() as u64;

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

        convo.push(Message::Assistant {
            content: text,
            tool_calls: calls.clone(),
        });

        if calls.is_empty() {
            if edited_this_turn
                && !plan_mode
                && verify_attempts < MAX_VERIFY_ATTEMPTS
                && let Some(t) = tools.as_ref()
            {
                let baseline = verify_baseline
                    .lock()
                    .expect("verify baseline poisoned")
                    .clone();
                if let Some(base) = baseline
                    && !base.checks.is_empty()
                {
                    let current = run_verify_checks(t.root(), &base.checks, &cancel).await;
                    if let Some(failure_msg) = diff_new_failures(&base, current) {
                        verify_attempts += 1;
                        convo.push(Message::user(failure_msg));
                        continue;
                    }
                }
            }

            if edited_this_turn
                && !plan_mode
                && let Some(t) = tools.as_ref()
                && t.root().join(".git").exists()
                && let Ok(diff) = crate::view::diff_all(t.root(), false)
            {
                let changed_lines = diff
                    .lines()
                    .filter(|l| {
                        (l.starts_with('+') && !l.starts_with("+++"))
                            || (l.starts_with('-') && !l.starts_with("---"))
                    })
                    .count();
                if changed_lines > 200
                    && let Some(verdict) =
                        review_large_diff(&provider, &model, &diff, &cancel).await
                {
                    emit(Event::Delta {
                        text: format!("\n\n[diff review]\n{verdict}"),
                    });
                }
            }

            let _ = done_tx
                .send(TurnReport::Completed {
                    convo: convo.clone(),
                    usage: total_usage,
                    tool_calls: tool_calls_done,
                    latency_ms: started.elapsed().as_millis() as u64,
                    cost_usd: total_cost,
                    tool_full_tokens: tool_totals.0,
                    tool_sent_tokens: tool_totals.1,
                    cache_hit_ratio: last_cache_ratio,
                    cache_break_reason: last_break_reason,
                })
                .await;
            return;
        }

        if tools.is_none() && terminal.is_none() && mcp.is_none() && skills.is_none() {
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

        if tool_calls_done > max_tool_calls {
            let _ = done_tx
                .send(TurnReport::Failed {
                    error: ProviderError::Stream(format!(
                        "tool call limit exceeded ({max_tool_calls})"
                    )),
                    convo: convo.clone(),
                })
                .await;
            return;
        }

        if calls.len() > 1
            && let Some(t) = tools.as_ref()
            && calls
                .iter()
                .all(|c| WorkspaceTools::is_read_only(c.name.as_str()))
        {
            for call in &calls {
                emit(Event::ToolStarted { call: call.clone() });
            }
            let outs =
                futures_util::future::join_all(calls.iter().map(|c| t.execute(c, &cancel))).await;
            for (call, out) in calls.into_iter().zip(outs) {
                record_tool_result(&events, &mut convo, &call, out, &mut tool_totals);
            }
            continue;
        }

        if calls.iter().all(|c| c.name.as_str() == "spawn_agent")
            && let Some(t) = tools.as_ref()
        {
            for call in &calls {
                emit(Event::ToolStarted { call: call.clone() });
            }
            let batch_count = calls.len().min(MAX_PARALLEL_SUBAGENTS);
            let reports = futures_util::future::join_all(
                calls[..batch_count]
                    .iter()
                    .map(|c| run_subagent(&provider, &model, t, c, 0, &cancel)),
            )
            .await;
            for (idx, call) in calls.into_iter().enumerate() {
                let out = if idx < batch_count {
                    subagent_tool_output(&reports[idx])
                } else {
                    t.tool_error(format!(
                        "spawn_agent: max {MAX_PARALLEL_SUBAGENTS} parallel subagents per round"
                    ))
                };
                record_tool_result(&events, &mut convo, &call, out, &mut tool_totals);
            }
            continue;
        }

        for call in calls {
            emit(Event::ToolStarted { call: call.clone() });
            if plan_mode && !plan_allows(call.name.as_str()) {
                let out = error_backend(
                    mcp.as_ref(),
                    tools.as_ref(),
                    skills.as_ref(),
                    terminal.as_ref(),
                )
                .tool_error(format!(
                    "plan mode is read-only — `{}` is blocked this turn. Finish the plan; \
                         the user approves it before anything changes.",
                    call.name.as_str()
                ));
                record_tool_result(&events, &mut convo, &call, out, &mut tool_totals);
                continue;
            }
            if call.name.as_str() == "tool_search" {
                let before_len = specs.len();
                let out = run_tool_search(
                    &call,
                    &mut specs,
                    &mut deferred,
                    tools.as_ref(),
                    mcp.as_ref(),
                )
                .await;
                if specs.len() > before_len {
                    tool_prefix_changed = true;
                }
                *deferred_shared.lock().expect("deferred poisoned") = deferred.clone();
                record_tool_result(&events, &mut convo, &call, out, &mut tool_totals);
                continue;
            }
            if call.name.as_str() == "spawn_agent" {
                let out = match tools.as_ref() {
                    Some(t) => {
                        let rep = run_subagent(&provider, &model, t, &call, 0, &cancel).await;
                        subagent_tool_output(&rep)
                    }
                    None => error_backend(
                        mcp.as_ref(),
                        tools.as_ref(),
                        skills.as_ref(),
                        terminal.as_ref(),
                    )
                    .tool_error("spawn_agent requires an attached workspace".to_string()),
                };
                record_tool_result(&events, &mut convo, &call, out, &mut tool_totals);
                continue;
            }
            let is_file_mutation = matches!(
                call.name.as_str(),
                "write_file" | "edit_file" | "apply_patch"
            );
            let backend = if call.name.as_str().starts_with(MCP_PREFIX) {
                mcp.as_ref().map(ToolBackend::Mcp)
            } else if matches!(call.name.as_str(), "skill_view" | "skill_save") {
                skills.as_ref().map(ToolBackend::Skill)
            } else if matches!(call.name.as_str(), "terminal_read" | "terminal_write") {
                terminal.as_ref().map(ToolBackend::Term)
            } else {
                tools.as_ref().map(ToolBackend::Ws)
            };
            let out = match backend {
                Some(b) => match b.gate_reason(&call) {
                    Some((reason, detail)) => {
                        let diff_preview = b.diff_preview(&call);
                        match gate_tool(
                            &permissions,
                            &cancel,
                            &events,
                            &call,
                            reason,
                            detail,
                            diff_preview,
                        )
                        .await
                        {
                            Gate::Run => {
                                if is_file_mutation
                                    && verify_baseline
                                        .lock()
                                        .expect("verify baseline poisoned")
                                        .is_none()
                                    && let Some(t) = tools.as_ref()
                                {
                                    let base = capture_baseline(t.root(), &cancel).await;
                                    *verify_baseline.lock().expect("verify baseline poisoned") =
                                        Some(base);
                                }
                                b.execute(&call, &cancel).await
                            }
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
                None => error_backend(
                    mcp.as_ref(),
                    tools.as_ref(),
                    skills.as_ref(),
                    terminal.as_ref(),
                )
                .tool_error(format!(
                    "unknown tool `{}` for this session",
                    call.name.as_str()
                )),
            };
            if out.ok && is_file_mutation {
                edited_this_turn = true;
            }
            if out.ok
                && call.name.as_str() == "skill_save"
                && let Some(store) = skills.as_ref()
            {
                let name = call
                    .args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .or_else(|| call.args.get("title").and_then(|v| v.as_str()))
                    .map_or_default(skills::slugify);
                if let Some(skill) = store.get(&name) {
                    let _ = events.send(Event::SkillSaved {
                        name: skill.name,
                        title: skill.title,
                        bytes: skill.body.len() as u32,
                    });
                }
            }
            if call.name.as_str().starts_with(MCP_PREFIX)
                && let Some(t) = tools.as_ref()
            {
                t.note_mutation();
            }
            record_tool_result(&events, &mut convo, &call, out, &mut tool_totals);
        }
    }
}

const PLAN_DIRECTIVE_WORKSPACE: &str = "[plan mode — this message only] Do not change anything yet. \
Investigate with read-only tools only (read_file, list_dir, glob, grep, symbol) — edits, shell \
commands, and other tools are blocked for this message. Then reply with a short numbered plan: one \
concrete step per line (which files, what changes, how it will be verified). Stop after the plan \
and wait for approval.";

const PLAN_DIRECTIVE_CHAT: &str = "[plan mode — this message only] Do not act yet and do not call \
tools that change anything. Reply with a short numbered plan: one concrete step per line. Stop \
after the plan and wait for approval.";

const LEGACY_PLAN_DIRECTIVE: &str = "\n\n[plan mode] Do not change anything yet.";

const PLAN_OFF_NOTE: &str = "[plan mode is off — respond to this message normally; edits and \
commands are allowed again]";

fn strip_plan_directive(history: &mut [Message], directive: &str) {
    for m in history.iter_mut().rev() {
        if let Message::User { content, .. } = m
            && let Some(task) = content.strip_suffix(directive)
        {
            let keep = task.trim_end().len();
            content.truncate(keep);
            return;
        }
    }
}

fn parse_plan_command(text: &str) -> Option<&str> {
    let rest = text.trim_start().strip_prefix("/plan")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let task = rest.trim();
    Some(if task.is_empty() {
        "Plan the next step of the task we've been discussing."
    } else {
        task
    })
}

async fn run_tool_search(
    call: &ToolCall,
    specs: &mut Vec<ToolSpec>,
    deferred: &mut DeferredState,
    tools: Option<&Arc<WorkspaceTools>>,
    mcp: Option<&Arc<McpHub>>,
) -> ToolOutput {
    let query = call
        .args
        .get("query")
        .and_then(|v| v.as_str())
        .map_or("", str::trim);
    let counter = O200kCounter::new().ok();
    let make_out = |text: String, ok: bool| -> ToolOutput {
        let content = if ok { text } else { format!("error: {text}") };
        let preview = content.chars().take(400).collect();
        let tokens = counter.as_ref().map_or((content.len() / 4) as u32, |c| {
            c.count_sync(&content) as u32
        });
        ToolOutput {
            content,
            preview,
            raw_preview: None,
            full_tokens: tokens,
            sent_tokens: tokens,
            ok,
        }
    };
    if query.is_empty() {
        return make_out("tool_search: missing `query`".to_string(), false);
    }
    let limit = call
        .args
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(3)
        .clamp(1, 3) as usize;

    let mut candidates: Vec<ToolCandidate> = Vec::new();
    if tools.is_some() {
        for s in WorkspaceTools::deferred_specs() {
            candidates.push(ToolCandidate::from_spec(s, ""));
        }
    }
    if let Some(hub) = mcp {
        candidates.extend(hub.catalog_for_search(query).await);
    }

    let ranked = search_tools(query, 8, &candidates);
    if ranked.is_empty() {
        return make_out(format!("no deferred tools matched {query:?}"), true);
    }

    let already: Vec<String> = ranked
        .iter()
        .filter(|s| specs.iter().any(|e| e.name == s.name))
        .map(|s| s.name.as_str().to_string())
        .collect();
    let fresh: Vec<ToolSpec> = ranked
        .into_iter()
        .filter(|s| !specs.iter().any(|e| e.name == s.name))
        .collect();

    if fresh.is_empty() {
        return make_out(
            format!("already loaded matching tool(s): {}", already.join(", ")),
            true,
        );
    }

    let room = MAX_ACTIVE_TOOLS.saturating_sub(specs.len());
    if deferred.loads >= MAX_DEFERRED_LOADS || room == 0 {
        let skipped: Vec<&str> = fresh.iter().map(|s| s.name.as_str()).collect();
        return make_out(
            format!(
                "deferred tool budget reached (max {MAX_DEFERRED_LOADS} loads / {MAX_ACTIVE_TOOLS} tools); unmatched loads skipped: {}",
                skipped.join(", ")
            ),
            true,
        );
    }

    let mut newly_loaded = Vec::new();
    for mut spec in fresh.into_iter().take(limit.min(room)) {
        if spec.name.as_str().starts_with(MCP_PREFIX)
            && let Some(hub) = mcp
            && let Some(live_spec) = hub.spec_for_tool(spec.name.as_str()).await
        {
            spec = live_spec;
        }
        if !specs.iter().any(|e| e.name == spec.name) {
            specs.push(spec.clone());
            deferred.loaded.push(spec.clone());
            newly_loaded.push(spec);
        }
    }
    if !newly_loaded.is_empty() {
        deferred.loads = deferred.loads.saturating_add(1);
    }

    let mut out = format!("loaded {} tool(s):", newly_loaded.len());
    for s in &newly_loaded {
        let _ = write!(out, "\n- {}: {}", s.name.as_str(), s.description);
    }
    if !already.is_empty() {
        let _ = write!(out, "\nalready loaded: {}", already.join(", "));
    }
    make_out(out, true)
}

fn subagent_tool_output(rep: &crate::subagent::SubagentReport) -> ToolOutput {
    let content = rep.format_for_parent();
    let preview = content.chars().take(400).collect();
    let tokens = O200kCounter::new().map_or((content.len() / 4) as u32, |c| {
        c.count_sync(&content) as u32
    });
    ToolOutput {
        content,
        preview,
        raw_preview: None,
        full_tokens: tokens,
        sent_tokens: tokens,
        ok: rep.ok,
    }
}

fn plan_allows(name: &str) -> bool {
    WorkspaceTools::is_read_only(name)
        || name == "tool_search"
        || name == "spawn_agent"
        || name == "skill_view"
        || TerminalToolkit::is_read_only(name)
}

#[allow(clippy::cast_precision_loss)]
fn compute_cache_ratio(input: u64, cached: u64) -> f32 {
    if input == 0 {
        0.0
    } else {
        (cached as f32 / input as f32).clamp(0.0, 1.0)
    }
}

fn record_tool_result(
    events: &broadcast::Sender<Event>,
    convo: &mut Vec<Message>,
    call: &ToolCall,
    out: ToolOutput,
    tool_totals: &mut (u32, u32),
) {
    tool_totals.0 = tool_totals.0.saturating_add(out.full_tokens);
    tool_totals.1 = tool_totals.1.saturating_add(out.sent_tokens);
    let sent_preview = Some(out.content.chars().take(2000).collect());
    let _ = events.send(Event::ToolOutput {
        call_id: call.id.clone(),
        preview: out.preview,
        tokens: out.sent_tokens,
    });
    let _ = events.send(Event::ToolFinished {
        call_id: call.id.clone(),
        ok: out.ok,
        full_tokens: out.full_tokens,
        sent_tokens: out.sent_tokens,
        full_preview: out.raw_preview,
        sent_preview,
    });
    convo.push(Message::Tool {
        call_id: call.id.clone(),
        name: call.name.as_str().to_string(),
        content: out.content,
    });
}

const COMPACT_SOFT: f64 = 0.72;
const COMPACT_HARD: f64 = 0.92;
const COMPACT_TAIL: usize = 12;
const NOTE_FILES_MAX: usize = 15;
const NOTE_SNIPPET: usize = 600;

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

fn prompt_bytes(system: &str, tools: &[crate::provider::ToolSpec], messages: &[Message]) -> usize {
    let tool_bytes = serde_json::to_string(tools).map_or(0, |s| s.len());
    let msg_bytes: usize = messages
        .iter()
        .map(|m| match m {
            Message::User { content, .. } => content.len() + 8,
            Message::Assistant {
                content,
                tool_calls,
            } => {
                content.len()
                    + 16
                    + tool_calls
                        .iter()
                        .map(|c| {
                            c.name.as_str().len()
                                + serde_json::to_string(&c.args).map_or(0, |s| s.len())
                                + 1
                        })
                        .sum::<usize>()
            }
            Message::Tool { name, content, .. } => name.len() + content.len() + 8,
        })
        .sum();
    system.len() + tool_bytes + msg_bytes
}

fn push_message_text(m: &Message, out: &mut String) {
    match m {
        Message::User { content, .. } => {
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

fn maybe_compact(
    convo: &mut Vec<Message>,
    system: &str,
    tools: &[crate::provider::ToolSpec],
    window: u64,
) -> Option<(u64, u64, CompactionReason)> {
    if window == 0 || convo.len() < COMPACT_TAIL + 4 {
        return None;
    }
    if (prompt_bytes(system, tools, convo) as f64) < COMPACT_SOFT * window as f64 {
        return None;
    }
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

fn compact_core(convo: &mut Vec<Message>) -> bool {
    let n = convo.len();
    let anchor = usize::from(matches!(convo.first(), Some(Message::User { .. })));
    let mut tail = n.saturating_sub(COMPACT_TAIL).max(anchor);
    while tail < n && matches!(convo[tail], Message::Tool { .. }) {
        tail += 1;
    }
    if tail <= anchor + 2 {
        return false;
    }

    let dropped: Vec<Message> = convo[anchor..tail].to_vec();
    let note = compaction_note(&dropped, anchor > 0);
    let mut next = Vec::with_capacity(anchor + 1 + (n - tail));
    next.extend(convo[..anchor].iter().cloned());
    next.push(Message::user(note));
    next.extend(convo[tail..].iter().cloned());
    *convo = next;
    true
}

fn compaction_note(dropped: &[Message], has_anchor: bool) -> String {
    use std::fmt::Write as _;
    let mut note = String::from(
        "[Context compacted]\nEarlier turns were compacted to fit the model's context window. \
         The visible transcript still shows the full history.\n",
    );
    if !has_anchor
        && let Some(first) = dropped.iter().find_map(|m| match m {
            Message::User { content, .. } if !content.starts_with('[') => Some(content.as_str()),
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

const STALE_TOOL_CAP: usize = 8 * 1024;
const STALE_HEAD: usize = 6 * 1024;
const STALE_TAIL: usize = 2 * 1024;
const DECAY_KEEP_TAIL: usize = 16;
const ELISION_MARKER: &str = "[tool output elided";

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
        Message::user(text.to_string())
    }

    fn assistant(text: &str) -> Message {
        Message::Assistant {
            content: text.to_string(),
            tool_calls: Vec::new(),
        }
    }

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
    fn plan_command_parses_only_the_exact_prefix() {
        assert_eq!(parse_plan_command("/plan add a cache"), Some("add a cache"));
        assert_eq!(
            parse_plan_command("  /plan\nmulti\nline"),
            Some("multi\nline")
        );
        assert!(parse_plan_command("/plan").is_some_and(|t| t.starts_with("Plan the next")));
        assert_eq!(parse_plan_command("/planet earth"), None);
        assert_eq!(parse_plan_command("please /plan this"), None);
        assert!(plan_allows("grep") && plan_allows("symbol") && plan_allows("skill_view"));
        assert!(!plan_allows("bash") && !plan_allows("edit_file") && !plan_allows("mcp__x__y"));
        assert!(!plan_allows("skill_save"));
    }

    #[test]
    fn plan_directive_is_stripped_from_the_turn_that_carried_it() {
        let d = PLAN_DIRECTIVE_WORKSPACE;
        let mut h = vec![
            user("older message"),
            user(&format!("rename the thing\n\n{d}")),
            assistant("1. do it"),
        ];
        strip_plan_directive(&mut h, d);
        assert!(matches!(&h[1], Message::User { content, .. } if content == "rename the thing"));
        assert!(matches!(&h[0], Message::User { content, .. } if content == "older message"));
        strip_plan_directive(&mut h, d);
        assert_eq!(h.len(), 3);
    }

    #[test]
    fn under_threshold_is_noop() {
        let mut convo = fat_convo();
        let n = convo.len();
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
        let r = maybe_compact(&mut convo, "sys", &[], 2_000).expect("should compact");
        assert!(matches!(
            r.2,
            CompactionReason::Threshold | CompactionReason::HardStop
        ));
        assert!(r.0 > r.1, "before={} after={}", r.0, r.1);

        assert_eq!(convo.len(), 1 + 1 + COMPACT_TAIL);
        assert!(matches!(&convo[0], Message::User { content, .. } if content.contains("THE TASK")));
        assert!(
            matches!(&convo[1], Message::User { content, .. } if content.contains("[Context compacted]"))
        );
        assert!(
            matches!(&convo[2], Message::User { .. })
                || matches!(&convo[2], Message::Assistant { .. })
        );
        assert!(matches!(
            convo.last(),
            Some(Message::Assistant { content, .. }) if content == "final answer pending"
        ));
        if let Message::User { content, .. } = &convo[1] {
            assert!(content.contains("earlier messages compacted"));
        }
    }

    #[test]
    fn force_compact_ignores_threshold() {
        let mut convo = fat_convo();
        let n = convo.len();
        let (before, after) = force_compact(&mut convo, "sys", &[]).expect("manual compacts");
        assert!(before > after, "before={before} after={after}");
        assert!(convo.len() < n);
        assert!(
            matches!(&convo[1], Message::User { content, .. } if content.contains("[Context compacted]"))
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
        while convo.len() < COMPACT_TAIL + 5 {
            convo.push(assistant("pad"));
        }
        let r = maybe_compact(&mut convo, "sys", &[], 500);
        assert!(r.is_some());
        let note_idx = convo
            .iter()
            .position(|m| matches!(m, Message::User { content, .. } if content.contains("[Context compacted]")))
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
        let mut s = "a".repeat(7000) + "é" + &"b".repeat(3000);
        let orig = s.len();
        elide_stale_tool_output(&mut s);
        assert!(s.len() < orig);
        assert!(s.len() <= STALE_HEAD + STALE_TAIL + 512);
        assert!(s.contains("tool output elided"));
        assert!(s.contains(&format!("was {orig} bytes")));
        assert!(s.starts_with("[tool output elided"));
        assert!(s.ends_with('b'));
    }

    #[test]
    fn multibyte_safe() {
        let mut s = "🦀".repeat(3000);
        elide_stale_tool_output(&mut s);
        assert!(s.is_char_boundary(s.len()));
        assert!(s.contains("elided"));
    }
}
