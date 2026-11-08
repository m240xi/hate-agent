//! Minimal SSE (Server-Sent Events) frame parser.
//!
//! Both OpenAI-style (`data:` only) and Anthropic-style (`event:` +
//! `data:`) streams ride on this. The parser is deliberately small: it
//! handles multi-line `data:`, ignores comments/heartbeats (`:` lines)
//! and unknown fields, and yields one [`SseFrame`] per blank-line
//! dispatch. `[DONE]` is *not* special-cased — adapters decide what it
//! means for their vendor.

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::io::StreamReader;

use hate_core::ProviderError;

/// One dispatched SSE event.
#[derive(Debug, Clone)]
pub struct SseFrame {
    /// Value of the `event:` field, when present (Anthropic uses this;
    /// OpenAI-compatible streams usually omit it).
    pub event: Option<String>,
    /// Joined `data:` payload (multi-line data joins with `\n`).
    pub data: String,
}

/// Parse an HTTP response body into SSE frames.
///
/// Runs a reader task internally; the returned stream yields
/// `Result<SseFrame, ProviderError>` and ends when the body ends or the
/// consumer is dropped. IO failures surface as [`ProviderError::Stream`].
pub fn parse(
    response: reqwest::Response,
) -> impl futures_util::Stream<Item = Result<SseFrame, ProviderError>> {
    let (tx, mut rx) = mpsc::channel::<Result<SseFrame, ProviderError>>(64);

    tokio::spawn(async move {
        // StreamReader needs the stream's error type to convert into
        // io::Error; reqwest::Error doesn't, so map it here.
        let body = response
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other));
        let stream = StreamReader::new(body);
        let mut lines = BufReader::new(stream).lines();
        let mut event: Option<String> = None;
        let mut data: Vec<String> = Vec::new();

        macro_rules! dispatch {
            () => {
                if data.is_empty() {
                    // No data accumulated — drop any stale event name too.
                    let _ = event.take();
                } else {
                    let frame = SseFrame {
                        event: event.take(),
                        data: data.join("\n"),
                    };
                    data.clear();
                    if tx.send(Ok(frame)).await.is_err() {
                        return; // consumer gone
                    }
                }
            };
        }

        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let line = line.strip_suffix('\r').unwrap_or(&line);
                    if line.is_empty() {
                        dispatch!();
                    } else if line.starts_with(':') {
                        // Comment / heartbeat — ignore.
                    } else if let Some((field, value)) = line.split_once(':') {
                        let value = value.strip_prefix(' ').unwrap_or(value);
                        match field {
                            "event" => event = Some(value.to_string()),
                            "data" => data.push(value.to_string()),
                            _ => {} // id:, retry:, unknown — ignored
                        }
                    }
                    // Lines without ':' are ignored per the SSE spec.
                }
                Ok(None) => {
                    dispatch!(); // flush a trailing unterminated frame
                    return;
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(ProviderError::Stream(format!("sse read: {e}"))))
                        .await;
                    return;
                }
            }
        }
    });

    futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx))
}

/// Shared reqwest client with sane timeouts for streaming APIs.
/// Connect must fail fast; the stream itself has no total timeout —
/// cancellation is the caller's job.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Map an HTTP error response to a typed provider error. Honors
/// `Retry-After` (delta-seconds form) so the retry layer can wait exactly
/// as the server asked.
pub async fn status_error(resp: reqwest::Response) -> ProviderError {
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(std::time::Duration::from_secs);
    let message = resp.text().await.unwrap_or_default();
    let snippet: String = message.chars().take(300).collect();
    match status {
        401 | 403 => ProviderError::Auth(format!("HTTP {status}: {snippet}")),
        _ => ProviderError::Http {
            status,
            message: snippet,
            retry_after,
        },
    }
}

/// A credential source boxed for adapters — re-exported so adapters and
/// auth agree on one type without importing each other.
pub type SharedCredential = Arc<dyn hate_core::CredentialSource>;
