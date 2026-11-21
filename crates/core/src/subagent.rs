use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;

use futures_util::StreamExt;
use hate_context::O200kCounter;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::policy::{SecretRedactor, redact_chat_request};
use crate::provider::{ChatRequest, Delta, Message, Provider, ThinkingLevel, ToolSpec};
use crate::tools::WorkspaceTools;
use crate::types::{TokenUsage, ToolCall, ToolName};

const MAX_SUMMARY_TOKENS: usize = 400;
const SUBAGENT_PROMPT: &str = "You are a scoped read-only subagent. Investigate the task using only the provided tools and return a concise, factual summary (under 400 tokens) citing exact file paths and line numbers.";
const ALLOWED_SUBAGENT_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "glob",
    "grep",
    "symbol",
    "outline",
    "git_diff",
    "git_log",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentReport {
    pub summary: String,
    pub files_touched: Vec<String>,
    pub ok: bool,
}

impl SubagentReport {
    pub fn format_for_parent(&self) -> String {
        let mut out = self.summary.clone();
        if !self.files_touched.is_empty() {
            let _ = write!(
                out,
                "\n\nfiles_inspected: {}",
                self.files_touched.join(", ")
            );
        }
        out
    }
}

pub fn spawn_agent_spec() -> ToolSpec {
    ToolSpec {
        name: ToolName::new("spawn_agent"),
        description:
            "Run an isolated read-only subagent for exploration, summarization, or diff review."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "focused exploration, summary, or review task"},
                "tools": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "optional read-only tool allowlist"
                },
                "max_turns": {"type": "integer", "description": "max rounds (1-10, default 6)"},
                "max_tokens": {"type": "integer", "description": "token budget (1000-32000, default 8000)"}
            },
            "required": ["task"],
            "additionalProperties": false
        }),
    }
}

pub async fn run_subagent(
    provider: &Arc<dyn Provider>,
    model: &str,
    tools: &Arc<WorkspaceTools>,
    call: &ToolCall,
    depth: u8,
    cancel: &CancellationToken,
) -> SubagentReport {
    if depth >= 1 {
        return SubagentReport {
            summary:
                "spawn_agent: max subagent depth (1) reached; child agents cannot spawn subagents."
                    .to_string(),
            files_touched: Vec::new(),
            ok: false,
        };
    }

    let Some(task) = call
        .args
        .get("task")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return SubagentReport {
            summary: "spawn_agent: missing `task`".to_string(),
            files_touched: Vec::new(),
            ok: false,
        };
    };

    let requested_tools: Option<Vec<&str>> = call
        .args
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect());
    let max_turns = call
        .args
        .get("max_turns")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(6)
        .clamp(1, 10) as usize;
    let max_tokens = call
        .args
        .get("max_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(8_000)
        .clamp(1_000, 32_000);

    let allowed_names: Vec<&str> = match requested_tools {
        Some(list) if !list.is_empty() => ALLOWED_SUBAGENT_TOOLS
            .iter()
            .copied()
            .filter(|name| list.contains(name))
            .collect(),
        _ => ALLOWED_SUBAGENT_TOOLS.to_vec(),
    };

    let mut specs: Vec<ToolSpec> = tools
        .specs()
        .into_iter()
        .chain(WorkspaceTools::deferred_specs())
        .filter(|s| allowed_names.contains(&s.name.as_str()))
        .collect();
    specs.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));

    let mut convo = vec![Message::user(task)];
    let mut files_touched = BTreeSet::new();
    let mut spent_tokens = 0u64;
    let mut final_text = String::new();

    for _round in 0..max_turns {
        if cancel.is_cancelled() {
            return SubagentReport {
                summary: "subagent cancelled".to_string(),
                files_touched: files_touched.into_iter().collect(),
                ok: false,
            };
        }

        let mut req = ChatRequest {
            model: model.to_string(),
            system: Some(SUBAGENT_PROMPT.to_string()),
            messages: convo.clone(),
            tools: specs.clone(),
            max_output_tokens: 1500,
            temperature: None,
            thinking: ThinkingLevel::Off,
        };
        let _ = redact_chat_request(
            &SecretRedactor::default(),
            &mut req.system,
            &mut req.messages,
        );

        let stream = provider.stream(req, cancel.clone());
        tokio::pin!(stream);

        let mut text = String::new();
        let mut calls = Vec::new();
        let mut round_usage = TokenUsage::default();

        while let Some(item) = stream.next().await {
            match item {
                Ok(Delta::Text(t)) => text.push_str(&t),
                Ok(Delta::ToolCall(c)) => calls.push(c),
                Ok(Delta::Usage(u)) => round_usage = u,
                Ok(Delta::Done) => break,
                Ok(Delta::Thinking(_)) => {}
                Err(e) => {
                    return SubagentReport {
                        summary: format!("subagent provider error: {e}"),
                        files_touched: files_touched.into_iter().collect(),
                        ok: false,
                    };
                }
            }
        }

        spent_tokens += round_usage.input + round_usage.output;
        if !text.is_empty() {
            final_text.clone_from(&text);
        }
        convo.push(Message::Assistant {
            content: text,
            tool_calls: calls.clone(),
        });

        if calls.is_empty() {
            return SubagentReport {
                summary: cap_to_tokens(&final_text, MAX_SUMMARY_TOKENS),
                files_touched: files_touched.into_iter().collect(),
                ok: true,
            };
        }

        for sub_call in calls {
            if let Some(p) = sub_call.args.get("path").and_then(|v| v.as_str())
                && !p.trim().is_empty()
                && p != "."
            {
                files_touched.insert(p.to_string());
            }
            let out = if allowed_names.contains(&sub_call.name.as_str()) {
                tools.execute(&sub_call, cancel).await
            } else {
                tools.tool_error(format!(
                    "subagent tool `{}` is not in allowlist",
                    sub_call.name.as_str()
                ))
            };
            convo.push(Message::Tool {
                call_id: sub_call.id,
                name: sub_call.name.as_str().to_string(),
                content: out.content,
            });
        }

        if spent_tokens >= max_tokens {
            break;
        }
    }

    let fallback = if final_text.trim().is_empty() {
        format!(
            "subagent reached budget ({max_turns} rounds / {spent_tokens} tokens) without final summary."
        )
    } else {
        final_text
    };
    SubagentReport {
        summary: cap_to_tokens(&fallback, MAX_SUMMARY_TOKENS),
        files_touched: files_touched.into_iter().collect(),
        ok: true,
    }
}

pub async fn review_large_diff(
    provider: &Arc<dyn Provider>,
    model: &str,
    diff: &str,
    cancel: &CancellationToken,
) -> Option<String> {
    if diff.trim().is_empty() || cancel.is_cancelled() {
        return None;
    }
    let clipped: String = diff.chars().take(8_000).collect();
    let mut req = ChatRequest {
        model: model.to_string(),
        system: Some(
            "You are a terse second-opinion code reviewer. Reply in at most 5 short lines: \
             risks, edge-case bugs, or 'looks solid'."
                .to_string(),
        ),
        messages: vec![Message::user(format!("Review this diff:\n\n{clipped}"))],
        tools: Vec::new(),
        max_output_tokens: 300,
        temperature: None,
        thinking: ThinkingLevel::Off,
    };
    let _ = redact_chat_request(
        &SecretRedactor::default(),
        &mut req.system,
        &mut req.messages,
    );
    let stream = provider.stream(req, cancel.clone());
    tokio::pin!(stream);
    let mut out = String::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(Delta::Text(t)) => out.push_str(&t),
            Ok(Delta::Done) => break,
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    let five_lines: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(5)
        .collect();
    if five_lines.is_empty() {
        None
    } else {
        Some(five_lines.join("\n"))
    }
}

fn cap_to_tokens(text: &str, max_tokens: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "(empty subagent summary)".to_string();
    }
    let Ok(counter) = O200kCounter::new() else {
        return trimmed.chars().take(max_tokens * 4).collect();
    };
    if counter.count_sync(trimmed) <= max_tokens as u64 {
        return trimmed.to_string();
    }
    let mut lo = 0usize;
    let mut hi = trimmed.len();
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let mut boundary = mid.min(trimmed.len());
        while boundary > 0 && !trimmed.is_char_boundary(boundary) {
            boundary -= 1;
        }
        if counter.count_sync(&trimmed[..boundary]) <= max_tokens as u64 {
            lo = boundary;
        } else {
            hi = boundary.saturating_sub(1);
        }
        if hi <= lo {
            break;
        }
    }
    while lo > 0 && !trimmed.is_char_boundary(lo) {
        lo -= 1;
    }
    format!("{}…", &trimmed[..lo])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_to_tokens_enforces_400_token_budget() {
        let big = "word ".repeat(1200);
        let capped = cap_to_tokens(&big, MAX_SUMMARY_TOKENS);
        let counter = O200kCounter::new().unwrap();
        assert!(counter.count_sync(&capped) <= (MAX_SUMMARY_TOKENS as u64) + 2);
        assert!(capped.ends_with('…'));
    }
}
