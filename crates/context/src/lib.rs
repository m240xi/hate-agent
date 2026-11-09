//! Token counting. `chars / 4` estimation is forbidden whenever a real
//! counter exists — every counter here is real: a bundled BPE for
//! OpenAI-compatible/local models, provider `count_tokens` endpoints for
//! Anthropic/Gemini (implemented in `hate-providers`, which owns HTTP).

use std::sync::OnceLock;

/// Why counting failed. Counting is best-effort infrastructure: callers
/// decide whether a failure is fatal (it almost never is — a wrong count
/// degrades compaction timing, it does not corrupt data).
#[derive(Debug, thiserror::Error)]
pub enum CountError {
    /// The underlying tokenizer failed to load or encode.
    #[error("tokenizer error: {0}")]
    Tokenizer(String),
}

/// Counts tokens in text. Implementations may be local (BPE) or remote
/// (provider `count_tokens` APIs); both are async so remote counters fit
/// the same trait.
pub trait TokenCounter: Send + Sync {
    /// Number of tokens `text` will occupy for this counter's model family.
    fn count<'a>(
        &'a self,
        text: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64, CountError>> + Send + 'a>>;
}

/// `o200k_base` BPE counter — the sanctioned approximation for
/// OpenAI-compatible and local models. The BPE table is loaded once and
/// shared; encoding is CPU-bound and fast enough to run inline.
pub struct O200kCounter {
    bpe: &'static tiktoken_rs::CoreBPE,
}

impl O200kCounter {
    /// Load the o200k BPE table (once per process).
    ///
    /// # Errors
    /// Returns [`CountError::Tokenizer`] if the embedded ranks cannot load.
    pub fn new() -> Result<Self, CountError> {
        static BPE: OnceLock<Result<tiktoken_rs::CoreBPE, String>> = OnceLock::new();
        let bpe = BPE
            .get_or_init(|| tiktoken_rs::o200k_base().map_err(|e| e.to_string()))
            .as_ref()
            .map_err(|e| CountError::Tokenizer(e.clone()))?;
        Ok(Self { bpe })
    }

    /// Synchronous count for CPU-adjacent callers (tool output shaping)
    /// that can't await the trait's boxed future.
    pub fn count_sync(&self, text: &str) -> u64 {
        self.bpe.encode_ordinary(text).len() as u64
    }
}

impl TokenCounter for O200kCounter {
    fn count<'a>(
        &'a self,
        text: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64, CountError>> + Send + 'a>>
    {
        let n = self.bpe.encode_ordinary(text).len() as u64;
        Box::pin(async move { Ok(n) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o200k_counts_real_tokens() {
        let counter = O200kCounter::new().expect("o200k loads");
        // "hello world" is 2 tokens in o200k; the assertion is deliberately
        // loose — we test that counting works, not the exact vocabulary.
        let n = futures_count(&counter, "hello world");
        assert!(n > 0 && n < 100, "sanity range, got {n}");
        let longer = futures_count(&counter, &"word ".repeat(1000));
        assert!(longer > n, "more text must count more tokens");
    }

    /// Poll the count future once — it is always immediately ready because
    /// the BPE encode is pure CPU work wrapped in `async move`.
    fn futures_count(counter: &O200kCounter, text: &str) -> u64 {
        use std::task::{Context, Poll, Waker};

        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut fut = counter.count(text);
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(Ok(n)) => n,
            other => panic!("count future not ready: {other:?}"),
        }
    }
}
