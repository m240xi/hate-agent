use std::sync::Arc;

use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::io::StreamReader;

use hate_core::ProviderError;

#[derive(Debug, Clone)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
}

pub fn parse(
    response: reqwest::Response,
) -> impl futures_util::Stream<Item = Result<SseFrame, ProviderError>> {
    let (tx, mut rx) = mpsc::channel::<Result<SseFrame, ProviderError>>(64);

    tokio::spawn(async move {
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
                    let _ = event.take();
                } else {
                    let frame = SseFrame {
                        event: event.take(),
                        data: data.join("\n"),
                    };
                    data.clear();
                    if tx.send(Ok(frame)).await.is_err() {
                        return;
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
                    } else if let Some((field, value)) = line.split_once(':') {
                        let value = value.strip_prefix(' ').unwrap_or(value);
                        match field {
                            "event" => event = Some(value.to_string()),
                            "data" => data.push(value.to_string()),
                            _ => {}
                        }
                    }
                }
                Ok(None) => {
                    dispatch!();
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

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("hate-agent/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(10))
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

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

pub type SharedCredential = Arc<dyn hate_core::CredentialSource>;
