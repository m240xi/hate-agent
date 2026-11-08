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
        limits: Limits::default(),
        ledger: None,
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
