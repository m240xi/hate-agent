use std::sync::OnceLock;

#[derive(Debug, thiserror::Error)]
pub enum CountError {
    #[error("tokenizer error: {0}")]
    Tokenizer(String),
}

pub trait TokenCounter: Send + Sync {
    fn count<'a>(
        &'a self,
        text: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64, CountError>> + Send + 'a>>;
}

pub struct O200kCounter {
    bpe: &'static tiktoken_rs::CoreBPE,
}

impl O200kCounter {
    pub fn new() -> Result<Self, CountError> {
        static BPE: OnceLock<Result<tiktoken_rs::CoreBPE, String>> = OnceLock::new();
        let bpe = BPE
            .get_or_init(|| tiktoken_rs::o200k_base().map_err(|e| e.to_string()))
            .as_ref()
            .map_err(|e| CountError::Tokenizer(e.clone()))?;
        Ok(Self { bpe })
    }

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
        let n = futures_count(&counter, "hello world");
        assert!(n > 0 && n < 100, "sanity range, got {n}");
        let longer = futures_count(&counter, &"word ".repeat(1000));
        assert!(longer > n, "more text must count more tokens");
    }

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
