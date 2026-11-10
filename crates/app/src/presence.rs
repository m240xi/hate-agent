use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

const APP_ID: &str = "1459800355243163846";
const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;
const RECONNECT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(400);

#[derive(Debug, Clone)]
struct Activity {
    state: String,
}

enum Msg {
    Set(String),
    Enabled(bool),
}

pub struct Presence {
    tx: mpsc::Sender<Msg>,
    last_state: Arc<std::sync::Mutex<String>>,
    enabled: Arc<std::sync::atomic::AtomicBool>,
}

impl Presence {
    pub fn start(enabled: bool) -> Self {
        let start_ms = now_ms();
        let (tx, rx) = mpsc::channel(16);
        let last_state = Arc::new(std::sync::Mutex::new("Idle".to_string()));
        let enabled_flag = Arc::new(std::sync::atomic::AtomicBool::new(enabled));
        tauri::async_runtime::spawn(run(
            rx,
            start_ms,
            Arc::clone(&last_state),
            Arc::clone(&enabled_flag),
        ));
        Self {
            tx,
            last_state,
            enabled: enabled_flag,
        }
    }

    pub fn set_mode(&self, state: impl Into<String>) {
        let state = state.into();
        if let Ok(mut last) = self.last_state.lock() {
            last.clone_from(&state);
        }
        let _ = self.tx.try_send(Msg::Set(state));
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
        let _ = self.tx.try_send(Msg::Enabled(enabled));
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

async fn run(
    mut rx: mpsc::Receiver<Msg>,
    start_ms: u64,
    last_state: Arc<std::sync::Mutex<String>>,
    enabled: Arc<std::sync::atomic::AtomicBool>,
) {
    let mut activity = Activity {
        state: last_state
            .lock()
            .map_or_else(|_| "Idle".to_string(), |s| s.clone()),
    };
    loop {
        if !enabled.load(Ordering::SeqCst) {
            match rx.recv().await {
                Some(Msg::Set(s)) => activity.state = s,
                Some(Msg::Enabled(true)) => {}
                Some(Msg::Enabled(false)) => continue,
                None => return,
            }
            continue;
        }
        if let Ok(mut conn) = connect().await
            && handshake(&mut conn).await
        {
            let _ = send_activity(&mut conn, Some(&activity), start_ms).await;
            let mut buf: Vec<u8> = Vec::new();
            let mut scratch = [0u8; 2048];
            'live: loop {
                while let Ok(msg) = rx.try_recv() {
                    match msg {
                        Msg::Set(s) => {
                            activity.state = s;
                            if send_activity(&mut conn, Some(&activity), start_ms)
                                .await
                                .is_err()
                            {
                                break 'live;
                            }
                        }
                        Msg::Enabled(false) => {
                            let _ = send_activity(&mut conn, None, start_ms).await;
                            break 'live;
                        }
                        Msg::Enabled(true) => {}
                    }
                }
                match tokio::time::timeout(POLL, conn.read(&mut scratch)).await {
                    Ok(Ok(0) | Err(_)) => break 'live,
                    Ok(Ok(n)) => {
                        buf.extend_from_slice(&scratch[..n]);
                        while let Some((op, _body)) = take_frame(&mut buf) {
                            if op == OP_PING {
                                let _ = write_frame(&mut conn, OP_PONG, b"{}").await;
                            } else if op == OP_CLOSE {
                                break 'live;
                            }
                        }
                    }
                    Err(_) => {}
                }
            }
        }
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(Msg::Set(s)) => activity.state = s,
                Some(Msg::Enabled(_)) => {}
                None => return,
            },
            () = tokio::time::sleep(RECONNECT) => {}
        }
    }
}

fn take_frame(buf: &mut Vec<u8>) -> Option<(u32, Vec<u8>)> {
    if buf.len() < 8 {
        return None;
    }
    let op = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let len = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    if buf.len() < 8 + len {
        return None;
    }
    let body = buf[8..8 + len].to_vec();
    buf.drain(..8 + len);
    Some((op, body))
}

async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    op: u32,
    body: &[u8],
) -> std::io::Result<()> {
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend_from_slice(&op.to_le_bytes());
    frame.extend_from_slice(&(u32::try_from(body.len()).unwrap_or(0)).to_le_bytes());
    frame.extend_from_slice(body);
    w.write_all(&frame).await?;
    w.flush().await
}

async fn handshake<R: AsyncRead + AsyncWrite + Unpin>(conn: &mut R) -> bool {
    let payload = json!({"v": 1, "client_id": APP_ID}).to_string();
    if write_frame(conn, OP_HANDSHAKE, payload.as_bytes())
        .await
        .is_err()
    {
        return false;
    }
    let mut buf: Vec<u8> = Vec::new();
    let mut scratch = [0u8; 2048];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        while let Some((op, body)) = take_frame(&mut buf) {
            if op == OP_FRAME
                && serde_json::from_slice::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v.get("evt").and_then(|e| e.as_str()).map(str::to_string))
                    .as_deref()
                    == Some("READY")
            {
                return true;
            }
            if op == OP_CLOSE {
                return false;
            }
            if op == OP_FRAME {
                let evt = serde_json::from_slice::<serde_json::Value>(&body).ok();
                let name = evt
                    .as_ref()
                    .and_then(|v| v.get("evt").and_then(|e| e.as_str()))
                    .unwrap_or("");
                if name == "ERROR" {
                    tracing::warn!(detail = %String::from_utf8_lossy(&body), "discord rejected presence handshake");
                    return false;
                }
            }
        }
        match tokio::time::timeout(POLL, conn.read(&mut scratch)).await {
            Ok(Ok(0) | Err(_)) => return false,
            Ok(Ok(n)) => buf.extend_from_slice(&scratch[..n]),
            Err(_) => {}
        }
    }
    false
}

async fn send_activity<W: AsyncWrite + Unpin>(
    conn: &mut W,
    activity: Option<&Activity>,
    start_ms: u64,
) -> std::io::Result<()> {
    let pid = std::process::id();
    let mut args = json!({ "pid": pid });
    if let Some(a) = activity {
        args["activity"] = json!({
            "details": "hate-agent",
            "state": a.state,
            "timestamps": { "start": start_ms },
            "assets": {
                "large_image": "https://i.pinimg.com/736x/85/58/42/8558427d815793b8b964c7e19292c90a.jpg",
                "large_text": "hate-agent",
            },
            "instance": true,
        });
    } else {
        args["activity"] = serde_json::Value::Null;
    }
    let payload = json!({
        "cmd": "SET_ACTIVITY",
        "args": args,
        "nonce": uuid::Uuid::new_v4().to_string(),
    })
    .to_string();
    write_frame(conn, OP_FRAME, payload.as_bytes()).await
}

#[cfg(windows)]
type Conn = tokio::net::windows::named_pipe::NamedPipeClient;

#[cfg(windows)]
fn pipe_path(i: u32) -> String {
    if cfg!(test) {
        format!(r"\\.\pipe\hate-agent-test-ipc-{i}")
    } else {
        format!(r"\\.\pipe\discord-ipc-{i}")
    }
}

#[cfg(windows)]
async fn connect() -> std::io::Result<Conn> {
    tokio::task::spawn_blocking(|| {
        let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no discord pipe");
        for i in 0..10 {
            let path = pipe_path(i);
            match tokio::net::windows::named_pipe::ClientOptions::new().open(&path) {
                Ok(c) => return Ok(c),
                Err(e) => last = e,
            }
        }
        Err(last)
    })
    .await
    .map_err(std::io::Error::other)?
}

#[cfg(unix)]
type Conn = tokio::net::UnixStream;

#[cfg(unix)]
async fn connect() -> std::io::Result<Conn> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    for var in ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"] {
        if let Ok(dir) = std::env::var(var) {
            dirs.push(std::path::PathBuf::from(dir));
        }
    }
    dirs.push(std::path::PathBuf::from("/tmp"));
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no discord socket");
    for dir in dirs {
        for i in 0..10 {
            let path = dir.join(format!("discord-ipc-{i}"));
            match tokio::net::UnixStream::connect(&path).await {
                Ok(s) => return Ok(s),
                Err(e) => last = e,
            }
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_works_without_an_ambient_runtime() {
        let presence = Presence::start(false);
        presence.set_mode("In Chat");
        presence.set_enabled(false);
    }

    #[test]
    fn frames_roundtrip_and_split() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&OP_FRAME.to_le_bytes());
        buf.extend_from_slice(&5u32.to_le_bytes());
        buf.extend_from_slice(b"hello");
        assert_eq!(take_frame(&mut buf), Some((OP_FRAME, b"hello".to_vec())));
        assert!(take_frame(&mut buf).is_none());
        buf.extend_from_slice(&OP_PING.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(take_frame(&mut buf), Some((OP_PING, Vec::new())));
    }

    #[test]
    fn partial_frame_waits() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&OP_FRAME.to_le_bytes());
        buf.extend_from_slice(&10u32.to_le_bytes());
        buf.extend_from_slice(b"abc");
        assert!(take_frame(&mut buf).is_none());
        assert_eq!(buf.len(), 11);
        buf.extend_from_slice(b"defghij");
        assert_eq!(
            take_frame(&mut buf),
            Some((OP_FRAME, b"abcdefghij".to_vec()))
        );
    }

    #[cfg(windows)]
    async fn read_owned<R: AsyncRead + Unpin>(r: &mut R) -> (u32, Vec<u8>) {
        let mut head = [0u8; 8];
        r.read_exact(&mut head).await.unwrap();
        let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        let mut body = vec![0u8; len];
        r.read_exact(&mut body).await.unwrap();
        (op, body)
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn speaks_the_ipc_protocol_to_a_fake_discord() {
        use tokio::net::windows::named_pipe::ServerOptions;
        let pipe = pipe_path(0);
        let Ok(server) = ServerOptions::new().first_pipe_instance(true).create(pipe) else {
            eprintln!("named pipes unavailable here; skipping protocol test");
            return;
        };
        let server_task = tokio::spawn(async move {
            let mut s = server;
            s.connect().await.unwrap();
            let (op, body) = read_owned(&mut s).await;
            assert_eq!(op, OP_HANDSHAKE);
            let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(v["v"], 1);
            assert_eq!(v["client_id"], APP_ID);
            write_frame(
                &mut s,
                OP_FRAME,
                br#"{"cmd":"DISPATCH","evt":"READY","data":{}}"#,
            )
            .await
            .unwrap();
            let (op, body) = read_owned(&mut s).await;
            assert_eq!(op, OP_FRAME);
            let first: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(first["cmd"], "SET_ACTIVITY");
            assert_eq!(first["args"]["activity"]["details"], "hate-agent");
            assert!(
                first["args"]["activity"]["assets"]["large_image"]
                    .as_str()
                    .unwrap()
                    .starts_with("https://")
            );
            assert!(
                first["args"]["activity"]["timestamps"]["start"]
                    .as_u64()
                    .unwrap()
                    > 1_600_000_000_000
            );
            let (_, body) = read_owned(&mut s).await;
            let second: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(second["args"]["activity"]["state"], "In Code");
        });
        let presence = Presence::start(true);
        presence.set_mode("In Code");
        tokio::time::timeout(Duration::from_secs(8), server_task)
            .await
            .expect("fake discord timed out")
            .expect("fake discord panicked");
    }

    #[tokio::test]
    async fn activity_json_carries_timer_and_clear() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let act = Activity {
            state: "In Code".to_string(),
        };
        send_activity(&mut a, Some(&act), 1_700_000_000_000)
            .await
            .unwrap();
        let mut head = [0u8; 8];
        b.read_exact(&mut head).await.unwrap();
        let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        let mut body = vec![0u8; len];
        b.read_exact(&mut body).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["cmd"], "SET_ACTIVITY");
        assert_eq!(v["args"]["activity"]["state"], "In Code");
        assert_eq!(
            v["args"]["activity"]["timestamps"]["start"],
            1_700_000_000_000u64
        );

        send_activity(&mut a, None, 0).await.unwrap();
        b.read_exact(&mut head).await.unwrap();
        let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        let mut body = vec![0u8; len];
        b.read_exact(&mut body).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["args"]["activity"].is_null());
    }
}
