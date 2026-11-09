//! Anthropic `/v1/messages` adapter.
//!
//! Vendor specifics handled here: `x-api-key` auth (not Bearer), the
//! top-level `system` field, content-block message shapes, tool results
//! grouped into a single user message (Anthropic requires it),
//! `content_block_*` SSE events, `cache_read_input_tokens` accounting,
//! and `count_tokens` via the free `/v1/messages/count_tokens` endpoint.

use std::sync::Arc;

use futures_util::{StreamExt, stream::BoxStream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use hate_core::provider::{
    BoxFuture, CacheMode, Capabilities, ChatRequest, Delta, Message, ModelInfo, Provider,
    ProviderError, ToolSpec,
};
use hate_core::{TokenUsage, ToolCall, ToolName};

use crate::sse::{self, SharedCredential, SseFrame};

const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic messages adapter. `base_url` is the API root
/// (`https://api.anthropic.com`); `/v1/messages` is appended.
pub struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    credential: SharedCredential,
}

impl AnthropicProvider {
    /// Build an adapter for `base_url` (API root, no `/v1` suffix needed —
    /// both `https://api.anthropic.com` and `.../v1` are normalized).
    pub fn new(base_url: impl Into<String>, credential: SharedCredential) -> Self {
        let base = base_url.into().trim_end_matches('/').to_string();
        let base = base.strip_suffix("/v1").unwrap_or(&base).to_string();
        Self {
            client: sse::http_client(),
            base_url: base,
            credential,
        }
    }

    fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }

    fn count_url(&self) -> String {
        format!("{}/v1/messages/count_tokens", self.base_url)
    }

    fn models_url(&self) -> String {
        format!("{}/v1/models", self.base_url)
    }
}

// ── wire types (request) ──────────────────────────────────────────────────

#[derive(Serialize)]
struct WireRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Value>,
    messages: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    stream: bool,
    /// Extended-thinking control; omitted entirely when off.
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingParam>,
}

#[derive(Serialize)]
struct ThinkingParam {
    #[serde(rename = "type")]
    kind: &'static str,
    budget_tokens: u32,
}

#[derive(Serialize)]
struct CountRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Value>,
    messages: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
}

/// Convert core messages to Anthropic's shape. Consecutive `Tool`
/// messages must collapse into ONE user message of `tool_result` blocks —
/// the API rejects split results.
fn wire_messages(req: &ChatRequest) -> Vec<Value> {
    fn flush_results(out: &mut Vec<Value>, pending: &mut Vec<Value>) {
        if !pending.is_empty() {
            out.push(json!({"role": "user", "content": std::mem::take(pending)}));
        }
    }

    let mut out: Vec<Value> = Vec::with_capacity(req.messages.len());
    let mut pending_results: Vec<Value> = Vec::new();

    for m in &req.messages {
        match m {
            Message::Tool {
                call_id, content, ..
            } => {
                pending_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": content,
                }));
            }
            other => {
                flush_results(&mut out, &mut pending_results);
                out.push(match other {
                    Message::User { content } => {
                        json!({"role": "user", "content": content})
                    }
                    Message::Assistant {
                        content,
                        tool_calls,
                    } => {
                        let mut blocks: Vec<Value> = Vec::new();
                        if !content.is_empty() {
                            blocks.push(json!({"type": "text", "text": content}));
                        }
                        for c in tool_calls {
                            blocks.push(json!({
                                "type": "tool_use",
                                "id": c.id,
                                "name": c.name.as_str(),
                                "input": c.args,
                            }));
                        }
                        if blocks.is_empty() {
                            blocks.push(json!({"type": "text", "text": ""}));
                        }
                        json!({"role": "assistant", "content": blocks})
                    }
                    Message::Tool { .. } => unreachable!("handled above"),
                });
            }
        }
    }
    flush_results(&mut out, &mut pending_results);

    // Explicit prompt caching (CacheMode::Explicit): mark the LAST
    // content block of the LAST message — caches the whole prefix, and
    // the breakpoint moving forward each round is what makes
    // multi-round tool loops incremental instead of re-billed.
    if let Some(last) = out.last_mut()
        && let Some(content) = last.get_mut("content")
    {
        match content {
            Value::String(s) => {
                let text = std::mem::take(s);
                *content = json!([{
                    "type": "text",
                    "text": text,
                    "cache_control": {"type": "ephemeral"},
                }]);
            }
            Value::Array(blocks) if !blocks.is_empty() => {
                if let Some(b) = blocks.last_mut() {
                    b["cache_control"] = json!({"type": "ephemeral"});
                }
            }
            _ => {}
        }
    }
    out
}

/// System prompt as a content-block array carrying the first cache
/// breakpoint — Anthropic only caches block form, not bare strings.
fn cached_system(system: Option<&String>) -> Option<Value> {
    system.map(|s| {
        json!([{
            "type": "text",
            "text": s,
            "cache_control": {"type": "ephemeral"},
        }])
    })
}

fn wire_tools(specs: &[ToolSpec]) -> Vec<Value> {
    let mut tools: Vec<Value> = specs
        .iter()
        .map(|t| {
            json!({
                "name": t.name.as_str(),
                "description": t.description,
                "input_schema": t.input_schema,
            })
        })
        .collect();
    // Second breakpoint: everything through the tool schemas is static
    // per session — prime it once, hit it every request.
    if let Some(last) = tools.last_mut() {
        last["cache_control"] = json!({"type": "ephemeral"});
    }
    tools
}

// ── SSE event payloads ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct EventPayload {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    message: Option<MessageStart>,
    #[serde(default)]
    index: usize,
    #[serde(default)]
    content_block: Option<ContentBlockStart>,
    #[serde(default)]
    delta: Option<DeltaBody>,
    #[serde(default)]
    usage: Option<OutputUsage>,
    #[serde(default)]
    error: Option<ApiError>,
}

#[derive(Debug, Deserialize)]
struct MessageStart {
    #[serde(default)]
    usage: InputUsage,
}

#[derive(Debug, Default, Deserialize)]
struct InputUsage {
    #[serde(rename = "input_tokens", default)]
    input: u64,
    #[serde(rename = "cache_read_input_tokens", default)]
    cache_read: u64,
    #[serde(rename = "cache_creation_input_tokens", default)]
    cache_creation: u64,
}

#[derive(Debug, Deserialize)]
struct CountTokensResponse {
    input_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct ModelsPage {
    #[serde(default)]
    data: Vec<ModelEntry>,
    #[serde(default)]
    has_more: bool,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    id: String,
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContentBlockStart {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeltaBody {
    /// `content_block_delta` deltas carry `type`; `message_delta`'s
    /// `delta` (`{"stop_reason": …}`) does not — default keeps both
    /// parseable.
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OutputUsage {
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    message: String,
}

/// Per-content-block accumulation for `tool_use` `input_json` deltas.
enum BlockAcc {
    /// text or thinking block — deltas already streamed through.
    Passthrough,
    /// `tool_use` block being assembled.
    Tool {
        /// Provider-assigned `tool_use` id.
        id: String,
        /// Tool name.
        name: String,
        /// Accumulated `input_json_delta` fragments.
        json: String,
    },
}

// ── provider impl ─────────────────────────────────────────────────────────

impl Provider for AnthropicProvider {
    fn stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> BoxStream<'static, Result<Delta, ProviderError>> {
        let client = self.client.clone();
        let credential = Arc::clone(&self.credential);
        let url = self.messages_url();
        let (tx, mut rx) = mpsc::channel::<Result<Delta, ProviderError>>(64);

        tokio::spawn(async move {
            if let Err(e) = pump(&client, &url, &credential, req, cancel, &tx).await {
                let _ = tx.send(Err(e)).await;
            }
        });

        Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
    }

    fn count_tokens<'a>(
        &'a self,
        req: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<u64, ProviderError>> {
        let client = self.client.clone();
        let credential = Arc::clone(&self.credential);
        let url = self.count_url();
        let body = CountRequest {
            model: req.model.clone(),
            system: cached_system(req.system.as_ref()),
            messages: wire_messages(req),
            tools: wire_tools(&req.tools),
        };
        Box::pin(async move {
            let key = credential
                .bearer()
                .await
                .map_err(|e| ProviderError::Auth(e.to_string()))?;
            let resp = client
                .post(&url)
                .header("x-api-key", &key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            if !resp.status().is_success() {
                return Err(sse::status_error(resp).await);
            }
            let body: CountTokensResponse = resp
                .json()
                .await
                .map_err(|e| ProviderError::Decode(format!("count_tokens: {e}")))?;
            Ok(body.input_tokens)
        })
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>> {
        let client = self.client.clone();
        let credential = Arc::clone(&self.credential);
        let url = self.models_url();
        Box::pin(async move {
            let key = credential
                .bearer()
                .await
                .map_err(|e| ProviderError::Auth(e.to_string()))?;
            let mut out = Vec::new();
            let mut page = String::new();
            // Paginate defensively; 10 pages × 1000 is far beyond reality.
            for _ in 0..10 {
                let mut rb = client
                    .get(&url)
                    .header("x-api-key", &key)
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    .query(&[("limit", "1000")]);
                if !page.is_empty() {
                    rb = rb.query(&[("after_id", page.as_str())]);
                }
                let resp = rb
                    .send()
                    .await
                    .map_err(|e| ProviderError::Network(e.to_string()))?;
                if !resp.status().is_success() {
                    return Err(sse::status_error(resp).await);
                }
                let body: ModelsPage = resp
                    .json()
                    .await
                    .map_err(|e| ProviderError::Decode(format!("models: {e}")))?;
                let last = body.data.last().map(|e| e.id.clone());
                out.extend(body.data.into_iter().map(|e| ModelInfo {
                    id: e.id.clone(),
                    name: e.display_name.or(Some(e.id)),
                    meta: None, // Anthropic's catalog doesn't publish pricing/context
                }));
                if body.has_more {
                    match last {
                        Some(id) => page = id,
                        None => break,
                    }
                } else {
                    break;
                }
            }
            Ok(out)
        })
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            vision: true,
            tools: true,
            cache: CacheMode::Explicit,
            thinking: true,
        }
    }
}

// ── streaming pump ────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn pump(
    client: &reqwest::Client,
    url: &str,
    credential: &SharedCredential,
    req: ChatRequest,
    cancel: CancellationToken,
    tx: &mpsc::Sender<Result<Delta, ProviderError>>,
) -> Result<(), ProviderError> {
    // Thinking tokens bill *inside* max_tokens, so the budget is added
    // on top of the requested output ceiling (Anthropic requires
    // budget_tokens < max_tokens).
    let thinking = match req.thinking {
        hate_core::provider::ThinkingLevel::Off => None,
        hate_core::provider::ThinkingLevel::Low => Some(4_096),
        hate_core::provider::ThinkingLevel::Medium => Some(12_288),
        hate_core::provider::ThinkingLevel::High => Some(32_768),
    };
    let body = WireRequest {
        model: req.model.clone(),
        max_tokens: req.max_output_tokens.saturating_add(thinking.unwrap_or(0)),
        system: cached_system(req.system.as_ref()),
        messages: wire_messages(&req),
        tools: wire_tools(&req.tools),
        stream: true,
        thinking: thinking.map(|budget_tokens| ThinkingParam {
            kind: "enabled",
            budget_tokens,
        }),
    };

    let key = tokio::select! {
        k = credential.bearer() => k?,
        () = cancel.cancelled() => return Ok(()),
    };

    let response = tokio::select! {
        r = client
            .post(url)
            .header("x-api-key", &key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send() => r,
        () = cancel.cancelled() => return Ok(()),
    }
    .map_err(|e| ProviderError::Network(e.to_string()))?;

    if !response.status().is_success() {
        return Err(sse::status_error(response).await);
    }

    let mut frames = std::pin::pin!(sse::parse(response));
    let mut blocks: Vec<Option<BlockAcc>> = Vec::new();
    let mut input = InputUsage::default();
    let mut output_tokens: u64 = 0;
    let mut usage_sent = false;
    let mut saw_stop = false;

    loop {
        let frame = tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            f = frames.next() => match f {
                Some(f) => f?,
                None => break,
            },
        };

        let SseFrame { event, data } = frame;
        // Anthropic puts the type in the JSON payload; `event:` mirrors it.
        let payload: EventPayload = match serde_json::from_str(&data) {
            Ok(p) => p,
            Err(e) => {
                debug!(error = %e, "skipping undecodable anthropic event");
                continue;
            }
        };
        let _ = event; // payload.type is authoritative

        match payload.kind.as_str() {
            "message_start" => {
                if let Some(m) = payload.message {
                    input = m.usage;
                }
            }
            "content_block_start" => {
                let idx = payload.index;
                if blocks.len() <= idx {
                    blocks.resize_with(idx + 1, || None);
                }
                blocks[idx] = match payload.content_block {
                    Some(b) if b.kind == "tool_use" => Some(BlockAcc::Tool {
                        id: b.id.unwrap_or_default(),
                        name: b.name.unwrap_or_default(),
                        json: String::new(),
                    }),
                    _ => Some(BlockAcc::Passthrough),
                };
            }
            "content_block_delta" => {
                let Some(delta) = payload.delta else { continue };
                match delta.kind.as_str() {
                    "text_delta" => {
                        if let Some(t) = delta.text
                            && !t.is_empty()
                            && tx.send(Ok(Delta::Text(t))).await.is_err()
                        {
                            return Ok(());
                        }
                    }
                    "thinking_delta" => {
                        if let Some(t) = delta.thinking
                            && !t.is_empty()
                            && tx.send(Ok(Delta::Thinking(t))).await.is_err()
                        {
                            return Ok(());
                        }
                    }
                    "input_json_delta" => {
                        if let (Some(json), Some(BlockAcc::Tool { json: acc, .. })) = (
                            delta.partial_json,
                            blocks.get_mut(payload.index).and_then(|b| b.as_mut()),
                        ) {
                            acc.push_str(&json);
                        }
                    }
                    _ => {} // signature_delta, citations, etc.
                }
            }
            "content_block_stop" => {
                if let Some(Some(BlockAcc::Tool { id, name, json })) =
                    blocks.get_mut(payload.index).map(Option::take)
                {
                    let args: Value = if json.trim().is_empty() {
                        json!({})
                    } else {
                        match serde_json::from_str(&json) {
                            Ok(v) => v,
                            Err(e) => {
                                warn!(error = %e, "tool_use input_json did not parse");
                                json!({})
                            }
                        }
                    };
                    let call = ToolCall {
                        id,
                        name: ToolName::new(name),
                        args,
                    };
                    if tx.send(Ok(Delta::ToolCall(call))).await.is_err() {
                        return Ok(());
                    }
                }
            }
            "message_delta" => {
                if let Some(u) = payload.usage {
                    output_tokens = u.output_tokens;
                }
                // Anthropic's output_tokens is cumulative — send the
                // combined usage record once, here.
                let usage = TokenUsage {
                    input: input.input + input.cache_read + input.cache_creation,
                    cached_input: input.cache_read,
                    output: output_tokens,
                    thinking: 0, // thinking bills inside output_tokens
                };
                if tx.send(Ok(Delta::Usage(usage))).await.is_err() {
                    return Ok(());
                }
                usage_sent = true;
            }
            "message_stop" => {
                saw_stop = true;
                break;
            }
            "error" => {
                let e = payload.error.unwrap_or(ApiError {
                    kind: "unknown".to_string(),
                    message: "provider error".to_string(),
                });
                return Err(map_api_error(&e));
            }
            _ => {} // ping, content_block_* we don't care about
        }
    }

    // Some streams end at body-close without message_stop; still report
    // usage if we never got the final delta.
    if !usage_sent {
        let usage = TokenUsage {
            input: input.input + input.cache_read + input.cache_creation,
            cached_input: input.cache_read,
            output: output_tokens,
            thinking: 0,
        };
        let _ = tx.send(Ok(Delta::Usage(usage))).await;
    }
    if saw_stop {
        let _ = tx.send(Ok(Delta::Done)).await;
    } else {
        let _ = tx
            .send(Err(ProviderError::Stream(
                "stream ended before message_stop".to_string(),
            )))
            .await;
    }
    Ok(())
}

fn map_api_error(e: &ApiError) -> ProviderError {
    let msg = format!("{}: {}", e.kind, e.message);
    if e.kind.contains("authentication") || e.kind.contains("permission") {
        ProviderError::Auth(msg)
    } else if e.kind.contains("rate_limit") {
        ProviderError::Http {
            status: 429,
            message: msg,
            retry_after: None,
        }
    } else if e.kind.contains("overloaded") {
        ProviderError::Http {
            status: 529,
            message: msg,
            retry_after: None,
        }
    } else {
        ProviderError::Stream(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hate_core::provider::ThinkingLevel;

    fn req(messages: Vec<Message>, tools: Vec<ToolSpec>) -> ChatRequest {
        ChatRequest {
            model: "claude-test".to_string(),
            system: Some("you are helpful".to_string()),
            messages,
            tools,
            max_output_tokens: 1024,
            temperature: None,
            thinking: ThinkingLevel::Off,
        }
    }

    fn bash_spec() -> ToolSpec {
        ToolSpec {
            name: ToolName::new("bash"),
            description: "run a command".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }

    #[test]
    fn breakpoints_cover_system_tools_and_last_message() {
        let r = req(
            vec![
                Message::User {
                    content: "hi".to_string(),
                },
                Message::Assistant {
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "c1".to_string(),
                        name: ToolName::new("bash"),
                        args: serde_json::json!({"command": "ls"}),
                    }],
                },
                Message::Tool {
                    call_id: "c1".to_string(),
                    name: "bash".to_string(),
                    content: "ok".to_string(),
                },
            ],
            vec![bash_spec()],
        );

        // System → block array with cache_control.
        let sys = cached_system(r.system.as_ref()).expect("system present");
        assert_eq!(sys[0]["cache_control"]["type"], "ephemeral");

        // Last tool carries the second breakpoint.
        let tools = wire_tools(&r.tools);
        assert_eq!(tools[0]["cache_control"]["type"], "ephemeral");

        // Last message (collapsed tool_result user msg) → last block.
        let msgs = wire_messages(&r);
        let last = msgs.last().expect("messages");
        let blocks = last["content"].as_array().expect("block array");
        assert_eq!(
            blocks.last().expect("block")["cache_control"]["type"],
            "ephemeral"
        );
        // Earlier messages stay breakpoint-free.
        assert!(
            msgs[0]["content"].is_string() || msgs[0]["content"][0].get("cache_control").is_none()
        );
    }

    #[test]
    fn bare_user_string_becomes_cached_block() {
        let r = req(
            vec![Message::User {
                content: "hello".to_string(),
            }],
            vec![],
        );
        let msgs = wire_messages(&r);
        assert!(msgs[0]["content"].is_array());
        assert_eq!(msgs[0]["content"][0]["cache_control"]["type"], "ephemeral");
        // No tools → no tool breakpoint.
        assert_eq!(wire_tools(&r.tools).len(), 0);
    }
}
