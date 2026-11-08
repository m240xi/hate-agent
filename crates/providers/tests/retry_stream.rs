//! Retry policy tests: classification, backoff bounds, and end-to-end
//! retry over a scripted flaky provider (no network).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use hate_core::provider::{
    BoxFuture, CacheMode, Capabilities, ChatRequest, Delta, ModelInfo, Provider, ProviderError,
};
use hate_providers::retry::RetryingProvider;
use tokio_util::sync::CancellationToken;

fn req() -> ChatRequest {
    ChatRequest {
        model: "m".to_string(),
        system: None,
        messages: Vec::new(),
        tools: Vec::new(),
        max_output_tokens: 16,
        temperature: None,
        thinking: hate_core::ThinkingLevel::Off,
    }
}

/// Fails `failures` times with `err`, then emits `ok` deltas + Done.
struct Flaky {
    failures: usize,
    err: ProviderError,
    calls: Arc<AtomicUsize>,
}

impl Provider for Flaky {
    fn stream(
        &self,
        _req: ChatRequest,
        _cancel: CancellationToken,
    ) -> BoxStream<'static, Result<Delta, ProviderError>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let err = if n < self.failures {
            Some(self.err.clone())
        } else {
            None
        };
        tokio::spawn(async move {
            if let Some(e) = err {
                let _ = tx.send(Err(e)).await;
                return;
            }
            let _ = tx.send(Ok(Delta::Text("ok".to_string()))).await;
            let _ = tx.send(Ok(Delta::Done)).await;
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

#[tokio::test]
async fn retries_transient_then_succeeds() {
    let calls = Arc::new(AtomicUsize::new(0));
    let flaky = Flaky {
        failures: 2,
        err: ProviderError::Http {
            status: 500,
            message: "boom".to_string(),
            retry_after: None,
        },
        calls: Arc::clone(&calls),
    };
    let provider = RetryingProvider::new(Arc::new(flaky));

    let mut stream = provider.stream(req(), CancellationToken::new());
    let mut saw_done = false;
    let mut saw_text = false;
    while let Some(item) = stream.next().await {
        match item.expect("no error after retries") {
            Delta::Text(t) => saw_text = t == "ok",
            Delta::Done => saw_done = true,
            _ => {}
        }
    }
    assert!(saw_text && saw_done);
    assert_eq!(calls.load(Ordering::SeqCst), 3, "initial + 2 retries");
}

#[tokio::test]
async fn gives_up_after_max_attempts() {
    let calls = Arc::new(AtomicUsize::new(0));
    let flaky = Flaky {
        failures: usize::MAX,
        err: ProviderError::Network("down".to_string()),
        calls: Arc::clone(&calls),
    };
    let provider = RetryingProvider::new(Arc::new(flaky));

    let mut stream = provider.stream(req(), CancellationToken::new());
    let mut last = None;
    while let Some(item) = stream.next().await {
        last = Some(item);
    }
    assert!(matches!(last, Some(Err(ProviderError::Network(_)))));
    assert_eq!(calls.load(Ordering::SeqCst), 3, "bounded at MAX_ATTEMPTS");
}

#[tokio::test]
async fn does_not_retry_auth_errors() {
    let calls = Arc::new(AtomicUsize::new(0));
    let flaky = Flaky {
        failures: usize::MAX,
        err: ProviderError::Auth("bad key".to_string()),
        calls: Arc::clone(&calls),
    };
    let provider = RetryingProvider::new(Arc::new(flaky));

    let mut stream = provider.stream(req(), CancellationToken::new());
    let mut last = None;
    while let Some(item) = stream.next().await {
        last = Some(item);
    }
    assert!(matches!(last, Some(Err(ProviderError::Auth(_)))));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "auth is never retried");
}

#[tokio::test]
async fn committed_stream_surfaces_later_errors() {
    // Provider yields a delta, then fails — retry would duplicate text,
    // so the error must surface instead.
    struct CommitThenFail;
    impl Provider for CommitThenFail {
        fn stream(
            &self,
            _req: ChatRequest,
            _cancel: CancellationToken,
        ) -> BoxStream<'static, Result<Delta, ProviderError>> {
            let (tx, mut rx) = tokio::sync::mpsc::channel(8);
            tokio::spawn(async move {
                let _ = tx.send(Ok(Delta::Text("partial".to_string()))).await;
                let _ = tx
                    .send(Err(ProviderError::Http {
                        status: 500,
                        message: "mid-stream".to_string(),
                        retry_after: None,
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

    let provider = RetryingProvider::new(Arc::new(CommitThenFail));
    let mut stream = provider.stream(req(), CancellationToken::new());
    let mut items = Vec::new();
    while let Some(item) = stream.next().await {
        items.push(item);
    }
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], Ok(Delta::Text(_))));
    assert!(matches!(&items[1], Err(ProviderError::Http { .. })));
}
