use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::BufRead as _;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use futures_util::StreamExt as _;
use hate_core::McpServerConfig;
use serde_json::{Value, json};
use tokio::sync::mpsc;

pub const SERVER_NAME: &str = "web_search";
pub const MCP_FLAG: &str = "--mcp-web-search";
pub const BUILTIN_COMMAND_TOKEN: &str = "builtin:web-search";

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_FETCH_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_MAX_RESULTS: usize = 6;
const DEFAULT_FETCH_CHARS: usize = 12_000;
const MAX_FETCH_CHARS: usize = 40_000;
const MAX_REDIRECTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub published: Option<String>,
}

pub fn resolved_self_command() -> String {
    std::env::current_exe().map_or_else(
        |_| "hate-agent".to_string(),
        |p| p.to_string_lossy().into_owned(),
    )
}

pub fn is_builtin_web_search(cfg: &McpServerConfig) -> bool {
    cfg.kind == "stdio" && cfg.args.first().map(String::as_str) == Some(MCP_FLAG)
}

pub fn default_config() -> McpServerConfig {
    McpServerConfig {
        name: SERVER_NAME.to_string(),
        kind: "stdio".to_string(),
        enabled: true,
        command: Some(resolved_self_command()),
        args: vec![MCP_FLAG.to_string()],
        env: BTreeMap::new(),
        url: None,
        headers: BTreeMap::new(),
    }
}

pub fn run_stdio_server_blocking() {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to start web search MCP runtime: {e}");
            std::process::exit(1);
        }
    };
    rt.block_on(run_stdio_server());
}

async fn run_stdio_server() {
    let client = match build_http_client() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("failed to build HTTP client: {e}");
            std::process::exit(1);
        }
    };

    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let reader = std::io::BufReader::new(stdin.lock());
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    if in_tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let writer_handle = std::thread::spawn(move || {
        use std::io::Write as _;
        let stdout = std::io::stdout();
        let mut out = std::io::BufWriter::new(stdout.lock());
        while let Some(msg) = out_rx.blocking_recv() {
            if out.write_all(msg.as_bytes()).is_err()
                || out.write_all(b"\n").is_err()
                || out.flush().is_err()
            {
                break;
            }
        }
    });

    while let Some(line) = in_rx.recv().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let Some(id) = msg.get("id").cloned() else {
            // Notification (e.g. notifications/initialized) expects no response.
            continue;
        };
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let client2 = Arc::clone(&client);
        let out_tx2 = out_tx.clone();

        tokio::spawn(async move {
            let resp = handle_request(&client2, id, &method, &params).await;
            if let Ok(encoded) = serde_json::to_string(&resp) {
                let _ = out_tx2.send(encoded);
            }
        });
    }

    drop(out_tx);
    let _ = writer_handle.join();
}

async fn handle_request(
    client: &reqwest::Client,
    id: Value,
    method: &str,
    params: &Value,
) -> Value {
    match method {
        "initialize" => {
            let proto = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-06-18");
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": proto,
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "hate-agent-web-search",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            })
        }
        "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
        "resources/list" => json!({ "jsonrpc": "2.0", "id": id, "result": { "resources": [] } }),
        "prompts/list" => json!({ "jsonrpc": "2.0", "id": id, "result": { "prompts": [] } }),
        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": tool_definitions()
            }
        }),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            let call_res = match name {
                "web_search" => execute_web_search(client, &args).await,
                "web_fetch" => execute_web_fetch(client, &args).await,
                other => Err(format!("unknown tool `{other}`")),
            };
            let result = match call_res {
                Ok(text) => json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                }),
                Err(err) => json!({
                    "content": [{ "type": "text", "text": err }],
                    "isError": true
                }),
            };
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            })
        }
        other => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("method not found: {other}")
            }
        }),
    }
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "web_search",
            "description": "Search the public internet for current information, news, documentation, and facts. No API key required.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query keywords or question."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum results to return (1-10, default 6).",
                        "minimum": 1,
                        "maximum": 10
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            },
            "annotations": {
                "title": "Web Search",
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": true
            }
        },
        {
            "name": "web_fetch",
            "description": "Fetch and extract readable text from a public http(s) webpage URL so you can read full articles or documentation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Public http:// or https:// URL to fetch."
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": "Maximum characters to return (1000-40000, default 12000).",
                        "minimum": 1000,
                        "maximum": 40000
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            },
            "annotations": {
                "title": "Web Fetch",
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true,
                "openWorldHint": true
            }
        }
    ])
}

fn build_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(8))
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}

async fn execute_web_search(client: &reqwest::Client, args: &Value) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or_else(|| "missing required `query` string".to_string())?;
    let max_results = args
        .get("max_results")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, 10);

    let (source, hits) = search_with_fallback(client, query, max_results).await?;
    Ok(format_search_results(query, source, &hits))
}

async fn search_with_fallback(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<(&'static str, Vec<SearchHit>), String> {
    let mut errors = Vec::new();

    match search_bing_rss(client, query, limit).await {
        Ok(hits) if !hits.is_empty() => return Ok(("Bing", hits)),
        Ok(_) => errors.push("Bing RSS: 0 results".to_string()),
        Err(e) => errors.push(format!("Bing RSS: {e}")),
    }

    match search_ddg_html(client, query, limit).await {
        Ok(hits) if !hits.is_empty() => return Ok(("DuckDuckGo", hits)),
        Ok(_) => errors.push("DuckDuckGo HTML: 0 results".to_string()),
        Err(e) => errors.push(format!("DuckDuckGo HTML: {e}")),
    }

    match search_bing_html(client, query, limit).await {
        Ok(hits) if !hits.is_empty() => return Ok(("Bing Web", hits)),
        Ok(_) => errors.push("Bing HTML: 0 results".to_string()),
        Err(e) => errors.push(format!("Bing HTML: {e}")),
    }

    match search_wikipedia(client, query, limit).await {
        Ok(hits) if !hits.is_empty() => return Ok(("Wikipedia", hits)),
        Ok(_) => errors.push("Wikipedia: 0 results".to_string()),
        Err(e) => errors.push(format!("Wikipedia: {e}")),
    }

    Err(format!(
        "No web search results found for `{query}` ({})",
        errors.join("; ")
    ))
}

async fn fetch_text_limited(
    client: &reqwest::Client,
    url: &str,
    accept: &str,
) -> Result<(u16, String), String> {
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        let resp = client
            .get(&current)
            .header(reqwest::header::ACCEPT, accept)
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status();
        if status.is_redirection()
            && let Some(loc) = resp.headers().get(reqwest::header::LOCATION)
            && let Ok(loc_str) = loc.to_str()
        {
            current = resolve_redirect_url(&current, loc_str)?;
            continue;
        }
        let code = status.as_u16();
        let body = read_body_capped(resp, MAX_FETCH_BYTES).await?;
        let text = String::from_utf8_lossy(&body).into_owned();
        return Ok((code, text));
    }
    Err("too many redirects".to_string())
}

async fn search_bing_rss(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, String> {
    let encoded = percent_encode(query);
    let url = format!("https://www.bing.com/search?q={encoded}&format=rss&mkt=en-US");
    let (status, body) = fetch_text_limited(
        client,
        &url,
        "application/rss+xml, application/xml, text/xml",
    )
    .await?;
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    Ok(parse_bing_rss(&body, limit))
}

async fn search_ddg_html(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, String> {
    let encoded = percent_encode(query);
    let url = format!("https://html.duckduckgo.com/html/?q={encoded}");
    let (status, body) =
        fetch_text_limited(client, &url, "text/html,application/xhtml+xml").await?;
    if status != 200 || body.contains("anomaly.js") || body.contains("anomaly-modal") {
        return Err(format!("rate limited / challenge (HTTP {status})"));
    }
    Ok(parse_ddg_html(&body, limit))
}

async fn search_bing_html(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, String> {
    let encoded = percent_encode(query);
    let url = format!("https://www.bing.com/search?q={encoded}&setlang=en");
    let (status, body) =
        fetch_text_limited(client, &url, "text/html,application/xhtml+xml").await?;
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    Ok(parse_bing_html(&body, limit))
}

async fn search_wikipedia(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, String> {
    let encoded = percent_encode(query);
    let url = format!(
        "https://en.wikipedia.org/w/api.php?action=query&list=search&srsearch={encoded}&utf8=&format=json&srlimit={limit}"
    );
    let (status, body) = fetch_text_limited(client, &url, "application/json").await?;
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    let val: Value = serde_json::from_str(&body).map_err(|e| format!("bad JSON: {e}"))?;
    let mut hits = Vec::new();
    if let Some(arr) = val.pointer("/query/search").and_then(Value::as_array) {
        for item in arr.iter().take(limit) {
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            let pageid = item.get("pageid").and_then(Value::as_u64);
            let raw_snip = item
                .get("snippet")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let snippet = clean_inline_html(raw_snip);
            if let Some(pid) = pageid
                && !title.is_empty()
            {
                hits.push(SearchHit {
                    title: format!("{title} - Wikipedia"),
                    url: format!("https://en.wikipedia.org/?curid={pid}"),
                    snippet,
                    published: item
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                });
            }
        }
    }
    Ok(hits)
}

fn format_search_results(query: &str, source: &str, hits: &[SearchHit]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Search results for \"{query}\" (via {source}):\n");
    for (i, hit) in hits.iter().enumerate() {
        let _ = writeln!(out, "{}. {}", i + 1, hit.title);
        let _ = writeln!(out, "   URL: {}", hit.url);
        if let Some(pub_date) = hit.published.as_deref()
            && !pub_date.is_empty()
        {
            let _ = writeln!(out, "   Date: {pub_date}");
        }
        if !hit.snippet.is_empty() {
            let _ = writeln!(out, "   {}", hit.snippet);
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

async fn execute_web_fetch(client: &reqwest::Client, args: &Value) -> Result<String, String> {
    let raw_url = args
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .ok_or_else(|| "missing required `url` string".to_string())?;
    let max_chars = args
        .get("max_chars")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(DEFAULT_FETCH_CHARS)
        .clamp(1_000, MAX_FETCH_CHARS);

    let mut current = raw_url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        validate_public_http_url(&current).await?;
        let resp = client
            .get(&current)
            .header(
                reqwest::header::ACCEPT,
                "text/html,application/xhtml+xml,text/plain,application/json,text/markdown;q=0.9,*/*;q=0.5",
            )
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .send()
            .await
            .map_err(|e| format!("fetch failed: {e}"))?;

        let status = resp.status();
        if status.is_redirection()
            && let Some(loc) = resp.headers().get(reqwest::header::LOCATION)
            && let Ok(loc_str) = loc.to_str()
        {
            current = resolve_redirect_url(&current, loc_str)?;
            continue;
        }
        if !status.is_success() {
            return Err(format!("HTTP {} fetching {current}", status.as_u16()));
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/plain")
            .to_string();
        let ct_lower = content_type.to_ascii_lowercase();
        if !(ct_lower.contains("text/")
            || ct_lower.contains("json")
            || ct_lower.contains("xml")
            || ct_lower.contains("xhtml")
            || ct_lower.contains("javascript"))
        {
            return Err(format!(
                "unsupported content-type `{content_type}` at {current}"
            ));
        }

        let bytes = read_body_capped(resp, MAX_FETCH_BYTES).await?;
        let raw_text = String::from_utf8_lossy(&bytes).into_owned();
        let (title, extracted) = if ct_lower.contains("html") {
            extract_html_readable(&raw_text)
        } else {
            (None, raw_text.trim().to_string())
        };

        let mut out = String::new();
        let _ = writeln!(out, "URL: {current}");
        if let Some(t) = title.filter(|t| !t.is_empty()) {
            let _ = writeln!(out, "Title: {t}");
        }
        let _ = writeln!(out, "Content-Type: {content_type}\n");

        let mut chars = extracted.chars();
        let body: String = chars.by_ref().take(max_chars).collect();
        out.push_str(&body);
        if chars.next().is_some() {
            out.push_str("\n\n[truncated]");
        }
        return Ok(out);
    }

    Err("too many redirects".to_string())
}

async fn read_body_capped(resp: reqwest::Response, max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("read body: {e}"))?;
        let remaining = max_bytes.saturating_sub(buf.len());
        if remaining == 0 {
            break;
        }
        let take = chunk.len().min(remaining);
        buf.extend_from_slice(&chunk[..take]);
        if buf.len() >= max_bytes {
            break;
        }
    }
    Ok(buf)
}

async fn validate_public_http_url(url: &str) -> Result<(), String> {
    let (host, port) = parse_http_host_port(url)?;
    let lower = host.trim_end_matches('.').to_ascii_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || std::path::Path::new(&lower)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("local"))
        || lower.ends_with(".internal")
    {
        return Err(format!("blocked private/local host `{host}`"));
    }

    let bare_ip = lower.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare_ip.parse::<IpAddr>() {
        if !is_public_ip(ip) {
            return Err(format!("blocked non-public IP `{ip}`"));
        }
        return Ok(());
    }

    let addrs = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| format!("DNS lookup failed for `{host}`: {e}"))?;
    let mut saw_any = false;
    for addr in addrs {
        saw_any = true;
        if !is_public_ip(addr.ip()) {
            return Err(format!(
                "blocked non-public address `{}` for `{host}`",
                addr.ip()
            ));
        }
    }
    if !saw_any {
        return Err(format!("no addresses resolved for `{host}`"));
    }
    Ok(())
}

fn parse_http_host_port(url: &str) -> Result<(String, u16), String> {
    let (default_port, rest) = if let Some(r) = url.strip_prefix("https://") {
        (443u16, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (80u16, r)
    } else {
        return Err("URL must start with http:// or https://".to_string());
    };

    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return Err("URL has no host".to_string());
    }
    if authority.contains('@') {
        return Err("URLs with embedded credentials are not allowed".to_string());
    }

    if let Some(after_open) = authority.strip_prefix('[') {
        let Some((host, tail)) = after_open.split_once(']') else {
            return Err("invalid IPv6 host in URL".to_string());
        };
        let port = if let Some(p_str) = tail.strip_prefix(':') {
            p_str
                .parse::<u16>()
                .map_err(|_| "invalid port in URL".to_string())?
        } else if tail.is_empty() {
            default_port
        } else {
            return Err("invalid host/port in URL".to_string());
        };
        return Ok((host.to_string(), port));
    }

    if let Some((host, p_str)) = authority.rsplit_once(':')
        && !host.is_empty()
        && p_str.chars().all(|c| c.is_ascii_digit())
    {
        let port = p_str
            .parse::<u16>()
            .map_err(|_| "invalid port in URL".to_string())?;
        return Ok((host.to_string(), port));
    }

    Ok((authority.to_string(), default_port))
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    let is_cgnat = o[0] == 100 && (o[1] & 0b1100_0000) == 64;
    let is_benchmark = o[0] == 198 && (o[1] == 18 || o[1] == 19);
    !(v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_multicast()
        || v4.is_unspecified()
        || o[0] == 0
        || is_cgnat
        || is_benchmark
        || o[0] >= 240)
}

fn is_public_v6(v6: Ipv6Addr) -> bool {
    if let Some(v4) = v6.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let segs = v6.segments();
    let is_ula = (segs[0] & 0xfe00) == 0xfc00;
    let is_link_local = (segs[0] & 0xffc0) == 0xfe80;
    let is_doc = segs[0] == 0x2001 && segs[1] == 0x0db8;
    !(v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        || is_ula
        || is_link_local
        || is_doc)
}

fn resolve_redirect_url(base: &str, location: &str) -> Result<String, String> {
    let loc = location.trim();
    if loc.starts_with("http://") || loc.starts_with("https://") {
        return Ok(loc.to_string());
    }
    let (scheme, after_scheme) = base
        .split_once("://")
        .ok_or_else(|| "invalid base URL".to_string())?;
    if let Some(rest) = loc.strip_prefix("//") {
        return Ok(format!("{scheme}://{rest}"));
    }
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    if loc.starts_with('/') {
        return Ok(format!("{scheme}://{authority}{loc}"));
    }
    let base_no_q = base.split(['?', '#']).next().unwrap_or(base);
    let prefix = base_no_q.rsplit_once('/').map_or(base_no_q, |(dir, _)| dir);
    Ok(format!("{prefix}/{loc}"))
}

pub fn parse_bing_rss(xml: &str, limit: usize) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    for chunk in xml.split("<item>").skip(1) {
        if hits.len() >= limit {
            break;
        }
        let body = chunk.split("</item>").next().unwrap_or(chunk);
        let title = extract_xml_tag(body, "title").unwrap_or_default();
        let url = extract_xml_tag(body, "link").unwrap_or_default();
        let snippet = extract_xml_tag(body, "description").unwrap_or_default();
        let published = extract_xml_tag(body, "pubDate").filter(|d| !d.is_empty());
        if !title.is_empty() && (url.starts_with("http://") || url.starts_with("https://")) {
            hits.push(SearchHit {
                title,
                url,
                snippet,
                published,
            });
        }
    }
    hits
}

fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let (_, after) = xml.split_once(&open)?;
    let (inner, _) = after.split_once(&close)?;
    let trimmed = inner.trim();
    let is_cdata = trimmed.starts_with("<![CDATA[") && trimmed.ends_with("]]>");
    let unwrapped = if is_cdata {
        &trimmed[9..trimmed.len() - 3]
    } else {
        trimmed
    };
    if is_cdata {
        Some(clean_inline_html(unwrapped))
    } else {
        let xml_decoded = decode_html_entities(unwrapped);
        Some(clean_inline_html(&xml_decoded))
    }
}

pub fn parse_ddg_html(html: &str, limit: usize) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    let mut rest = html;
    while hits.len() < limit {
        let Some(idx) = rest.find("result__a") else {
            break;
        };
        let after_marker = &rest[idx..];
        let tag_start = rest[..idx].rfind('<').unwrap_or(idx);
        let from_tag = &rest[tag_start..];
        let Some(tag_end_rel) = from_tag.find('>') else {
            break;
        };
        let open_tag = &from_tag[..tag_end_rel];
        let href = extract_attr(open_tag, "href").unwrap_or_default();
        let after_open = &from_tag[tag_end_rel + 1..];
        let Some((raw_title, after_a)) = after_open.split_once("</a>") else {
            rest = &after_marker[9..];
            continue;
        };
        let url = decode_ddg_href(&href);
        let title = clean_inline_html(raw_title);

        let window_end = after_a.find("result__a").unwrap_or(after_a.len());
        let window = &after_a[..window_end];
        let snippet = extract_class_text(window, "result__snippet").unwrap_or_default();

        if !title.is_empty() && (url.starts_with("http://") || url.starts_with("https://")) {
            hits.push(SearchHit {
                title,
                url,
                snippet,
                published: None,
            });
        }
        rest = &after_a[window_end..];
    }
    hits
}

pub fn parse_bing_html(html: &str, limit: usize) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    for block in html.split("class=\"b_algo\"").skip(1) {
        if hits.len() >= limit {
            break;
        }
        let Some(h2_start) = block.find("<h2") else {
            continue;
        };
        let h2_block = &block[h2_start..];
        let Some((_, h2_inner_and_rest)) = h2_block.split_once('>') else {
            continue;
        };
        let Some((h2_inner, after_h2)) = h2_inner_and_rest.split_once("</h2>") else {
            continue;
        };
        let Some(a_start) = h2_inner.find("<a ") else {
            continue;
        };
        let a_slice = &h2_inner[a_start..];
        let Some((a_tag, a_rest)) = a_slice.split_once('>') else {
            continue;
        };
        let raw_title = a_rest.split("</a>").next().unwrap_or_default();
        let href = extract_attr(a_tag, "href").unwrap_or_default();
        let url = decode_bing_href(&href);
        let title = clean_inline_html(raw_title);

        let snippet =
            extract_tag_inner(after_h2, "<p", "</p>").map_or_default(|s| clean_inline_html(&s));

        if !title.is_empty() && (url.starts_with("http://") || url.starts_with("https://")) {
            hits.push(SearchHit {
                title,
                url,
                snippet,
                published: None,
            });
        }
    }
    hits
}

fn extract_class_text(html: &str, class_name: &str) -> Option<String> {
    let idx = html.find(class_name)?;
    let from = &html[idx..];
    let (_, after_gt) = from.split_once('>')?;
    let end = after_gt
        .find("</a>")
        .or_else(|| after_gt.find("</div>"))
        .or_else(|| after_gt.find("</span>"))
        .unwrap_or_else(|| after_gt.len().min(600));
    Some(clean_inline_html(&after_gt[..end]))
}

fn extract_tag_inner(html: &str, open_prefix: &str, close_tag: &str) -> Option<String> {
    let start = html.find(open_prefix)?;
    let (_, after_gt) = html[start..].split_once('>')?;
    let (inner, _) = after_gt.split_once(close_tag)?;
    Some(inner.to_string())
}

fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let dq = format!("{attr}=\"");
    if let Some((_, rest)) = tag.split_once(&dq)
        && let Some((val, _)) = rest.split_once('"')
    {
        return Some(decode_html_entities(val));
    }
    let sq = format!("{attr}='");
    if let Some((_, rest)) = tag.split_once(&sq)
        && let Some((val, _)) = rest.split_once('\'')
    {
        return Some(decode_html_entities(val));
    }
    None
}

fn decode_ddg_href(href: &str) -> String {
    let normalized = if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_string()
    };
    if let Some(query) = normalized.split_once('?').map(|(_, q)| q) {
        for pair in query.split('&') {
            if let Some(val) = pair.strip_prefix("uddg=") {
                return percent_decode(val);
            }
        }
    }
    normalized
}

fn decode_bing_href(href: &str) -> String {
    if href.contains("bing.com/ck/a")
        && let Some((_, query)) = href.split_once('?')
    {
        for pair in query.split('&') {
            if let Some(u) = pair.strip_prefix("u=")
                && let Some(b64) = u.strip_prefix("a1")
                && let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(b64.trim_end_matches('='))
                && let Ok(decoded) = String::from_utf8(bytes)
                && (decoded.starts_with("http://") || decoded.starts_with("https://"))
            {
                return decoded;
            }
        }
    }
    href.to_string()
}

pub fn extract_html_readable(html: &str) -> (Option<String>, String) {
    let title = extract_tag_ci(html, "title").map(|t| clean_inline_html(&t));
    let mut cleaned = strip_block_ci(html, "head");
    cleaned = strip_block_ci(&cleaned, "script");
    cleaned = strip_block_ci(&cleaned, "style");
    cleaned = strip_block_ci(&cleaned, "noscript");
    cleaned = strip_block_ci(&cleaned, "svg");
    cleaned = strip_html_comments(&cleaned);

    let mut text = String::with_capacity(cleaned.len() / 2);
    let mut in_tag = false;
    let mut tag_buf = String::new();
    for ch in cleaned.chars() {
        if in_tag {
            if ch == '>' {
                in_tag = false;
                let tag_name = tag_buf
                    .trim()
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(
                    tag_name.as_str(),
                    "p" | "div"
                        | "br"
                        | "li"
                        | "tr"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "header"
                        | "footer"
                        | "nav"
                        | "main"
                        | "section"
                        | "article"
                        | "blockquote"
                        | "pre"
                ) {
                    text.push('\n');
                }
                tag_buf.clear();
            } else {
                tag_buf.push(ch);
            }
        } else if ch == '<' {
            in_tag = true;
            tag_buf.clear();
        } else {
            text.push(ch);
        }
    }

    let decoded = decode_html_entities(&text);
    let mut lines_out = Vec::new();
    for line in decoded.lines() {
        let collapsed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !collapsed.is_empty() {
            lines_out.push(collapsed);
        }
    }
    (title, lines_out.join("\n"))
}

fn extract_tag_ci(html: &str, tag: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = lower.find(&open)?;
    let gt = lower[start..].find('>')? + start + 1;
    let end = lower[gt..].find(&close)? + gt;
    Some(html[gt..end].to_string())
}

fn strip_block_ci(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(html.len());
    let mut pos = 0usize;
    while let Some(rel_start) = lower[pos..].find(&open) {
        let start = pos + rel_start;
        let after_name = start + open.len();
        let next_byte = lower.as_bytes().get(after_name).copied();
        let is_tag_boundary = matches!(next_byte, Some(b'>' | b' ' | b'\t' | b'\r' | b'\n' | b'/'));
        if !is_tag_boundary {
            out.push_str(&html[pos..after_name]);
            pos = after_name;
            continue;
        }
        out.push_str(&html[pos..start]);
        if let Some(rel_end) = lower[after_name..].find(&close) {
            pos = after_name + rel_end + close.len();
        } else {
            out.push_str(&html[start..after_name]);
            pos = after_name;
        }
    }
    if pos < html.len() {
        out.push_str(&html[pos..]);
    }
    out
}

fn strip_html_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some((before, after)) = rest.split_once("<!--") {
        out.push_str(before);
        if let Some((_, tail)) = after.split_once("-->") {
            rest = tail;
        } else {
            return out;
        }
    }
    out.push_str(rest);
    out
}

fn clean_inline_html(raw: &str) -> String {
    let mut stripped = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some((before, after_lt)) = rest.split_once('<') {
        stripped.push_str(before);
        let starts_tag = after_lt
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'));
        if starts_tag
            && let Some((tag_inner, tail)) = after_lt.split_once('>')
            && !tag_inner.contains('<')
        {
            stripped.push(' ');
            rest = tail;
            continue;
        }
        stripped.push('<');
        rest = after_lt;
    }
    stripped.push_str(rest);
    let decoded = decode_html_entities(&stripped);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_html_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some((before, after)) = rest.split_once('&') {
        out.push_str(before);
        if let Some((ent, tail)) = after.split_once(';')
            && ent.len() <= 12
        {
            let replaced = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" | "#x27" => Some('\''),
                "nbsp" => Some(' '),
                "ndash" => Some('-'),
                "mdash" => Some('-'),
                "hellip" => Some('…'),
                _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                    u32::from_str_radix(&ent[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                }
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(ch) = replaced {
                out.push(ch);
                rest = tail;
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            other => {
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bing_rss_parser_extracts_items() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0"><channel><title>Bing: test</title><link>http://www.bing.com</link>
<item><title>Rust 1.90 &amp; News</title><link>https://releases.rs/docs/1.90.0</link><description>Release &lt;b&gt;notes&lt;/b&gt; here.</description><pubDate>Mon, 16 Nov 2026 17:00:00 GMT</pubDate></item>
<item><title>Second</title><link>https://example.com/2</link><description><![CDATA[Summary &amp; more]]></description></item>
</channel></rss>"#;
        let hits = parse_bing_rss(xml, 5);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Rust 1.90 & News");
        assert_eq!(hits[0].url, "https://releases.rs/docs/1.90.0");
        assert_eq!(hits[0].snippet, "Release notes here.");
        assert_eq!(
            hits[0].published.as_deref(),
            Some("Mon, 16 Nov 2026 17:00:00 GMT")
        );
        assert_eq!(hits[1].snippet, "Summary & more");
    }

    #[test]
    fn ddg_html_parser_decodes_redirect_urls() {
        let html = r##"
<div class="result">
  <h2 class="result__title">
    <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust-lang.org%2Fstable%2F&amp;rut=abc">Rust Docs</a>
  </h2>
  <a class="result__snippet" href="#">Official <b>Rust</b> documentation.</a>
</div>"##;
        let hits = parse_ddg_html(html, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Rust Docs");
        assert_eq!(hits[0].url, "https://doc.rust-lang.org/stable/");
        assert_eq!(hits[0].snippet, "Official Rust documentation.");
    }

    #[test]
    fn bing_html_parser_decodes_base64_ck_urls() {
        let html = r#"<li class="b_algo"><h2><a href="https://www.bing.com/ck/a?!&&u=a1aHR0cHM6Ly9yZWxlYXNlcy5ycy8&ntb=1"><strong>Rust</strong> Versions</a></h2><div class="b_caption"><p>Changelog details.</p></div></li>"#;
        let hits = parse_bing_html(html, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Rust Versions");
        assert_eq!(hits[0].url, "https://releases.rs/");
        assert_eq!(hits[0].snippet, "Changelog details.");
    }

    #[test]
    fn ssrf_blocks_private_and_loopback_ips() {
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));
        assert!(!is_public_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(!is_public_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_public_ip(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
    }

    #[test]
    fn readable_html_strips_head_scripts_and_preserves_header() {
        let html = "<html><head><title>Hello &amp; World</title><style>body{color:red}</style></head><body><header>Site Nav</header><script>evil()</script><h1>Heading</h1><p>First &nbsp; line.</p></body></html>";
        let (title, body) = extract_html_readable(html);
        assert_eq!(title.as_deref(), Some("Hello & World"));
        assert_eq!(body, "Site Nav\nHeading\nFirst line.");
    }
}
