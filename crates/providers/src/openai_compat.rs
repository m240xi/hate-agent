//! Generic OpenAI-compatible `/chat/completions` adapter.
//!
//! Covers Nous Portal inference, Ollama, LM Studio, `OpenRouter`, `DeepSeek`,
//! Gemini's compat endpoint, and anything else that speaks the dialect.
//! Vendor quirks handled here: `reasoning_content`/`reasoning` thinking
//! deltas (DeepSeek/Nous), `tool_calls` index-fragment accumulation,
//! `stream_options.include_usage`, and `prompt_tokens_details.cached_tokens`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use futures_util::{StreamExt, stream::BoxStream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, instrument, warn};

use hate_core::provider::{
    BoxFuture, CacheMode, Capabilities, ChatRequest, Delta, Message, ModelInfo, Provider,
    ProviderError, ThinkingLevel, ToolSpec,
};
use hate_core::{TokenUsage, ToolCall, ToolName};

use crate::sse::{self, SharedCredential};

/// OpenAI-compatible chat completions adapter.
pub struct OpenAiCompatProvider {
    client: reqwest::Client,
    /// e.g. `https://inference-api.nousresearch.com/v1` — no trailing slash.
    base_url: String,
    credential: SharedCredential,
    /// Nous Portal speaks a `reasoning` object, not bare `reasoning_effort`.
    nous_dialect: bool,
    /// o200k BPE counter, loaded lazily on first `count_tokens`.
    counter: OnceLock<hate_context::O200kCounter>,
}

impl OpenAiCompatProvider {
    /// Build an adapter for `base_url` (the `/v1` root, not `/chat/completions`).
    pub fn new(base_url: impl Into<String>, credential: SharedCredential) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        // Dialect is a property of the endpoint, not the config name —
        // a user-named provider pointed at Nous still gets Nous wire.
        let nous_dialect = base_url.contains("nousresearch.com");
        Self {
            client: sse::http_client(),
            base_url,
            credential,
            nous_dialect,
            counter: OnceLock::new(),
        }
    }

    fn completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

// ── wire types (request) ──────────────────────────────────────────────────

#[derive(Serialize)]
struct WireRequest {
    model: String,
    messages: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    max_tokens: u32,
    stream: bool,
    stream_options: StreamOptions,
    /// Generic OpenAI-style effort hint (`"low"|"medium"|"high"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    /// Nous Portal shape: `{"enabled": true, "effort": "…"}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningParam>,
}

#[derive(Serialize)]
struct ReasoningParam {
    enabled: bool,
    effort: &'static str,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

fn wire_messages(req: &ChatRequest) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(req.messages.len() + 1);
    if let Some(system) = &req.system {
        out.push(json!({"role": "system", "content": system}));
    }
    for m in &req.messages {
        out.push(match m {
            Message::User { content } => json!({"role": "user", "content": content}),
            Message::Assistant {
                content,
                tool_calls,
            } => {
                let mut v = json!({"role": "assistant"});
                // OpenAI wants content:null (not "") when only tool calls ran.
                if content.is_empty() && !tool_calls.is_empty() {
                    v["content"] = Value::Null;
                } else {
                    v["content"] = json!(content);
                }
                if !tool_calls.is_empty() {
                    v["tool_calls"] = Value::Array(
                        tool_calls
                            .iter()
                            .map(|c| {
                                json!({
                                    "id": c.id,
                                    "type": "function",
                                    "function": {
                                        "name": c.name.as_str(),
                                        "arguments": serde_json::to_string(&c.args)
                                            .unwrap_or_else(|_| "{}".to_string()),
                                    }
                                })
                            })
                            .collect(),
                    );
                }
                v
            }
            Message::Tool {
                call_id, content, ..
            } => json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": content,
            }),
        });
    }
    out
}

fn wire_tools(specs: &[ToolSpec]) -> Vec<Value> {
    specs
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name.as_str(),
                    "description": t.description,
                    "parameters": t.input_schema,
                }
            })
        })
        .collect()
}

// ── wire types (response) ─────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Option<DeltaBody>,
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DeltaBody {
    #[serde(default)]
    content: Option<String>,
    /// DeepSeek/Nous reasoning field.
    #[serde(default)]
    reasoning_content: Option<String>,
    /// Alternate reasoning field some vendors emit.
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct ToolCallDelta {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionDelta>,
}

#[derive(Debug, Default, Deserialize)]
struct FunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelListEntry>,
}

/// OpenRouter-shaped catalog entry — plain `OpenAI` `/models` endpoints
/// only fill `id`; routers (Nous Portal) publish the rest.
#[derive(Debug, Deserialize)]
struct ModelListEntry {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    pricing: Option<EntryPricing>,
    #[serde(default)]
    reasoning: Option<EntryReasoning>,
    #[serde(default)]
    supported_parameters: Option<Vec<String>>,
    #[serde(default)]
    architecture: Option<EntryArchitecture>,
}

#[derive(Debug, Deserialize)]
struct EntryPricing {
    /// USD per *token* as a decimal string.
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    completion: Option<String>,
    #[serde(default)]
    input_cache_read: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EntryReasoning {
    #[serde(default)]
    supported_efforts: Vec<String>,
    #[serde(default)]
    default_effort: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EntryArchitecture {
    #[serde(default)]
    input_modalities: Vec<String>,
}

/// Per-token USD string → USD per 1M tokens. `None` on absent/garbage.
fn price_per_mtok(raw: Option<&str>) -> Option<f64> {
    raw?.parse::<f64>().ok().map(|v| v * 1_000_000.0)
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<PromptDetails>,
    #[serde(default)]
    completion_tokens_details: Option<CompletionDetails>,
}

#[derive(Debug, Deserialize)]
struct PromptDetails {
    #[serde(default)]
    cached_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct CompletionDetails {
    #[serde(default)]
    reasoning_tokens: u64,
}

impl Usage {
    fn into_token_usage(self) -> TokenUsage {
        let thinking = self
            .completion_tokens_details
            .map_or(0, |d| d.reasoning_tokens);
        TokenUsage {
            input: self.prompt_tokens,
            cached_input: self.prompt_tokens_details.map_or(0, |d| d.cached_tokens),
            // Convention: `output` excludes thinking; they sum to the
            // billed completion_tokens.
            output: self.completion_tokens.saturating_sub(thinking),
            thinking,
        }
    }
}

/// In-flight accumulation for one streamed tool call (keyed by `index`).
#[derive(Default)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

// ── provider impl ─────────────────────────────────────────────────────────

impl Provider for OpenAiCompatProvider {
    fn stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> BoxStream<'static, Result<Delta, ProviderError>> {
        let client = self.client.clone();
        let credential = Arc::clone(&self.credential);
        let url = self.completions_url();
        let nous_dialect = self.nous_dialect;
        let (tx, mut rx) = mpsc::channel::<Result<Delta, ProviderError>>(64);

        tokio::spawn(async move {
            if let Err(e) = pump(&client, &url, &credential, req, cancel, &tx, nous_dialect).await {
                let _ = tx.send(Err(e)).await;
            }
        });

        Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
    }

    fn count_tokens<'a>(
        &'a self,
        req: &'a ChatRequest,
    ) -> BoxFuture<'a, Result<u64, ProviderError>> {
        Box::pin(async move {
            use hate_context::TokenCounter as _;
            let counter = self.counter.get_or_init(|| {
                hate_context::O200kCounter::new()
                    .expect("o200k BPE table is embedded; load cannot fail")
            });
            // o200k approximation over the request's text surface: system
            // prompt + message contents + tool schemas. Chat-template
            // overhead is a few tokens and constant — acceptable for the
            // compaction thresholds this feeds (spec §11 sanctions o200k
            // for OpenAI-compatible models).
            let mut text = String::new();
            if let Some(s) = &req.system {
                text.push_str(s);
            }
            for m in &req.messages {
                match m {
                    Message::User { content } | Message::Tool { content, .. } => {
                        text.push_str(content);
                    }
                    Message::Assistant {
                        content,
                        tool_calls,
                    } => {
                        text.push_str(content);
                        for c in tool_calls {
                            text.push_str(&serde_json::to_string(&c.args).unwrap_or_default());
                        }
                    }
                }
            }
            for t in &req.tools {
                text.push_str(&serde_json::to_string(&t.input_schema).unwrap_or_default());
            }
            counter
                .count(&text)
                .await
                .map_err(|e| ProviderError::Stream(format!("token count: {e}")))
        })
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, ProviderError>> {
        let client = self.client.clone();
        let credential = Arc::clone(&self.credential);
        let url = format!("{}/models", self.base_url);
        Box::pin(async move {
            let token = credential
                .bearer()
                .await
                .map_err(|e| ProviderError::Auth(e.to_string()))?;
            let resp = client
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;
            if !resp.status().is_success() {
                return Err(sse::status_error(resp).await);
            }
            let body: ModelsResponse = resp
                .json()
                .await
                .map_err(|e| ProviderError::Decode(format!("models: {e}")))?;
            Ok(body
                .data
                .into_iter()
                .map(|m| {
                    let tools = m
                        .supported_parameters
                        .as_ref()
                        .is_some_and(|p| p.iter().any(|s| s == "tools"));
                    let vision = m
                        .architecture
                        .as_ref()
                        .is_some_and(|a| a.input_modalities.iter().any(|s| s == "image"));
                    let has_catalog_meta = m.pricing.is_some()
                        || m.context_length.is_some()
                        || m.reasoning.is_some()
                        || m.supported_parameters.is_some()
                        || m.architecture.is_some();
                    ModelInfo {
                        name: m.name.clone().or_else(|| Some(m.id.clone())),
                        meta: has_catalog_meta.then(|| hate_core::ModelMeta {
                            context_tokens: m
                                .context_length
                                .map(|n| u32::try_from(n).unwrap_or(u32::MAX)),
                            price_in_mtok: m
                                .pricing
                                .as_ref()
                                .and_then(|p| price_per_mtok(p.prompt.as_deref())),
                            price_out_mtok: m
                                .pricing
                                .as_ref()
                                .and_then(|p| price_per_mtok(p.completion.as_deref())),
                            price_cache_mtok: m
                                .pricing
                                .as_ref()
                                .and_then(|p| price_per_mtok(p.input_cache_read.as_deref())),
                            reasoning_efforts: m
                                .reasoning
                                .as_ref()
                                .map(|r| r.supported_efforts.clone())
                                .unwrap_or_default(),
                            default_effort: m
                                .reasoning
                                .as_ref()
                                .and_then(|r| r.default_effort.clone()),
                            supports_tools: m.supported_parameters.as_ref().map(|_| tools),
                            supports_vision: m.architecture.as_ref().map(|_| vision),
                        }),
                        id: m.id,
                    }
                })
                .collect())
        })
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            vision: false, // model-dependent; per-model map lands with model routing
            tools: true,
            cache: CacheMode::Automatic,
            thinking: false, // detected per-model later; deltas still handled if emitted
        }
    }
}

// ── streaming pump ────────────────────────────────────────────────────────

#[instrument(skip_all)]
#[allow(clippy::too_many_arguments)]
async fn pump(
    client: &reqwest::Client,
    url: &str,
    credential: &SharedCredential,
    req: ChatRequest,
    cancel: CancellationToken,
    tx: &mpsc::Sender<Result<Delta, ProviderError>>,
    nous_dialect: bool,
) -> Result<(), ProviderError> {
    let messages = wire_messages(&req);
    let tools = wire_tools(&req.tools);
    // `Off` omits the field entirely — the endpoint's own default then
    // applies (Nous defaults to medium effort per its API docs).
    let (reasoning_effort, reasoning) = match req.thinking {
        ThinkingLevel::Off => (None, None),
        level if nous_dialect => (
            None,
            Some(ReasoningParam {
                enabled: true,
                effort: level.as_str(),
            }),
        ),
        level => (Some(level.as_str().to_string()), None),
    };
    let body = WireRequest {
        model: req.model,
        messages,
        tools,
        temperature: req.temperature,
        max_tokens: req.max_output_tokens,
        stream: true,
        stream_options: StreamOptions {
            include_usage: true,
        },
        reasoning_effort,
        reasoning,
    };

    let token = tokio::select! {
        t = credential.bearer() => t?,
        () = cancel.cancelled() => return Ok(()),
    };

    let response = tokio::select! {
        r = client.post(url).bearer_auth(&token).json(&body).send() => r,
        () = cancel.cancelled() => return Ok(()),
    }
    .map_err(|e| ProviderError::Network(e.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        return Err(sse::status_error(response).await);
    }

    let mut frames = std::pin::pin!(sse::parse(response));
    let mut pending: HashMap<usize, PendingToolCall> = HashMap::new();
    let mut tool_order: Vec<usize> = Vec::new();
    let mut saw_finish = false;
    let mut saw_done_marker = false;

    macro_rules! flush_tools {
        () => {
            for idx in tool_order.drain(..) {
                if let Some(p) = pending.remove(&idx) {
                    if let Ok(call) = finish_tool_call(p) {
                        if tx.send(Ok(Delta::ToolCall(call))).await.is_err() {
                            return Ok(());
                        }
                    } else {
                        warn!("dropping undecodable tool call args");
                    }
                }
            }
        };
    }

    loop {
        let frame = tokio::select! {
            () = cancel.cancelled() => return Ok(()),
            f = frames.next() => match f {
                Some(f) => f?,
                None => break,
            },
        };

        if frame.data.trim() == "[DONE]" {
            saw_done_marker = true;
            break;
        }

        let chunk: Chunk = match serde_json::from_str(&frame.data) {
            Ok(c) => c,
            Err(e) => {
                debug!(error = %e, data = %frame.data, "skipping undecodable chunk");
                continue;
            }
        };

        if let Some(u) = chunk.usage {
            let _ = tx.send(Ok(Delta::Usage(u.into_token_usage()))).await;
        }

        let Some(choice) = chunk.choices.into_iter().next() else {
            continue;
        };
        if let Some(delta) = choice.delta {
            if let Some(text) = delta.content
                && !text.is_empty()
                && tx.send(Ok(Delta::Text(text))).await.is_err()
            {
                return Ok(());
            }
            let thinking = delta.reasoning_content.or(delta.reasoning);
            if let Some(t) = thinking
                && !t.is_empty()
                && tx.send(Ok(Delta::Thinking(t))).await.is_err()
            {
                return Ok(());
            }
            if let Some(calls) = delta.tool_calls {
                for tc in calls {
                    let entry = pending.entry(tc.index).or_insert_with(|| {
                        tool_order.push(tc.index);
                        PendingToolCall::default()
                    });
                    if let Some(id) = tc.id {
                        entry.id = id;
                    }
                    if let Some(f) = tc.function {
                        if let Some(name) = f.name {
                            entry.name = name;
                        }
                        if let Some(args) = f.arguments {
                            entry.arguments.push_str(&args);
                        }
                    }
                }
            }
        }

        // Do NOT break on finish_reason: OpenAI sends the usage chunk
        // *after* it. Keep reading until [DONE] or body end.
        if choice.finish_reason.is_some() {
            flush_tools!();
            saw_finish = true;
        }
    }

    flush_tools!();
    if saw_done_marker || saw_finish {
        let _ = tx.send(Ok(Delta::Done)).await;
    } else {
        // Body ended with neither [DONE] nor a finish_reason — the stream
        // was truncated. Report honestly rather than fake completion.
        let _ = tx
            .send(Err(ProviderError::Stream(
                "stream ended before finish_reason/[DONE]".to_string(),
            )))
            .await;
    }
    Ok(())
}

fn finish_tool_call(p: PendingToolCall) -> Result<ToolCall, serde_json::Error> {
    let args: Value = if p.arguments.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&p.arguments)?
    };
    Ok(ToolCall {
        id: p.id,
        name: ToolName::new(p.name),
        args,
    })
}
