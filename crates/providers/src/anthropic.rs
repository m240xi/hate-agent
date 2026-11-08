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
    system: Option<String>,
    messages: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    stream: bool,
}

#[derive(Serialize)]
struct CountRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
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
    out
}

fn wire_tools(specs: &[ToolSpec]) -> Vec<Value> {
    specs
        .iter()
        .map(|t| {
            json!({
                "name": t.name.as_str(),
                "description": t.description,
                "input_schema": t.input_schema,
            })
        })
        .collect()
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
            system: req.system.clone(),
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
    let body = WireRequest {
        model: req.model.clone(),
        max_tokens: req.max_output_tokens,
        system: req.system.clone(),
        messages: wire_messages(&req),
        tools: wire_tools(&req.tools),
        stream: true,
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
