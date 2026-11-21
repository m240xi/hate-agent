use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::BufRead;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use hate_context::O200kCounter;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::checkpoint::CheckpointBundle;
use crate::index::{SymKind, SymbolHit, collect_file_symbols, collect_workspace_symbols_all};
use crate::process_util::{CmdRun, hide_std_cmd, run_tokio_cmd_cancellable};
use crate::provider::ToolSpec;
use crate::tool_search::{ToolCandidate, search_tools};
use crate::types::{ToolCall, ToolName};

pub(crate) const SENT_CAP: usize = 48 * 1024;
pub(crate) const FULL_COUNT_CAP: usize = 256 * 1024;
const MAX_LIST_ENTRIES: usize = 500;
const MAX_GLOB_MATCHES: usize = 200;
const MAX_GREP_MATCHES: usize = 100;
const MAX_GREP_PER_FILE: usize = 20;
const MAX_GREP_CONTEXT: u64 = 3;
const MAX_GREP_FILE_BYTES: u64 = 512 * 1024;
const MAX_SYMBOL_HITS: usize = 30;
const SYMBOL_CACHE_TTL: Duration = Duration::from_secs(30);
const BASH_MAX_LINES: usize = 160;
const BASH_MAX_BYTES: usize = 12 * 1024;
const DIFF_PREVIEW_MAX_LINES: usize = 80;
const DIFF_PREVIEW_MAX_BYTES: usize = 4096;
const READ_ONLY_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "glob",
    "grep",
    "symbol",
    "git_diff",
    "git_log",
    "outline",
    "web_fetch",
];
pub(crate) const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".venv",
    "__pycache__",
    ".idea",
    ".vs",
];

pub struct ToolOutput {
    pub content: String,
    pub preview: String,
    pub raw_preview: Option<String>,
    pub full_tokens: u32,
    pub sent_tokens: u32,
    pub ok: bool,
}

pub struct WorkspaceTools {
    root: PathBuf,
    counter: O200kCounter,
    checkpoint_root: Mutex<Option<PathBuf>>,
    pending: Mutex<BTreeMap<String, Option<Vec<u8>>>>,
    instructions: RwLock<Option<String>>,
    trusted: AtomicBool,
    auto_approve_safe_bash: AtomicBool,
    generation: AtomicU64,
    memo: Mutex<HashMap<String, ReadMemo>>,
    symbols: Mutex<Option<SymbolCache>>,
}

struct ReadMemo {
    generation: u64,
    stamp: Option<(SystemTime, u64)>,
    tokens: u32,
}

struct SymbolCache {
    generation: u64,
    built: Instant,
    hits: Arc<Vec<SymbolHit>>,
}

pub const PROJECT_INSTRUCTIONS: &[&str] = &[
    "AGENTS.md",
    ".hate-agent.md",
    "CLAUDE.md",
    ".github/copilot-instructions.md",
    "GEMINI.md",
    ".cursorrules",
];

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkspaceRule {
    pub file: String,
    pub active: bool,
    pub bytes: u32,
    pub preview: String,
}

pub fn inspect_instructions(root: &Path) -> Vec<WorkspaceRule> {
    let mut out = Vec::new();
    let mut found_active = false;
    for rel in PROJECT_INSTRUCTIONS {
        let p = root.join(rel);
        let Ok(meta) = p.metadata() else { continue };
        if !meta.is_file() || meta.len() > INSTRUCTIONS_CAP {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&p) {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            let active = !found_active;
            found_active = true;
            let preview: String = trimmed.chars().take(240).collect();
            out.push(WorkspaceRule {
                file: (*rel).to_string(),
                active,
                bytes: u32::try_from(meta.len()).unwrap_or(u32::MAX),
                preview,
            });
        }
    }
    out
}
const INSTRUCTIONS_CAP: u64 = 12 * 1024;

impl WorkspaceTools {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        Self::new_with_trust(root, true)
    }

    pub fn new_with_trust(root: impl AsRef<Path>, trusted: bool) -> Result<Self, String> {
        let canon = root
            .as_ref()
            .canonicalize()
            .map_err(|e| format!("workspace root: {e}"))?;
        let instructions = if trusted {
            load_instructions(&canon)
        } else {
            None
        };
        Ok(Self {
            root: canon,
            counter: O200kCounter::new().map_err(|e| e.to_string())?,
            checkpoint_root: Mutex::new(None),
            pending: Mutex::new(BTreeMap::new()),
            instructions: RwLock::new(instructions),
            trusted: AtomicBool::new(trusted),
            auto_approve_safe_bash: AtomicBool::new(true),
            generation: AtomicU64::new(0),
            memo: Mutex::new(HashMap::new()),
            symbols: Mutex::new(None),
        })
    }

    pub fn is_read_only(name: &str) -> bool {
        READ_ONLY_TOOLS.contains(&name)
    }

    pub fn reset_read_memo(&self) {
        self.memo.lock().expect("read memo poisoned").clear();
    }

    pub fn note_mutation(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn project_context(&self) -> Option<String> {
        self.instructions
            .read()
            .expect("instructions lock poisoned")
            .clone()
    }

    pub fn is_trusted(&self) -> bool {
        self.trusted.load(Ordering::Relaxed)
    }

    pub fn set_trusted(&self, trusted: bool) {
        self.trusted.store(trusted, Ordering::Relaxed);
        let next = if trusted {
            load_instructions(&self.root)
        } else {
            None
        };
        *self
            .instructions
            .write()
            .expect("instructions lock poisoned") = next;
    }

    pub fn set_auto_approve_safe_bash(&self, allow: bool) {
        self.auto_approve_safe_bash.store(allow, Ordering::Relaxed);
    }

    pub fn enable_checkpoints(&self, dir: PathBuf) {
        *self
            .checkpoint_root
            .lock()
            .expect("checkpoint root poisoned") = Some(dir);
    }

    fn capture_original(&self, fs_path: &Path) {
        if self
            .checkpoint_root
            .lock()
            .expect("checkpoint root poisoned")
            .is_none()
        {
            return;
        }
        let Ok(rel) = fs_path.strip_prefix(&self.root) else {
            return;
        };
        let key = rel.to_string_lossy().replace('\\', "/");
        let mut pending = self.pending.lock().expect("checkpoint pending poisoned");
        if pending.contains_key(&key) {
            return;
        }
        pending.insert(key, std::fs::read(fs_path).ok());
    }

    pub fn drain_checkpoint(&self) -> Option<CheckpointBundle> {
        let files = std::mem::take(&mut *self.pending.lock().expect("checkpoint pending poisoned"));
        if files.is_empty() {
            return None;
        }
        Some(CheckpointBundle {
            id: Uuid::new_v4().to_string(),
            root: self
                .checkpoint_root
                .lock()
                .expect("checkpoint root poisoned")
                .clone()?,
            files,
        })
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut list = vec![
            spec(
                "bash",
                "Run a shell command in the workspace root. Long output is clipped to head + tail; filter it yourself.",
                json!({
                    "type": "object",
                    "properties": {
                        "command": {"type": "string"},
                        "timeout_ms": {"type": "integer", "description": "default 120000"}
                    },
                    "required": ["command"],
                    "additionalProperties": false
                }),
            ),
            spec(
                "edit_file",
                "Replace exact text in a file. old_string must match exactly once unless replace_all is true.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "old_string": {"type": "string"},
                        "new_string": {"type": "string"},
                        "replace_all": {"type": "boolean"}
                    },
                    "required": ["path", "old_string", "new_string"],
                    "additionalProperties": false
                }),
            ),
            spec(
                "glob",
                "Find files by name pattern (* ** ?), optionally under path.",
                json!({
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string"},
                        "path": {"type": "string"}
                    },
                    "required": ["pattern"],
                    "additionalProperties": false
                }),
            ),
            spec(
                "grep",
                "Search file contents by regex or literal text. Matches are grouped by file as line: text.",
                json!({
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "regex; also matched as literal text"},
                        "path": {"type": "string", "description": "file or dir, default '.'"},
                        "glob": {"type": "string", "description": "filename filter, e.g. *.rs"},
                        "ignore_case": {"type": "boolean"},
                        "context": {"type": "integer", "description": "lines around each match, 0-3"}
                    },
                    "required": ["pattern"],
                    "additionalProperties": false
                }),
            ),
            spec(
                "list_dir",
                "List a directory tree. path defaults to '.', depth to 1 (max 3).",
                json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "depth": {"type": "integer"}
                    },
                    "additionalProperties": false
                }),
            ),
            spec(
                "read_file",
                "Read a file with line numbers. Page large files with offset (1-based line) and limit (default 400).",
                json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "path relative to workspace root"},
                        "offset": {"type": "integer", "description": "1-based first line"},
                        "limit": {"type": "integer", "description": "max lines to return"}
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }),
            ),
            spec(
                "symbol",
                "Find symbol definitions, references, or call sites. Returns path, line, and signature.",
                json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "symbol name or part of it"},
                        "kind": {"type": "string", "enum": ["def", "ref", "call"], "description": "def (default), ref, or call"},
                        "path": {"type": "string", "description": "limit to this dir or file"}
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
            ),
            Self::tool_search_spec(),
            spec(
                "write_file",
                "Create or overwrite a file (parent dirs are created). Prefer edit_file for existing files.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["path", "content"],
                    "additionalProperties": false
                }),
            ),
        ];
        list.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
        list
    }

    pub fn tool_search_spec() -> ToolSpec {
        spec(
            "tool_search",
            "Search and load deferred tools (git_diff, git_log, apply_patch, outline, web_fetch, MCP tools) by keyword.",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "keywords describing the tool you need"},
                    "limit": {"type": "integer", "description": "max tools to load (1-3, default 3)"}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        )
    }

    pub fn deferred_specs() -> Vec<ToolSpec> {
        vec![
            spec(
                "apply_patch",
                "Apply a unified diff patch across one or more workspace files.",
                json!({
                    "type": "object",
                    "properties": {
                        "patch": {"type": "string", "description": "unified diff text (---/+++ and @@ hunks)"}
                    },
                    "required": ["patch"],
                    "additionalProperties": false
                }),
            ),
            spec(
                "git_diff",
                "Show git diff for working tree or staged changes, optionally scoped to path.",
                json!({
                    "type": "object",
                    "properties": {
                        "staged": {"type": "boolean", "description": "inspect staged index changes"},
                        "path": {"type": "string", "description": "optional workspace-relative path"}
                    },
                    "additionalProperties": false
                }),
            ),
            spec(
                "git_log",
                "Show recent git commit history, optionally filtered by path.",
                json!({
                    "type": "object",
                    "properties": {
                        "limit": {"type": "integer", "description": "max commits (1-50, default 15)"},
                        "path": {"type": "string", "description": "optional workspace-relative path"}
                    },
                    "additionalProperties": false
                }),
            ),
            spec(
                "outline",
                "Show a structural outline (symbols or headings) with line numbers for a file.",
                json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "workspace-relative file path"}
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }),
            ),
            spec(
                "web_fetch",
                "Fetch an HTTP(S) URL and return readable plain text.",
                json!({
                    "type": "object",
                    "properties": {
                        "url": {"type": "string", "description": "http:// or https:// URL to fetch"}
                    },
                    "required": ["url"],
                    "additionalProperties": false
                }),
            ),
            crate::subagent::spawn_agent_spec(),
        ]
    }

    pub fn deferred_spec(name: &str) -> Option<ToolSpec> {
        Self::deferred_specs()
            .into_iter()
            .find(|s| s.name.as_str() == name)
    }

    pub fn execute_tool_search_local(&self, call: &ToolCall) -> ToolOutput {
        let query = match arg_str(call, "query").map(str::trim) {
            Some(q) if !q.is_empty() => q,
            _ => return self.output_for(Err("tool_search: missing `query`".to_string())),
        };
        let limit = arg_u64(call, "limit").unwrap_or(3).clamp(1, 3) as usize;
        let candidates: Vec<ToolCandidate> = Self::deferred_specs()
            .into_iter()
            .map(|s| ToolCandidate::from_spec(s, ""))
            .collect();
        let hits = search_tools(query, limit, &candidates);
        if hits.is_empty() {
            return self.output_for(Ok(format!("no deferred tools matched {query:?}")));
        }
        let mut out = format!("loaded {} tool(s):", hits.len());
        for s in &hits {
            let _ = write!(out, "\n- {}: {}", s.name.as_str(), s.description);
        }
        self.output_for(Ok(out))
    }

    pub async fn execute(
        self: &Arc<Self>,
        call: &ToolCall,
        cancel: &CancellationToken,
    ) -> ToolOutput {
        if call.name.as_str() == "bash" {
            let out = self.bash(call, cancel).await;
            self.note_mutation();
            return out;
        }
        if call.name.as_str() == "web_fetch" {
            return self.web_fetch(call, cancel).await;
        }
        let this = Arc::clone(self);
        let call = call.clone();
        match tokio::task::spawn_blocking(move || this.exec_memoized(&call)).await {
            Ok(out) => out,
            Err(e) => self.output_for(Err(format!("tool task failed: {e}"))),
        }
    }

    fn exec_memoized(&self, call: &ToolCall) -> ToolOutput {
        let name = call.name.as_str();
        if !Self::is_read_only(name) {
            return self.exec(call);
        }
        let key = format!("{name}\u{0}{}", call.args);
        let generation = self.generation.load(Ordering::Relaxed);
        let stamp = (name == "read_file")
            .then(|| arg_str(call, "path").and_then(|p| self.file_stamp(p)))
            .flatten();

        if let Some(m) = self.memo.lock().expect("read memo poisoned").get(&key)
            && m.generation == generation
            && m.stamp == stamp
        {
            let content = format!(
                "[duplicate call — this exact {name} already ran this turn and nothing has changed since; use that result]"
            );
            let preview = content.clone();
            return ToolOutput {
                full_tokens: m.tokens,
                sent_tokens: self.counter.count_sync(&content) as u32,
                raw_preview: None,
                content,
                preview,
                ok: true,
            };
        }

        let out = self.exec(call);
        if out.ok {
            self.memo.lock().expect("read memo poisoned").insert(
                key,
                ReadMemo {
                    generation,
                    stamp,
                    tokens: out.sent_tokens,
                },
            );
        }
        out
    }

    fn file_stamp(&self, rel: &str) -> Option<(SystemTime, u64)> {
        let meta = std::fs::metadata(self.resolve(rel).ok()?).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }

    pub fn gate_reason(&self, call: &ToolCall) -> Option<(String, Option<String>)> {
        match call.name.as_str() {
            "bash" => {
                let raw_cmd = arg_str(call, "command").unwrap_or("");
                let cls = crate::policy::classify_bash(raw_cmd);
                let preview: String = raw_cmd.chars().take(200).collect();
                match cls.class {
                    crate::policy::CommandClass::Safe => {
                        if self.auto_approve_safe_bash.load(Ordering::Relaxed) {
                            None
                        } else {
                            Some(("runs safe shell command".to_string(), Some(preview)))
                        }
                    }
                    crate::policy::CommandClass::Write => {
                        Some(("runs shell command (Write)".to_string(), Some(preview)))
                    }
                    crate::policy::CommandClass::Network => Some((
                        "runs network shell command (Network)".to_string(),
                        Some(preview),
                    )),
                    crate::policy::CommandClass::Destructive => Some((
                        "runs DESTRUCTIVE shell command (never remembered)".to_string(),
                        Some(preview),
                    )),
                }
            }
            "write_file" | "edit_file" => Some(("modifies files".to_string(), None)),
            "apply_patch" => Some(("applies unified patch to files".to_string(), None)),
            _ => None,
        }
    }

    pub fn diff_preview(&self, call: &ToolCall) -> Option<String> {
        match call.name.as_str() {
            "write_file" => {
                let path = arg_str(call, "path")?;
                let content = arg_str(call, "content")?;
                let existing = self
                    .resolve(path)
                    .ok()
                    .and_then(|p| std::fs::read_to_string(p).ok());
                Some(format_write_diff_preview(
                    path,
                    existing.as_deref(),
                    content,
                ))
            }
            "edit_file" => {
                let path = arg_str(call, "path")?;
                let old = arg_str(call, "old_string")?;
                let new = arg_str(call, "new_string")?;
                let total_lines = old.lines().count() + new.lines().count();
                if total_lines > 4 || old.len() + new.len() > 200 {
                    Some(format_edit_diff_preview(path, old, new))
                } else {
                    None
                }
            }
            "apply_patch" => {
                let patch = arg_str(call, "patch")?;
                Some(clip_diff_preview(patch))
            }
            _ => None,
        }
    }

    pub fn tool_error(&self, msg: String) -> ToolOutput {
        self.output_for(Err(msg))
    }

    fn output_for(&self, result: Result<String, String>) -> ToolOutput {
        match result {
            Ok(full) => {
                let sent = shape(&full, SENT_CAP);
                let raw_preview = (full != sent).then(|| full.chars().take(2000).collect());
                let preview: String = sent.chars().take(400).collect();
                ToolOutput {
                    full_tokens: self.count_capped(&full),
                    sent_tokens: self.counter.count_sync(&sent) as u32,
                    raw_preview,
                    content: sent,
                    preview,
                    ok: true,
                }
            }
            Err(e) => {
                let content = format!("error: {e}");
                let preview: String = content.chars().take(400).collect();
                let tokens = self.counter.count_sync(&content) as u32;
                ToolOutput {
                    full_tokens: tokens,
                    sent_tokens: tokens,
                    raw_preview: None,
                    content,
                    preview,
                    ok: false,
                }
            }
        }
    }

    fn exec(&self, call: &ToolCall) -> ToolOutput {
        let result = match call.name.as_str() {
            "read_file" => self.read_file(call),
            "list_dir" => self.list_dir(call),
            "glob" => self.glob(call),
            "grep" => self.grep(call),
            "symbol" => self.symbol(call),
            "outline" => self.outline(call),
            "git_diff" => self.git_diff(call),
            "git_log" => self.git_log(call),
            "tool_search" => return self.execute_tool_search_local(call),
            "write_file" => self.mutating(|| self.write_file(call)),
            "edit_file" => self.mutating(|| self.edit_file(call)),
            "apply_patch" => self.mutating(|| self.apply_patch(call)),
            "bash" | "web_fetch" => Err(format!("{} runs on the async path", call.name.as_str())),
            other => Err(format!("unknown tool {other:?}")),
        };
        self.output_for(result)
    }

    fn mutating(&self, f: impl FnOnce() -> Result<String, String>) -> Result<String, String> {
        let r = f();
        if r.is_ok() {
            self.note_mutation();
        }
        r
    }

    fn count_capped(&self, text: &str) -> u32 {
        let capped = &text[..floor_char_boundary(text, FULL_COUNT_CAP)];
        self.counter.count_sync(capped) as u32
    }

    fn resolve(&self, rel: &str) -> Result<PathBuf, String> {
        let rel = rel.trim();
        let rel = if rel.is_empty() { "." } else { rel };
        let joined = if Path::new(rel).is_absolute() {
            PathBuf::from(rel)
        } else {
            self.root.join(rel)
        };
        let norm = normalize_lexical(&joined);
        if !norm.starts_with(&self.root) {
            return Err(format!("path {rel:?} escapes the workspace root"));
        }
        if let Ok(canon) = norm.canonicalize()
            && !canon.starts_with(&self.root)
        {
            return Err(format!("path {rel:?} resolves outside the workspace"));
        }
        Ok(norm)
    }

    fn read_file(&self, call: &ToolCall) -> Result<String, String> {
        let path = arg_str(call, "path").ok_or("read_file: missing `path`")?;
        let fs_path = self.resolve(path)?;
        if !fs_path.is_file() {
            return Err(format!("{path:?} is not a file"));
        }
        let meta = std::fs::metadata(&fs_path).map_err(|e| e.to_string())?;
        if meta.len() > 64 * 1024 * 1024 {
            return Err(format!(
                "{path:?} is too large ({} MB)",
                meta.len() / 1_048_576
            ));
        }
        {
            use std::io::Read;
            let mut f = std::fs::File::open(&fs_path).map_err(|e| e.to_string())?;
            let mut buf = [0u8; 8192];
            let n = f.read(&mut buf).map_err(|e| e.to_string())?;
            if buf[..n].contains(&0) {
                return Err(format!("{path:?} looks binary ({n} bytes sampled)"));
            }
        }

        let offset = arg_u64(call, "offset").unwrap_or(1).max(1) as usize;
        let limit = arg_u64(call, "limit").unwrap_or(400).clamp(1, 2000) as usize;

        let file = std::fs::File::open(&fs_path).map_err(|e| e.to_string())?;
        let reader = std::io::BufReader::new(file);
        let mut lines = Vec::with_capacity(limit.min(1024));
        let mut total = 0usize;
        let mut bytes = 0usize;
        for (i, line) in reader.lines().enumerate() {
            let line = line.map_err(|e| e.to_string())?;
            total += 1;
            if i + 1 < offset {
                continue;
            }
            if lines.len() >= limit {
                break;
            }
            bytes += line.len() + 8;
            if bytes > FULL_COUNT_CAP {
                break;
            }
            lines.push(format!("{}\t{}", i + 1, line));
        }
        let mut out = if lines.is_empty() {
            format!("{path}: no lines at offset {offset}")
        } else {
            lines.join("\n")
        };
        if total > offset - 1 + lines.len() {
            let _ = write!(
                out,
                "\n\n[… file continues — page with offset/limit; {} lines shown from {offset}]",
                lines.len()
            );
        } else {
            let _ = write!(out, "\n\n[… end of file — {total} lines]");
        }
        Ok(out)
    }

    fn list_dir(&self, call: &ToolCall) -> Result<String, String> {
        let path = arg_str(call, "path").unwrap_or(".");
        let depth = arg_u64(call, "depth").unwrap_or(1).clamp(1, 3) as usize;
        let base = self.resolve(path)?;
        if !base.is_dir() {
            return Err(format!("{path:?} is not a directory"));
        }
        let mut out = String::new();
        let mut count = 0usize;
        walk(&base, depth, &base, &mut |rel, is_dir, _size| {
            if count >= MAX_LIST_ENTRIES {
                return;
            }
            count += 1;
            let indent = "  ".repeat(rel.components().count().saturating_sub(1));
            let name = rel.file_name().map_or_default(|n| n.to_string_lossy());
            let slash = if is_dir { "/" } else { "" };
            let _ = writeln!(out, "{indent}{name}{slash}");
        });
        if count >= MAX_LIST_ENTRIES {
            let _ = write!(out, "\n[… listing truncated at {MAX_LIST_ENTRIES} entries]");
        }
        Ok(out)
    }

    fn glob(&self, call: &ToolCall) -> Result<String, String> {
        let pattern = arg_str(call, "pattern").ok_or("glob: missing `pattern`")?;
        let base = self.resolve(arg_str(call, "path").unwrap_or("."))?;
        if !base.is_dir() {
            return Err("glob: search path is not a directory".to_string());
        }
        let mut matches = Vec::new();
        walk(&base, usize::MAX, &self.root, &mut |rel, is_dir, _| {
            if is_dir || matches.len() >= MAX_GLOB_MATCHES {
                return;
            }
            let s = rel.to_string_lossy().replace('\\', "/");
            if glob_match(pattern, &s) {
                matches.push(s);
            }
        });
        matches.sort();
        let mut out = matches.join("\n");
        if matches.len() >= MAX_GLOB_MATCHES {
            let _ = write!(out, "\n[… truncated at {MAX_GLOB_MATCHES} matches]");
        }
        if out.is_empty() {
            out = "(no matches)".to_string();
        }
        Ok(out)
    }

    fn grep(&self, call: &ToolCall) -> Result<String, String> {
        let pattern = arg_str(call, "pattern").ok_or("grep: missing `pattern`")?;
        if pattern.is_empty() {
            return Err("grep: `pattern` must not be empty".to_string());
        }
        let base = self.resolve(arg_str(call, "path").unwrap_or("."))?;
        let glob_filter = arg_str(call, "glob");
        let ignore_case = arg_bool(call, "ignore_case");
        let context = arg_u64(call, "context").unwrap_or(0).min(MAX_GREP_CONTEXT) as usize;
        let matcher = Matcher::new(pattern, ignore_case);

        let mut out = String::new();
        let mut count = 0usize;
        let mut scanned = 0usize;
        let mut scan = |rel: &Path, size: u64| {
            if count >= MAX_GREP_MATCHES || size > MAX_GREP_FILE_BYTES {
                return;
            }
            let rel_s = rel.to_string_lossy().replace('\\', "/");
            if let Some(g) = glob_filter {
                let name = rel.file_name().map_or_default(|n| n.to_string_lossy());
                if !glob_match(g, &rel_s) && !glob_match(g, &name) {
                    return;
                }
            }
            scanned += 1;
            let Ok(content) = std::fs::read_to_string(self.root.join(rel)) else {
                return;
            };
            if content.contains('\0') {
                return;
            }
            count += grep_file(
                &matcher,
                &rel_s,
                &content,
                context,
                MAX_GREP_MATCHES - count,
                &mut out,
            );
        };
        if base.is_file() {
            let rel = base.strip_prefix(&self.root).unwrap_or(&base).to_path_buf();
            let size = base.metadata().map_or(0, |m| m.len());
            scan(&rel, size);
        } else {
            walk(&base, usize::MAX, &self.root, &mut |rel, is_dir, size| {
                if !is_dir {
                    scan(&rel, size);
                }
            });
        }
        if out.is_empty() {
            out = format!("(no matches for {pattern:?} in {scanned} files)");
        } else if count >= MAX_GREP_MATCHES {
            let _ = write!(
                out,
                "[… stopped at {MAX_GREP_MATCHES} matches — narrow the pattern, path, or glob]"
            );
        } else {
            out.truncate(out.trim_end().len());
        }
        Ok(out)
    }

    fn symbol(&self, call: &ToolCall) -> Result<String, String> {
        let query = arg_str(call, "query")
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or("symbol: missing `query`")?;
        let kind_filter = match arg_str(call, "kind").map(str::trim) {
            Some("def") | None => SymKind::Def,
            Some("ref") => SymKind::Ref,
            Some("call") => SymKind::Call,
            Some(other) => {
                return Err(format!(
                    "symbol: unknown kind {other:?} (expected def|ref|call)"
                ));
            }
        };
        let scope = match arg_str(call, "path").map(str::trim) {
            Some(p) if !p.is_empty() && p != "." => {
                let abs = self.resolve(p)?;
                let rel = abs.strip_prefix(&self.root).unwrap_or(&abs);
                Some(rel.to_string_lossy().replace('\\', "/"))
            }
            _ => None,
        };
        let hits = self.symbol_index();
        let q_lower = query.to_lowercase();

        let mut ranked: Vec<(u8, &SymbolHit)> = hits
            .iter()
            .filter(|h| h.kind == kind_filter)
            .filter(|h| {
                scope.as_deref().is_none_or(|s| {
                    h.path == s || h.path.strip_prefix(s).is_some_and(|r| r.starts_with('/'))
                })
            })
            .filter_map(|h| {
                let n = h.name.to_lowercase();
                let rank = if h.name == query {
                    0
                } else if n == q_lower {
                    1
                } else if n.starts_with(&q_lower) {
                    2
                } else if n.contains(&q_lower) {
                    3
                } else {
                    return None;
                };
                Some((rank, h))
            })
            .collect();
        if ranked.is_empty() {
            return Ok(format!(
                "(no definition matching {query:?} — try grep for usages or non-declaration sites)"
            ));
        }
        ranked.sort_by_key(|(rank, _)| *rank);
        let total = ranked.len();
        ranked.truncate(MAX_SYMBOL_HITS);

        let mut order: Vec<&str> = Vec::new();
        let mut by_file: HashMap<&str, Vec<u32>> = HashMap::new();
        for (_, h) in &ranked {
            by_file
                .entry(h.path.as_str())
                .or_insert_with(|| {
                    order.push(h.path.as_str());
                    Vec::new()
                })
                .push(h.line);
        }
        let mut out = String::new();
        for path in order {
            let text = std::fs::read_to_string(self.root.join(path)).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            let mut nums = by_file.remove(path).unwrap_or_default();
            nums.sort_unstable();
            let _ = writeln!(out, "{path}");
            for n in nums {
                let sig = lines.get(n as usize - 1).map_or("", |l| l.trim());
                let _ = writeln!(out, "  {n}: {}", clip_line(sig, 160));
            }
        }
        if total > MAX_SYMBOL_HITS {
            let _ = write!(
                out,
                "[… {} more definitions — use a more specific query or path]",
                total - MAX_SYMBOL_HITS
            );
        } else {
            out.truncate(out.trim_end().len());
        }
        Ok(out)
    }

    fn symbol_index(&self) -> Arc<Vec<SymbolHit>> {
        let generation = self.generation.load(Ordering::Relaxed);
        let mut cache = self.symbols.lock().expect("symbol cache poisoned");
        if let Some(c) = cache.as_ref()
            && c.generation == generation
            && c.built.elapsed() < SYMBOL_CACHE_TTL
        {
            return Arc::clone(&c.hits);
        }
        let hits = Arc::new(collect_workspace_symbols_all(&self.root));
        *cache = Some(SymbolCache {
            generation,
            built: Instant::now(),
            hits: Arc::clone(&hits),
        });
        hits
    }

    fn write_file(&self, call: &ToolCall) -> Result<String, String> {
        let path = arg_str(call, "path").ok_or("write_file: missing `path`")?;
        let content = arg_str(call, "content").ok_or("write_file: missing `content`")?;
        if content.len() > 1024 * 1024 {
            return Err("write_file: content exceeds 1MB".to_string());
        }
        let fs_path = self.resolve(path)?;
        if fs_path.is_dir() {
            return Err(format!("{path:?} is a directory"));
        }
        if let Some(parent) = fs_path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let existed = fs_path.exists();
        self.capture_original(&fs_path);
        std::fs::write(&fs_path, content).map_err(|e| e.to_string())?;
        Ok(format!(
            "{} {path}: {} bytes",
            if existed { "overwrote" } else { "wrote" },
            content.len()
        ))
    }

    fn edit_file(&self, call: &ToolCall) -> Result<String, String> {
        let path = arg_str(call, "path").ok_or("edit_file: missing `path`")?;
        let old = arg_str(call, "old_string").ok_or("edit_file: missing `old_string`")?;
        let new = arg_str(call, "new_string").ok_or("edit_file: missing `new_string`")?;
        if old.is_empty() {
            return Err("edit_file: `old_string` must not be empty".to_string());
        }
        let replace_all = call
            .args
            .get("replace_all")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let fs_path = self.resolve(path)?;
        if !fs_path.is_file() {
            return Err(format!("{path:?} is not a file"));
        }
        let meta = std::fs::metadata(&fs_path).map_err(|e| e.to_string())?;
        if meta.len() > 16 * 1024 * 1024 {
            return Err(format!("{path:?} too large to edit ({})", meta.len()));
        }
        let content = std::fs::read_to_string(&fs_path).map_err(|e| e.to_string())?;
        let exact_offsets: Vec<usize> = content.match_indices(old).map(|(i, _)| i).collect();
        let (updated, count, fuzzy) = if exact_offsets.is_empty() {
            let fuzzy_spans = find_trimmed_spans(&content, old);
            if fuzzy_spans.is_empty() {
                let hint = closest_region_hint(&content, old);
                return Err(format!("edit_file: `old_string` not found in {path}{hint}"));
            }
            if fuzzy_spans.len() > 1 && !replace_all {
                let lines: Vec<String> = fuzzy_spans
                    .iter()
                    .take(10)
                    .map(|s| s.start_line.to_string())
                    .collect();
                return Err(format!(
                    "edit_file: `old_string` (whitespace-normalized) matches {} times in {path} (at lines {}) — add context or set replace_all",
                    fuzzy_spans.len(),
                    lines.join(", ")
                ));
            }
            let apply_count = if replace_all { fuzzy_spans.len() } else { 1 };
            let updated = apply_trimmed_spans(&content, old, new, &fuzzy_spans[..apply_count]);
            (updated, apply_count, true)
        } else {
            if exact_offsets.len() > 1 && !replace_all {
                let lines = format_match_lines(&content, &exact_offsets);
                return Err(format!(
                    "edit_file: `old_string` matches {} times in {path} (at lines {lines}) — add context or set replace_all",
                    exact_offsets.len()
                ));
            }
            let updated = if replace_all {
                content.replace(old, new)
            } else {
                content.replacen(old, new, 1)
            };
            (
                updated,
                if replace_all { exact_offsets.len() } else { 1 },
                false,
            )
        };
        self.capture_original(&fs_path);
        std::fs::write(&fs_path, &updated).map_err(|e| e.to_string())?;
        let delta = (new.lines().count() as i128 - old.lines().count() as i128) * count as i128;
        let note = if fuzzy {
            " (whitespace-normalized)"
        } else {
            ""
        };
        Ok(format!(
            "edited {path}: {count} replacement(s){note}, {delta:+} lines"
        ))
    }

    fn outline(&self, call: &ToolCall) -> Result<String, String> {
        let path = arg_str(call, "path").ok_or("outline: missing `path`")?;
        let fs_path = self.resolve(path)?;
        if !fs_path.is_file() {
            return Err(format!("{path:?} is not a file"));
        }
        let text = std::fs::read_to_string(&fs_path).map_err(|e| e.to_string())?;
        let is_markdown = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown"));

        let mut out = String::new();
        if is_markdown {
            for (idx, line) in text.lines().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.starts_with('#') {
                    let _ = writeln!(out, "{}: {}", idx + 1, clip_line(trimmed, 160));
                }
            }
        } else {
            let lines: Vec<&str> = text.lines().collect();
            let rel = fs_path
                .strip_prefix(&self.root)
                .unwrap_or(&fs_path)
                .to_string_lossy()
                .replace('\\', "/");
            let hits = collect_file_symbols(&fs_path, &rel);
            for h in hits {
                let sig = lines.get(h.line as usize - 1).map_or("", |l| l.trim());
                let _ = writeln!(out, "{}: {}", h.line, clip_line(sig, 160));
            }
        }
        if out.is_empty() {
            Ok(format!("{path}: (no symbols or headings found)"))
        } else {
            out.truncate(out.trim_end().len());
            Ok(out)
        }
    }

    fn git_diff(&self, call: &ToolCall) -> Result<String, String> {
        let staged = arg_bool(call, "staged");
        let scope = match arg_str(call, "path").map(str::trim) {
            Some(p) if !p.is_empty() && p != "." => {
                let abs = self.resolve(p)?;
                let rel = abs.strip_prefix(&self.root).unwrap_or(&abs);
                Some(rel.to_string_lossy().replace('\\', "/"))
            }
            _ => None,
        };
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(&self.root).arg("diff");
        if staged {
            cmd.arg("--cached");
        }
        if let Some(rel) = scope.as_deref() {
            cmd.arg("--").arg(rel);
        }
        hide_std_cmd(&mut cmd);
        let out = cmd.output().map_err(|e| format!("git unavailable: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("git diff failed: {}", err.trim()));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            Ok(if staged {
                "(no staged changes)".to_string()
            } else {
                "(no working tree changes)".to_string()
            })
        } else {
            Ok(trimmed.to_string())
        }
    }

    fn git_log(&self, call: &ToolCall) -> Result<String, String> {
        let limit = arg_u64(call, "limit").unwrap_or(15).clamp(1, 50);
        let scope = match arg_str(call, "path").map(str::trim) {
            Some(p) if !p.is_empty() && p != "." => {
                let abs = self.resolve(p)?;
                let rel = abs.strip_prefix(&self.root).unwrap_or(&abs);
                Some(rel.to_string_lossy().replace('\\', "/"))
            }
            _ => None,
        };
        let mut cmd = std::process::Command::new("git");
        cmd.arg("-C").arg(&self.root).args([
            "log",
            "-n",
            &limit.to_string(),
            "--oneline",
            "--decorate",
        ]);
        if let Some(rel) = scope.as_deref() {
            cmd.arg("--").arg(rel);
        }
        hide_std_cmd(&mut cmd);
        let out = cmd.output().map_err(|e| format!("git unavailable: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("git log failed: {}", err.trim()));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            Ok("(no commits)".to_string())
        } else {
            Ok(trimmed.to_string())
        }
    }

    fn apply_patch(&self, call: &ToolCall) -> Result<String, String> {
        let patch = arg_str(call, "patch").ok_or("apply_patch: missing `patch`")?;
        if patch.trim().is_empty() {
            return Err("apply_patch: `patch` must not be empty".to_string());
        }
        let ops = parse_unified_patch(patch)?;
        if ops.is_empty() {
            return Err("apply_patch: no file hunks found in patch".to_string());
        }
        // Validate and stage all mutations before writing any file.
        let mut planned = Vec::with_capacity(ops.len());
        for op in ops {
            let rel = op.target_rel();
            let fs_path = self.resolve(rel)?;
            let existing = if op.is_create {
                if fs_path.exists() {
                    return Err(format!("apply_patch: {rel} already exists"));
                }
                String::new()
            } else {
                if !fs_path.is_file() {
                    return Err(format!("apply_patch: {rel:?} is not a file"));
                }
                std::fs::read_to_string(&fs_path).map_err(|e| e.to_string())?
            };
            let (updated, added, removed) = apply_file_hunks(rel, &existing, &op.hunks)?;
            planned.push(PlannedPatchWrite {
                fs_path,
                new_content: if op.is_delete { None } else { Some(updated) },
                added,
                removed,
            });
        }

        let mut total_added = 0usize;
        let mut total_removed = 0usize;
        let file_count = planned.len();
        for p in planned {
            self.capture_original(&p.fs_path);
            match p.new_content {
                Some(text) => {
                    if let Some(parent) = p.fs_path.parent()
                        && !parent.exists()
                    {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    std::fs::write(&p.fs_path, text).map_err(|e| e.to_string())?;
                }
                None => {
                    std::fs::remove_file(&p.fs_path).map_err(|e| e.to_string())?;
                }
            }
            total_added += p.added;
            total_removed += p.removed;
        }
        Ok(format!(
            "applied patch: {file_count} file(s), +{total_added} -{total_removed}"
        ))
    }

    async fn web_fetch(&self, call: &ToolCall, cancel: &CancellationToken) -> ToolOutput {
        let url = match arg_str(call, "url").map(str::trim) {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => return self.output_for(Err("web_fetch: missing `url`".to_string())),
        };
        let res = tokio::select! {
            () = cancel.cancelled() => Err("cancelled".to_string()),
            r = crate::web_fetch::fetch_url_text(&url) => r,
        };
        self.output_for(res)
    }

    async fn bash(&self, call: &ToolCall, cancel: &CancellationToken) -> ToolOutput {
        let command = match arg_str(call, "command") {
            Some(c) if !c.trim().is_empty() => c.to_string(),
            _ => return self.output_for(Err("bash: missing `command`".to_string())),
        };
        let timeout_ms = arg_u64(call, "timeout_ms")
            .unwrap_or(120_000)
            .clamp(1_000, 600_000);

        let mut cmd = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            c.args(["/C", &command]);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.args(["-c", &command]);
            c
        };
        cmd.current_dir(&self.root);

        match run_tokio_cmd_cancellable(&mut cmd, Duration::from_millis(timeout_ms), cancel).await {
            Ok(CmdRun::Finished {
                status,
                stdout,
                stderr,
            }) => {
                let code = status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&stdout);
                let stderr = String::from_utf8_lossy(&stderr);
                let sent = format_command_output(code, &stdout, &stderr);
                let full_tokens = self
                    .count_capped(&stdout)
                    .saturating_add(self.count_capped(&stderr));
                let raw_preview = (stdout.len() + stderr.len() > sent.len()).then(|| {
                    let combined = if stderr.is_empty() {
                        stdout.to_string()
                    } else if stdout.is_empty() {
                        stderr.to_string()
                    } else {
                        format!("{stdout}\n[stderr]\n{stderr}")
                    };
                    combined.chars().take(2000).collect()
                });
                self.output_shaped(sent, raw_preview, full_tokens, status.success())
            }
            Ok(CmdRun::TimedOut) => {
                self.output_for(Err(format!("bash timed out after {timeout_ms}ms (killed)")))
            }
            Ok(CmdRun::Cancelled) => self.output_for(Err("bash cancelled (killed)".to_string())),
            Err(e) => self.output_for(Err(format!("bash failed to start: {e}"))),
        }
    }

    fn output_shaped(
        &self,
        sent: String,
        raw_preview: Option<String>,
        full_tokens: u32,
        ok: bool,
    ) -> ToolOutput {
        let content = if ok { sent } else { format!("error: {sent}") };
        let sent_tokens = self.counter.count_sync(&content) as u32;
        ToolOutput {
            preview: content.chars().take(400).collect(),
            raw_preview,
            full_tokens: full_tokens.max(sent_tokens),
            sent_tokens,
            content,
            ok,
        }
    }
}

fn spec(name: &'static str, description: &'static str, schema: serde_json::Value) -> ToolSpec {
    ToolSpec {
        name: ToolName::new(name),
        description: description.chars().take(120).collect(),
        input_schema: schema,
    }
}

fn arg_str<'a>(call: &'a ToolCall, key: &str) -> Option<&'a str> {
    call.args.get(key)?.as_str()
}

fn arg_u64(call: &ToolCall, key: &str) -> Option<u64> {
    call.args.get(key)?.as_u64()
}

fn arg_bool(call: &ToolCall, key: &str) -> bool {
    call.args
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

struct Matcher {
    regex: Option<regex::Regex>,
    literal: String,
    ignore_case: bool,
}

impl Matcher {
    fn new(pattern: &str, ignore_case: bool) -> Self {
        let regex = regex::RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .size_limit(1 << 20)
            .build()
            .ok();
        let literal = if ignore_case {
            pattern.to_lowercase()
        } else {
            pattern.to_string()
        };
        Self {
            regex,
            literal,
            ignore_case,
        }
    }

    fn is_match(&self, line: &str) -> bool {
        let literal_hit = if self.ignore_case {
            line.to_lowercase().contains(&self.literal)
        } else {
            line.contains(&self.literal)
        };
        literal_hit || self.regex.as_ref().is_some_and(|r| r.is_match(line))
    }
}

fn grep_file(
    matcher: &Matcher,
    rel: &str,
    content: &str,
    context: usize,
    budget: usize,
    out: &mut String,
) -> usize {
    let lines: Vec<&str> = content.lines().collect();
    let hits: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| matcher.is_match(l))
        .map(|(i, _)| i)
        .collect();
    if hits.is_empty() {
        return 0;
    }
    let shown = hits.len().min(MAX_GREP_PER_FILE).min(budget);
    let _ = writeln!(out, "{rel}");
    let mut last_printed: Option<usize> = None;
    for &i in &hits[..shown] {
        let from = i.saturating_sub(context);
        let to = (i + context).min(lines.len() - 1);
        if context > 0
            && let Some(last) = last_printed
            && from > last + 1
        {
            out.push_str("  --\n");
        }
        for (j, line) in lines.iter().enumerate().take(to + 1).skip(from) {
            if last_printed.is_some_and(|last| j <= last) {
                continue;
            }
            let sep = if j == i || hits.binary_search(&j).is_ok() {
                ':'
            } else {
                '-'
            };
            let _ = writeln!(out, "  {}{sep} {}", j + 1, clip_line(line.trim(), 200));
            last_printed = Some(j);
        }
    }
    if hits.len() > shown {
        let _ = writeln!(out, "  (+{} more matches in this file)", hits.len() - shown);
    }
    shown
}

fn clip_line(line: &str, max: usize) -> std::borrow::Cow<'_, str> {
    if line.len() <= max {
        return line.into();
    }
    match line.char_indices().nth(max) {
        Some((idx, _)) => format!("{}…", &line[..idx]).into(),
        None => line.into(),
    }
}

fn format_command_output(code: i32, stdout: &str, stderr: &str) -> String {
    let stdout = crate::shape::clean(stdout);
    let stderr = crate::shape::clean(stderr);
    let mut out = format!("exit code: {code}");
    let share = if stdout.is_empty() || stderr.is_empty() {
        1
    } else {
        2
    };
    if !stdout.is_empty() {
        out.push('\n');
        out.push_str(&crate::shape::clip(
            &stdout,
            BASH_MAX_LINES / share,
            BASH_MAX_BYTES / share,
        ));
    }
    if !stderr.is_empty() {
        out.push_str("\n--- stderr ---\n");
        out.push_str(&crate::shape::clip(
            &stderr,
            BASH_MAX_LINES / share,
            BASH_MAX_BYTES / share,
        ));
    }
    if stdout.is_empty() && stderr.is_empty() {
        out.push_str("\n(no output)");
    }
    out
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn walk(dir: &Path, depth_left: usize, rel_root: &Path, visit: &mut dyn FnMut(PathBuf, bool, u64)) {
    if depth_left == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<_> = entries.flatten().collect();
    items.sort_by_key(std::fs::DirEntry::file_name);
    for e in items {
        let p = e.path();
        let name = e.file_name();
        let name_s = name.to_string_lossy();
        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
        if is_dir && (name_s.starts_with('.') || SKIP_DIRS.contains(&name_s.as_ref())) {
            continue;
        }
        let rel = p.strip_prefix(rel_root).unwrap_or(&p).to_path_buf();
        let size = e.metadata().map_or(0, |m| m.len());
        visit(rel.clone(), is_dir, size);
        if is_dir {
            walk(&p, depth_left - 1, rel_root, visit);
        }
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = text.split('/').collect();
    match_segments(&pat, &path)
}

fn match_segments(pat: &[&str], path: &[&str]) -> bool {
    match pat.first() {
        None => path.is_empty(),
        Some(&"**") => (0..=path.len()).any(|i| match_segments(&pat[1..], &path[i..])),
        Some(p) => {
            !path.is_empty() && segment_match(p, path[0]) && match_segments(&pat[1..], &path[1..])
        }
    }
}

fn segment_match(pat: &str, text: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star_p, mut star_t) = (usize::MAX, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_p = pi;
            star_t = ti;
            pi += 1;
        } else if star_p != usize::MAX {
            pi = star_p + 1;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

pub(crate) fn shape(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    let head = cap * 7 / 10;
    let tail = cap - head - 64;
    let mut out = String::with_capacity(cap + 128);
    let head_end = floor_char_boundary(text, head);
    let tail_start = floor_char_boundary_rev(text, tail);
    out.push_str(&text[..head_end]);
    let _ = write!(out, "\n\n[… {} bytes omitted …]\n\n", tail_start - head_end);
    out.push_str(&text[tail_start..]);
    out
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn floor_char_boundary_rev(s: &str, tail: usize) -> usize {
    let mut i = s.len().saturating_sub(tail);
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn load_instructions(root: &Path) -> Option<String> {
    for rel in PROJECT_INSTRUCTIONS {
        let p = root.join(rel);
        let Ok(meta) = p.metadata() else { continue };
        if !meta.is_file() || meta.len() > INSTRUCTIONS_CAP {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&p) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

fn format_match_lines(content: &str, offsets: &[usize]) -> String {
    let mut nums: Vec<String> = offsets
        .iter()
        .take(10)
        .map(|&off| (content[..off].bytes().filter(|&b| b == b'\n').count() + 1).to_string())
        .collect();
    if offsets.len() > 10 {
        nums.push(format!("… +{} more", offsets.len() - 10));
    }
    nums.join(", ")
}

struct LineSpan<'a> {
    start_byte: usize,
    end_byte_incl_nl: usize,
    end_byte_excl_nl: usize,
    text: &'a str,
}

fn split_line_spans(s: &str) -> Vec<LineSpan<'_>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    for chunk in s.split_inclusive('\n') {
        let end_incl = pos + chunk.len();
        let no_nl = chunk.strip_suffix('\n').unwrap_or(chunk);
        let text = no_nl.strip_suffix('\r').unwrap_or(no_nl);
        let end_excl = pos + text.len();
        out.push(LineSpan {
            start_byte: pos,
            end_byte_incl_nl: end_incl,
            end_byte_excl_nl: end_excl,
            text,
        });
        pos = end_incl;
    }
    out
}

struct TrimmedSpan {
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    leading_ws: String,
}

fn find_trimmed_spans(content: &str, old: &str) -> Vec<TrimmedSpan> {
    let old_lines: Vec<&str> = old
        .lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    if old_lines.is_empty() || old_lines.iter().all(|l| l.trim().is_empty()) {
        return Vec::new();
    }
    let spans = split_line_spans(content);
    if spans.len() < old_lines.len() {
        return Vec::new();
    }
    let include_trailing_nl = old.ends_with('\n');
    let mut matches = Vec::new();
    let mut i = 0usize;
    while i + old_lines.len() <= spans.len() {
        let ok = old_lines
            .iter()
            .enumerate()
            .all(|(k, ol)| spans[i + k].text.trim() == ol.trim());
        if ok {
            let first_non_empty = spans[i..i + old_lines.len()]
                .iter()
                .find(|s| !s.text.trim().is_empty())
                .map_or("", |s| s.text);
            let leading: String = first_non_empty
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect();
            let last = &spans[i + old_lines.len() - 1];
            matches.push(TrimmedSpan {
                start_byte: spans[i].start_byte,
                end_byte: if include_trailing_nl {
                    last.end_byte_incl_nl
                } else {
                    last.end_byte_excl_nl
                },
                start_line: i + 1,
                leading_ws: leading,
            });
            i += old_lines.len();
        } else {
            i += 1;
        }
    }
    matches
}

fn apply_trimmed_spans(content: &str, old: &str, new: &str, spans: &[TrimmedSpan]) -> String {
    let use_crlf = content.contains("\r\n");
    let old_first_ws: String = old
        .lines()
        .find(|l| !l.trim().is_empty())
        .map_or("", |l| l)
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();

    let mut out = String::with_capacity(content.len() + new.len() * spans.len());
    let mut cursor = 0usize;
    for sp in spans {
        out.push_str(&content[cursor..sp.start_byte]);
        let mut adjusted = reindent_block(new, &old_first_ws, &sp.leading_ws);
        if use_crlf && !adjusted.contains("\r\n") {
            adjusted = adjusted.replace('\n', "\r\n");
        }
        if old.ends_with('\n') && !adjusted.is_empty() && !adjusted.ends_with('\n') {
            adjusted.push_str(if use_crlf { "\r\n" } else { "\n" });
        }
        out.push_str(&adjusted);
        cursor = sp.end_byte;
    }
    out.push_str(&content[cursor..]);
    out
}

fn reindent_block(block: &str, old_ws: &str, target_ws: &str) -> String {
    if old_ws == target_ws || block.is_empty() {
        return block.to_string();
    }
    let mut out = String::with_capacity(block.len() + 32);
    for chunk in block.split_inclusive('\n') {
        let (body, nl) = if let Some(b) = chunk.strip_suffix("\r\n") {
            (b, "\r\n")
        } else if let Some(b) = chunk.strip_suffix('\n') {
            (b, "\n")
        } else {
            (chunk, "")
        };
        if body.trim().is_empty() {
            out.push_str(nl);
        } else if let Some(rest) = body.strip_prefix(old_ws) {
            out.push_str(target_ws);
            out.push_str(rest);
            out.push_str(nl);
        } else {
            out.push_str(body);
            out.push_str(nl);
        }
    }
    out
}

fn closest_region_hint(content: &str, old: &str) -> String {
    let target_lines: Vec<String> = old
        .lines()
        .map(|l| l.trim().to_ascii_lowercase())
        .filter(|l| !l.is_empty())
        .collect();
    let content_lines: Vec<&str> = content.lines().collect();
    if target_lines.is_empty() || content_lines.is_empty() {
        return String::new();
    }
    let win = target_lines.len().clamp(1, content_lines.len());
    let mut best_score = 0.0f64;
    let mut best_start = 0usize;
    for start in 0..=content_lines.len().saturating_sub(win) {
        let slice = &content_lines[start..start + win];
        let mut score = 0.0f64;
        for tl in &target_lines {
            let best_line = slice
                .iter()
                .map(|cl| line_similarity(tl, &cl.trim().to_ascii_lowercase()))
                .fold(0.0f64, f64::max);
            score += best_line;
        }
        let avg = score / target_lines.len() as f64;
        if avg > best_score {
            best_score = avg;
            best_start = start;
        }
    }
    if best_score < 0.25 {
        return String::new();
    }
    let from = best_start.saturating_sub(1);
    let to = (best_start + win + 1)
        .min(content_lines.len())
        .min(from + 12);
    let mut out = format!("\nClosest region around lines {}-{to}:", from + 1);
    for (idx, line) in content_lines.iter().enumerate().take(to).skip(from) {
        let _ = write!(out, "\n  {}: {}", idx + 1, clip_line(line, 160));
    }
    out
}

fn line_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a.contains(b) || b.contains(a) {
        let min_len = a.len().min(b.len()) as f64;
        let max_len = a.len().max(b.len()) as f64;
        return 0.5 + 0.5 * (min_len / max_len);
    }
    let wa: std::collections::HashSet<&str> = a
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|w| !w.is_empty())
        .collect();
    let wb: std::collections::HashSet<&str> = b
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|w| !w.is_empty())
        .collect();
    if wa.is_empty() || wb.is_empty() {
        return 0.0;
    }
    let inter = wa.intersection(&wb).count() as f64;
    let union = wa.union(&wb).count() as f64;
    inter / union
}

fn format_write_diff_preview(path: &str, existing: Option<&str>, new_content: &str) -> String {
    let mut out = match existing {
        None => format!("--- /dev/null\n+++ b/{path}\n"),
        Some(_) => format!("--- a/{path}\n+++ b/{path}\n"),
    };
    if let Some(old) = existing {
        for line in old.lines().take(20) {
            let _ = writeln!(out, "-{line}");
        }
        if old.lines().count() > 20 {
            let _ = writeln!(out, "-… ({} total old lines)", old.lines().count());
        }
    }
    for line in new_content.lines().take(40) {
        let _ = writeln!(out, "+{line}");
    }
    if new_content.lines().count() > 40 {
        let _ = writeln!(out, "+… ({} total new lines)", new_content.lines().count());
    }
    clip_diff_preview(&out)
}

fn format_edit_diff_preview(path: &str, old: &str, new: &str) -> String {
    let mut out = format!("--- a/{path}\n+++ b/{path}\n");
    for line in old.lines().take(30) {
        let _ = writeln!(out, "-{line}");
    }
    for line in new.lines().take(30) {
        let _ = writeln!(out, "+{line}");
    }
    clip_diff_preview(&out)
}

fn clip_diff_preview(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let mut out = lines
        .iter()
        .take(DIFF_PREVIEW_MAX_LINES)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    if lines.len() > DIFF_PREVIEW_MAX_LINES {
        let _ = write!(
            out,
            "\n… ({} more diff lines)",
            lines.len() - DIFF_PREVIEW_MAX_LINES
        );
    }
    if out.len() > DIFF_PREVIEW_MAX_BYTES {
        let end = floor_char_boundary(&out, DIFF_PREVIEW_MAX_BYTES);
        out.truncate(end);
        out.push_str("\n… (truncated)");
    }
    out
}

struct PlannedPatchWrite {
    fs_path: PathBuf,
    new_content: Option<String>,
    added: usize,
    removed: usize,
}

struct PatchFileOp {
    old_path: String,
    new_path: String,
    is_create: bool,
    is_delete: bool,
    hunks: Vec<PatchHunk>,
}

impl PatchFileOp {
    fn target_rel(&self) -> &str {
        if self.is_delete {
            &self.old_path
        } else {
            &self.new_path
        }
    }
}

struct PatchHunk {
    old_start: usize,
    lines: Vec<PatchLine>,
}

enum PatchLine {
    Context(String),
    Remove(String),
    Add(String),
}

fn clean_patch_path(raw: &str) -> String {
    let trimmed = raw.split('\t').next().unwrap_or(raw).trim();
    trimmed
        .strip_prefix("a/")
        .or_else(|| trimmed.strip_prefix("b/"))
        .unwrap_or(trimmed)
        .to_string()
}

fn parse_unified_patch(patch: &str) -> Result<Vec<PatchFileOp>, String> {
    let lines: Vec<&str> = patch
        .lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut ops = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];
        if let Some(old_raw) = line.strip_prefix("--- ") {
            let old_raw = old_raw.trim();
            i += 1;
            let Some(new_line) = lines.get(i).and_then(|l| l.strip_prefix("+++ ")) else {
                return Err("apply_patch: expected `+++ ` after `--- `".to_string());
            };
            let new_raw = new_line.trim();
            i += 1;
            let is_create = old_raw == "/dev/null";
            let is_delete = new_raw == "/dev/null";
            let old_path = clean_patch_path(old_raw);
            let new_path = clean_patch_path(new_raw);
            let mut hunks = Vec::new();

            while i < lines.len() {
                let cur = lines[i];
                if cur.starts_with("--- ") || cur.starts_with("diff ") {
                    break;
                }
                if cur.starts_with("@@ ") {
                    let old_start = parse_hunk_old_start(cur)?;
                    i += 1;
                    let mut hlines = Vec::new();
                    while i < lines.len() {
                        let hl = lines[i];
                        if hl.starts_with("@@ ")
                            || hl.starts_with("--- ")
                            || hl.starts_with("diff ")
                        {
                            break;
                        }
                        if hl.starts_with("\\ No newline at end of file") {
                            i += 1;
                            continue;
                        }
                        if let Some(rest) = hl.strip_prefix(' ') {
                            hlines.push(PatchLine::Context(rest.to_string()));
                        } else if let Some(rest) = hl.strip_prefix('-') {
                            hlines.push(PatchLine::Remove(rest.to_string()));
                        } else if let Some(rest) = hl.strip_prefix('+') {
                            hlines.push(PatchLine::Add(rest.to_string()));
                        } else if hl.is_empty() {
                            hlines.push(PatchLine::Context(String::new()));
                        } else {
                            return Err(format!("apply_patch: unexpected hunk line {hl:?}"));
                        }
                        i += 1;
                    }
                    hunks.push(PatchHunk {
                        old_start,
                        lines: hlines,
                    });
                } else {
                    i += 1;
                }
            }
            ops.push(PatchFileOp {
                old_path,
                new_path,
                is_create,
                is_delete,
                hunks,
            });
        } else {
            i += 1;
        }
    }
    Ok(ops)
}

fn parse_hunk_old_start(header: &str) -> Result<usize, String> {
    let rest = header
        .strip_prefix("@@ -")
        .ok_or_else(|| format!("apply_patch: invalid hunk header {header:?}"))?;
    let num_part = rest.split([',', ' ']).next().unwrap_or("");
    num_part
        .parse::<usize>()
        .map_err(|_| format!("apply_patch: invalid hunk start in {header:?}"))
}

fn apply_file_hunks(
    rel: &str,
    existing: &str,
    hunks: &[PatchHunk],
) -> Result<(String, usize, usize), String> {
    let use_crlf = existing.contains("\r\n");
    let had_trailing_nl = existing.ends_with('\n') || existing.is_empty();
    let mut file_lines: Vec<String> = existing
        .lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    let mut offset_delta: isize = 0;
    let mut added = 0usize;
    let mut removed = 0usize;

    for hunk in hunks {
        let old_seq: Vec<&str> = hunk
            .lines
            .iter()
            .filter_map(|l| match l {
                PatchLine::Context(s) | PatchLine::Remove(s) => Some(s.as_str()),
                PatchLine::Add(_) => None,
            })
            .collect();
        let new_seq: Vec<String> = hunk
            .lines
            .iter()
            .filter_map(|l| match l {
                PatchLine::Context(s) | PatchLine::Add(s) => Some(s.clone()),
                PatchLine::Remove(_) => None,
            })
            .collect();

        added += hunk
            .lines
            .iter()
            .filter(|l| matches!(l, PatchLine::Add(_)))
            .count();
        removed += hunk
            .lines
            .iter()
            .filter(|l| matches!(l, PatchLine::Remove(_)))
            .count();

        let expected_idx = (hunk.old_start.saturating_sub(1).cast_signed() + offset_delta)
            .max(0)
            .cast_unsigned();
        let pos = find_hunk_pos(&file_lines, &old_seq, expected_idx).ok_or_else(|| {
            format!(
                "apply_patch: hunk at line {} did not match {rel}",
                hunk.old_start
            )
        })?;

        offset_delta += new_seq.len().cast_signed() - old_seq.len().cast_signed();
        file_lines.splice(pos..pos + old_seq.len(), new_seq);
    }

    let nl = if use_crlf { "\r\n" } else { "\n" };
    let mut out = file_lines.join(nl);
    if !out.is_empty() && had_trailing_nl {
        out.push_str(nl);
    }
    Ok((out, added, removed))
}

fn find_hunk_pos(file_lines: &[String], old_seq: &[&str], expected_idx: usize) -> Option<usize> {
    if old_seq.is_empty() {
        return Some(expected_idx.min(file_lines.len()));
    }
    if file_lines.len() < old_seq.len() {
        return None;
    }
    let max_start = file_lines.len() - old_seq.len();
    let matches_at = |idx: usize| -> bool {
        old_seq
            .iter()
            .enumerate()
            .all(|(k, ol)| file_lines[idx + k] == *ol)
    };
    if expected_idx <= max_start && matches_at(expected_idx) {
        return Some(expected_idx);
    }
    // Search outwards from expected line offset before giving up.
    for dist in 1..=max_start.max(expected_idx) {
        if expected_idx >= dist
            && expected_idx - dist <= max_start
            && matches_at(expected_idx - dist)
        {
            return Some(expected_idx - dist);
        }
        if expected_idx + dist <= max_start && matches_at(expected_idx + dist) {
            return Some(expected_idx + dist);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_context_reads_instruction_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".github")).unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "  use tabs  ").unwrap();
        std::fs::write(dir.path().join(".github/copilot-instructions.md"), "other").unwrap();
        let t = WorkspaceTools::new(dir.path()).unwrap();
        assert_eq!(t.project_context().as_deref(), Some("use tabs"));

        let untrusted = WorkspaceTools::new_with_trust(dir.path(), false).unwrap();
        assert_eq!(untrusted.project_context(), None);
        untrusted.set_trusted(true);
        assert_eq!(untrusted.project_context().as_deref(), Some("use tabs"));
    }

    #[test]
    fn project_context_none_without_file() {
        let dir = tempfile::tempdir().unwrap();
        let t = WorkspaceTools::new(dir.path()).unwrap();
        assert_eq!(t.project_context(), None);
    }

    fn tools() -> (tempfile::TempDir, Arc<WorkspaceTools>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/b.rs"), "fn main() {}\n").unwrap();
        let t = Arc::new(WorkspaceTools::new(dir.path()).unwrap());
        (dir, t)
    }

    fn call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "t1".into(),
            name: ToolName::new(name),
            args,
        }
    }

    #[test]
    fn rejects_escape() {
        let (_d, t) = tools();
        assert!(t.resolve("../outside.txt").is_err());
        assert!(t.resolve("a.txt").is_ok());
    }

    #[test]
    fn read_file_numbers_lines() {
        let (_d, t) = tools();
        let out = t.exec(&call("read_file", json!({"path": "a.txt"})));
        assert!(out.ok);
        assert!(out.content.contains("1\talpha"));
        assert!(out.content.contains("3\tgamma"));
    }

    #[test]
    fn glob_and_list() {
        let (_d, t) = tools();
        let g = t.exec(&call("glob", json!({"pattern": "**/*.rs"})));
        assert!(g.content.contains("src/b.rs"));
        let l = t.exec(&call("list_dir", json!({"path": ".", "depth": 2})));
        assert!(l.content.contains("src/"));
        assert!(l.content.contains("a.txt"));
    }

    #[test]
    fn grep_finds_lines() {
        let (_d, t) = tools();
        let g = t.exec(&call("grep", json!({"pattern": "alpha"})));
        assert_eq!(g.content, "a.txt\n  1: alpha");
    }

    #[test]
    fn grep_regex_literal_fallback_case_and_context() {
        let (dir, t) = tools();
        std::fs::write(
            dir.path().join("src/c.rs"),
            "use std::fmt;\nfn parse(input: Vec<u8>) {}\nfn render() {}\nfn Parse2() {}\n",
        )
        .unwrap();
        let g = t.exec(&call("grep", json!({"pattern": "fn (parse|render)\\("})));
        assert!(
            g.content.contains("src/c.rs\n  2: fn parse"),
            "{}",
            g.content
        );
        assert!(g.content.contains("  3: fn render() {}"));
        let g = t.exec(&call("grep", json!({"pattern": "parse(input: Vec<u8>)"})));
        assert!(g.content.contains("  2: fn parse(input"), "{}", g.content);
        let g = t.exec(&call("grep", json!({"pattern": "main() {}"})));
        assert!(
            g.content.contains("src/b.rs\n  1: fn main() {}"),
            "{}",
            g.content
        );
        let g = t.exec(&call(
            "grep",
            json!({"pattern": "PARSE", "ignore_case": true, "path": "src/c.rs"}),
        ));
        assert!(
            g.content.contains("  2:") && g.content.contains("  4:"),
            "{}",
            g.content
        );
        let g = t.exec(&call(
            "grep",
            json!({"pattern": "render", "context": 1, "glob": "*.rs"}),
        ));
        assert_eq!(
            g.content,
            "src/c.rs\n  2- fn parse(input: Vec<u8>) {}\n  3: fn render() {}\n  4- fn Parse2() {}"
        );
        let g = t.exec(&call("grep", json!({"pattern": "zzz_nope"})));
        assert!(g.content.starts_with("(no matches"));
    }

    #[test]
    fn grep_caps_per_file_and_reports_remainder() {
        let (dir, t) = tools();
        let body = "hit\n".repeat(MAX_GREP_PER_FILE + 7);
        std::fs::write(dir.path().join("many.txt"), body).unwrap();
        let g = t.exec(&call("grep", json!({"pattern": "hit", "path": "many.txt"})));
        assert_eq!(g.content.matches(": hit").count(), MAX_GREP_PER_FILE);
        assert!(
            g.content.contains("(+7 more matches in this file)"),
            "{}",
            g.content
        );
    }

    #[test]
    fn symbol_finds_definitions_with_signature() {
        let (dir, t) = tools();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub struct Parser;\n\nimpl Parser {\n    pub fn parse_expr(&self, depth: u8) -> u8 { depth }\n}\n// fn parse_comment() is prose\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("notes.md"), "class Parser is neat\n").unwrap();
        let out = t.exec(&call("symbol", json!({"query": "parse_expr"})));
        assert_eq!(
            out.content,
            "src/lib.rs\n  4: pub fn parse_expr(&self, depth: u8) -> u8 { depth }"
        );
        let out = t.exec(&call("symbol", json!({"query": "parser"})));
        assert!(
            out.content.contains("  1: pub struct Parser;"),
            "{}",
            out.content
        );
        assert!(!out.content.contains("notes.md"));
        let out = t.exec(&call("symbol", json!({"query": "main", "path": "src"})));
        assert!(out.content.contains("src/b.rs"), "{}", out.content);
        let out = t.exec(&call("symbol", json!({"query": "nope_nothing"})));
        assert!(out.ok && out.content.starts_with("(no definition"));
    }

    #[test]
    fn symbol_index_refreshes_after_agent_edits() {
        let (_d, t) = tools();
        assert!(
            t.exec(&call("symbol", json!({"query": "fresh_fn"})))
                .content
                .starts_with("(no")
        );
        let w = t.exec(&call(
            "write_file",
            json!({"path": "src/new.rs", "content": "fn fresh_fn() {}\n"}),
        ));
        assert!(w.ok);
        let out = t.exec(&call("symbol", json!({"query": "fresh_fn"})));
        assert!(
            out.content.contains("src/new.rs\n  1: fn fresh_fn() {}"),
            "{}",
            out.content
        );
    }

    #[test]
    fn duplicate_reads_collapse_until_something_changes() {
        let (dir, t) = tools();
        let read = call("read_file", json!({"path": "a.txt"}));
        let first = t.exec_memoized(&read);
        assert!(first.content.contains("1\talpha"));

        let dup = t.exec_memoized(&read);
        assert!(
            dup.ok && dup.content.starts_with("[duplicate call"),
            "{}",
            dup.content
        );
        assert_eq!(dup.full_tokens, first.sent_tokens);
        assert!(dup.sent_tokens < 40);

        let paged = t.exec_memoized(&call("read_file", json!({"path": "a.txt", "offset": 2})));
        assert!(paged.content.contains("2\tbeta"));

        let e = t.exec_memoized(&call(
            "edit_file",
            json!({"path": "a.txt", "old_string": "beta", "new_string": "BETA"}),
        ));
        assert!(e.ok);
        let after = t.exec_memoized(&read);
        assert!(after.content.contains("2\tBETA"), "{}", after.content);

        assert!(
            t.exec_memoized(&read)
                .content
                .starts_with("[duplicate call")
        );
        std::fs::write(
            dir.path().join("a.txt"),
            "changed by the user, longer now\n",
        )
        .unwrap();
        assert!(
            t.exec_memoized(&read)
                .content
                .contains("changed by the user")
        );

        assert!(
            t.exec_memoized(&read)
                .content
                .starts_with("[duplicate call")
        );
        t.reset_read_memo();
        assert!(
            t.exec_memoized(&read)
                .content
                .contains("changed by the user")
        );

        let miss = call("read_file", json!({"path": "missing.txt"}));
        assert!(!t.exec_memoized(&miss).ok);
        assert!(!t.exec_memoized(&miss).ok);
    }

    #[test]
    fn shape_keeps_head_and_tail_within_cap() {
        let text = format!("HEAD{}é{}TAIL", "a".repeat(60_000), "b".repeat(60_000));
        let out = shape(&text, 10_000);
        assert!(out.len() <= 10_000 + 64, "len {}", out.len());
        assert!(out.starts_with("HEAD") && out.ends_with("TAIL"));
        assert!(out.contains("bytes omitted"));
        let all_wide = "é".repeat(40_000);
        assert!(shape(&all_wide, 10_001).len() <= 10_001 + 64);
    }

    #[test]
    fn tool_descriptions_fit_the_budget_untruncated() {
        let (_d, t) = tools();
        for s in t.specs() {
            let n = s.description.chars().count();
            assert!(n <= 120, "{} description is {n} chars", s.name);
            assert!(
                s.description.ends_with('.'),
                "{} description was cut mid-sentence: {:?}",
                s.name,
                s.description
            );
        }
    }

    #[test]
    fn command_output_is_cleaned_and_clipped() {
        let mut stdout = String::from("\u{1b}[32mCompiling\u{1b}[0m foo\n");
        stdout.push_str("fetch 1%\rfetch 100%\n");
        for _ in 0..50 {
            stdout.push_str("warning: unused variable\n");
        }
        for i in 0..2000 {
            let _ = writeln!(stdout, "test case_{i} ... ok");
        }
        stdout.push_str("test result: FAILED. 1999 passed; 1 failed\n");
        let out = format_command_output(101, &stdout, "");
        assert!(
            out.starts_with("exit code: 101\nCompiling foo\nfetch 100%\n"),
            "{out}"
        );
        assert!(out.contains("warning: unused variable (×50)"));
        assert!(out.contains("lines omitted"));
        assert!(out.ends_with("test result: FAILED. 1999 passed; 1 failed"));
        assert!(
            !out.contains("stderr"),
            "empty stream must not cost a header"
        );
        assert!(out.len() <= BASH_MAX_BYTES + 256, "len {}", out.len());
        assert!(out.lines().count() <= BASH_MAX_LINES + 3);

        assert_eq!(
            format_command_output(0, "", "\n"),
            "exit code: 0\n(no output)"
        );
        assert_eq!(
            format_command_output(1, "out\n", "boom\n"),
            "exit code: 1\nout\n--- stderr ---\nboom"
        );
    }

    #[test]
    fn unknown_tool_errors_not_panics() {
        let (_d, t) = tools();
        let out = t.exec(&call("rm_rf", json!({})));
        assert!(!out.ok);
        assert!(out.content.contains("unknown tool"));
    }

    #[test]
    fn write_then_edit_file() {
        let (dir, t) = tools();
        let w = t.exec(&call(
            "write_file",
            json!({"path": "new.txt", "content": "one\ntwo\n"}),
        ));
        assert!(w.ok, "{}", w.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("new.txt")).unwrap(),
            "one\ntwo\n"
        );

        let e = t.exec(&call(
            "edit_file",
            json!({"path": "a.txt", "old_string": "beta", "new_string": "BETA"}),
        ));
        assert!(e.ok, "{}", e.content);
        assert!(
            std::fs::read_to_string(dir.path().join("a.txt"))
                .unwrap()
                .contains("BETA")
        );

        let amb = t.exec(&call(
            "edit_file",
            json!({"path": "a.txt", "old_string": "a", "new_string": "x"}),
        ));
        assert!(!amb.ok);
        assert!(amb.content.contains("matches"));

        let miss = t.exec(&call(
            "edit_file",
            json!({"path": "a.txt", "old_string": "zzz", "new_string": "x"}),
        ));
        assert!(!miss.ok);
    }

    #[test]
    fn checkpoints_capture_and_restore() {
        let (dir, t) = tools();
        let ck = dir.path().join("ckpts");
        t.enable_checkpoints(ck.clone());

        t.exec(&call(
            "edit_file",
            json!({"path": "a.txt", "old_string": "beta", "new_string": "BETA"}),
        ));
        t.exec(&call(
            "edit_file",
            json!({"path": "a.txt", "old_string": "gamma", "new_string": "GAMMA"}),
        ));
        t.exec(&call(
            "write_file",
            json!({"path": "n.txt", "content": "x"}),
        ));

        let b = t.drain_checkpoint().expect("mutations captured");
        assert_eq!(b.files.len(), 2);
        assert_eq!(
            b.files.get("a.txt").and_then(|o| o.as_deref()),
            Some(b"alpha\nbeta\ngamma\n".as_slice())
        );
        assert!(b.files.get("n.txt").is_some_and(Option::is_none));
        assert!(t.drain_checkpoint().is_none(), "drain is one-shot");

        crate::checkpoint::write_checkpoint(&b).unwrap();
        std::fs::write(dir.path().join("a.txt"), "garbage").unwrap();
        let restored = crate::checkpoint::restore_checkpoint(&ck.join(&b.id), dir.path()).unwrap();
        assert_eq!(restored, 2);
        assert!(
            std::fs::read_to_string(dir.path().join("a.txt"))
                .unwrap()
                .contains("beta")
        );
        assert!(!dir.path().join("n.txt").exists());
    }

    #[tokio::test]
    async fn bash_runs_and_reports_exit() {
        let (_d, t) = tools();
        let out = t
            .execute(
                &call("bash", json!({"command": "echo hi"})),
                &CancellationToken::new(),
            )
            .await;
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("exit code: 0"));
        assert!(out.content.contains("hi"));
    }

    #[tokio::test]
    async fn bash_cancels_cleanly_and_times_out() {
        let (_d, t) = tools();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let out = t
            .execute(&call("bash", json!({"command": "echo hi"})), &cancel)
            .await;
        assert!(!out.ok);
        assert!(out.content.contains("cancelled"), "{}", out.content);

        let cmd = if cfg!(windows) {
            "ping -n 6 127.0.0.1 >nul"
        } else {
            "sleep 5"
        };
        let timed = t
            .execute(
                &call("bash", json!({"command": cmd, "timeout_ms": 1000})),
                &CancellationToken::new(),
            )
            .await;
        assert!(!timed.ok);
        assert!(timed.content.contains("timed out"), "{}", timed.content);
    }

    #[test]
    fn gate_reason_covers_mutations_only() {
        let (_d, t) = tools();
        assert!(
            t.gate_reason(&call("read_file", json!({"path": "a.txt"})))
                .is_none()
        );
        assert!(
            t.gate_reason(&call("grep", json!({"pattern": "x"})))
                .is_none()
        );
        assert!(
            t.gate_reason(&call("bash", json!({"command": "ls"})))
                .is_none()
        );
        t.set_auto_approve_safe_bash(false);
        assert!(
            t.gate_reason(&call("bash", json!({"command": "ls"})))
                .is_some()
        );
        t.set_auto_approve_safe_bash(true);
        let write_gate = t
            .gate_reason(&call("bash", json!({"command": "touch x"})))
            .expect("write bash prompts");
        assert!(write_gate.0.contains("Write"), "{}", write_gate.0);
        let net_gate = t
            .gate_reason(&call(
                "bash",
                json!({"command": "curl https://example.com"}),
            ))
            .expect("network bash prompts");
        assert!(net_gate.0.contains("Network"), "{}", net_gate.0);
        let destr_gate = t
            .gate_reason(&call("bash", json!({"command": "rm -rf target"})))
            .expect("destructive bash prompts");
        assert!(destr_gate.0.contains("DESTRUCTIVE"), "{}", destr_gate.0);
        assert!(
            t.gate_reason(&call("write_file", json!({"path": "x", "content": "y"})))
                .is_some()
        );
        assert!(
            t.gate_reason(&call(
                "edit_file",
                json!({"path": "x", "old_string": "a", "new_string": "b"})
            ))
            .is_some()
        );
        assert!(
            t.gate_reason(&call("apply_patch", json!({"patch": "--- a/x\n+++ b/x\n"})))
                .is_some()
        );
    }

    #[test]
    fn deferred_specs_are_compact_and_searchable() {
        for s in WorkspaceTools::deferred_specs() {
            let n = s.description.chars().count();
            assert!(n <= 120, "{} description is {n} chars", s.name);
            assert!(s.description.ends_with('.'));
        }
        let (_d, t) = tools();
        let out = t.exec(&call("tool_search", json!({"query": "git commit history"})));
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("git_log"), "{}", out.content);
    }

    #[test]
    fn edit_file_supports_whitespace_fallback_and_diagnostics() {
        let (dir, t) = tools();
        std::fs::write(
            dir.path().join("indent.rs"),
            "fn outer() {\n    let x = 1;\n    let y = 2;\n}\n",
        )
        .unwrap();

        let fuzzy = t.exec(&call(
            "edit_file",
            json!({
                "path": "indent.rs",
                "old_string": "let x = 1;\nlet y = 2;",
                "new_string": "let x = 10;\nlet y = 20;"
            }),
        ));
        assert!(fuzzy.ok, "{}", fuzzy.content);
        assert!(fuzzy.content.contains("whitespace-normalized"));
        let after = std::fs::read_to_string(dir.path().join("indent.rs")).unwrap();
        assert!(
            after.contains("    let x = 10;\n    let y = 20;"),
            "{after}"
        );

        let miss = t.exec(&call(
            "edit_file",
            json!({
                "path": "indent.rs",
                "old_string": "fn outer() {\n    let x = 999;\n}",
                "new_string": "noop"
            }),
        ));
        assert!(!miss.ok);
        assert!(miss.content.contains("Closest region"), "{}", miss.content);

        std::fs::write(dir.path().join("dup.txt"), "foo\nbar\nfoo\n").unwrap();
        let amb = t.exec(&call(
            "edit_file",
            json!({"path": "dup.txt", "old_string": "foo", "new_string": "baz"}),
        ));
        assert!(!amb.ok);
        assert!(amb.content.contains("lines 1, 3"), "{}", amb.content);
    }

    #[test]
    fn outline_and_apply_patch_and_diff_preview_work() {
        let (dir, t) = tools();
        std::fs::write(
            dir.path().join("doc.md"),
            "# Overview\ntext\n## Details\nmore\n",
        )
        .unwrap();
        let md_outline = t.exec(&call("outline", json!({"path": "doc.md"})));
        assert!(md_outline.ok, "{}", md_outline.content);
        assert!(md_outline.content.contains("1: # Overview"));
        assert!(md_outline.content.contains("3: ## Details"));

        let rs_outline = t.exec(&call("outline", json!({"path": "src/b.rs"})));
        assert!(rs_outline.ok, "{}", rs_outline.content);
        assert!(rs_outline.content.contains("1: fn main() {}"));

        let patch = "--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n alpha\n-beta\n+BETA\n gamma\n";
        let preview = t
            .diff_preview(&call("apply_patch", json!({"patch": patch})))
            .expect("patch preview");
        assert!(preview.contains("+BETA"));

        let applied = t.exec(&call("apply_patch", json!({"patch": patch})));
        assert!(applied.ok, "{}", applied.content);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "alpha\nBETA\ngamma\n"
        );
    }
}
