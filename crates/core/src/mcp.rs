use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

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

pub const MCP_PREFIX: &str = "mcp__";
const CALL_TIMEOUT: Duration = Duration::from_secs(180);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_SERVERS: usize = 16;
const SWEEP_INTERVAL: Duration = Duration::from_secs(10);
const MAX_TOOLS_PER_SERVER: usize = 48;
const DESC_CHARS: usize = 220;
const SCHEMA_TEXT_CHARS: usize = 140;
const SCHEMA_BYTES: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub kind: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

const fn default_enabled() -> bool {
    true
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct McpConfigFile {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

impl McpConfigFile {
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

#[derive(Debug, Clone, Serialize)]
pub struct McpServerStatus {
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub state: String,
    pub tools: usize,
    pub info: String,
    pub detail: String,
}

struct McpServer {
    name: String,
    service: RunningService<RoleClient, ()>,
    tools: Vec<McpTool>,
    info: String,
}

struct McpTool {
    wire: String,
    orig: String,
    spec: ToolSpec,
    read_only: bool,
}

pub struct McpHub {
    servers: RwLock<Vec<Arc<McpServer>>>,
    configs: RwLock<Vec<McpServerConfig>>,
    reload: tokio::sync::Mutex<()>,
    configuring: std::sync::atomic::AtomicUsize,
    last_sweep: Mutex<Option<Instant>>,
    counter: O200kCounter,
}

impl McpHub {
    pub fn new() -> Self {
        Self {
            servers: RwLock::new(Vec::new()),
            configs: RwLock::new(Vec::new()),
            reload: tokio::sync::Mutex::new(()),
            configuring: std::sync::atomic::AtomicUsize::new(0),
            last_sweep: Mutex::new(None),
            counter: O200kCounter::new().expect("o200k tokenizer init"),
        }
    }

    pub async fn from_config_file(path: &Path) -> Self {
        let hub = Self::new();
        let servers = McpConfigFile::load(path);
        if !servers.is_empty() {
            hub.configure(servers).await;
        }
        hub
    }

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
        *self.servers.write().expect("mcp servers poisoned") =
            results.into_iter().flatten().collect();
    }

    fn is_live(&self, name: &str) -> bool {
        self.servers
            .read()
            .expect("mcp servers poisoned")
            .iter()
            .any(|s| s.name == name && !s.tools.is_empty())
    }

    /// Retry enabled servers that aren't live yet. Servers can appear
    /// after the app starts (Roblox Studio launches later, a proxy takes
    /// time to expose tools), so the hub re-checks on demand instead of
    /// only at configure time. Throttled: returns `false` when a sweep
    /// ran within [`SWEEP_INTERVAL`].
    pub async fn ensure_live(&self, force: bool) -> bool {
        {
            let mut last = self.last_sweep.lock().expect("sweep poisoned");
            let now = Instant::now();
            if !force && last.is_some_and(|t| now.duration_since(t) < SWEEP_INTERVAL) {
                return false;
            }
            *last = Some(now);
        }
        let missing: Vec<McpServerConfig> = self
            .configs()
            .into_iter()
            .filter(|c| c.enabled && !self.is_live(&c.name))
            .take(MAX_SERVERS)
            .collect();
        if missing.is_empty() {
            return false;
        }
        let _guard = self.reload.lock().await;
        self.configuring
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let results = futures_util::future::join_all(missing.iter().map(|cfg| async move {
            match tokio::time::timeout(CONNECT_TIMEOUT, connect(cfg)).await {
                Ok(Ok(s)) => Some(s),
                Ok(Err(e)) => {
                    tracing::debug!(server = %cfg.name, error = %e, "mcp retry failed");
                    None
                }
                Err(_) => {
                    tracing::debug!(server = %cfg.name, "mcp retry timed out");
                    None
                }
            }
        }))
        .await;
        self.configuring
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        let mut servers = self.servers.write().expect("mcp servers poisoned");
        for s in results.into_iter().flatten() {
            servers.retain(|x| x.name != s.name);
            servers.push(s);
        }
        true
    }

    /// Fire-and-forget sweep for hot paths (turn start): never blocks
    /// the turn, the new tools show up on the next one.
    pub fn kick_refresh(self: &Arc<Self>) {
        let hub = Arc::clone(self);
        tokio::spawn(async move {
            hub.ensure_live(false).await;
        });
    }

    pub fn configs(&self) -> Vec<McpServerConfig> {
        self.configs.read().expect("mcp configs poisoned").clone()
    }

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

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.servers
            .read()
            .expect("mcp servers poisoned")
            .iter()
            .flat_map(|s| s.tools.iter().map(|t| t.spec.clone()))
            .collect()
    }

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

    pub fn tool_error(&self, msg: String) -> ToolOutput {
        self.err(msg)
    }

    fn err(&self, msg: String) -> ToolOutput {
        self.output(Err(msg))
    }

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
                #[cfg(windows)]
                c.creation_flags(0x0800_0000);
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

fn command_for(command: &str) -> Command {
    if !command.contains(['/', '\\'])
        && let Ok(c) = rmcp::transport::which_command(command)
    {
        return c;
    }
    Command::new(command)
}

async fn finish_connect(
    name: &str,
    service: RunningService<RoleClient, ()>,
) -> Result<Arc<McpServer>, String> {
    let remote = service
        .list_all_tools()
        .await
        .map_err(|e| format!("tools/list: {e}"))?;

    let slug = slugify(name);
    let mut tools = Vec::with_capacity(remote.len().min(MAX_TOOLS_PER_SERVER));
    let mut used: HashMap<String, usize> = HashMap::new();
    for t in remote.into_iter().take(MAX_TOOLS_PER_SERVER) {
        let orig = t.name.to_string();
        let base = format!("{MCP_PREFIX}{slug}__{}", slugify(&orig));
        let n = used.entry(base.clone()).or_insert(0);
        *n += 1;
        let wire = if *n == 1 {
            base
        } else {
            format!("{base}_{}", *n)
        };
        let description: String = t
            .description
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(DESC_CHARS)
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
                input_schema: sanitize_schema(&t.input_schema),
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

fn sanitize_schema(schema: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    let v = serde_json::Value::Object(schema.clone());
    let trimmed = trim_schema_text(&v, SCHEMA_TEXT_CHARS, false);
    if serde_json::to_string(&trimmed).map_or(0, |s| s.len()) <= SCHEMA_BYTES {
        return trimmed;
    }
    trim_schema_text(&v, 0, true)
}

fn trim_schema_text(v: &serde_json::Value, max: usize, strip_all: bool) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .filter_map(|(k, val)| {
                    if strip_all && matches!(k.as_str(), "description" | "title") {
                        return None;
                    }
                    Some((
                        k.clone(),
                        if matches!(k.as_str(), "description" | "title")
                            && let serde_json::Value::String(s) = val
                        {
                            let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
                            if strip_all || flat.chars().count() > max {
                                serde_json::Value::String(
                                    flat.chars().take(max).collect::<String>() + "…",
                                )
                            } else {
                                serde_json::Value::String(flat)
                            }
                        } else {
                            trim_schema_text(val, max, strip_all)
                        },
                    ))
                })
                .collect(),
        ),
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|i| trim_schema_text(i, max, strip_all))
                .collect(),
        ),
        _ => v.clone(),
    }
}

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

    async fn fake_server(io: tokio::io::DuplexStream) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (rd, mut wr) = tokio::io::split(io);
        let mut lines = BufReader::new(rd).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
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

        let specs = hub.specs();
        assert_eq!(specs.len(), 2);
        assert!(specs.iter().any(|s| s.name.as_str() == "mcp__fake__ping"));
        assert!(
            specs
                .iter()
                .any(|s| s.name.as_str() == "mcp__fake__write_thing")
        );

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

        let out = hub.execute(&ping, &CancellationToken::new()).await;
        assert!(out.ok, "ping failed: {}", out.content);
        assert!(out.content.contains("pong:ping"));
        assert!(out.content.contains("hi"));

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
