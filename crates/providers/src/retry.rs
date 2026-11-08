//! Retry policy: 3 attempts, exponential backoff + jitter, only on
//! transient failures (429/5xx/connect). Once *any* delta has been
//! yielded the stream is committed — retrying mid-response would
//! duplicate text, so later errors surface immediately (spec §14, §28.13:
//! retries are bounded, never unbounded).

use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::BoxStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use hate_core::provider::{ChatRequest, Delta, Provider, ProviderError};

/// Total attempts per request (initial try + retries).
const MAX_ATTEMPTS: u32 = 3;
/// First backoff step; doubles each attempt, capped at 8s.
const BASE_DELAY: Duration = Duration::from_millis(400);
const MAX_DELAY: Duration = Duration::from_secs(8);

/// Whether an error is worth retrying. Auth, decode, and cancel are
/// never retried; 429/5xx/network are (spec §14).
fn retriable(e: &ProviderError) -> bool {
    match e {
        ProviderError::Network(_) => true,
        ProviderError::Http { status, .. } => *status == 429 || *status >= 500,
        ProviderError::Auth(_)
        | ProviderError::Decode(_)
        | ProviderError::Cancelled
        | ProviderError::Stream(_) => false,
    }
}

/// Backoff for attempt `n` (1-based): `BASE * 2^(n-1)` + up to 25% jitter,
/// honoring a server `Retry-After` when the error carried one.
fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(d) = retry_after {
        return d.min(MAX_DELAY);
    }
    let exp = BASE_DELAY.saturating_mul(1u32 << attempt.saturating_sub(1).min(5));
    let jitter = Duration::from_millis((fastrand::u64(0..=exp.as_millis() as u64 / 4)) as u64);
    exp.saturating_add(jitter).min(MAX_DELAY)
}

/// Wrap any provider so every `stream()` call gets the retry policy.
/// Cheap to construct; the inner provider is shared.
pub struct RetryingProvider {
    inner: Arc<dyn Provider>,
}

impl RetryingProvider {
    /// Wrap `inner` with the standard retry policy.
    pub fn new(inner: Arc<dyn Provider>) -> Self {
        Self { inner }
    }
}

impl Provider for RetryingProvider {
    fn stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> BoxStream<'static, Result<Delta, ProviderError>> {
        let inner = Arc::clone(&self.inner);
        let (tx, mut rx) = mpsc::channel::<Result<Delta, ProviderError>>(64);

        tokio::spawn(async move {
            let mut attempt: u32 = 0;
            // Once any Ok(delta) is forwarded the response is committed —
            // a later failure must surface, not restart.
            let mut committed = false;

            loop {
                attempt += 1;
                let mut stream = inner.stream(req.clone(), cancel.clone());
                let mut failure: Option<ProviderError> = None;

                while let Some(item) = futures_util::StreamExt::next(&mut stream).await {
                    match item {
                        Ok(delta) => {
                            committed = true;
                            let is_done = matches!(delta, Delta::Done);
                            if tx.send(Ok(delta)).await.is_err() {
                                return; // consumer gone
                            }
                            if is_done {
                                return;
                            }
                        }
                        Err(e) => {
                            failure = Some(e);
                            break;
                        }
                    }
                }

                let Some(err) = failure else {
                    return; // clean end without Done — inner's contract issue, pass through
                };

                if committed || attempt >= MAX_ATTEMPTS || !retriable(&err) {
                    let _ = tx.send(Err(err)).await;
                    return;
                }

                let delay = backoff(attempt, retry_after_of(&err));
                warn!(attempt, delay_ms = delay.as_millis(), error = %err, "provider retry");
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = tokio::time::sleep(delay) => {}
                }
            }
        });

        Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
    }

    fn count_tokens<'a>(
        &'a self,
        req: &'a ChatRequest,
    ) -> hate_core::provider::BoxFuture<'a, Result<u64, ProviderError>> {
        self.inner.count_tokens(req)
    }

    fn list_models(
        &self,
    ) -> hate_core::provider::BoxFuture<'_, Result<Vec<hate_core::ModelInfo>, ProviderError>> {
        self.inner.list_models()
    }

    fn capabilities(&self) -> hate_core::provider::Capabilities {
        self.inner.capabilities()
    }
}

fn retry_after_of(e: &ProviderError) -> Option<Duration> {
    match e {
        ProviderError::Http { retry_after, .. } => *retry_after,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retriable_classification() {
        assert!(retriable(&ProviderError::Network("x".into())));
        assert!(retriable(&ProviderError::Http {
            status: 429,
            message: String::new(),
            retry_after: None,
        }));
        assert!(retriable(&ProviderError::Http {
            status: 503,
            message: String::new(),
            retry_after: None,
        }));
        assert!(!retriable(&ProviderError::Http {
            status: 400,
            message: String::new(),
            retry_after: None,
        }));
        assert!(!retriable(&ProviderError::Auth("x".into())));
        assert!(!retriable(&ProviderError::Decode("x".into())));
        assert!(!retriable(&ProviderError::Stream("x".into())));
        assert!(!retriable(&ProviderError::Cancelled));
    }

    #[test]
    fn backoff_grows_and_stays_bounded() {
        let d1 = backoff(1, None);
        let d3 = backoff(3, None);
        let d9 = backoff(9, None);
        assert!(d1 >= BASE_DELAY && d1 <= BASE_DELAY * 2);
        assert!(d3 > d1, "exponential growth: {d1:?} vs {d3:?}");
        assert!(d9 <= MAX_DELAY, "capped at MAX_DELAY: {d9:?}");
    }

    #[test]
    fn retry_after_wins_but_is_capped() {
        let short = backoff(1, Some(Duration::from_millis(10)));
        assert_eq!(short, Duration::from_millis(10));
        let long = backoff(1, Some(Duration::from_secs(3600)));
        assert_eq!(long, MAX_DELAY, "server hint capped at MAX_DELAY");
    }
}
