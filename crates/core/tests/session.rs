//! Session actor tests: turn lifecycle, event stream, usage accounting,
//! and the 250ms-class cancellation path — all against a scripted
//! in-process provider (no network, deterministic).

use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::BoxStream;
use hate_core::provider::{
    BoxFuture, CacheMode, Capabilities, ChatRequest, Delta, ModelInfo, Provider, ProviderError,
};
use hate_core::{Command, Event, Limits, Outcome, SessionConfig, TokenUsage, spawn_session};
use tokio_util::sync::CancellationToken;

/// Scripted provider: emits `script` deltas with optional per-delta
/// delay, honoring cancellation between deltas.
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
        .command(Command::UserMessage("hi".to_string()))
        .expect("command");

    let mut text = String::new();
    let mut saw_usage = false;
    // Generous deadline so a hung actor fails the test, not the suite.
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
            .command(Command::UserMessage("hi".to_string()))
            .expect("command");
        // Drain until TurnComplete.
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
    // 100 deltas × 50ms = 5s of stream; cancel must land near-instantly.
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
        .command(Command::UserMessage("go".to_string()))
        .expect("command");

    // Wait for the first delta so the turn is definitely in flight.
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
        .command(Command::UserMessage("hi".to_string()))
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

/// Multi-round scripted provider: each `stream()` call replays the next
/// script and records the request's messages so tests can assert what
/// the loop fed back.
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
        context_window: 0,
    });
    let mut rx = session.subscribe();
    session
        .command(Command::UserMessage("read note.txt".to_string()))
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

    // Two provider requests: round 2 must contain the tool result.
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
        .command(Command::UserMessage("hi".to_string()))
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
                    args: json!({"command": "echo hi"}),
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
        context_window: 0,
    });
    let mut rx = session.subscribe();
    session
        .command(Command::UserMessage("run echo".to_string()))
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

    // Round 2 must carry the denial text back to the model.
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

    // Two identical gated calls across two rounds — after allow_session
    // on the first, the second must run without another prompt.
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
        context_window: 0,
    });
    let mut rx = session.subscribe();
    session
        .command(Command::UserMessage("write twice".to_string()))
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

// ── context compaction ────────────────────────────────────────────────────

/// Journal that retains the last snapshot for assertions.
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

/// The risky path: compaction mutates `convo` inside `run_turn`, and the
/// actor must adopt the *compacted* history — not rebuild it from the
/// turn's appended tail. This drives enough turns to force compaction
/// and asserts the journaled history stays bounded with a note present.
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
        // Tiny window → compacts as soon as history reaches the tail+4
        // minimum; the system prompt alone already exceeds 72% of it.
        context_window: 400,
    });
    let mut rx = session.subscribe();

    for i in 0..14 {
        session
            .command(Command::UserMessage(format!("turn number {i}")))
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
            Message::User { content } if content.contains("[Context compacted]")
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
