use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use hate_core::{
    AgentError, Command, Event, ExternalRef, ExternalSession, FailureReason, Message, Outcome,
    PermissionRequest, PersistCtx, PersistedSession, SessionHandle, SessionSnapshot, ThinkingLevel,
    TokenUsage, ToolCall, ToolName, TurnOutcome,
};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::agent::{self, Agent};
use crate::rpc::{Connection, Incoming, RpcError};
use crate::runtime::{AGENT_ID, Installed};

const SEED_CHARS: usize = 12_000;
const PREVIEW_CHARS: usize = 4_000;
pub const SIGN_IN_HINT: &str =
    "Sign in with Google first: Settings → Providers → Antigravity → Sign in with Google.";

pub struct AcpSessionConfig {
    pub installed: Option<Installed>,
    pub cwd: PathBuf,
    pub model: String,
    pub provider_name: String,
    pub mcp_servers: Value,
    pub persist: Option<PersistCtx>,
}

pub fn spawn_acp_session(cfg: AcpSessionConfig) -> SessionHandle {
    let ext = SessionHandle::external();
    let handle = ext.handle.clone();
    tokio::spawn(Actor::new(cfg, ext, None).run());
    handle
}

pub fn spawn_with_connection(
    cfg: AcpSessionConfig,
    conn: Arc<Connection>,
    incoming: mpsc::UnboundedReceiver<Incoming>,
) -> SessionHandle {
    let ext = SessionHandle::external();
    let handle = ext.handle.clone();
    tokio::spawn(Actor::new(cfg, ext, Some((conn, incoming))).run());
    handle
}

struct Turn {
    started: Instant,
    done: oneshot::Receiver<Result<Value, RpcError>>,
    assistant: String,
    tool_calls: u32,
    cancelled: bool,
}

struct ToolInfo {
    name: String,
    args: Value,
}

struct PendingPermission {
    rpc_id: Value,
    options: Vec<Value>,
}

struct Actor {
    cfg: AcpSessionConfig,
    commands: mpsc::Receiver<Command>,
    events: broadcast::Sender<Event>,
    snapshot: Arc<std::sync::RwLock<SessionSnapshot>>,
    agent: Option<Agent>,
    injected: Option<(Arc<Connection>, mpsc::UnboundedReceiver<Incoming>)>,
    incoming: Option<mpsc::UnboundedReceiver<Incoming>>,
    session: Option<String>,
    config_options: Value,
    external: Option<ExternalRef>,
    history: Vec<Message>,
    seed_pending: bool,
    turn: Option<Turn>,
    turn_no: u32,
    tools: HashMap<String, ToolInfo>,
    finished_tools: HashSet<String>,
    permissions: HashMap<String, PendingPermission>,
    perm_seq: u64,
    cost_usd: f64,
}

impl Actor {
    fn new(
        mut cfg: AcpSessionConfig,
        ext: ExternalSession,
        injected: Option<(Arc<Connection>, mpsc::UnboundedReceiver<Incoming>)>,
    ) -> Self {
        let restore = cfg.persist.as_mut().and_then(|p| p.restore.take());
        let (history, external, cost_usd, turn_no) = match restore {
            Some(r) => (r.messages, r.external, r.cost_usd, r.turn),
            None => (Vec::new(), None, 0.0, 0),
        };
        let external = external.filter(|e| e.agent == AGENT_ID);
        let seed_pending = external.is_none() && !history.is_empty();
        Self {
            cfg,
            commands: ext.commands,
            events: ext.events,
            snapshot: ext.snapshot,
            agent: None,
            injected,
            incoming: None,
            session: None,
            config_options: Value::Null,
            external,
            history,
            seed_pending,
            turn: None,
            turn_no,
            tools: HashMap::new(),
            finished_tools: HashSet::new(),
            permissions: HashMap::new(),
            perm_seq: 0,
            cost_usd,
        }
    }

    async fn run(mut self) {
        loop {
            tokio::select! {
                cmd = self.commands.recv() => match cmd {
                    Some(c) => self.on_command(c).await,
                    None => break,
                },
                msg = recv_opt(&mut self.incoming) => match msg {
                    Some(m) => self.on_incoming(m),
                    None => self.on_agent_gone(),
                },
                res = wait_turn(&mut self.turn) => self.finish_turn(res),
            }
        }
        if let Some(a) = self.agent.take() {
            a.shutdown().await;
        }
    }

    fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    fn set_state(&self, state: &str, running: bool) {
        if let Ok(mut s) = self.snapshot.write() {
            s.state = state.to_string();
            s.running = running;
            s.turn = self.turn_no;
        }
    }

    fn fail(&self, message: String) {
        self.set_state("failed", false);
        self.emit(Event::Failed {
            reason: FailureReason::Provider { message },
        });
    }

    async fn on_command(&mut self, cmd: Command) {
        match cmd {
            Command::UserMessage { text, images } => self.start_turn(text, images).await,
            Command::Cancel => self.cancel(),
            Command::RespondPermission {
                request_id,
                decision,
            } => self.respond_permission(&request_id, &decision),
            Command::SetOptions { model, .. } => {
                if let Some(m) = model.filter(|m| !m.is_empty() && *m != self.cfg.model) {
                    self.cfg.model = m;
                    if self.session.is_some()
                        && let Err(e) = self.apply_model().await
                    {
                        self.emit(Event::Error {
                            error: AgentError::Provider(format!("model switch failed: {e}")),
                        });
                    }
                    self.persist();
                }
            }
            Command::SetProvider { .. } => self.emit(Event::Error {
                error: AgentError::Internal(
                    "switching away from Antigravity starts a new session backend".to_string(),
                ),
            }),
            Command::Compact => self.emit(Event::Error {
                error: AgentError::Internal(
                    "Antigravity manages its own context; /compact isn't available".to_string(),
                ),
            }),
            Command::AnswerPrompt(_) => {}
        }
    }

    async fn ensure_session(&mut self) -> Result<(), String> {
        if self.agent.is_some() && self.session.is_some() {
            return Ok(());
        }
        let agent = if let Some((conn, incoming)) = self.injected.take() {
            let mut a = Agent::from_connection(conn);
            self.incoming = Some(incoming);
            a.initialize().await.map_err(|e| e.detail())?;
            a
        } else {
            let started = Agent::start(self.cfg.installed.as_ref())
                .await
                .map_err(|e| e.to_string())?;
            self.incoming = Some(started.incoming);
            drop(started.stderr);
            started.agent
        };
        let _ = std::fs::create_dir_all(&self.cfg.cwd);
        let cwd = self.cfg.cwd.to_string_lossy().to_string();
        let mcp = self.cfg.mcp_servers.clone();

        let mut resumed = None;
        if let Some(ext) = self.external.clone()
            && agent.can_resume()
        {
            match agent
                .resume_session(&ext.session_id, &cwd, mcp.clone())
                .await
            {
                Ok(opts) => resumed = Some((ext.session_id, opts)),
                Err(e) if e.is_auth_required() => return Err(auth_message(&e)),
                Err(e) => {
                    tracing::warn!(error = %e, "antigravity resume failed; starting fresh");
                    self.seed_pending = !self.history.is_empty();
                }
            }
        }
        let (sid, opts) = if let Some(r) = resumed {
            r
        } else {
            let s = agent.new_session(&cwd, mcp).await.map_err(|e| {
                if e.is_auth_required() {
                    auth_message(&e)
                } else {
                    format!("Antigravity couldn't start a session: {}", e.detail())
                }
            })?;
            (s.session_id, s.config_options)
        };
        self.external = Some(ExternalRef {
            agent: AGENT_ID.to_string(),
            session_id: sid.clone(),
        });
        self.session = Some(sid);
        self.config_options = opts;
        self.agent = Some(agent);
        if !self.cfg.model.is_empty() {
            self.apply_model().await.map_err(|e| e.detail())?;
        }
        Ok(())
    }

    async fn apply_model(&mut self) -> Result<(), RpcError> {
        let (Some(agent), Some(sid)) = (self.agent.as_ref(), self.session.as_ref()) else {
            return Ok(());
        };
        let (choices, current) = agent::models(&self.config_options);
        if current.as_deref() == Some(self.cfg.model.as_str()) {
            return Ok(());
        }
        if !choices.is_empty() && !choices.iter().any(|c| c.value == self.cfg.model) {
            return Err(RpcError {
                code: -32602,
                message: format!("{} isn't offered by Antigravity", self.cfg.model),
                data: None,
            });
        }
        let id =
            agent::model_config_id(&self.config_options).unwrap_or_else(|| "model".to_string());
        let opts = agent.set_config_option(sid, &id, &self.cfg.model).await?;
        if !opts.is_null() {
            self.config_options = opts;
        }
        Ok(())
    }

    async fn start_turn(&mut self, text: String, images: Vec<hate_core::ImagePart>) {
        if self.turn.is_some() {
            self.emit(Event::Error {
                error: AgentError::Internal(
                    "Antigravity is still working on the last message".into(),
                ),
            });
            return;
        }
        self.set_state("thinking", true);
        if let Err(e) = self.ensure_session().await {
            self.fail(e);
            return;
        }
        let (Some(agent), Some(sid)) = (self.agent.as_ref(), self.session.clone()) else {
            return;
        };
        let prompt_text = if self.seed_pending {
            self.seed_pending = false;
            seeded_prompt(&self.history, &text)
        } else {
            text.clone()
        };
        let mut prompt_parts = vec![json!({"type": "text", "text": prompt_text})];
        for img in &images {
            prompt_parts.push(json!({
                "type": "image",
                "mimeType": img.mime,
                "data": img.data,
            }));
        }
        self.history.push(Message::user_with_images(text, images));
        self.turn_no += 1;
        self.tools.clear();
        self.finished_tools.clear();

        let conn = Arc::clone(&agent.conn);
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let res = conn
                .request(
                    "session/prompt",
                    json!({"sessionId": sid, "prompt": prompt_parts}),
                )
                .await;
            let _ = tx.send(res);
        });
        self.turn = Some(Turn {
            started: Instant::now(),
            done: rx,
            assistant: String::new(),
            tool_calls: 0,
            cancelled: false,
        });
        self.set_state("streaming", true);
    }

    fn cancel(&mut self) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        turn.cancelled = true;
        if let (Some(agent), Some(sid)) = (self.agent.as_ref(), self.session.as_ref()) {
            agent.cancel(sid);
            for (_, p) in self.permissions.drain() {
                agent
                    .conn
                    .respond(p.rpc_id, Ok(json!({"outcome": {"outcome": "cancelled"}})));
            }
        }
    }

    fn finish_turn(&mut self, res: Result<Value, RpcError>) {
        let Some(turn) = self.turn.take() else { return };
        if !turn.assistant.is_empty() {
            self.history.push(Message::Assistant {
                content: turn.assistant.clone(),
                tool_calls: Vec::new(),
            });
        }
        for id in self.tools.keys() {
            if !self.finished_tools.contains(id) {
                self.emit(Event::ToolFinished {
                    call_id: id.clone(),
                    ok: false,
                    full_tokens: 0,
                    sent_tokens: 0,
                });
            }
        }
        self.permissions.clear();
        let elapsed_ms = u64::try_from(turn.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match res {
            Ok(v) => {
                let stop = v
                    .get("stopReason")
                    .and_then(Value::as_str)
                    .unwrap_or("end_turn");
                let outcome = if stop == "cancelled" || turn.cancelled {
                    Outcome::Cancelled
                } else {
                    Outcome::Completed
                };
                if let Some(note) = stop_note(stop) {
                    self.emit(Event::Error {
                        error: AgentError::Provider(note.to_string()),
                    });
                }
                self.persist();
                self.set_state("done", false);
                self.emit(Event::TurnComplete {
                    outcome: TurnOutcome {
                        outcome,
                        usage: TokenUsage::default(),
                        tool_calls: turn.tool_calls,
                        elapsed_ms,
                        cost_usd: (self.cost_usd > 0.0).then_some(self.cost_usd),
                    },
                });
            }
            Err(e) => {
                self.persist();
                if e == RpcError::closed() {
                    self.on_agent_gone();
                    return;
                }
                if turn.cancelled {
                    self.set_state("done", false);
                    self.emit(Event::TurnComplete {
                        outcome: TurnOutcome {
                            outcome: Outcome::Cancelled,
                            usage: TokenUsage::default(),
                            tool_calls: turn.tool_calls,
                            elapsed_ms,
                            cost_usd: None,
                        },
                    });
                    return;
                }
                let msg = if e.is_auth_required() {
                    auth_message(&e)
                } else {
                    format!("Antigravity: {}", e.detail())
                };
                self.fail(msg);
            }
        }
    }

    fn on_agent_gone(&mut self) {
        self.incoming = None;
        self.agent = None;
        self.session = None;
        self.permissions.clear();
        if let Some(turn) = self.turn.take() {
            if !turn.assistant.is_empty() {
                self.history.push(Message::Assistant {
                    content: turn.assistant,
                    tool_calls: Vec::new(),
                });
            }
            self.persist();
            self.fail("Antigravity stopped unexpectedly; send again to reconnect.".to_string());
        }
    }

    fn on_incoming(&mut self, msg: Incoming) {
        match msg {
            Incoming::Notification { method, params } if method == "session/update" => {
                if params.get("sessionId").and_then(Value::as_str) == self.session.as_deref() {
                    self.on_update(params.get("update").unwrap_or(&Value::Null));
                }
            }
            Incoming::Notification { .. } => {}
            Incoming::Request { id, method, params } => {
                if method == "session/request_permission" {
                    self.on_permission(id, &params);
                } else if let Some(agent) = self.agent.as_ref() {
                    agent.conn.respond(
                        id,
                        Err(RpcError {
                            code: -32601,
                            message: format!("{method} is not supported by hate-agent"),
                            data: None,
                        }),
                    );
                }
            }
        }
    }

    fn on_update(&mut self, u: &Value) {
        let kind = u.get("sessionUpdate").and_then(Value::as_str).unwrap_or("");
        match kind {
            "agent_message_chunk" => {
                if let Some(source) = image_source(u.get("content")) {
                    if self.turn.is_some() {
                        self.emit(Event::Image {
                            source,
                            call_id: None,
                        });
                    }
                    return;
                }
                let Some(text) = content_text(u.get("content")) else {
                    return;
                };
                if let Some(t) = self.turn.as_mut() {
                    t.assistant.push_str(&text);
                    self.emit(Event::Delta { text });
                }
            }
            "agent_thought_chunk" => {
                if self.turn.is_some()
                    && let Some(text) = content_text(u.get("content"))
                {
                    self.emit(Event::ThinkingDelta { text });
                }
            }
            "tool_call" => self.on_tool_call(u),
            "tool_call_update" => self.on_tool_update(u),
            "usage_update" => {
                let used = u.get("used").and_then(Value::as_u64);
                let size = u.get("size").and_then(Value::as_u64);
                if let (Some(used), Some(size)) = (used, size) {
                    self.emit(Event::ContextUsage { used, size });
                }
                if let Some(cost) = u.get("cost")
                    && cost.get("currency").and_then(Value::as_str) == Some("USD")
                    && let Some(amount) = cost.get("amount").and_then(Value::as_f64)
                {
                    self.cost_usd = amount;
                }
            }
            _ => {}
        }
    }

    fn on_tool_call(&mut self, u: &Value) {
        let Some(id) = u.get("toolCallId").and_then(Value::as_str) else {
            return;
        };
        if self.turn.is_none() {
            return;
        }
        let name = tool_name(u);
        let args = tool_args(u);
        if !self.tools.contains_key(id) {
            if let Some(t) = self.turn.as_mut() {
                t.tool_calls += 1;
            }
            self.emit(Event::ToolStarted {
                call: ToolCall {
                    id: id.to_string(),
                    name: ToolName::new(name.clone()),
                    args: args.clone(),
                },
            });
            self.tools.insert(id.to_string(), ToolInfo { name, args });
        }
        self.on_tool_update(u);
    }

    fn on_tool_update(&mut self, u: &Value) {
        let Some(id) = u.get("toolCallId").and_then(Value::as_str) else {
            return;
        };
        if !self.tools.contains_key(id) {
            return;
        }
        for item in u
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(source) = image_source(item.get("content")) {
                self.emit(Event::Image {
                    source,
                    call_id: Some(id.to_string()),
                });
            }
        }
        if let Some(preview) = tool_preview(u) {
            self.emit(Event::ToolOutput {
                call_id: id.to_string(),
                preview,
                tokens: 0,
            });
        }
        let status = u.get("status").and_then(Value::as_str);
        if matches!(status, Some("completed" | "failed"))
            && self.finished_tools.insert(id.to_string())
        {
            self.emit(Event::ToolFinished {
                call_id: id.to_string(),
                ok: status == Some("completed"),
                full_tokens: 0,
                sent_tokens: 0,
            });
        }
    }

    fn on_permission(&mut self, rpc_id: Value, params: &Value) {
        let tc = params.get("toolCall").cloned().unwrap_or(Value::Null);
        let call_id = tc
            .get("toolCallId")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let known = self.tools.get(&call_id);
        let name = known.map_or_else(|| tool_name(&tc), |t| t.name.clone());
        let args = known.map_or_else(|| tool_args(&tc), |t| t.args.clone());
        let title = tc
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                args.get("query")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| format!("Antigravity wants to run {name}"));
        let options: Vec<Value> = params
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let detail = tc
            .get("rawInput")
            .filter(|v| !v.is_null())
            .and_then(|v| serde_json::to_string_pretty(v).ok())
            .map(|s| truncate(&s, PREVIEW_CHARS));
        self.perm_seq += 1;
        let id = format!("acp-perm-{}", self.perm_seq);
        self.permissions
            .insert(id.clone(), PendingPermission { rpc_id, options });
        self.set_state("awaiting_permission", true);
        self.emit(Event::PermissionRequested {
            request: PermissionRequest {
                id,
                call: ToolCall {
                    id: call_id,
                    name: ToolName::new(name),
                    args,
                },
                reason: title,
                detail,
            },
        });
    }

    fn respond_permission(&mut self, request_id: &str, decision: &str) {
        let Some(p) = self.permissions.remove(request_id) else {
            return;
        };
        let Some(agent) = self.agent.as_ref() else {
            return;
        };
        let outcome = match pick_option(&p.options, decision) {
            Some(option_id) => json!({"outcome": {"outcome": "selected", "optionId": option_id}}),
            None => json!({"outcome": {"outcome": "cancelled"}}),
        };
        agent.conn.respond(p.rpc_id, Ok(outcome));
        if self.turn.is_some() {
            self.set_state("streaming", true);
        }
    }

    fn persist(&self) {
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
        p.journal.save(&PersistedSession {
            key: p.key.clone(),
            mode: p.mode.clone(),
            provider: self.cfg.provider_name.clone(),
            model: self.cfg.model.clone(),
            thinking: ThinkingLevel::Off,
            workspace: p.workspace.clone(),
            title,
            messages: self.history.clone(),
            turn: self.turn_no,
            usage: TokenUsage::default(),
            cost_usd: self.cost_usd,
            external: self.external.clone(),
        });
    }
}

async fn recv_opt(rx: &mut Option<mpsc::UnboundedReceiver<Incoming>>) -> Option<Incoming> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

async fn wait_turn(turn: &mut Option<Turn>) -> Result<Value, RpcError> {
    match turn {
        Some(t) => (&mut t.done)
            .await
            .unwrap_or_else(|_| Err(RpcError::closed())),
        None => std::future::pending().await,
    }
}

fn auth_message(e: &RpcError) -> String {
    tracing::debug!(error = %e, "antigravity needs sign-in");
    SIGN_IN_HINT.to_string()
}

fn stop_note(stop: &str) -> Option<&'static str> {
    match stop {
        "refusal" => Some("Antigravity declined to continue this request."),
        "max_tokens" => Some("Antigravity hit its output limit; ask it to continue."),
        "max_turn_requests" => Some("Antigravity hit its per-turn step limit; ask it to continue."),
        _ => None,
    }
}

fn content_text(c: Option<&Value>) -> Option<String> {
    let c = c?;
    match c.get("type").and_then(Value::as_str) {
        Some("text") | None => c.get("text").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
    .filter(|t| !t.is_empty())
}

fn image_source(content: Option<&Value>) -> Option<String> {
    let c = content?;
    if c.get("type")?.as_str()? != "image" {
        return None;
    }
    let mime = c.get("mimeType")?.as_str()?;
    if !matches!(
        mime,
        "image/png" | "image/jpeg" | "image/webp" | "image/gif" | "image/bmp"
    ) {
        return None;
    }
    let data = c.get("data")?.as_str()?;
    Some(format!("data:{mime};base64,{data}"))
}

fn tool_name(u: &Value) -> String {
    if let Some(n) = u
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
    {
        return n.to_string();
    }
    match u.get("kind").and_then(Value::as_str).unwrap_or("other") {
        "read" => "read_file",
        "edit" => "edit_file",
        "delete" => "delete",
        "move" => "move",
        "search" => "grep",
        "execute" => "bash",
        "think" => "think",
        "fetch" => "fetch",
        _ => "tool",
    }
    .to_string()
}

fn tool_args(u: &Value) -> Value {
    let raw = u.get("rawInput").filter(|v| v.is_object());
    let path = u
        .pointer("/locations/0/path")
        .and_then(Value::as_str)
        .or_else(|| raw.and_then(|r| r.get("path").and_then(Value::as_str)));
    let command = raw.and_then(|r| r.get("command").and_then(Value::as_str));
    let title = u.get("title").and_then(Value::as_str);
    let mut args = raw.and_then(Value::as_object).cloned().unwrap_or_default();
    if let Some(p) = path {
        args.insert("path".into(), json!(p));
    }
    if let Some(c) = command {
        args.insert("command".into(), json!(c));
    }
    if let Some(t) = title {
        args.insert("query".into(), json!(t));
    }
    Value::Object(args)
}

fn tool_preview(u: &Value) -> Option<String> {
    use std::fmt::Write as _;
    let mut out = String::new();
    for item in u
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str) {
            Some("content") => {
                if let Some(t) = content_text(item.get("content")) {
                    out.push_str(&t);
                    out.push('\n');
                }
            }
            Some("diff") => {
                let path = item.get("path").and_then(Value::as_str).unwrap_or("file");
                let old = item
                    .get("oldText")
                    .and_then(Value::as_str)
                    .map_or(0, |s| s.lines().count());
                let new = item
                    .get("newText")
                    .and_then(Value::as_str)
                    .map_or(0, |s| s.lines().count());
                let _ = writeln!(out, "edited {path} ({old} → {new} lines)");
            }
            Some("terminal") => out.push_str("[terminal output]\n"),
            _ => {}
        }
    }
    if out.is_empty()
        && let Some(raw) = u.get("rawOutput").filter(|v| !v.is_null())
    {
        out = match raw.as_str() {
            Some(s) => s.to_string(),
            None => serde_json::to_string_pretty(raw).unwrap_or_default(),
        };
    }
    let out = out.trim_end();
    (!out.is_empty()).then(|| truncate(out, PREVIEW_CHARS))
}

fn pick_option(options: &[Value], decision: &str) -> Option<String> {
    let want: &[&str] = match decision {
        "allow" => &["allow_once", "allow_always"],
        "allow_session" => &["allow_always", "allow_once"],
        _ => &["reject_once", "reject_always"],
    };
    want.iter().find_map(|k| {
        options
            .iter()
            .find(|o| o.get("kind").and_then(Value::as_str) == Some(k))
            .and_then(|o| o.get("optionId").and_then(Value::as_str))
            .map(str::to_string)
    })
}

fn seeded_prompt(history: &[Message], text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for m in history.iter().rev() {
        let line = match m {
            Message::User { content, .. } => format!("User: {content}"),
            Message::Assistant { content, .. } if !content.is_empty() => {
                format!("Assistant: {content}")
            }
            _ => continue,
        };
        if lines.iter().map(String::len).sum::<usize>() + line.len() > SEED_CHARS {
            break;
        }
        lines.push(line);
    }
    if lines.is_empty() {
        return text.to_string();
    }
    lines.reverse();
    format!(
        "Earlier in this conversation (with a different model), for context:\n\n{}\n\n---\n\n{text}",
        lines.join("\n\n")
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

pub fn mcp_servers(configs: &[hate_core::McpServerConfig]) -> Value {
    let pairs = |m: &std::collections::BTreeMap<String, String>| -> Vec<Value> {
        m.iter()
            .map(|(k, v)| json!({"name": k, "value": v}))
            .collect()
    };
    Value::Array(
        configs
            .iter()
            .filter(|c| c.enabled)
            .filter_map(|c| match c.kind.as_str() {
                "stdio" => c.command.as_ref().map(|cmd| {
                    json!({"name": c.name, "command": cmd, "args": c.args, "env": pairs(&c.env)})
                }),
                "http" => c.url.as_ref().map(|url| {
                    json!({"type": "http", "name": c.name, "url": url, "headers": pairs(&c.headers)})
                }),
                _ => None,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_decisions_map_to_offered_options() {
        let opts = vec![
            json!({"optionId": "a1", "name": "Allow", "kind": "allow_once"}),
            json!({"optionId": "a2", "name": "Always", "kind": "allow_always"}),
            json!({"optionId": "r1", "name": "Reject", "kind": "reject_once"}),
        ];
        assert_eq!(pick_option(&opts, "allow").as_deref(), Some("a1"));
        assert_eq!(pick_option(&opts, "allow_session").as_deref(), Some("a2"));
        assert_eq!(pick_option(&opts, "deny").as_deref(), Some("r1"));
        let only_once = vec![json!({"optionId": "x", "kind": "allow_once"})];
        assert_eq!(
            pick_option(&only_once, "allow_session").as_deref(),
            Some("x")
        );
        assert_eq!(pick_option(&only_once, "deny"), None);
    }

    #[test]
    fn tool_metadata_maps_to_cards() {
        let u = json!({"toolCallId": "c1", "title": "Reading config", "kind": "read",
                       "locations": [{"path": "/repo/src/main.rs"}], "rawInput": {"command": "ls"}});
        assert_eq!(tool_name(&u), "read_file");
        let a = tool_args(&u);
        assert_eq!(a["path"], "/repo/src/main.rs");
        assert_eq!(a["command"], "ls");
        assert_eq!(a["query"], "Reading config");
        assert_eq!(
            tool_name(&json!({"name": "run_shell", "kind": "execute"})),
            "run_shell"
        );

        let diff = json!({"content": [{"type": "diff", "path": "/a.rs", "oldText": "x\ny", "newText": "x\ny\nz"}]});
        assert_eq!(
            tool_preview(&diff).as_deref(),
            Some("edited /a.rs (2 → 3 lines)")
        );
        let text =
            json!({"content": [{"type": "content", "content": {"type": "text", "text": "ok"}}]});
        assert_eq!(tool_preview(&text).as_deref(), Some("ok"));
        assert!(tool_preview(&json!({})).is_none());
    }

    #[test]
    fn images_and_raw_image_arguments_are_preserved() {
        assert_eq!(
            image_source(Some(
                &json!({"type":"image", "mimeType":"image/png", "data":"YWJj"})
            )),
            Some("data:image/png;base64,YWJj".into())
        );
        assert!(
            image_source(Some(
                &json!({"type":"image", "mimeType":"image/svg+xml", "data":"unsafe"})
            ))
            .is_none()
        );
        let args =
            tool_args(&json!({"rawInput":{"imagePath":"C:\\images\\car.jpg", "prompt":"car"}}));
        assert_eq!(args["imagePath"], "C:\\images\\car.jpg");
        assert_eq!(args["prompt"], "car");
    }

    #[test]
    fn seed_keeps_recent_history_within_budget() {
        let mut h = vec![Message::user("x".repeat(SEED_CHARS))];
        h.push(Message::Assistant {
            content: "recent answer".into(),
            tool_calls: vec![],
        });
        let p = seeded_prompt(&h, "next");
        assert!(p.contains("Assistant: recent answer"));
        assert!(!p.contains(&"x".repeat(100)));
        assert!(p.ends_with("next"));
        assert_eq!(seeded_prompt(&[], "hi"), "hi");
    }

    #[test]
    fn mcp_servers_use_acp_shapes() {
        let cfgs = vec![
            hate_core::McpServerConfig {
                name: "fs".into(),
                kind: "stdio".into(),
                enabled: true,
                command: Some("npx".into()),
                args: vec!["-y".into(), "server".into()],
                env: [("K".to_string(), "V".to_string())].into(),
                url: None,
                headers: std::collections::BTreeMap::default(),
            },
            hate_core::McpServerConfig {
                name: "off".into(),
                kind: "stdio".into(),
                enabled: false,
                command: Some("x".into()),
                args: vec![],
                env: std::collections::BTreeMap::default(),
                url: None,
                headers: std::collections::BTreeMap::default(),
            },
            hate_core::McpServerConfig {
                name: "web".into(),
                kind: "http".into(),
                enabled: true,
                command: None,
                args: vec![],
                env: std::collections::BTreeMap::default(),
                url: Some("https://x/mcp".into()),
                headers: [("Authorization".to_string(), "t".to_string())].into(),
            },
        ];
        let v = mcp_servers(&cfgs);
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["env"][0], json!({"name": "K", "value": "V"}));
        assert_eq!(arr[1]["type"], "http");
        assert_eq!(arr[1]["headers"][0]["name"], "Authorization");
    }
}
