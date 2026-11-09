//! `MockProvider` — replays a scripted delta sequence with zero network.
//!
//! Used by session-loop tests, CLI `--dry-run`, and the bench harness's
//! warm-up path. It implements the real `Provider` trait so nothing
//! downstream can tell it apart from a live adapter.

use std::sync::Arc;

use futures_util::stream::BoxStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use hate_context::{O200kCounter, TokenCounter};
use hate_core::TokenUsage;
use hate_core::provider::{
    BoxFuture, CacheMode, Capabilities, ChatRequest, Delta, ModelInfo, Provider, ProviderError,
};

/// A provider that emits a fixed script of deltas.
pub struct MockProvider {
    /// Deltas to emit, in order. [`Delta::Done`] is appended automatically
    /// if the script doesn't end with one.
    pub script: Vec<Delta>,
    /// Usage record emitted just before `Done` (zeros if unset).
    pub usage: TokenUsage,
    /// Artificial per-delta delay — keeps cancellation tests honest.
    pub delta_delay: std::time::Duration,
}

impl MockProvider {
    /// A provider that says `text` and reports `usage`.
    pub fn echo(text: impl Into<String>, usage: TokenUsage) -> Self {
        Self {
            script: vec![Delta::Text(text.into())],
            usage,
            delta_delay: std::time::Duration::ZERO,
        }
    }
}

impl Provider for MockProvider {
    fn stream(
        &self,
        _req: ChatRequest,
        cancel: CancellationToken,
    ) -> BoxStream<'static, Result<Delta, ProviderError>> {
        let mut script = self.script.clone();
        if !matches!(script.last(), Some(Delta::Done)) {
            script.push(Delta::Done);
        }
        let usage = self.usage;
        let delay = self.delta_delay;
        let (tx, mut rx) = mpsc::channel(64);

        tokio::spawn(async move {
            for delta in script {
                if cancel.is_cancelled() {
                    return;
                }
                if delay > std::time::Duration::ZERO {
                    tokio::select! {
                        () = cancel.cancelled() => return,
                        () = tokio::time::sleep(delay) => {}
                    }
                }
                // Inject the usage record right before Done.
                if matches!(delta, Delta::Done) {
                    let _ = tx.send(Ok(Delta::Usage(usage))).await;
                }
                if tx.send(Ok(delta)).await.is_err() {
                    return;
                }
            }
        });

        Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
    }

    fn count_tokens<'a>(
        &'a self,
        req: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<u64, ProviderError>> {
        Box::pin(async move {
            // Same o200k approximation as the OpenAI-compatible adapter —
            // the mock must count like the real thing it stands in for.
            static COUNTER: std::sync::OnceLock<O200kCounter> = std::sync::OnceLock::new();
            let counter =
                COUNTER.get_or_init(|| O200kCounter::new().expect("o200k BPE table is embedded"));
            let mut text = String::new();
            if let Some(s) = &req.system {
                text.push_str(s);
            }
            for m in &req.messages {
                match m {
                    hate_core::Message::User { content }
                    | hate_core::Message::Tool { content, .. } => text.push_str(content),
                    hate_core::Message::Assistant { content, .. } => text.push_str(content),
                }
            }
            counter
                .count(&text)
                .await
                .map_err(|e| ProviderError::Stream(format!("mock count: {e}")))
        })
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>> {
        Box::pin(async move {
            Ok(vec![ModelInfo {
                id: "mock-model".to_string(),
                name: Some("Mock model".to_string()),
                meta: None,
            }])
        })
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

/// Convenience: `Arc<MockProvider>` for `SessionConfig.provider`.
pub fn mock_arc(script: Vec<Delta>, usage: TokenUsage) -> Arc<dyn Provider> {
    Arc::new(MockProvider {
        script,
        usage,
        delta_delay: std::time::Duration::ZERO,
    })
}
