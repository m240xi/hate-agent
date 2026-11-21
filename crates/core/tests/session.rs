use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::BoxStream;
use hate_core::provider::{
    BoxFuture, CacheMode, Capabilities, ChatRequest, Delta, ModelInfo, Provider, ProviderError,
};
use hate_core::{Command, Event, Limits, Outcome, SessionConfig, TokenUsage, spawn_session};
use tokio_util::sync::CancellationToken;

struct Scripted {
    script: Vec<Delta>,
    usage: TokenUsage,
    delta_delay: Duration,
}

impl Provider for Scripted {
    fn stream(
        &self,
        _req: ChatRequest,
        cancel: CancellationToken,
    ) -> BoxStream<'static, Result<Delta, ProviderError>> {
        let script = self.script.clone();
        let usage = self.usage;
        let delay = self.delta_delay;
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            for d in script {
                if delay > Duration::ZERO {
                    tokio::select! {
                        () = cancel.cancelled() => return,
                        () = tokio::time::sleep(delay) => {}
                    }
                }
                if matches!(d, Delta::Done) {
                    let _ = tx.send(Ok(Delta::Usage(usage))).await;
                }
                if tx.send(Ok(d)).await.is_err() {
                    return;
                }
            }
        });
        Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
    }

    fn count_tokens<'a>(
        &'a self,
        _req: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<u64, ProviderError>> {
        Box::pin(async { Ok(0) })
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            vision: false,
            tools: false,
            cache: CacheMode::None,
            thinking: false,
        }
    }
}

fn session_with(provider: Arc<dyn Provider>) -> hate_core::SessionHandle {
    spawn_session(SessionConfig {
        provider,
        model: "test".to_string(),
        provider_name: "mock".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits: Limits::default(),
        ledger: None,
        persist: None,
        tools: None,
        permissions: Arc::new(hate_core::PermissionHub::default()),
        mcp: None,
        skills: None,
        auto_skills: false,
        terminal: None,
        context_window: 0,
    })
}

#[tokio::test]
async fn echo_turn_emits_deltas_usage_and_completion() {
    let provider: Arc<dyn Provider> = Arc::new(Scripted {
        script: vec![
            Delta::Text("Hello".to_string()),
            Delta::Text(" world".to_string()),
            Delta::Done,
        ],
        usage: TokenUsage {
            input: 11,
            cached_input: 3,
            output: 7,
            thinking: 0,
        },
        delta_delay: Duration::ZERO,
    });
    let session = session_with(provider);
    let mut rx = session.subscribe();

    session
        .command(Command::user("hi".to_string()))
        .expect("command");

    let mut text = String::new();
    let mut saw_usage = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let outcome = loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("event timeout")
            .expect("stream open");
        match ev {
            Event::Delta { text: t } => text.push_str(&t),
            Event::TokensCounted { usage, .. } => {
                saw_usage = usage.input == 11 && usage.output == 7;
            }
            Event::TurnComplete { outcome: o } => break o,
            Event::Failed { reason } => panic!("turn failed: {reason:?}"),
            _ => {}
        }
    };

    assert_eq!(text, "Hello world");
    assert!(saw_usage, "TokensCounted carried provider usage");
    assert!(matches!(outcome.outcome, Outcome::Completed));
    assert_eq!(outcome.usage.input, 11);
    assert!(!session.snapshot().running);
}

#[tokio::test]
async fn second_message_continues_session() {
    let provider: Arc<dyn Provider> = Arc::new(Scripted {
        script: vec![Delta::Text("reply".to_string()), Delta::Done],
        usage: TokenUsage::default(),
        delta_delay: Duration::ZERO,
    });
    let session = session_with(provider);
    let mut rx = session.subscribe();

    for _ in 0..2 {
        session
            .command(Command::user("hi".to_string()))
            .expect("command");
        loop {
            match rx.recv().await.expect("stream") {
                Event::TurnComplete { .. } => break,
                Event::Failed { reason } => panic!("failed: {reason:?}"),
                _ => {}
            }
        }
    }
    assert_eq!(session.snapshot().turn, 2);
}

#[tokio::test]
async fn cancel_interrupts_in_flight_stream() {
    let script: Vec<Delta> = (0..100)
        .map(|i| Delta::Text(format!("d{i} ")))
        .chain(std::iter::once(Delta::Done))
        .collect();
    let provider: Arc<dyn Provider> = Arc::new(Scripted {
        script,
        usage: TokenUsage::default(),
        delta_delay: Duration::from_millis(50),
    });
    let session = session_with(provider);
    let mut rx = session.subscribe();

    session
        .command(Command::user("go".to_string()))
        .expect("command");

    let start = std::time::Instant::now();
    loop {
        match rx.recv().await.expect("stream") {
            Event::Delta { .. } => break,
            Event::Failed { reason } => panic!("failed: {reason:?}"),
            _ => {}
        }
    }
    session.command(Command::Cancel).expect("cancel");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let outcome = loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("cancel timeout — stream did not stop")
            .expect("stream open");
        if let Event::TurnComplete { outcome } = ev {
            break outcome;
        }
    };
    let elapsed = start.elapsed();
    assert!(
        matches!(outcome.outcome, Outcome::Cancelled),
        "turn reported Cancelled"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "cancel took {elapsed:?} — budget is 250ms of *stream* abort"
    );
}

#[tokio::test]
async fn provider_error_surfaces_as_failed() {
    struct Fails;
    impl Provider for Fails {
        fn stream(
            &self,
            _req: ChatRequest,
            _cancel: CancellationToken,
        ) -> BoxStream<'static, Result<Delta, ProviderError>> {
            let (tx, mut rx) = tokio::sync::mpsc::channel(4);
            tokio::spawn(async move {
                let _ = tx
                    .send(Err(ProviderError::Http {
                        status: 429,
                        message: "rate limited".to_string(),
                        retry_after: Some(Duration::from_secs(1)),
                    }))
                    .await;
            });
            Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
        }
        fn count_tokens<'a>(
            &'a self,
            _req: &'a ChatRequest,
        ) -> BoxFuture<'a, Result<u64, ProviderError>> {
            Box::pin(async { Ok(0) })
        }
        fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                vision: false,
                tools: false,
                cache: CacheMode::None,
                thinking: false,
            }
        }
    }

    let session = session_with(Arc::new(Fails));
    let mut rx = session.subscribe();
    session
        .command(Command::user("hi".to_string()))
        .expect("command");
    loop {
        match rx.recv().await.expect("stream") {
            Event::Failed { reason } => {
                assert!(format!("{reason:?}").contains("429"));
                break;
            }
            Event::TurnComplete { .. } => panic!("should not complete"),
            _ => {}
        }
    }
}

struct Rounds {
    rounds: std::sync::Mutex<std::collections::VecDeque<Vec<Delta>>>,
    seen: std::sync::Mutex<Vec<String>>,
}

impl Provider for Rounds {
    fn stream(
        &self,
        req: ChatRequest,
        _cancel: CancellationToken,
    ) -> BoxStream<'static, Result<Delta, ProviderError>> {
        self.seen
            .lock()
            .unwrap()
            .push(serde_json::to_string(&req.messages).unwrap());
        let script = self
            .rounds
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| vec![Delta::Done]);
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            for d in script {
                if tx.send(Ok(d)).await.is_err() {
                    return;
                }
            }
        });
        Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
    }

    fn count_tokens<'a>(
        &'a self,
        _req: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<u64, ProviderError>> {
        Box::pin(async { Ok(0) })
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            vision: false,
            tools: true,
            cache: CacheMode::None,
            thinking: false,
        }
    }
}

#[tokio::test]
async fn tool_calls_execute_and_feed_results_back() {
    use hate_core::{ToolCall, ToolName, WorkspaceTools};
    use serde_json::json;

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello tool\n").unwrap();
    let tools = Arc::new(WorkspaceTools::new(dir.path()).unwrap());

    let provider = Arc::new(Rounds {
        rounds: std::sync::Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                Delta::ToolCall(ToolCall {
                    id: "c1".into(),
                    name: ToolName::new("read_file"),
                    args: json!({"path": "note.txt"}),
                }),
                Delta::Done,
            ],
            vec![Delta::Text("read it".into()), Delta::Done],
        ])),
        seen: std::sync::Mutex::new(Vec::new()),
    });

    let session = spawn_session(SessionConfig {
        provider: provider.clone(),
        model: "test".to_string(),
        provider_name: "mock".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits: Limits::default(),
        ledger: None,
        persist: None,
        tools: Some(tools),
        permissions: Arc::new(hate_core::PermissionHub::default()),
        mcp: None,
        skills: None,
        auto_skills: false,
        terminal: None,
        context_window: 0,
    });
    let mut rx = session.subscribe();
    session
        .command(Command::user("read note.txt".to_string()))
        .expect("command");

    let mut saw_tool_events = false;
    let tool_calls;
    loop {
        match rx.recv().await.expect("stream") {
            Event::ToolStarted { call } => {
                assert_eq!(call.name.as_str(), "read_file");
                saw_tool_events = true;
            }
            Event::ToolFinished {
                ok, full_tokens, ..
            } => {
                assert!(ok);
                assert!(full_tokens > 0);
                saw_tool_events = true;
            }
            Event::TurnComplete { outcome } => {
                tool_calls = outcome.tool_calls;
                break;
            }
            Event::Failed { reason } => panic!("turn failed: {reason:?}"),
            _ => {}
        }
    }

    assert!(saw_tool_events, "tool lifecycle events emitted");
    assert_eq!(tool_calls, 1);

    let seen = provider.seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "expected two provider rounds");
    assert!(
        seen[1].contains("hello tool"),
        "tool output fed back to model: {}",
        &seen[1][..seen[1].len().min(400)]
    );
}

#[tokio::test]
async fn tool_call_without_tools_fails_honestly() {
    use hate_core::{ToolCall, ToolName};
    use serde_json::json;

    let provider = Arc::new(Rounds {
        rounds: std::sync::Mutex::new(std::collections::VecDeque::from(vec![vec![
            Delta::ToolCall(ToolCall {
                id: "c1".into(),
                name: ToolName::new("read_file"),
                args: json!({"path": "x"}),
            }),
            Delta::Done,
        ]])),
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let session = session_with(provider);
    let mut rx = session.subscribe();
    session
        .command(Command::user("hi".to_string()))
        .expect("command");
    loop {
        match rx.recv().await.expect("stream") {
            Event::Failed { reason } => {
                assert!(format!("{reason:?}").contains("no tools"));
                break;
            }
            Event::TurnComplete { .. } => panic!("should not complete"),
            _ => {}
        }
    }
}

#[tokio::test]
async fn gated_tool_waits_for_permission_then_denies() {
    use hate_core::{PermissionHub, ToolCall, ToolName, WorkspaceTools};
    use serde_json::json;

    let dir = tempfile::tempdir().unwrap();
    let tools = Arc::new(WorkspaceTools::new(dir.path()).unwrap());
    let permissions = Arc::new(PermissionHub::default());

    let provider = Arc::new(Rounds {
        rounds: std::sync::Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                Delta::ToolCall(ToolCall {
                    id: "c1".into(),
                    name: ToolName::new("bash"),
                    args: json!({"command": "touch blocked.txt"}),
                }),
                Delta::Done,
            ],
            vec![Delta::Text("denied, ok".into()), Delta::Done],
        ])),
        seen: std::sync::Mutex::new(Vec::new()),
    });

    let session = spawn_session(SessionConfig {
        provider: provider.clone(),
        model: "test".to_string(),
        provider_name: "mock".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits: Limits::default(),
        ledger: None,
        persist: None,
        tools: Some(tools),
        permissions,
        mcp: None,
        skills: None,
        auto_skills: false,
        terminal: None,
        context_window: 0,
    });
    let mut rx = session.subscribe();
    session
        .command(Command::user("run echo".to_string()))
        .expect("command");

    let mut denied = false;
    let mut saw_request = false;
    loop {
        match rx.recv().await.expect("stream") {
            Event::PermissionRequested { request } => {
                saw_request = true;
                assert_eq!(request.call.name.as_str(), "bash");
                session
                    .command(Command::RespondPermission {
                        request_id: request.id,
                        decision: "deny".to_string(),
                    })
                    .expect("respond");
            }
            Event::ToolFinished { ok, .. } => {
                assert!(!ok, "denied call must report failure");
                denied = true;
            }
            Event::TurnComplete { .. } => break,
            Event::Failed { reason } => panic!("turn failed: {reason:?}"),
            _ => {}
        }
    }
    assert!(saw_request, "permission request emitted");
    assert!(denied, "denial surfaced as failed tool result");

    let seen = provider.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[1].contains("denied"), "denial fed back: {seen:?}");
}

#[tokio::test]
async fn allow_session_skips_repeat_prompts() {
    use hate_core::{PermissionHub, ToolCall, ToolName, WorkspaceTools};
    use serde_json::json;

    let dir = tempfile::tempdir().unwrap();
    let tools = Arc::new(WorkspaceTools::new(dir.path()).unwrap());

    let call = || {
        Delta::ToolCall(ToolCall {
            id: "c1".into(),
            name: ToolName::new("write_file"),
            args: json!({"path": "x.txt", "content": "hi"}),
        })
    };
    let provider = Arc::new(Rounds {
        rounds: std::sync::Mutex::new(std::collections::VecDeque::from(vec![
            vec![call(), Delta::Done],
            vec![call(), Delta::Done],
            vec![Delta::Text("done".into()), Delta::Done],
        ])),
        seen: std::sync::Mutex::new(Vec::new()),
    });

    let session = spawn_session(SessionConfig {
        provider,
        model: "test".to_string(),
        provider_name: "mock".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits: Limits::default(),
        ledger: None,
        persist: None,
        tools: Some(tools),
        permissions: Arc::new(PermissionHub::default()),
        mcp: None,
        skills: None,
        auto_skills: false,
        terminal: None,
        context_window: 0,
    });
    let mut rx = session.subscribe();
    session
        .command(Command::user("write twice".to_string()))
        .expect("command");

    let mut prompts = 0u32;
    loop {
        match rx.recv().await.expect("stream") {
            Event::PermissionRequested { request } => {
                prompts += 1;
                session
                    .command(Command::RespondPermission {
                        request_id: request.id,
                        decision: "allow_session".to_string(),
                    })
                    .expect("respond");
            }
            Event::TurnComplete { .. } => break,
            Event::Failed { reason } => panic!("turn failed: {reason:?}"),
            _ => {}
        }
    }
    assert_eq!(prompts, 1, "second identical call auto-allowed");
    assert!(dir.path().join("x.txt").exists());
}

#[derive(Default)]
struct CaptureJournal(std::sync::Mutex<Option<hate_core::PersistedSession>>);

impl hate_core::SessionJournal for CaptureJournal {
    fn load(&self, _key: &str) -> Option<hate_core::PersistedSession> {
        None
    }
    fn save(&self, snap: &hate_core::PersistedSession) {
        *self.0.lock().expect("journal poisoned") = Some(snap.clone());
    }
    fn delete(&self, _key: &str) {}
    fn list(&self, _mode: Option<&str>) -> Vec<hate_core::SessionMeta> {
        Vec::new()
    }
}

#[tokio::test]
async fn compaction_bounds_journaled_history() {
    use hate_core::provider::Message;

    let journal = Arc::new(CaptureJournal::default());
    let session = spawn_session(SessionConfig {
        provider: Arc::new(Scripted {
            script: vec![Delta::Text("ok".to_string()), Delta::Done],
            usage: TokenUsage::default(),
            delta_delay: Duration::ZERO,
        }),
        model: "test".to_string(),
        provider_name: "mock".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits: Limits::default(),
        ledger: None,
        persist: Some(hate_core::PersistCtx {
            key: "compact-test".to_string(),
            mode: "chat".to_string(),
            workspace: None,
            journal: journal.clone(),
            restore: None,
        }),
        tools: None,
        permissions: Arc::new(hate_core::PermissionHub::default()),
        mcp: None,
        skills: None,
        auto_skills: false,
        terminal: None,
        context_window: 400,
    });
    let mut rx = session.subscribe();

    for i in 0..14 {
        session
            .command(Command::user(format!("turn number {i}")))
            .expect("command");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let ev = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .expect("event timeout")
                .expect("stream open");
            if matches!(ev, Event::TurnComplete { .. } | Event::Failed { .. }) {
                break;
            }
        }
    }

    let snap = journal
        .0
        .lock()
        .expect("journal poisoned")
        .clone()
        .expect("journal saved at turn end");
    let msgs = &snap.messages;
    assert!(
        msgs.iter().any(|m| matches!(
            m,
            Message::User { content, .. } if content.contains("[Context compacted]")
        )),
        "expected a compaction note in journaled history ({} msgs)",
        msgs.len()
    );
    assert!(
        msgs.len() < 40,
        "adopted history must stay bounded across turns, got {}",
        msgs.len()
    );
}

async fn drive_turn(
    session: &hate_core::SessionHandle,
    rx: &mut tokio::sync::broadcast::Receiver<Event>,
    msg: &str,
) {
    session
        .command(Command::user(msg.to_string()))
        .expect("command");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("event timeout")
            .expect("stream open");
        if matches!(ev, Event::TurnComplete { .. } | Event::Failed { .. }) {
            break;
        }
    }
}

#[tokio::test]
async fn stale_tool_outputs_elide_next_turn() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("big.txt"), "x".repeat(20 * 1024)).unwrap();
    let tools = Arc::new(hate_core::WorkspaceTools::new(dir.path()).expect("workspace tools"));

    let provider = Arc::new(Rounds {
        rounds: std::sync::Mutex::new(
            vec![
                vec![
                    Delta::ToolCall(hate_core::ToolCall {
                        id: "c1".to_string(),
                        name: hate_core::ToolName::new("read_file"),
                        args: serde_json::json!({"path": "big.txt"}),
                    }),
                    Delta::Done,
                ],
                vec![Delta::Text("read it".to_string()), Delta::Done],
                vec![Delta::Text("ok".to_string()), Delta::Done],
            ]
            .into_iter()
            .collect(),
        ),
        seen: std::sync::Mutex::new(Vec::new()),
    });

    let session = spawn_session(SessionConfig {
        provider: provider.clone(),
        model: "test".to_string(),
        provider_name: "mock".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits: Limits::default(),
        ledger: None,
        persist: None,
        tools: Some(tools),
        permissions: Arc::new(hate_core::PermissionHub::default()),
        mcp: None,
        skills: None,
        auto_skills: false,
        terminal: None,
        context_window: 0,
    });
    let mut rx = session.subscribe();

    drive_turn(&session, &mut rx, "read big.txt").await;
    drive_turn(&session, &mut rx, "what did it say?").await;

    let seen = provider.seen.lock().unwrap();
    assert_eq!(seen.len(), 3, "expected 3 provider rounds");
    let turn2_req = &seen[2];
    assert!(
        turn2_req.contains("tool output elided"),
        "turn 2 request must carry the elided stub"
    );
    assert!(
        turn2_req.contains("re-run the tool"),
        "stub must tell the model how to recover"
    );
    assert!(
        !turn2_req.contains(&"x".repeat(9000)),
        "full tool output leaked into turn 2 request"
    );
}

struct CaptureSystem {
    script: Vec<Delta>,
    systems: Arc<std::sync::Mutex<Vec<Option<String>>>>,
}

impl Provider for CaptureSystem {
    fn stream(
        &self,
        req: ChatRequest,
        _cancel: CancellationToken,
    ) -> BoxStream<'static, Result<Delta, ProviderError>> {
        self.systems.lock().unwrap().push(req.system);
        let script = self.script.clone();
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            for d in script {
                if tx.send(Ok(d)).await.is_err() {
                    return;
                }
            }
        });
        Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
    }

    fn count_tokens<'a>(
        &'a self,
        _req: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<u64, ProviderError>> {
        Box::pin(async { Ok(0) })
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            vision: false,
            tools: true,
            cache: CacheMode::None,
            thinking: false,
        }
    }
}

#[tokio::test]
async fn skill_save_tool_persists_and_emits() {
    use hate_core::{SkillStore, ToolCall, ToolName};
    use serde_json::json;

    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(SkillStore::new(dir.path().join("skills")));
    let body = "Always use Rojo for file sync. ".repeat(20);

    let provider = Arc::new(Rounds {
        rounds: std::sync::Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                Delta::ToolCall(ToolCall {
                    id: "c1".to_string(),
                    name: ToolName::new("skill_save"),
                    args: json!({
                        "title": "Roblox Luau",
                        "triggers": ["roblox", "luau"],
                        "body": body,
                    }),
                }),
                Delta::Done,
            ],
            vec![Delta::Text("saved".to_string()), Delta::Done],
        ])),
        seen: std::sync::Mutex::new(Vec::new()),
    });

    let session = spawn_session(SessionConfig {
        provider,
        model: "test".to_string(),
        provider_name: "mock".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits: Limits::default(),
        ledger: None,
        persist: None,
        tools: None,
        mcp: None,
        skills: Some(store.clone()),
        auto_skills: false,
        permissions: Arc::new(hate_core::PermissionHub::default()),
        terminal: None,
        context_window: 0,
    });
    let mut rx = session.subscribe();
    session
        .command(Command::user("save what you learned".to_string()))
        .expect("command");

    let mut saw_skill_event = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("event timeout")
            .expect("stream open");
        if let Event::SkillSaved { name, bytes, .. } = &ev {
            assert_eq!(name, "roblox-luau");
            assert!(*bytes > 200);
            saw_skill_event = true;
        }
        if matches!(ev, Event::TurnComplete { .. } | Event::Failed { .. }) {
            break;
        }
    }
    assert!(saw_skill_event, "expected a SkillSaved event");
    let saved = store.get("roblox-luau").expect("skill on disk");
    assert_eq!(saved.triggers, vec!["roblox", "luau"]);
    assert!(saved.body.contains("Rojo"));
}

#[tokio::test]
async fn matching_skill_injected_into_system_prompt() {
    use hate_core::{Skill, SkillStore, WorkspaceTools};

    let skills_dir = tempfile::tempdir().expect("skills tempdir");
    let store = Arc::new(SkillStore::new(skills_dir.path().join("skills")));
    store
        .save(&Skill {
            name: "roblox-luau".to_string(),
            title: "Roblox Luau".to_string(),
            triggers: vec!["roblox".to_string(), "luau".to_string()],
            updated: 0,
            body: format!("# Luau\n\n{}", "Use --!strict everywhere. ".repeat(20)),
        })
        .expect("save skill");

    let ws = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(ws.path().join("main.luau"), "print('hi')").expect("write");
    let tools = Arc::new(WorkspaceTools::new(ws.path()).expect("tools"));

    let systems = Arc::new(std::sync::Mutex::new(Vec::new()));
    let provider = Arc::new(CaptureSystem {
        script: vec![Delta::Text("ok".to_string()), Delta::Done],
        systems: systems.clone(),
    });

    let session = spawn_session(SessionConfig {
        provider,
        model: "test".to_string(),
        provider_name: "mock".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits: Limits::default(),
        ledger: None,
        persist: None,
        tools: Some(tools),
        mcp: None,
        skills: Some(store),
        auto_skills: false,
        permissions: Arc::new(hate_core::PermissionHub::default()),
        terminal: None,
        context_window: 0,
    });
    let mut rx = session.subscribe();
    drive_turn(&session, &mut rx, "hello").await;

    let sys = systems
        .lock()
        .unwrap()
        .first()
        .cloned()
        .flatten()
        .expect("system prompt recorded");
    assert!(sys.contains("## Learned skills"), "skills section missing");
    assert!(sys.contains("- roblox-luau — Roblox Luau"));
    assert!(
        sys.contains("Use --!strict everywhere."),
        "matching skill body not inlined"
    );
}

#[tokio::test]
async fn reflection_authors_skill_after_three_turns() {
    use hate_core::SkillStore;

    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(SkillStore::new(dir.path().join("skills")));
    let body = "Prefer `luau-lsp` for type checking. ".repeat(15);
    let reflection = format!("TITLE: Roblox Luau Basics\nTRIGGERS: roblox, luau\n{body}");

    let provider = Arc::new(Rounds {
        rounds: std::sync::Mutex::new(std::collections::VecDeque::from(vec![
            vec![Delta::Text("a".to_string()), Delta::Done],
            vec![Delta::Text("b".to_string()), Delta::Done],
            vec![Delta::Text("c".to_string()), Delta::Done],
            vec![Delta::Text(reflection), Delta::Done],
        ])),
        seen: std::sync::Mutex::new(Vec::new()),
    });

    let session = spawn_session(SessionConfig {
        provider,
        model: "test".to_string(),
        provider_name: "mock".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits: Limits::default(),
        ledger: None,
        persist: None,
        tools: None,
        mcp: None,
        skills: Some(store.clone()),
        auto_skills: true,
        permissions: Arc::new(hate_core::PermissionHub::default()),
        terminal: None,
        context_window: 0,
    });
    let mut rx = session.subscribe();
    drive_turn(&session, &mut rx, "working on my roblox game").await;
    drive_turn(&session, &mut rx, "adding a part with luau").await;
    drive_turn(&session, &mut rx, "now a click detector").await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_event = false;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(Event::SkillSaved { name, .. })) => {
                assert_eq!(name, "roblox-luau-basics");
                saw_event = true;
                break;
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) | Err(_) => break,
        }
    }
    assert!(saw_event, "expected reflection to emit SkillSaved");
    let saved = store.get("roblox-luau-basics").expect("skill on disk");
    assert!(saved.body.contains("luau-lsp"));
    assert_eq!(saved.triggers, vec!["roblox", "luau"]);
}

#[tokio::test]
async fn set_provider_swaps_adapter_mid_session() {
    use hate_core::provider::Provider;

    let a: Arc<dyn Provider> = Arc::new(Scripted {
        script: vec![Delta::Text("from-a".to_string()), Delta::Done],
        usage: TokenUsage::default(),
        delta_delay: Duration::ZERO,
    });
    let b: Arc<dyn Provider> = Arc::new(Scripted {
        script: vec![Delta::Text("from-b".to_string()), Delta::Done],
        usage: TokenUsage::default(),
        delta_delay: Duration::ZERO,
    });

    let session = spawn_session(SessionConfig {
        provider: a,
        model: "m1".to_string(),
        provider_name: "a".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits: Limits::default(),
        ledger: None,
        persist: None,
        tools: None,
        mcp: None,
        skills: None,
        auto_skills: false,
        permissions: Arc::new(hate_core::PermissionHub::default()),
        terminal: None,
        context_window: 0,
    });
    let mut rx = session.subscribe();

    session
        .command(Command::user("hi".to_string()))
        .expect("command");
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("event timeout")
            .expect("stream open");
        match &ev {
            Event::Delta { text: t } => text.push_str(t),
            Event::TurnComplete { .. } | Event::Failed { .. } => break,
            _ => {}
        }
    }
    assert_eq!(text, "from-a");

    session
        .command(Command::SetProvider {
            provider: b,
            provider_name: "b".to_string(),
            model: Some("m2".to_string()),
        })
        .expect("set provider");
    session
        .command(Command::user("again".to_string()))
        .expect("command");
    text.clear();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("event timeout")
            .expect("stream open");
        match &ev {
            Event::Delta { text: t } => text.push_str(t),
            Event::TurnComplete { .. } | Event::Failed { .. } => break,
            _ => {}
        }
    }
    assert_eq!(text, "from-b", "turn 2 must run on the swapped adapter");
}

#[tokio::test]
async fn system_prompt_names_current_model_after_swap() {
    let systems = Arc::new(std::sync::Mutex::new(Vec::new()));
    let make = |systems: &Arc<std::sync::Mutex<Vec<Option<String>>>>| -> Arc<dyn Provider> {
        Arc::new(CaptureSystem {
            script: vec![Delta::Text("ok".to_string()), Delta::Done],
            systems: systems.clone(),
        })
    };
    let session = spawn_session(SessionConfig {
        provider: make(&systems),
        model: "deepseek/deepseek-v4-flash".to_string(),
        provider_name: "nous".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits: Limits::default(),
        ledger: None,
        persist: None,
        tools: None,
        mcp: None,
        skills: None,
        auto_skills: false,
        permissions: Arc::new(hate_core::PermissionHub::default()),
        terminal: None,
        context_window: 0,
    });
    let mut rx = session.subscribe();
    drive_turn(&session, &mut rx, "which model are you?").await;

    session
        .command(Command::SetProvider {
            provider: make(&systems),
            provider_name: "claude".to_string(),
            model: Some("claude-sonnet-5".to_string()),
        })
        .expect("set provider");
    drive_turn(&session, &mut rx, "and now?").await;

    let seen = systems.lock().unwrap().clone();
    let first = seen[0].clone().expect("system 1");
    let second = seen[1].clone().expect("system 2");
    assert!(first.contains("`deepseek/deepseek-v4-flash`, served via the `nous` provider"));
    assert!(second.contains("`claude-sonnet-5`, served via the `claude` provider"));
    assert!(!second.contains("deepseek-v4-flash"));
}

fn tool_session(
    dir: &std::path::Path,
    rounds: Vec<Vec<Delta>>,
    limits: Limits,
) -> (Arc<Rounds>, hate_core::SessionHandle) {
    let provider = Arc::new(Rounds {
        rounds: std::sync::Mutex::new(std::collections::VecDeque::from(rounds)),
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let session = spawn_session(SessionConfig {
        provider: provider.clone(),
        model: "test".to_string(),
        provider_name: "mock".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits,
        ledger: None,
        persist: None,
        tools: Some(Arc::new(hate_core::WorkspaceTools::new(dir).unwrap())),
        permissions: Arc::new(hate_core::PermissionHub::default()),
        mcp: None,
        skills: None,
        auto_skills: false,
        terminal: None,
        context_window: 0,
    });
    (provider, session)
}

fn read_call(id: &str, path: &str) -> Delta {
    Delta::ToolCall(hate_core::ToolCall {
        id: id.into(),
        name: hate_core::ToolName::new("read_file"),
        args: serde_json::json!({"path": path}),
    })
}

#[tokio::test]
async fn read_batches_keep_order_and_duplicate_reads_cost_a_stub() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "AAA-unique-body\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "BBB-unique-body\n").unwrap();

    let (provider, session) = tool_session(
        dir.path(),
        vec![
            vec![
                read_call("c1", "a.txt"),
                read_call("c2", "b.txt"),
                Delta::Done,
            ],
            vec![read_call("c3", "a.txt"), Delta::Done],
            vec![Delta::Text("done".into()), Delta::Done],
            vec![read_call("c4", "a.txt"), Delta::Done],
            vec![Delta::Text("done again".into()), Delta::Done],
        ],
        Limits::default(),
    );
    let mut rx = session.subscribe();

    session
        .command(Command::user("read both".to_string()))
        .expect("command");
    let mut finished: Vec<(String, u32, u32)> = Vec::new();
    loop {
        match rx.recv().await.expect("stream") {
            Event::ToolFinished {
                call_id,
                ok,
                full_tokens,
                sent_tokens,
                ..
            } => {
                assert!(ok);
                finished.push((call_id, full_tokens, sent_tokens));
            }
            Event::TurnComplete { outcome } => {
                assert_eq!(outcome.tool_calls, 3);
                break;
            }
            Event::Failed { reason } => panic!("turn failed: {reason:?}"),
            _ => {}
        }
    }
    let ids: Vec<&str> = finished.iter().map(|f| f.0.as_str()).collect();
    assert_eq!(ids, ["c1", "c2", "c3"], "results recorded in call order");
    assert_eq!(finished[2].1, finished[0].2);

    drive_turn(&session, &mut rx, "again").await;

    let seen = provider.seen.lock().unwrap();
    assert_eq!(seen.len(), 5);
    let (a, b) = (
        seen[1].find("AAA-unique-body").expect("a fed back"),
        seen[1].find("BBB-unique-body").expect("b fed back"),
    );
    assert!(a < b);
    assert!(seen[2].contains("duplicate call"), "{}", seen[2]);
    assert_eq!(seen[2].matches("AAA-unique-body").count(), 1);
    assert_eq!(seen[4].matches("AAA-unique-body").count(), 2);
}

#[tokio::test]
async fn tool_round_limit_is_configurable_and_fails_honestly() {
    let dir = tempfile::tempdir().unwrap();
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(dir.path().join(n), "x\n").unwrap();
    }
    let (provider, session) = tool_session(
        dir.path(),
        vec![
            vec![read_call("c1", "a.txt"), Delta::Done],
            vec![read_call("c2", "b.txt"), Delta::Done],
            vec![read_call("c3", "c.txt"), Delta::Done],
        ],
        Limits {
            max_tool_rounds: 2,
            ..Limits::default()
        },
    );
    let mut rx = session.subscribe();
    session
        .command(Command::user("loop".to_string()))
        .expect("command");
    loop {
        match rx.recv().await.expect("stream") {
            Event::Failed { reason } => {
                let msg = format!("{reason:?}");
                assert!(msg.contains("exceeded 2 provider rounds"), "{msg}");
                break;
            }
            Event::TurnComplete { .. } => panic!("limit should have tripped"),
            _ => {}
        }
    }
    assert_eq!(provider.seen.lock().unwrap().len(), 2, "no third request");
}

#[tokio::test]
async fn plan_turn_is_read_only_without_prompting() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "plan-me\n").unwrap();
    let write = Delta::ToolCall(hate_core::ToolCall {
        id: "w1".into(),
        name: hate_core::ToolName::new("write_file"),
        args: serde_json::json!({"path": "new.txt", "content": "x"}),
    });
    let (provider, session) = tool_session(
        dir.path(),
        vec![
            vec![read_call("r1", "a.txt"), write.clone(), Delta::Done],
            vec![
                Delta::Text("1. edit a.txt\n2. run tests".into()),
                Delta::Done,
            ],
            vec![Delta::Text("hi".into()), Delta::Done],
            vec![Delta::Text("hi again".into()), Delta::Done],
        ],
        Limits::default(),
    );
    let mut rx = session.subscribe();
    session
        .command(Command::user("/plan rename the thing".to_string()))
        .expect("command");
    let mut results: Vec<(String, bool)> = Vec::new();
    loop {
        match rx.recv().await.expect("stream") {
            Event::PermissionRequested { .. } => panic!("plan turns must never prompt"),
            Event::ToolFinished { call_id, ok, .. } => results.push((call_id, ok)),
            Event::TurnComplete { .. } => break,
            Event::Failed { reason } => panic!("turn failed: {reason:?}"),
            _ => {}
        }
    }
    assert_eq!(
        results,
        [("r1".to_string(), true), ("w1".to_string(), false)]
    );
    assert!(!dir.path().join("new.txt").exists(), "write was blocked");

    drive_turn(&session, &mut rx, "hello there").await;
    drive_turn(&session, &mut rx, "and again").await;

    let seen = provider.seen.lock().unwrap();
    assert!(seen[0].contains("rename the thing") && seen[0].contains("[plan mode —"));
    assert!(!seen[0].contains("/plan"), "the command itself is not sent");
    assert!(seen[1].contains("plan-me"), "reads still feed back");
    assert!(seen[1].contains("plan mode is read-only"), "{}", seen[1]);

    assert!(
        !seen[2].contains("[plan mode —"),
        "directive outlived its turn: {}",
        seen[2]
    );
    assert!(
        seen[2].contains("rename the thing"),
        "the task itself stays in history"
    );
    assert!(seen[2].contains("hello there") && seen[2].contains("plan mode is off"));
    assert_eq!(
        seen[3].matches("plan mode is off").count(),
        1,
        "{}",
        seen[3]
    );
    assert!(!seen[3].contains("[plan mode —"));
}

#[tokio::test]
async fn plan_turn_in_chat_session_names_no_workspace_tools() {
    let provider = Arc::new(Rounds {
        rounds: std::sync::Mutex::new(std::collections::VecDeque::from(vec![vec![
            Delta::Text("1. think\n2. answer".into()),
            Delta::Done,
        ]])),
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let session = session_with(provider.clone());
    let mut rx = session.subscribe();
    drive_turn(&session, &mut rx, "/plan organise my week").await;
    let seen = provider.seen.lock().unwrap();
    assert!(seen[0].contains("organise my week") && seen[0].contains("[plan mode —"));
    assert!(!seen[0].contains("read_file"), "{}", seen[0]);
}

#[tokio::test]
async fn tool_search_appends_deferred_tools_without_mutating_prefix() {
    struct CapturingTools {
        rounds: std::sync::Mutex<std::collections::VecDeque<Vec<Delta>>>,
        tool_lists: std::sync::Mutex<Vec<Vec<String>>>,
    }

    impl Provider for CapturingTools {
        fn stream(
            &self,
            req: ChatRequest,
            _cancel: CancellationToken,
        ) -> BoxStream<'static, Result<Delta, ProviderError>> {
            self.tool_lists.lock().unwrap().push(
                req.tools
                    .iter()
                    .map(|t| t.name.as_str().to_string())
                    .collect(),
            );
            let script = self
                .rounds
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| vec![Delta::Done]);
            let (tx, mut rx) = tokio::sync::mpsc::channel(64);
            tokio::spawn(async move {
                for d in script {
                    if tx.send(Ok(d)).await.is_err() {
                        return;
                    }
                }
            });
            Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
        }

        fn count_tokens<'a>(
            &'a self,
            _req: &'a ChatRequest,
        ) -> BoxFuture<'a, Result<u64, ProviderError>> {
            Box::pin(async { Ok(0) })
        }

        fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                vision: false,
                tools: true,
                cache: CacheMode::None,
                thinking: false,
            }
        }
    }

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lib.rs"), "pub fn alpha() {}\n").unwrap();

    let provider = Arc::new(CapturingTools {
        rounds: std::sync::Mutex::new(std::collections::VecDeque::from(vec![
            vec![
                Delta::ToolCall(hate_core::ToolCall {
                    id: "s1".into(),
                    name: hate_core::ToolName::new("tool_search"),
                    args: serde_json::json!({"query": "outline symbol headings", "limit": 1}),
                }),
                Delta::Done,
            ],
            vec![
                Delta::ToolCall(hate_core::ToolCall {
                    id: "o1".into(),
                    name: hate_core::ToolName::new("outline"),
                    args: serde_json::json!({"path": "lib.rs"}),
                }),
                Delta::Done,
            ],
            vec![Delta::Text("done".into()), Delta::Done],
        ])),
        tool_lists: std::sync::Mutex::new(Vec::new()),
    });

    let session = spawn_session(SessionConfig {
        provider: provider.clone(),
        model: "test".to_string(),
        provider_name: "mock".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits: Limits::default(),
        ledger: None,
        persist: None,
        tools: Some(Arc::new(
            hate_core::WorkspaceTools::new(dir.path()).unwrap(),
        )),
        permissions: Arc::new(hate_core::PermissionHub::default()),
        mcp: None,
        skills: None,
        auto_skills: false,
        terminal: None,
        context_window: 0,
    });
    let mut rx = session.subscribe();
    drive_turn(&session, &mut rx, "inspect lib.rs outline").await;

    let lists = provider.tool_lists.lock().unwrap();
    assert_eq!(lists.len(), 3);
    // Round 1 has only Tier 1 tools (no `outline` yet).
    assert!(!lists[0].iter().any(|n| n == "outline"));
    // Round 2 keeps the exact Tier 1 prefix and appends `outline` at the end.
    assert_eq!(&lists[1][..lists[0].len()], lists[0].as_slice());
    assert_eq!(lists[1].last().map(String::as_str), Some("outline"));
}

#[tokio::test]
async fn permission_request_carries_diff_preview_for_write_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "old line\n").unwrap();
    let (_p, session) = tool_session(
        dir.path(),
        vec![
            vec![
                Delta::ToolCall(hate_core::ToolCall {
                    id: "w1".into(),
                    name: hate_core::ToolName::new("write_file"),
                    args: serde_json::json!({"path": "a.txt", "content": "new line\n"}),
                }),
                Delta::Done,
            ],
            vec![Delta::Text("ok".into()), Delta::Done],
        ],
        Limits::default(),
    );
    let mut rx = session.subscribe();
    session.command(Command::user("update a.txt")).unwrap();

    let mut saw_diff = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .unwrap()
            .unwrap();
        match ev {
            Event::PermissionRequested { request } => {
                let diff = request.diff_preview.expect("write_file has diff_preview");
                assert!(
                    diff.contains("-old line") && diff.contains("+new line"),
                    "{diff}"
                );
                saw_diff = true;
                session
                    .command(Command::RespondPermission {
                        request_id: request.id,
                        decision: "allow".into(),
                    })
                    .unwrap();
            }
            Event::TurnComplete { .. } => break,
            Event::Failed { reason } => panic!("failed: {reason:?}"),
            _ => {}
        }
    }
    assert!(saw_diff);
}

#[tokio::test]
async fn outbound_secrets_are_redacted_and_destructive_bash_never_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let (provider, session) = tool_session(
        dir.path(),
        vec![
            vec![
                Delta::ToolCall(hate_core::ToolCall {
                    id: "d1".into(),
                    name: hate_core::ToolName::new("bash"),
                    args: serde_json::json!({"command": "rm -rf nonexistent_tmp_dir_test"}),
                }),
                Delta::Done,
            ],
            vec![
                Delta::ToolCall(hate_core::ToolCall {
                    id: "d2".into(),
                    name: hate_core::ToolName::new("bash"),
                    args: serde_json::json!({"command": "rm -rf nonexistent_tmp_dir_test"}),
                }),
                Delta::Done,
            ],
            vec![Delta::Text("done".into()), Delta::Done],
        ],
        Limits::default(),
    );
    let mut rx = session.subscribe();
    session
        .command(Command::user(
            "use sk-proj-abcdefghijklmnopqrstuvwxyz1234 please",
        ))
        .unwrap();

    let mut prompts = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let ev = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .unwrap()
            .unwrap();
        match ev {
            Event::PermissionRequested { request } => {
                prompts += 1;
                // Click "Run & remember" on the destructive command: it should still prompt again!
                session
                    .command(Command::RespondPermission {
                        request_id: request.id,
                        decision: "allow_session".into(),
                    })
                    .unwrap();
            }
            Event::TurnComplete { .. } => break,
            Event::Failed { reason } => panic!("failed: {reason:?}"),
            _ => {}
        }
    }
    assert_eq!(prompts, 2, "destructive command must never be remembered");
    let seen = provider.seen.lock().unwrap();
    assert!(
        !seen[0].contains("sk-proj-abcdefghijklmnopqrstuvwxyz1234"),
        "secret leaked in outbound request: {}",
        seen[0]
    );
    assert!(seen[0].contains("[REDACTED:API_KEY]"), "{}", seen[0]);
}

#[tokio::test]
async fn repo_map_injects_in_s4_not_s0_and_spawn_agent_isolates_history() {
    struct InspectProvider {
        rounds: std::sync::Mutex<std::collections::VecDeque<Vec<Delta>>>,
        reqs: std::sync::Mutex<Vec<(Option<String>, String, usize)>>,
    }

    impl Provider for InspectProvider {
        fn stream(
            &self,
            req: ChatRequest,
            _cancel: CancellationToken,
        ) -> BoxStream<'static, Result<Delta, ProviderError>> {
            self.reqs.lock().unwrap().push((
                req.system.clone(),
                serde_json::to_string(&req.messages).unwrap(),
                req.tools.len(),
            ));
            let script = self
                .rounds
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| vec![Delta::Text("child summary".into()), Delta::Done]);
            let (tx, mut rx) = tokio::sync::mpsc::channel(64);
            tokio::spawn(async move {
                for d in script {
                    if tx.send(Ok(d)).await.is_err() {
                        return;
                    }
                }
            });
            Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
        }

        fn count_tokens<'a>(
            &'a self,
            _req: &'a ChatRequest,
        ) -> BoxFuture<'a, Result<u64, ProviderError>> {
            Box::pin(async { Ok(0) })
        }

        fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                vision: false,
                tools: true,
                cache: CacheMode::None,
                thinking: false,
            }
        }
    }

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn alpha() {}\n").unwrap();

    let provider = Arc::new(InspectProvider {
        rounds: std::sync::Mutex::new(std::collections::VecDeque::from(vec![
            // Parent round 1: calls spawn_agent
            vec![
                Delta::ToolCall(hate_core::ToolCall {
                    id: "sa1".into(),
                    name: hate_core::ToolName::new("spawn_agent"),
                    args: serde_json::json!({
                        "task": "summarize src/main.rs",
                        "tools": ["read_file"]
                    }),
                }),
                Delta::Done,
            ],
            // Child round 1: calls read_file
            vec![
                Delta::ToolCall(hate_core::ToolCall {
                    id: "r1".into(),
                    name: hate_core::ToolName::new("read_file"),
                    args: serde_json::json!({"path": "src/main.rs"}),
                }),
                Delta::Done,
            ],
            // Child round 2: final answer
            vec![
                Delta::Text("found fn alpha in src/main.rs".into()),
                Delta::Done,
            ],
            // Parent round 2: final answer
            vec![Delta::Text("parent done".into()), Delta::Done],
        ])),
        reqs: std::sync::Mutex::new(Vec::new()),
    });

    let session = spawn_session(SessionConfig {
        provider: provider.clone(),
        model: "test".to_string(),
        provider_name: "mock".to_string(),
        thinking: hate_core::ThinkingLevel::Off,
        limits: Limits::default(),
        ledger: None,
        persist: None,
        tools: Some(Arc::new(
            hate_core::WorkspaceTools::new(dir.path()).unwrap(),
        )),
        permissions: Arc::new(hate_core::PermissionHub::default()),
        mcp: None,
        skills: None,
        auto_skills: false,
        terminal: None,
        context_window: 0,
    });
    let mut rx = session.subscribe();
    drive_turn(&session, &mut rx, "SECRET_PARENT_TOKEN investigate").await;

    let reqs = provider.reqs.lock().unwrap();
    assert_eq!(reqs.len(), 4);
    // Parent round 1: repo_map is in S4 messages, NOT in S0 system prompt.
    let parent_sys = reqs[0].0.as_deref().unwrap_or("");
    assert!(!parent_sys.contains("repo_map"), "{parent_sys}");
    assert!(reqs[0].1.contains("[repo_map"), "{}", reqs[0].1);
    assert!(reqs[0].1.contains("src/main.rs"), "{}", reqs[0].1);

    // Child subagent has isolated history (no parent prompt token) and only 1 allowed tool.
    assert!(!reqs[1].1.contains("SECRET_PARENT_TOKEN"), "{}", reqs[1].1);
    assert_eq!(reqs[1].2, 1);

    // Parent round 2 received the subagent summary.
    assert!(
        reqs[3].1.contains("found fn alpha in src/main.rs"),
        "{}",
        reqs[3].1
    );
}

#[tokio::test]
async fn verification_loop_reenters_on_new_failure_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"verify_tmp\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn ok() -> i32 { 1 }\n").unwrap();

    let (provider, session) = tool_session(
        dir.path(),
        vec![
            // Round 1: introduce a compile error in src/lib.rs
            vec![
                Delta::ToolCall(hate_core::ToolCall {
                    id: "w1".into(),
                    name: hate_core::ToolName::new("write_file"),
                    args: serde_json::json!({
                        "path": "src/lib.rs",
                        "content": "pub fn ok() -> i32 { \"broken\" }\n"
                    }),
                }),
                Delta::Done,
            ],
            // Round 2: tries to finish without checking; verification should force another round
            vec![Delta::Text("looks good".into()), Delta::Done],
            // Round 3: receives verification failure and fixes src/lib.rs
            vec![
                Delta::ToolCall(hate_core::ToolCall {
                    id: "w2".into(),
                    name: hate_core::ToolName::new("write_file"),
                    args: serde_json::json!({
                        "path": "src/lib.rs",
                        "content": "pub fn ok() -> i32 { 2 }\n"
                    }),
                }),
                Delta::Done,
            ],
            // Round 4: finishes cleanly after verification passes
            vec![Delta::Text("fixed and verified".into()), Delta::Done],
        ],
        Limits::default(),
    );

    let mut rx = session.subscribe();
    session
        .command(Command::user("update ok()".to_string()))
        .expect("command");
    let mut final_text = String::new();
    loop {
        match rx.recv().await.expect("stream") {
            Event::PermissionRequested { request, .. } => {
                session
                    .command(Command::RespondPermission {
                        request_id: request.id,
                        decision: "allow_session".to_string(),
                    })
                    .expect("perm");
            }
            Event::Delta { text } => final_text.push_str(&text),
            Event::TurnComplete { .. } => {
                assert!(final_text.contains("fixed and verified"), "{final_text}");
                break;
            }
            Event::Failed { reason } => panic!("turn failed: {reason:?}"),
            _ => {}
        }
    }

    let seen = provider.seen.lock().unwrap();
    assert_eq!(seen.len(), 4);
    assert!(
        seen[2].contains("[verification failed"),
        "expected verification feedback on round 3, got: {}",
        seen[2]
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
        "pub fn ok() -> i32 { 2 }\n"
    );
}

#[tokio::test]
async fn mid_turn_steer_injects_between_tool_rounds() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello world\n").unwrap();

    let (provider, session) = tool_session(
        dir.path(),
        vec![
            vec![read_call("r1", "a.txt"), Delta::Done],
            vec![Delta::Text("steered ok".into()), Delta::Done],
        ],
        Limits::default(),
    );
    let mut rx = session.subscribe();
    session
        .command(Command::user("initial task".to_string()))
        .expect("user");

    let mut steered = false;
    loop {
        match rx.recv().await.expect("stream") {
            Event::ToolStarted { .. } if !steered => {
                steered = true;
                session
                    .command(Command::Steer {
                        text: "FOCUS_ON_ERROR_HANDLING".to_string(),
                    })
                    .expect("steer");
            }
            Event::TurnComplete { .. } => break,
            Event::Failed { reason } => panic!("turn failed: {reason:?}"),
            _ => {}
        }
    }

    assert!(steered);
    let seen = provider.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(
        seen[1].contains("[steering note from user]: FOCUS_ON_ERROR_HANDLING"),
        "{}",
        seen[1]
    );
}

#[tokio::test]
async fn queued_follow_up_runs_after_turn_completes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "alpha\n").unwrap();

    let (provider, session) = tool_session(
        dir.path(),
        vec![
            vec![read_call("r1", "a.txt"), Delta::Done],
            vec![Delta::Text("first done".into()), Delta::Done],
            vec![Delta::Text("second done".into()), Delta::Done],
        ],
        Limits::default(),
    );
    let mut rx = session.subscribe();
    session
        .command(Command::user("first task".to_string()))
        .expect("first");

    let mut queued = false;
    let mut completions = 0usize;
    loop {
        match rx.recv().await.expect("stream") {
            Event::ToolStarted { .. } if !queued => {
                queued = true;
                session
                    .command(Command::QueueUserMessage {
                        text: "second queued task".to_string(),
                        images: Vec::new(),
                    })
                    .expect("queue");
            }
            Event::TurnComplete { outcome } => {
                assert!(matches!(outcome.outcome, Outcome::Completed));
                completions += 1;
                if completions == 2 {
                    break;
                }
            }
            Event::Failed { reason } => panic!("turn failed: {reason:?}"),
            _ => {}
        }
    }

    let seen = provider.seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert!(seen[2].contains("second queued task"), "{}", seen[2]);
}
