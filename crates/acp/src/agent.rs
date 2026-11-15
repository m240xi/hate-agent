use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::rpc::{Connection, Incoming, RpcError};
use crate::runtime::{self, Installed};

pub const PROTOCOL_VERSION: u64 = 1;
pub const GOOGLE_LOGIN: &str = "oauth-personal";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelChoice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewSession {
    pub session_id: String,
    pub config_options: Value,
}

pub struct Agent {
    pub conn: Arc<Connection>,
    pub init: Value,
    child: Option<tokio::process::Child>,
}

#[derive(Debug, Clone)]
pub enum StartError {
    NotInstalled,
    Spawn(String),
    Handshake(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInstalled => write!(
                f,
                "Antigravity isn't installed yet — open Settings → Providers → Antigravity"
            ),
            Self::Spawn(e) => write!(f, "couldn't start Antigravity: {e}"),
            Self::Handshake(e) => write!(f, "Antigravity handshake failed: {e}"),
        }
    }
}

pub struct Started {
    pub agent: Agent,
    pub incoming: mpsc::UnboundedReceiver<Incoming>,
    pub stderr: mpsc::UnboundedReceiver<String>,
}

impl Agent {
    pub async fn start(inst: Option<&Installed>) -> Result<Started, StartError> {
        let inst = inst.ok_or(StartError::NotInstalled)?;
        let mut child = runtime::command(inst)
            .spawn()
            .map_err(|e| StartError::Spawn(e.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or(StartError::Spawn("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or(StartError::Spawn("no stdout".into()))?;
        let (err_tx, err_rx) = mpsc::unbounded_channel();
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "antigravity", "{line}");
                    if err_tx.send(line).is_err() {
                        while let Ok(Some(_)) = lines.next_line().await {}
                        break;
                    }
                }
            });
        }
        let (conn, incoming) = Connection::new(stdout, stdin);
        let mut agent = Agent {
            conn,
            init: Value::Null,
            child: Some(child),
        };
        agent
            .initialize()
            .await
            .map_err(|e| StartError::Handshake(e.detail()))?;
        Ok(Started {
            agent,
            incoming,
            stderr: err_rx,
        })
    }

    pub fn from_connection(conn: Arc<Connection>) -> Self {
        Self {
            conn,
            init: Value::Null,
            child: None,
        }
    }

    pub async fn initialize(&mut self) -> Result<(), RpcError> {
        let res = self
            .conn
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "clientCapabilities": {},
                    "clientInfo": {"name": "hate-agent", "title": "hate-agent", "version": env!("CARGO_PKG_VERSION")}
                }),
            )
            .await?;
        let v = res.get("protocolVersion").and_then(Value::as_u64);
        if v != Some(PROTOCOL_VERSION) {
            return Err(RpcError {
                code: -32600,
                message: format!("unsupported protocol version {v:?}"),
                data: None,
            });
        }
        self.init = res;
        Ok(())
    }

    pub fn supports_google_login(&self) -> bool {
        self.init
            .get("authMethods")
            .and_then(Value::as_array)
            .is_some_and(|m| {
                m.iter()
                    .any(|x| x.get("id").and_then(Value::as_str) == Some(GOOGLE_LOGIN))
            })
    }

    pub fn can_resume(&self) -> bool {
        self.init
            .pointer("/agentCapabilities/sessionCapabilities/resume")
            .is_some_and(|v| !v.is_null())
    }

    pub fn can_logout(&self) -> bool {
        self.init
            .pointer("/agentCapabilities/auth/logout")
            .is_some_and(|v| !v.is_null())
    }

    pub fn version(&self) -> Option<&str> {
        self.init
            .pointer("/agentInfo/version")
            .and_then(Value::as_str)
    }

    pub async fn authenticate(&self, method: &str) -> Result<(), RpcError> {
        self.conn
            .request("authenticate", json!({"methodId": method}))
            .await
            .map(|_| ())
    }

    pub async fn logout(&self) -> Result<(), RpcError> {
        self.conn.request("logout", json!({})).await.map(|_| ())
    }

    pub async fn new_session(&self, cwd: &str, mcp: Value) -> Result<NewSession, RpcError> {
        let res = self
            .conn
            .request("session/new", json!({"cwd": cwd, "mcpServers": mcp}))
            .await?;
        let session_id = res
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError {
                code: -32603,
                message: "session/new returned no sessionId".to_string(),
                data: None,
            })?
            .to_string();
        Ok(NewSession {
            session_id,
            config_options: res.get("configOptions").cloned().unwrap_or(Value::Null),
        })
    }

    pub async fn resume_session(
        &self,
        session_id: &str,
        cwd: &str,
        mcp: Value,
    ) -> Result<Value, RpcError> {
        let res = self
            .conn
            .request(
                "session/resume",
                json!({"sessionId": session_id, "cwd": cwd, "mcpServers": mcp}),
            )
            .await?;
        Ok(res.get("configOptions").cloned().unwrap_or(Value::Null))
    }

    pub async fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<Value, RpcError> {
        let res = self
            .conn
            .request(
                "session/set_config_option",
                json!({"sessionId": session_id, "configId": config_id, "value": value}),
            )
            .await?;
        Ok(res.get("configOptions").cloned().unwrap_or(Value::Null))
    }

    pub fn cancel(&self, session_id: &str) {
        self.conn
            .notify("session/cancel", json!({"sessionId": session_id}));
    }

    pub async fn shutdown(mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
        }
    }
}

pub fn model_option(config_options: &Value) -> Option<&Value> {
    let opts = config_options.as_array()?;
    opts.iter()
        .find(|o| o.get("category").and_then(Value::as_str) == Some("model"))
        .or_else(|| {
            opts.iter()
                .find(|o| o.get("id").and_then(Value::as_str) == Some("model"))
        })
}

pub fn models(config_options: &Value) -> (Vec<ModelChoice>, Option<String>) {
    let Some(opt) = model_option(config_options) else {
        return (Vec::new(), None);
    };
    let current = opt
        .get("currentValue")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut out = Vec::new();
    let mut push = |v: &Value| {
        if let (Some(value), Some(name)) = (
            v.get("value").and_then(Value::as_str),
            v.get("name").and_then(Value::as_str),
        ) {
            out.push(ModelChoice {
                value: value.to_string(),
                name: name.to_string(),
                description: v
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
    };
    for item in opt
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("options").and_then(Value::as_array) {
            Some(group) => group.iter().for_each(&mut push),
            None => push(item),
        }
    }
    (out, current)
}

pub fn model_config_id(config_options: &Value) -> Option<String> {
    model_option(config_options)
        .and_then(|o| o.get("id").and_then(Value::as_str))
        .map(str::to_string)
}

pub fn login_url(stderr_line: &str) -> Option<String> {
    let at = stderr_line.find("https://accounts.google.com/")?;
    let url: String = stderr_line[at..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_flat_and_grouped_model_options() {
        let flat = json!([
            {"id": "mode", "category": "mode", "type": "select", "currentValue": "ask", "options": [{"value": "ask", "name": "Ask"}]},
            {"id": "model", "category": "model", "type": "select", "currentValue": "gemini-3-flash",
             "options": [{"value": "gemini-3-pro", "name": "Gemini 3 Pro", "description": "smart"},
                         {"value": "gemini-3-flash", "name": "Gemini 3 Flash"}]}
        ]);
        let (m, cur) = models(&flat);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].name, "Gemini 3 Pro");
        assert_eq!(m[0].description.as_deref(), Some("smart"));
        assert_eq!(cur.as_deref(), Some("gemini-3-flash"));
        assert_eq!(model_config_id(&flat).as_deref(), Some("model"));

        let grouped = json!([
            {"id": "llm", "category": "model", "type": "select", "currentValue": "a",
             "options": [{"group": "fast", "name": "Fast", "options": [{"value": "a", "name": "A"}]},
                         {"group": "smart", "name": "Smart", "options": [{"value": "b", "name": "B"}]}]}
        ]);
        let (m, _) = models(&grouped);
        assert_eq!(
            m.iter().map(|x| x.value.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(model_config_id(&grouped).as_deref(), Some("llm"));

        assert_eq!(models(&Value::Null).0, Vec::new());
    }

    #[test]
    fn finds_google_login_url_in_agent_stderr() {
        let line = "Open the following link to authenticate the ACP server: https://accounts.google.com/o/oauth2/v2/auth?response_type=code&client_id=x&state=y";
        assert_eq!(
            login_url(line).as_deref(),
            Some(
                "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&client_id=x&state=y"
            )
        );
        assert!(login_url("I1114 settings: path=... status=missing").is_none());
    }
}
