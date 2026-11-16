use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn closed() -> Self {
        Self {
            code: -32099,
            message: "agent connection closed".to_string(),
            data: None,
        }
    }

    pub fn detail(&self) -> String {
        let inner = self
            .data
            .as_ref()
            .and_then(|d| d.get("message").and_then(Value::as_str));
        match inner {
            Some(m) if !m.is_empty() => format!("{} — {m}", self.message),
            _ => self.message.clone(),
        }
    }

    pub fn is_auth_required(&self) -> bool {
        self.code == -32000 || self.message.to_ascii_lowercase().contains("auth")
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.detail(), self.code)
    }
}

#[derive(Debug, Clone)]
pub enum Incoming {
    Notification {
        method: String,
        params: Value,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

pub struct Connection {
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicU64,
}

impl Connection {
    pub fn new<R, W>(reader: R, writer: W) -> (Arc<Self>, mpsc::UnboundedReceiver<Incoming>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
        let (in_tx, in_rx) = mpsc::unbounded_channel::<Incoming>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        let mut writer = writer;
        tokio::spawn(async move {
            while let Some(line) = out_rx.recv().await {
                if writer.write_all(line.as_bytes()).await.is_err()
                    || writer.write_all(b"\n").await.is_err()
                    || writer.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let pending_r = Arc::clone(&pending);
        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(msg) = serde_json::from_str::<Value>(line) else {
                    tracing::debug!(line, "acp: non-JSON line on stdout");
                    continue;
                };
                route(&msg, &pending_r, &in_tx);
            }
            let drained: Vec<_> = pending_r
                .lock()
                .map_or_default(|mut p| p.drain().map(|(_, tx)| tx).collect());
            for tx in drained {
                let _ = tx.send(Err(RpcError::closed()));
            }
        });

        (
            Arc::new(Self {
                out: out_tx,
                pending,
                next_id: AtomicU64::new(0),
            }),
            in_rx,
        )
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| RpcError::closed())?
            .insert(id, tx);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if self.out.send(msg.to_string()).is_err() {
            self.pending.lock().ok().map(|mut p| p.remove(&id));
            return Err(RpcError::closed());
        }
        rx.await.unwrap_or_else(|_| Err(RpcError::closed()))
    }

    pub fn notify(&self, method: &str, params: Value) {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let _ = self.out.send(msg.to_string());
    }

    pub fn respond(&self, id: Value, result: Result<Value, RpcError>) {
        let msg = match result {
            Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
            Err(e) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": e.code, "message": e.message}})
            }
        };
        let _ = self.out.send(msg.to_string());
    }
}

fn route(msg: &Value, pending: &Pending, incoming: &mpsc::UnboundedSender<Incoming>) {
    let method = msg.get("method").and_then(Value::as_str);
    let id = msg.get("id").filter(|v| !v.is_null());
    match (method, id) {
        (Some(m), Some(id)) => {
            let _ = incoming.send(Incoming::Request {
                id: id.clone(),
                method: m.to_string(),
                params: msg.get("params").cloned().unwrap_or(Value::Null),
            });
        }
        (Some(m), None) => {
            let _ = incoming.send(Incoming::Notification {
                method: m.to_string(),
                params: msg.get("params").cloned().unwrap_or(Value::Null),
            });
        }
        (None, Some(id)) => {
            let Some(n) = id.as_u64() else { return };
            let Some(tx) = pending.lock().ok().and_then(|mut p| p.remove(&n)) else {
                return;
            };
            let res = match msg.get("error") {
                Some(e) => Err(RpcError {
                    code: e.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                    message: e
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("agent error")
                        .to_string(),
                    data: e.get("data").cloned(),
                }),
                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(res);
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader, duplex};

    #[tokio::test]
    async fn request_response_and_inbound_traffic() {
        let (client_side, agent_side) = duplex(64 * 1024);
        let (cr, cw) = tokio::io::split(client_side);
        let (ar, mut aw) = tokio::io::split(agent_side);
        let (conn, mut incoming) = Connection::new(cr, cw);

        let agent = tokio::spawn(async move {
            let mut lines = BufReader::new(ar).lines();
            let line = lines.next_line().await.unwrap().unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(req["method"], "initialize");
            let id = req["id"].clone();
            let notif = json!({"jsonrpc":"2.0","method":"session/update","params":{"x":1}});
            let ask = json!({"jsonrpc":"2.0","id":"p1","method":"session/request_permission","params":{}});
            let resp = json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":1}});
            for m in [notif, ask, resp] {
                aw.write_all(format!("{m}\n").as_bytes()).await.unwrap();
            }
            let line = lines.next_line().await.unwrap().unwrap();
            let reply: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(reply["id"], "p1");
            assert_eq!(reply["result"]["ok"], true);
        });

        let res = conn.request("initialize", json!({})).await.unwrap();
        assert_eq!(res["protocolVersion"], 1);
        match incoming.recv().await.unwrap() {
            Incoming::Notification { method, params } => {
                assert_eq!(method, "session/update");
                assert_eq!(params["x"], 1);
            }
            other @ Incoming::Request { .. } => panic!("unexpected {other:?}"),
        }
        match incoming.recv().await.unwrap() {
            Incoming::Request { id, method, .. } => {
                assert_eq!(method, "session/request_permission");
                conn.respond(id, Ok(json!({"ok": true})));
            }
            other @ Incoming::Notification { .. } => panic!("unexpected {other:?}"),
        }
        agent.await.unwrap();
    }

    #[tokio::test]
    async fn errors_carry_data_and_close_fails_pending() {
        let (client_side, agent_side) = duplex(64 * 1024);
        let (cr, cw) = tokio::io::split(client_side);
        let (ar, mut aw) = tokio::io::split(agent_side);
        let (conn, _incoming) = Connection::new(cr, cw);

        let agent = tokio::spawn(async move {
            let mut lines = BufReader::new(ar).lines();
            let line = lines.next_line().await.unwrap().unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            let err = json!({"jsonrpc":"2.0","id":req["id"],"error":{"code":-32000,"message":"Authentication required","data":{"message":"sign in first"}}});
            aw.write_all(format!("{err}\n").as_bytes()).await.unwrap();
            let _ = lines.next_line().await;
        });

        let err = conn.request("session/new", json!({})).await.unwrap_err();
        assert!(err.is_auth_required());
        assert_eq!(err.detail(), "Authentication required — sign in first");

        let pending = {
            let conn = Arc::clone(&conn);
            tokio::spawn(async move { conn.request("session/prompt", json!({})).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        agent.abort();
        let _ = agent.await;
        drop(conn);
        let res = tokio::time::timeout(std::time::Duration::from_secs(2), pending)
            .await
            .expect("pending request must resolve when the agent goes away")
            .unwrap();
        assert!(res.is_err());
    }
}
