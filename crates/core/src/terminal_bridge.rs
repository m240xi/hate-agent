use std::fmt::Write as _;
use std::sync::Arc;

use hate_context::O200kCounter;
use serde::Serialize;
use serde_json::json;

use crate::provider::ToolSpec;
use crate::tools::{FULL_COUNT_CAP, SENT_CAP, ToolOutput, shape};
use crate::types::{ToolCall, ToolName};

#[derive(Debug, Clone, Serialize)]
pub struct TerminalTabSummary {
    pub id: String,
    pub title: String,
    pub shell: String,
    pub cwd: String,
    pub alive: bool,
}

pub trait TerminalBridge: Send + Sync {
    fn list_terminals(&self) -> Vec<TerminalTabSummary>;
    fn read_output(&self, id: Option<&str>, tail_lines: usize) -> Result<String, String>;
    fn send_input(&self, id: Option<&str>, input: &str) -> Result<String, String>;
}

pub struct TerminalToolkit {
    bridge: Arc<dyn TerminalBridge>,
    counter: O200kCounter,
}

impl TerminalToolkit {
    pub fn new(bridge: Arc<dyn TerminalBridge>) -> Result<Self, String> {
        Ok(Self {
            bridge,
            counter: O200kCounter::new().map_err(|e| e.to_string())?,
        })
    }

    pub fn has_open_terminals(&self) -> bool {
        !self.bridge.list_terminals().is_empty()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        if !self.has_open_terminals() {
            return Vec::new();
        }
        vec![
            ToolSpec {
                name: ToolName::new("terminal_read"),
                description: "List open interactive workspace terminal tabs and read recent output from a tab.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Optional terminal tab ID; defaults to the active/latest tab." },
                        "tail_lines": { "type": "integer", "description": "Number of recent output lines to return (default 80, max 300)." }
                    },
                    "additionalProperties": false
                }),
            },
            ToolSpec {
                name: ToolName::new("terminal_write"),
                description: "Send a command or input text to an open interactive workspace terminal tab.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Optional terminal tab ID; defaults to the active/latest tab." },
                        "input": { "type": "string", "description": "Text to write to the terminal (include \\n or set press_enter to submit)." },
                        "press_enter": { "type": "boolean", "description": "Append a newline after input (default true)." }
                    },
                    "required": ["input"],
                    "additionalProperties": false
                }),
            },
        ]
    }

    pub fn gate_reason(&self, call: &ToolCall) -> Option<(String, Option<String>)> {
        match call.name.as_str() {
            "terminal_write" => {
                let text = call
                    .args
                    .get("input")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .chars()
                    .take(200)
                    .collect();
                Some((
                    "writes to an interactive terminal tab".to_string(),
                    Some(text),
                ))
            }
            _ => None,
        }
    }

    pub fn is_read_only(name: &str) -> bool {
        name == "terminal_read"
    }

    pub fn execute(&self, call: &ToolCall) -> ToolOutput {
        let res = match call.name.as_str() {
            "terminal_read" => self.exec_read(call),
            "terminal_write" => self.exec_write(call),
            other => Err(format!("unknown terminal tool {other:?}")),
        };
        self.output_for(res)
    }

    pub fn tool_error(&self, msg: String) -> ToolOutput {
        self.output_for(Err(msg))
    }

    fn exec_read(&self, call: &ToolCall) -> Result<String, String> {
        let tabs = self.bridge.list_terminals();
        if tabs.is_empty() {
            return Err("no interactive workspace terminal tabs are currently open".to_string());
        }
        let id = call.args.get("id").and_then(|v| v.as_str());
        let tail = call
            .args
            .get("tail_lines")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(80)
            .clamp(1, 300) as usize;
        let output = self.bridge.read_output(id, tail)?;
        let mut out = String::from("Open terminal tabs:\n");
        for t in &tabs {
            let status = if t.alive { "alive" } else { "exited" };
            let _ = writeln!(
                out,
                "- {} ({}, {}, cwd: {}, {status})",
                t.id, t.title, t.shell, t.cwd
            );
        }
        out.push_str("\nRecent output:\n");
        out.push_str(if output.trim().is_empty() {
            "(no output yet)"
        } else {
            &output
        });
        Ok(out)
    }

    fn exec_write(&self, call: &ToolCall) -> Result<String, String> {
        let input = call
            .args
            .get("input")
            .and_then(|v| v.as_str())
            .ok_or("terminal_write: missing `input`")?;
        let press_enter = call
            .args
            .get("press_enter")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let id = call.args.get("id").and_then(|v| v.as_str());
        let payload = if press_enter && !input.ends_with('\n') && !input.ends_with('\r') {
            format!("{input}\r")
        } else {
            input.to_string()
        };
        self.bridge.send_input(id, &payload)
    }

    fn output_for(&self, result: Result<String, String>) -> ToolOutput {
        match result {
            Ok(full) => {
                let sent = shape(&full, SENT_CAP);
                let preview: String = sent.chars().take(400).collect();
                let cap_end = floor_char_boundary(&full, FULL_COUNT_CAP);
                let full_tokens = self.counter.count_sync(&full[..cap_end]) as u32;
                let sent_tokens = self.counter.count_sync(&sent) as u32;
                ToolOutput {
                    content: sent,
                    preview,
                    full_tokens: full_tokens.max(sent_tokens),
                    sent_tokens,
                    ok: true,
                }
            }
            Err(e) => {
                let content = format!("error: {e}");
                let preview: String = content.chars().take(400).collect();
                let tokens = self.counter.count_sync(&content) as u32;
                ToolOutput {
                    content,
                    preview,
                    full_tokens: tokens,
                    sent_tokens: tokens,
                    ok: false,
                }
            }
        }
    }
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}
