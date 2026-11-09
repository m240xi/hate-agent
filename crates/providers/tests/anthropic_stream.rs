mod common;

use std::sync::Arc;

use futures_util::StreamExt;
use hate_core::ThinkingLevel;
use hate_core::provider::{
    BoxFuture, ChatRequest, CredentialSource, Delta, Message, Provider, ProviderError,
};
use hate_providers::anthropic::AnthropicProvider;
use tokio_util::sync::CancellationToken;

struct StaticCred(&'static str);

impl CredentialSource for StaticCred {
    fn bearer(&self) -> BoxFuture<'_, Result<String, ProviderError>> {
        let k = self.0.to_string();
        Box::pin(async move { Ok(k) })
    }
}

fn req() -> ChatRequest {
    ChatRequest {
        model: "claude-test".to_string(),
        system: Some("sys".to_string()),
        messages: vec![
            Message::User {
                content: "hi".to_string(),
            },
            Message::Assistant {
                content: "hello".to_string(),
                tool_calls: vec![],
            },
            Message::Tool {
                call_id: "t1".to_string(),
                name: "read".to_string(),
                content: "file body".to_string(),
            },
            Message::User {
                content: "next".to_string(),
            },
        ],
        tools: Vec::new(),
        max_output_tokens: 128,
        temperature: None,
        thinking: ThinkingLevel::Off,
    }
}

async fn collect(provider: &AnthropicProvider) -> Vec<Result<Delta, ProviderError>> {
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
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,",
        "\"cache_read_input_tokens\":4,\"cache_creation_input_tokens\":2}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" there\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let base = common::serve_once(200, "text/event-stream", body).await;
    let provider = AnthropicProvider::new(base, Arc::new(StaticCred("k")));

    let items = collect(&provider).await;
    let deltas: Vec<Delta> = items.into_iter().map(|r| r.expect("no error")).collect();

    assert!(matches!(&deltas[0], Delta::Text(t) if t == "Hi"));
    assert!(matches!(&deltas[1], Delta::Text(t) if t == " there"));
    let usage = deltas
        .iter()
        .find_map(|d| match d {
            Delta::Usage(u) => Some(*u),
            _ => None,
        })
        .expect("usage emitted");
    assert_eq!(usage.input, 16);
    assert_eq!(usage.cached_input, 4);
    assert_eq!(usage.output, 5);
    assert!(matches!(deltas.last(), Some(Delta::Done)));
}

#[tokio::test]
async fn thinking_and_tool_use_block() {
    let body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"hmm\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a.rs\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":9}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let base = common::serve_once(200, "text/event-stream", body).await;
    let provider = AnthropicProvider::new(base, Arc::new(StaticCred("k")));

    let items = collect(&provider).await;
    let deltas: Vec<Delta> = items.into_iter().map(|r| r.expect("no error")).collect();

    assert!(matches!(&deltas[0], Delta::Thinking(t) if t == "hmm"));
    let call = deltas
        .iter()
        .find_map(|d| match d {
            Delta::ToolCall(c) => Some(c),
            _ => None,
        })
        .expect("tool call emitted");
    assert_eq!(call.id, "toolu_1");
    assert_eq!(call.name.as_str(), "read");
    assert_eq!(call.args["path"], "a.rs");
    assert!(matches!(deltas.last(), Some(Delta::Done)));
}

#[tokio::test]
async fn api_error_event_maps_to_error() {
    let body = concat!(
        "event: error\n",
        "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
    );
    let base = common::serve_once(200, "text/event-stream", body).await;
    let provider = AnthropicProvider::new(base, Arc::new(StaticCred("k")));
    let items = collect(&provider).await;
    assert!(matches!(items.last(), Some(Err(_))), "got {items:?}");
}

#[tokio::test]
async fn missing_message_stop_is_error() {
    let body = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1}}}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
    );
    let base = common::serve_once(200, "text/event-stream", body).await;
    let provider = AnthropicProvider::new(base, Arc::new(StaticCred("k")));
    let items = collect(&provider).await;
    assert!(matches!(items.last(), Some(Err(ProviderError::Stream(_)))));
}
