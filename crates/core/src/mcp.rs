//! MCP client: connect to Model Context Protocol servers — stdio child
//! processes (the dominant deployment: `npx`, `uvx`, plugins) and
//! streamable-HTTP endpoints (hosted/localhost bridges like the Roblox
//! Studio MCP) — and expose their tools to the session beside the
//! workspace tools.
//!
//! Wire names are namespaced `mcp__<server>__<tool>` so they can never
//! collide with built-ins. Results go through the same shaping/token
//! accounting as workspace tools, and non-read-only calls pass the same
//! permission gate as `bash`/`write_file`.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use hate_context::O200kCounter;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::provider::ToolSpec;
use crate::tools::{FULL_COUNT_CAP, SENT_CAP, ToolOutput, shape};
use crate::types::{ToolCall, ToolName};

/// Wire-name prefix for every MCP tool — also the session's routing key.
pub const MCP_PREFIX: &str = "mcp__";
/// Per-call ceiling; Studio-style bridges can be slow but not forever.
const CALL_TIMEOUT: Duration = Duration::from_secs(180);
/// Handshake + `tools/list` ceiling per server at (re)configure time.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Cap on servers connected concurrently during configure.
const MAX_SERVERS: usize = 16;

/// One configured MCP server. Flat shape keeps the settings form and
/// the JSON file identical; `kind` selects which fields are meaningful.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Unique display name — becomes the `mcp__<name>__` wire prefix.
    pub name: String,
    /// `"stdio"` or `"http"`.
    pub kind: String,
    /// Disabled servers stay in config but never connect.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// stdio: executable (`npx`, `uvx`, a binary path).
    #[serde(default)]
    pub command: Option<String>,
    /// stdio: arguments after the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// stdio: extra environment (`PATH` etc. inherited regardless).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// http: streamable-HTTP endpoint (`http://localhost:3002/mcp`).
    #[serde(default)]
    pub url: Option<String>,
    /// http: headers sent on every request (`Authorization: Bearer …`).
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

const fn default_enabled() -> bool {
    true
}

/// On-disk shape: just the server list.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct McpConfigFile {
    /// Configured servers, in display order.
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

impl McpConfigFile {
    /// Read `path` → server configs. Missing file → empty; unreadable
    /// or malformed JSON → warn + empty (a bad file must never keep
    /// the agent from starting).
    pub fn load(path: &Path) -> Vec<McpServerConfig> {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(file) => file.servers,
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "mcp.json unreadable — no MCP servers");
                Vec::new()
            }
        }
    }
}

/// Live status of one configured server, for the settings UI.
#[derive(Debug, Clone, Serialize)]
pub struct McpServerStatus {
    /// Configured name.
    pub name: String,
    /// `"stdio"` | `"http"`.
    pub kind: String,
    /// Config `enabled` flag.
    pub enabled: bool,
    /// `"connected"` | `"connecting"` | `"error"` | `"disabled"`.
    pub state: String,
    /// Tools advertised by the server (0 until connected).
    pub tools: usize,
    /// Server-reported name/version once connected.
    pub info: String,
    /// Last connect/call error, if any.
    pub detail: String,
}

/// A connected server: the rmcp service plus its cached tool table.
struct McpServer {
    /// Config name (unslugged, for status/errors).
    name: String,
    /// Running rmcp service — `call_tool`/`list_*` go through it.
    service: RunningService<RoleClient, ()>,
    /// Cached tools with wire names resolved.
    tools: Vec<McpTool>,
    /// `server_info` from the initialize handshake, for the UI.
    info: String,
}

/// One advertised MCP tool, mapped to our wire name.
struct McpTool {
    /// `mcp__<server>__<tool>` as sent to the provider.
    wire: String,
    /// Original name the server expects back.
    orig: String,
    /// Advertised schema (already namespaced).
    spec: ToolSpec,
    /// `readOnlyHint == true` → permission gate auto-allows.
    read_only: bool,
}

/// All configured MCP servers, connectable at runtime. Sessions hold an
/// `Arc` and see `configure()` reloads on their next turn — the hub is
/// the single source of truth, no per-session snapshots.
pub struct McpHub {
    /// Live connected servers (empty while a configure is in flight).
    servers: RwLock<Vec<Arc<McpServer>>>,
    /// Last applied config — `mcp_list` merges this with live status.
    configs: RwLock<Vec<McpServerConfig>>,
    /// Serializes `configure` calls so overlapping saves can't interleave
    /// spawns; readers never take this lock.
    reload: tokio::sync::Mutex<()>,
    /// In-flight `configure` count — `status()` reports "connecting"
    /// instead of "error" while one is running.
    configuring: std::sync::atomic::AtomicUsize,
    /// Token counter for error-path `ToolOutput`s.
    counter: O200kCounter,
}

impl McpHub {
    /// Empty hub — `configure` populates it.
    pub fn new() -> Self {
        Self {
            servers: RwLock::new(Vec::new()),
            configs: RwLock::new(Vec::new()),
            reload: tokio::sync::Mutex::new(()),
            configuring: std::sync::atomic::AtomicUsize::new(0),
            counter: O200kCounter::new().expect("o200k tokenizer init"),
        }
    }

    /// Load `<path>` if it exists, then connect every enabled server.
    /// Missing/unreadable file → empty hub, never an error (first run).
    pub async fn from_config_file(path: &Path) -> Self {
        let hub = Self::new();
        let servers = McpConfigFile::load(path);
        if !servers.is_empty() {
            hub.configure(servers).await;
        }
        hub
    }

    /// Persist `configs` to `path` (pretty JSON, `servers: []` when
    /// empty) — the file is the source of truth across restarts.
    pub fn save_config(path: &Path, configs: &[McpServerConfig]) -> Result<(), String> {
        let file = McpConfigFile {
            servers: configs.to_vec(),
        };
        let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("config dir: {e}"))?;
        }
        std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
    }

    /// Replace the server set: drop old connections, connect every
    /// enabled config (bounded, 20s each), swap in the survivors.
    /// Failed servers stay configured but absent — their tools simply
    /// don't exist until a later configure succeeds.
    pub async fn configure(&self, configs: Vec<McpServerConfig>) {
        let _guard = self.reload.lock().await;
        self.configs
            .write()
            .expect("mcp configs poisoned")
            .clone_from(&configs);

        let enabled: Vec<McpServerConfig> = configs
            .into_iter()
            .filter(|c| c.enabled)
            .take(MAX_SERVERS)
            .collect();
        self.configuring
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Connect concurrently — one dead server must not serialize-
        // block the rest behind its 20s handshake timeout.
        let results = futures_util::future::join_all(enabled.iter().map(|cfg| async move {
            match tokio::time::timeout(CONNECT_TIMEOUT, connect(cfg)).await {
                Ok(Ok(s)) => Some(s),
                Ok(Err(e)) => {
                    tracing::warn!(server = %cfg.name, error = %e, "mcp server failed to connect");
                    None
                }
                Err(_) => {
                    tracing::warn!(server = %cfg.name, "mcp server connect timed out");
                    None
                }
            }
        }))
        .await;
        self.configuring
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        // Old services drop here → rmcp cancels transports, child
        // processes are killed by TokioChildProcess's Drop.
        *self.servers.write().expect("mcp servers poisoned") =
            results.into_iter().flatten().collect();
    }

    /// Configs as last applied (for `mcp_list`).
    pub fn configs(&self) -> Vec<McpServerConfig> {
        self.configs.read().expect("mcp configs poisoned").clone()
    }

    /// Merge config + live state into UI rows.
    pub fn status(&self) -> Vec<McpServerStatus> {
        let live = self.servers.read().expect("mcp servers poisoned");
        self.configs
            .read()
            .expect("mcp configs poisoned")
            .iter()
            .map(|c| {
                let connected = live.iter().find(|s| s.name == c.name);
                McpServerStatus {
                    name: c.name.clone(),
                    kind: c.kind.clone(),
                    enabled: c.enabled,
                    state: if !c.enabled {
                        "disabled".to_string()
                    } else if connected.is_some() {
                        "connected".to_string()
                    } else if self.configuring.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                        "connecting".to_string()
                    } else {
                        "error".to_string()
                    },
                    tools: connected.map_or(0, |s| s.tools.len()),
                    info: connected.map_or_else(String::new, |s| s.info.clone()),
                    detail: String::new(),
                }
            })
            .collect()
    }

    /// All advertised tools across connected servers.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.servers
            .read()
            .expect("mcp servers poisoned")
            .iter()
            .flat_map(|s| s.tools.iter().map(|t| t.spec.clone()))
            .collect()
    }

    /// `Some((reason, detail))` when the call needs user approval.
    /// Servers that declare `readOnlyHint` skip the gate entirely —
    /// everything else prompts, same policy as `bash`.
    pub fn gate_reason(&self, call: &ToolCall) -> Option<(String, Option<String>)> {
        let found = {
            let servers = self.servers.read().expect("mcp servers poisoned");
            servers.iter().find_map(|s| {
                s.tools
                    .iter()
                    .find(|t| t.wire == call.name.as_str())
                    .map(|t| (s.name.clone(), t.orig.clone(), t.read_only))
            })
        }?;
        let (server, orig, read_only) = found;
        if read_only {
            return None;
        }
        let detail = serde_json::to_string(&call.args)
            .map(|s| s.chars().take(200).collect())
            .unwrap_or_default();
        Some((format!("calls MCP tool {server}.{orig}"), Some(detail)))
    }

    /// Execute one `mcp__…` call: route → `tools/call` → shaped text.
    /// Timeouts and transport errors become honest tool errors the
    /// model can react to; they never fail the turn.
    pub async fn execute(&self, call: &ToolCall, cancel: &CancellationToken) -> ToolOutput {
        let Some(server) = self
            .servers
            .read()
            .expect("mcp servers poisoned")
            .iter()
            .find(|s| s.tools.iter().any(|t| t.wire == call.name.as_str()))
            .cloned()
        else {
            return self.err(format!("unknown MCP tool `{}`", call.name));
        };
        let Some(tool) = server.tools.iter().find(|t| t.wire == call.name.as_str()) else {
            return self.err(format!("unknown MCP tool `{}`", call.name));
        };
        let args = match call.args.as_object() {
            Some(obj) => obj.clone(),
            None => {
                return self.err(format!(
                    "MCP tool `{}` expects an arguments object",
                    call.name
                ));
            }
        };

        let params = CallToolRequestParams::new(tool.orig.clone()).with_arguments(args);
        let result = tokio::select! {
            () = cancel.cancelled() => return self.err("cancelled".to_string()),
            r = tokio::time::timeout(CALL_TIMEOUT, server.service.call_tool(params)) => r,
        };
        match result {
            Ok(Ok(res)) => {
                let is_err = res.is_error.unwrap_or(false);
                let text = render_result(&res);
                self.output(if is_err {
                    Err(format!("MCP tool error: {text}"))
                } else {
                    Ok(text)
                })
            }
            Ok(Err(e)) => self.err(format!("MCP call failed: {e}")),
            Err(_) => self.err(format!(
                "MCP call timed out after {}s",
                CALL_TIMEOUT.as_secs()
            )),
        }
    }

    /// Error-shaped output for denied/unknown calls — same accounting
    /// path as real results.
    pub fn tool_error(&self, msg: String) -> ToolOutput {
        self.err(msg)
    }

    fn err(&self, msg: String) -> ToolOutput {
        self.output(Err(msg))
    }

    /// Shared result → [`ToolOutput`]: shape, count both sides, preview.
    fn output(&self, result: Result<String, String>) -> ToolOutput {
        match result {
            Ok(full) => {
                let sent = shape(&full, SENT_CAP);
                let preview: String = sent.chars().take(400).collect();
                ToolOutput {
                    full_tokens: self.count_capped(&full),
                    sent_tokens: self.counter.count_sync(&sent) as u32,
                    content: sent,
                    preview,
                    ok: true,
                }
            }
            Err(e) => {
                let content = format!("error: {e}");
                ToolOutput {
                    full_tokens: self.counter.count_sync(&content) as u32,
                    sent_tokens: self.counter.count_sync(&content) as u32,
                    content,
                    preview: e.chars().take(400).collect(),
                    ok: false,
                }
            }
        }
    }

    fn count_capped(&self, text: &str) -> u32 {
        let capped: String = text.chars().take(FULL_COUNT_CAP).collect();
        self.counter.count_sync(&capped) as u32
    }
}

impl Default for McpHub {
    fn default() -> Self {
        Self::new()
    }
}

/// Spawn + handshake one server. Returns `Err` with a human-readable
/// reason — the hub logs it and moves on; one bad server never blocks
/// the others.
async fn connect(cfg: &McpServerConfig) -> Result<Arc<McpServer>, String> {
    let service: RunningService<RoleClient, ()> = match cfg.kind.as_str() {
        "stdio" => {
            let command = cfg
                .command
                .as_deref()
                .ok_or("stdio server needs `command`")?;
            let cmd = command_for(command).configure(|c| {
                for arg in &cfg.args {
                    c.arg(arg);
                }
                for (k, v) in &cfg.env {
                    c.env(k, v);
                }
            });
            let transport = TokioChildProcess::new(cmd).map_err(|e| format!("spawn: {e}"))?;
            ().serve(transport)
                .await
                .map_err(|e| format!("handshake: {e}"))?
        }
        "http" => {
            let url = cfg.url.as_deref().ok_or("http server needs `url`")?;
            let mut http_cfg = StreamableHttpClientTransportConfig::with_uri(url.to_string());
            let mut headers = HashMap::new();
            for (k, v) in &cfg.headers {
                let (Ok(name), Ok(value)) = (
                    http::header::HeaderName::from_bytes(k.as_bytes()),
                    http::header::HeaderValue::from_str(v),
                ) else {
                    tracing::warn!(server = %cfg.name, header = %k, "skipping invalid MCP header");
                    continue;
                };
                headers.insert(name, value);
            }
            http_cfg = http_cfg.custom_headers(headers);
            ().serve(StreamableHttpClientTransport::from_config(http_cfg))
                .await
                .map_err(|e| format!("handshake: {e}"))?
        }
        other => return Err(format!("unknown kind `{other}` (stdio|http)")),
    };

    finish_connect(&cfg.name, service).await
}

/// Resolve a bare command name through PATH+PATHEXT so Windows `.cmd`/
/// `.bat` shims (`npx`, `uvx`) spawn correctly — `CreateProcess` won't
/// find them on its own. Paths (anything with a separator) and names
/// `which` can't resolve fall back to a literal `Command::new`, which
/// still covers App-Paths-style registrations.
fn command_for(command: &str) -> Command {
    if !command.contains(['/', '\\'])
        && let Ok(c) = rmcp::transport::which_command(command)
    {
        return c;
    }
    Command::new(command)
}

/// Post-handshake setup shared by every transport: `tools/list` →
/// cached tool table → [`McpServer`].
async fn finish_connect(
    name: &str,
    service: RunningService<RoleClient, ()>,
) -> Result<Arc<McpServer>, String> {
    let remote = service
        .list_all_tools()
        .await
        .map_err(|e| format!("tools/list: {e}"))?;

    let slug = slugify(name);
    let mut tools = Vec::with_capacity(remote.len());
    let mut used: HashMap<String, usize> = HashMap::new();
    for t in remote {
        let orig = t.name.to_string();
        let base = format!("{MCP_PREFIX}{slug}__{}", slugify(&orig));
        // Same-slug collisions get a numeric suffix — wire names must
        // stay unique across the whole tool list.
        let n = used.entry(base.clone()).or_insert(0);
        *n += 1;
        let wire = if *n == 1 {
            base
        } else {
            format!("{base}_{}", *n)
        };
        // ToolSpec descriptions are one-line; MCP servers sometimes send
        // multi-paragraph text — flatten and cap.
        let description: String = t
            .description
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(300)
            .collect();
        tools.push(McpTool {
            wire: wire.clone(),
            orig,
            spec: ToolSpec {
                name: ToolName::new(wire),
                description: if description.is_empty() {
                    format!("MCP tool from {name}")
                } else {
                    description
                },
                input_schema: serde_json::Value::Object((*t.input_schema).clone()),
            },
            read_only: t
                .annotations
                .as_ref()
                .and_then(|a| a.read_only_hint)
                .unwrap_or(false),
        });
    }

    let info = service
        .peer_info()
        .and_then(|i| {
            i.server_info
                .as_ref()
                .map(|si| format!("{} v{}", si.name, si.version))
        })
        .unwrap_or_default();

    Ok(Arc::new(McpServer {
        name: name.to_string(),
        service,
        tools,
        info,
    }))
}

/// Lowercase `[a-z0-9_]` slug for wire names — keeps `mcp__…` names
/// valid on every provider's tool-name rules.
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_us = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_us = false;
        } else if !last_us && !out.is_empty() {
            out.push('_');
            last_us = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        "server".to_string()
    } else {
        out
    }
}

/// `CallToolResult` → the text the model sees. Text blocks pass
/// through; embedded text resources contribute their text; binary
/// content becomes an honest placeholder (vision wiring is future
/// work); `structured_content` is appended as JSON when present.
fn render_result(res: &rmcp::model::CallToolResult) -> String {
    let mut out = String::new();
    for block in &res.content {
        match block {
            ContentBlock::Text(t) => {
                if !out.is_empty() {
                    out.push_str("\n---\n");
                }
                out.push_str(&t.text);
            }
            ContentBlock::Resource(r) => match &r.resource {
                rmcp::model::ResourceContents::TextResourceContents {
                    uri,
                    text,
                    mime_type,
                    ..
                } => {
                    if !out.is_empty() {
                        out.push_str("\n---\n");
                    }
                    let _ = mime_type;
                    let _ = write!(out, "[resource {uri}]\n{text}");
                }
                rmcp::model::ResourceContents::BlobResourceContents {
                    uri,
                    blob,
                    mime_type,
                    ..
                } => {
                    if !out.is_empty() {
                        out.push_str("\n---\n");
                    }
                    let _ = write!(
                        out,
                        "[binary resource {uri} — {} blob, {} bytes base64 — not shown to model]",
                        mime_type.as_deref().unwrap_or("application/octet-stream"),
                        blob.len()
                    );
                }
                _ => {
                    if !out.is_empty() {
                        out.push_str("\n---\n");
                    }
                    let _ = write!(out, "[unsupported resource content]");
                }
            },
            ContentBlock::ResourceLink(l) => {
                if !out.is_empty() {
                    out.push_str("\n---\n");
                }
                let _ = write!(out, "[link {} — {}]", l.name, l.uri);
            }
            other => {
                if !out.is_empty() {
                    out.push_str("\n---\n");
                }
                let _ = write!(out, "[unsupported MCP content block: {other:?}]");
            }
        }
    }
    if let Some(structured) = &res.structured_content {
        if !out.is_empty() {
            out.push_str("\n---\n");
        }
        let _ = write!(
            out,
            "[structured]\n{}",
            serde_json::to_string_pretty(structured).unwrap_or_default()
        );
    }
    if out.is_empty() {
        "(empty result)".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_normalizes() {
        assert_eq!(slugify("Roblox Studio!"), "roblox_studio");
        assert_eq!(slugify("my--server"), "my_server");
        assert_eq!(slugify("!!!"), "server");
        assert_eq!(slugify("Already_Fine_9"), "already_fine_9");
    }

    #[test]
    fn config_roundtrip() {
        let cfg = McpServerConfig {
            name: "roblox".into(),
            kind: "stdio".into(),
            enabled: true,
            command: Some("npx".into()),
            args: vec!["-y".into(), "@x/mcp".into()],
            env: BTreeMap::from([("KEY".to_string(), "v".to_string())]),
            url: None,
            headers: BTreeMap::new(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "roblox");
        assert_eq!(back.args, vec!["-y", "@x/mcp"]);
        assert!(back.enabled);
    }

    #[test]
    fn render_text_and_structured() {
        let res = rmcp::model::CallToolResult::success(vec![ContentBlock::text("hello")]);
        assert_eq!(render_result(&res), "hello");

        let res2 = rmcp::model::CallToolResult::structured(serde_json::json!({"a": 1}));
        let text = render_result(&res2);
        assert!(text.contains("[structured]"));
        assert!(text.contains("\"a\": 1"));
    }

    // ── wire-level: real rmcp client ↔ fake server over duplex ────────

    /// Minimal MCP server: newline-JSON-RPC answering `initialize`,
    /// `tools/list`, `tools/call` with canned results.
    async fn fake_server(io: tokio::io::DuplexStream) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (rd, mut wr) = tokio::io::split(io);
        let mut lines = BufReader::new(rd).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            // Notifications carry no id — nothing to answer.
            let Some(id) = msg.get("id").cloned() else {
                continue;
            };
            let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let result = match method {
                "initialize" => serde_json::json!({
                    "protocolVersion": msg
                        .pointer("/params/protocolVersion")
                        .cloned()
                        .unwrap_or(serde_json::json!("2025-06-18")),
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fake-mcp", "version": "0.1.0"}
                }),
                "tools/list" => serde_json::json!({
                    "tools": [
                        {
                            "name": "ping",
                            "description": "Ping the fake server",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"msg": {"type": "string"}}
                            },
                            "annotations": {"readOnlyHint": true}
                        },
                        {
                            "name": "write_thing",
                            "description": "Mutates fake state",
                            "inputSchema": {"type": "object", "properties": {}}
                        }
                    ]
                }),
                "tools/call" => {
                    let name = msg
                        .pointer("/params/name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("");
                    let args = msg
                        .pointer("/params/arguments")
                        .cloned()
                        .unwrap_or(serde_json::json!({}));
                    serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": format!("pong:{name}:{args}")
                        }],
                        "isError": name == "boom"
                    })
                }
                _ => serde_json::json!({}),
            };
            let resp = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result});
            if wr.write_all(format!("{resp}\n").as_bytes()).await.is_err() {
                break;
            }
        }
    }

    #[tokio::test]
    async fn duplex_mcp_server_end_to_end() {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let server = tokio::spawn(fake_server(server_io));
        let service = ().serve(client_io).await.expect("mcp handshake");
        let srv = finish_connect("fake", service).await.expect("tools/list");

        let hub = McpHub::new();
        hub.servers.write().expect("servers poisoned").push(srv);

        // Advertised specs are namespaced and carry the server's schema.
        let specs = hub.specs();
        assert_eq!(specs.len(), 2);
        assert!(specs.iter().any(|s| s.name.as_str() == "mcp__fake__ping"));
        assert!(
            specs
                .iter()
                .any(|s| s.name.as_str() == "mcp__fake__write_thing")
        );

        // readOnlyHint → gate auto-allows; unannotated → prompts.
        let ping = ToolCall {
            id: "1".to_string(),
            name: ToolName::new("mcp__fake__ping"),
            args: serde_json::json!({"msg": "hi"}),
        };
        assert!(hub.gate_reason(&ping).is_none());
        let write = ToolCall {
            id: "2".to_string(),
            name: ToolName::new("mcp__fake__write_thing"),
            args: serde_json::json!({}),
        };
        assert!(hub.gate_reason(&write).is_some());

        // A real tools/call round-trip through rmcp → fake server.
        let out = hub.execute(&ping, &CancellationToken::new()).await;
        assert!(out.ok, "ping failed: {}", out.content);
        assert!(out.content.contains("pong:ping"));
        assert!(out.content.contains("hi"));

        // Unknown wire name → honest error, not a panic.
        let bad = ToolCall {
            id: "3".to_string(),
            name: ToolName::new("mcp__fake__nope"),
            args: serde_json::json!({}),
        };
        let out2 = hub.execute(&bad, &CancellationToken::new()).await;
        assert!(!out2.ok);
        assert!(out2.content.contains("unknown MCP tool"));

        server.abort();
    }
}
