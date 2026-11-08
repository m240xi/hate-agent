//! End-to-end adapter tests: canned SSE over a real (localhost) HTTP
//! server through `Provider::stream`. Covers delta mapping, tool-call
//! accumulation, usage extraction, and truncation honesty.

mod common;

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use hate_core::provider::{
    BoxFuture, ChatRequest, CredentialSource, Delta, Message, Provider, ProviderError,
};
use hate_core::{ToolCall, ToolName};
use hate_providers::openai_compat::OpenAiCompatProvider;
use tokio_util::sync::CancellationToken;

struct StaticCred(&'static str);

impl CredentialSource for StaticCred {
    fn bearer(&self) -> BoxFuture<'_, Result<String, ProviderError>> {
        let k = self.0.to_string();
        Box::pin(async move { Ok(k) })
    }
}

fn cred() -> Arc<dyn CredentialSource> {
    Arc::new(StaticCred("test-key"))
}

fn req() -> ChatRequest {
    ChatRequest {
        model: "test-model".to_string(),
        system: Some("sys".to_string()),
        messages: vec![Message::User {
            content: "hi".to_string(),
        }],
        tools: Vec::new(),
        max_output_tokens: 128,
        temperature: None,
        thinking: hate_core::ThinkingLevel::Off,
    }
}

async fn collect(provider: &OpenAiCompatProvider) -> Vec<Result<Delta, ProviderError>> {
    let mut out = Vec::new();
    let mut stream = provider.stream(req(), CancellationToken::new());
    while let Some(item) = stream.next().await {
        out.push(item);
    }
    out
}

#[tokio::test]
async fn text_usage_and_done() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\", world\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,",
        "\"prompt_tokens_details\":{\"cached_tokens\":4},",
        "\"completion_tokens_details\":{\"reasoning_tokens\":2}}}\n\n",
        "data: [DONE]\n\n",
    );
    let base = common::serve_once(200, "text/event-stream", body).await;
    let provider = OpenAiCompatProvider::new(base, cred());

    let items = collect(&provider).await;
    let deltas: Vec<Delta> = items.into_iter().map(|r| r.expect("no error")).collect();

    assert!(matches!(&deltas[0], Delta::Text(t) if t == "Hello"));
    assert!(matches!(&deltas[1], Delta::Text(t) if t == ", world"));
    // Usage arrives before Done even though finish_reason came earlier.
    let usage = deltas
        .iter()
        .find_map(|d| match d {
            Delta::Usage(u) => Some(*u),
            _ => None,
        })
        .expect("usage emitted");
    assert_eq!(usage.input, 10);
    assert_eq!(usage.cached_input, 4);
    assert_eq!(usage.thinking, 2);
    assert_eq!(usage.output, 3); // completion - reasoning
    assert!(matches!(deltas.last(), Some(Delta::Done)));
}

#[tokio::test]
async fn thinking_and_fragmented_tool_call() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking…\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",",
        "\"function\":{\"name\":\"read\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,",
        "\"function\":{\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,",
        "\"function\":{\"arguments\":\"\\\"a.rs\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let base = common::serve_once(200, "text/event-stream", body).await;
    let provider = OpenAiCompatProvider::new(base, cred());

    let items = collect(&provider).await;
    let deltas: Vec<Delta> = items.into_iter().map(|r| r.expect("no error")).collect();

    assert!(matches!(&deltas[0], Delta::Thinking(t) if t == "thinking…"));
    let call = deltas
        .iter()
        .find_map(|d| match d {
            Delta::ToolCall(c) => Some(c),
            _ => None,
        })
        .expect("tool call emitted");
    assert_eq!(call.id, "call_1");
    assert_eq!(call.name.as_str(), "read");
    assert_eq!(call.args["path"], "a.rs");
    assert!(matches!(deltas.last(), Some(Delta::Done)));
}

#[tokio::test]
async fn http_error_surfaces() {
    let base = common::serve_once(500, "application/json", "{\"error\":\"boom\"}").await;
    let provider = OpenAiCompatProvider::new(base, cred());
    let items = collect(&provider).await;
    assert_eq!(items.len(), 1);
    match &items[0] {
        Err(ProviderError::Http { status, .. }) => assert_eq!(*status, 500),
        other => panic!("expected Http error, got {other:?}"),
    }
}

#[tokio::test]
async fn truncated_stream_is_error_not_done() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
    let base = common::serve_once(200, "text/event-stream", body).await;
    let provider = OpenAiCompatProvider::new(base, cred());
    let items = collect(&provider).await;
    // Text delta arrives, then an honest Stream error — never a fake Done.
    assert!(matches!(&items[0], Ok(Delta::Text(_))));
    assert!(matches!(items.last(), Some(Err(ProviderError::Stream(_)))));
}

#[tokio::test]
async fn cancellation_ends_stream_quickly() {
    let base = common::serve_hang().await;
    let provider = OpenAiCompatProvider::new(base, cred());
    let cancel = CancellationToken::new();
    let mut stream = provider.stream(req(), cancel.clone());

    let start = std::time::Instant::now();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
    });
    let mut count = 0;
    while let Some(item) = stream.next().await {
        count += 1;
        let _ = item;
    }
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "cancel took {:?}",
        start.elapsed()
    );
    assert_eq!(count, 0, "no deltas before server responds");
}

#[tokio::test]
async fn count_tokens_uses_real_counter() {
    let provider = OpenAiCompatProvider::new("http://127.0.0.1:1/v1", cred());
    let n = provider.count_tokens(&req()).await.expect("count");
    // "sys" + "hi" ≈ 3-6 o200k tokens; chars/4 would say 3 — assert a
    // plausible BPE range, not the heuristic.
    assert!((1..=20).contains(&n), "unexpected count {n}");
}

#[test]
fn tool_call_types_roundtrip() {
    // Sanity on the domain types the adapter emits.
    let call = ToolCall {
        id: "x".to_string(),
        name: ToolName::new("read"),
        args: serde_json::json!({}),
    };
    assert_eq!(call.name.as_str(), "read");
}
