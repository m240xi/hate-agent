//! Shared test helper: a one-shot HTTP/1.1 server on localhost.
//!
//! Each test binds an ephemeral port, serves exactly one request with a
//! canned response, and returns the base URL to point a provider at.
//! No external test deps — raw `tokio::net` is enough for SSE fixtures.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serve `body` once with `status`/`content_type`; returns `http://addr/v1`.
/// The spawned task ends after one response (or client disconnect).
pub async fn serve_once(status: u16, content_type: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        // Read request head + declared body so the client sees a clean
        // response instead of a reset mid-upload.
        let mut buf = Vec::with_capacity(8192);
        let header_end;
        let content_length;
        loop {
            let mut chunk = [0u8; 8192];
            let n = match sock.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                header_end = pos + 4;
                let head = String::from_utf8_lossy(&buf[..pos]);
                content_length = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.trim()
                            .eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                break;
            }
            if buf.len() > 1_000_000 {
                return;
            }
        }
        while buf.len() < header_end + content_length {
            let mut chunk = [0u8; 8192];
            match sock.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        let response = format!(
            "HTTP/1.1 {status} X\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = sock.write_all(response.as_bytes()).await;
        let _ = sock.shutdown().await;
    });
    format!("http://{addr}/v1")
}

/// Serve `body` but never respond — for cancellation tests. Returns the
/// base URL; the connection is held open until the test runtime ends.
#[allow(dead_code)] // unused in test crates that never cancel
pub async fn serve_hang() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            // Hold the socket open without responding.
            tokio::spawn(async move {
                let _held = sock;
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            });
        }
    });
    format!("http://{addr}/v1")
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}
