//! Workspace tools: `read_file`, `list_dir`, `glob`, `grep`, `symbol`
//! (read-only), `write_file`, `edit_file`, `bash` (gated). Every path
//! resolves under a canonicalized workspace root — `..` escapes and
//! symlink breakouts are rejected before any I/O.
//!
//! Outputs are *shaped* before re-entering model context: files are
//! line-numbered and capped, listings and searches are entry-capped, and
//! every result reports `full_tokens` vs `sent_tokens` so the UI can
//! show what truncation saved. Tool *errors* (missing file, bad args)
//! come back as error text the model can read and recover from — they
//! are never turn failures.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::BufRead;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use hate_context::O200kCounter;
use serde_json::json;
use uuid::Uuid;

use crate::checkpoint::CheckpointBundle;
use crate::index::{SymbolHit, collect_workspace_symbols};
use crate::provider::ToolSpec;
use crate::types::{ToolCall, ToolName};

/// Bytes of formatted output we allow into model context per call.
/// ~48KB ≈ 12k tokens — enough for a large file view, small enough that
/// a tool-happy loop can't flood the window.
pub(crate) const SENT_CAP: usize = 48 * 1024;
/// Bytes of *unshaped* output we bother counting exactly. Beyond this
/// `full_tokens` saturates — the sent count (what actually costs) is
/// always exact.
pub(crate) const FULL_COUNT_CAP: usize = 256 * 1024;
/// Hard caps on collected results before shaping.
const MAX_LIST_ENTRIES: usize = 500;
const MAX_GLOB_MATCHES: usize = 200;
const MAX_GREP_MATCHES: usize = 100;
/// Matches shown per file before the rest collapse to a count — one
/// noisy file must not eat the whole match budget.
const MAX_GREP_PER_FILE: usize = 20;
const MAX_GREP_CONTEXT: u64 = 3;
const MAX_GREP_FILE_BYTES: u64 = 512 * 1024;
const MAX_SYMBOL_HITS: usize = 30;
/// A cached symbol scan older than this is rebuilt even without a
/// known mutation — the user may be editing alongside the agent.
const SYMBOL_CACHE_TTL: Duration = Duration::from_secs(30);
/// `bash` output budget after cleaning: a short head for context and a
/// long tail, where builds and test runners put their verdict.
const BASH_MAX_LINES: usize = 160;
const BASH_MAX_BYTES: usize = 12 * 1024;
/// Tools with no side effects: safe to run concurrently and to
/// de-duplicate within a turn.
const READ_ONLY_TOOLS: &[&str] = &["read_file", "list_dir", "glob", "grep", "symbol"];
/// Directories never descended into during walks.
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

/// What one tool call produced: the shaped text for the model, a UI
/// preview, and the token accounting that makes savings visible.
pub struct ToolOutput {
    /// Shaped content sent back as the `Message::Tool` payload.
    pub content: String,
    /// First ~400 chars for the live tool card.
    pub preview: String,
    /// Tokens the unshaped result would have cost (saturates at
    /// [`FULL_COUNT_CAP`]).
    pub full_tokens: u32,
    /// Tokens actually sent after shaping.
    pub sent_tokens: u32,
    /// `false` when the tool itself failed (bad path, missing file) —
    /// the model still sees `content` and can recover.
    pub ok: bool,
}

/// The tool surface, bound to one canonicalized workspace root.
pub struct WorkspaceTools {
    root: PathBuf,
    counter: O200kCounter,
    /// Session checkpoint root (`<data>/hate-agent/checkpoints/<key>`);
    /// `None` disables capture (CLI one-shots, chat sessions).
    checkpoint_root: Mutex<Option<PathBuf>>,
    /// This turn's captured originals: rel path → pre-mutation bytes.
    pending: Mutex<BTreeMap<String, Option<Vec<u8>>>>,
    /// Project instructions (AGENTS.md etc.) — S3 of prompt assembly.
    instructions: Option<String>,
    /// Bumped on every mutation the agent makes (write/edit/bash/MCP).
    /// Read memos and the symbol cache are only valid for the
    /// generation they were recorded in.
    generation: AtomicU64,
    /// This turn's successful read-only calls, keyed by name + args —
    /// an identical repeat with nothing changed answers with a stub
    /// instead of re-sending the same tokens.
    memo: Mutex<HashMap<String, ReadMemo>>,
    /// Cached definition index for the `symbol` tool.
    symbols: Mutex<Option<SymbolCache>>,
}

/// What a memoized read-only call remembers about its first run.
struct ReadMemo {
    /// Mutation generation the result was produced in.
    generation: u64,
    /// `read_file` only: the file's (mtime, len) — catches edits made
    /// outside the agent (the user's editor) that no generation sees.
    stamp: Option<(SystemTime, u64)>,
    /// Tokens the original result cost — what the stub saves.
    tokens: u32,
}

struct SymbolCache {
    generation: u64,
    built: Instant,
    hits: Arc<Vec<SymbolHit>>,
}

/// Project-instruction files checked in order; first readable match
/// wins so conflicting conventions can't stack.
const PROJECT_INSTRUCTIONS: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    ".github/copilot-instructions.md",
    "GEMINI.md",
    ".cursorrules",
];
/// Instruction files bigger than this are almost certainly not meant
/// for a prompt — skip rather than blow the context budget.
const INSTRUCTIONS_CAP: u64 = 12 * 1024;

impl WorkspaceTools {
    /// Bind tools to `root`. Fails only when the root can't be
    /// canonicalized — callers treat `None` as "no tools this session".
    ///
    /// # Errors
    /// `Err` with the io error message when `root` doesn't exist or
    /// can't be resolved.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        let canon = root
            .as_ref()
            .canonicalize()
            .map_err(|e| format!("workspace root: {e}"))?;
        Ok(Self {
            root: canon.clone(),
            counter: O200kCounter::new().map_err(|e| e.to_string())?,
            checkpoint_root: Mutex::new(None),
            pending: Mutex::new(BTreeMap::new()),
            instructions: load_instructions(&canon),
            generation: AtomicU64::new(0),
            memo: Mutex::new(HashMap::new()),
            symbols: Mutex::new(None),
        })
    }

    /// Whether `name` is a side-effect-free workspace tool — the turn
    /// loop runs batches of these concurrently.
    pub fn is_read_only(name: &str) -> bool {
        READ_ONLY_TOOLS.contains(&name)
    }

    /// Forget this turn's read memos. The turn loop calls this at turn
    /// start and after every compaction: a stub may only ever point at
    /// a result that is still verbatim in the model's context.
    pub fn reset_read_memo(&self) {
        self.memo.lock().expect("read memo poisoned").clear();
    }

    /// Record that something outside the file tools may have changed
    /// the workspace (an MCP tool ran) — invalidates memos and the
    /// symbol cache.
    pub fn note_mutation(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Canonicalized workspace root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Workspace instruction-file contents for prompt assembly (S3) —
    /// `None` when the repo declares none.
    pub fn project_context(&self) -> Option<&str> {
        self.instructions.as_deref()
    }

    /// Enable per-turn file checkpoints under `dir`. Called once at
    /// session spawn; the actor drains [`Self::drain_checkpoint`] at
    /// every turn end.
    pub fn enable_checkpoints(&self, dir: PathBuf) {
        *self
            .checkpoint_root
            .lock()
            .expect("checkpoint root poisoned") = Some(dir);
    }

    /// Snapshot a file's pre-mutation bytes — once per path per turn,
    /// so the FIRST original survives even across several edits.
    /// No-op when checkpoints are disabled or the path is outside root.
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

    /// Drain this turn's captured originals for the actor to persist.
    /// `None` when checkpoints are off or nothing mutated.
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

    /// Advertised schemas — rebuilt per request; cheap and always fresh.
    pub fn specs(&self) -> Vec<ToolSpec> {
        vec![
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
                "symbol",
                "Find where a function, type, or constant is defined. Returns path, line, and signature. Cheaper than grep.",
                json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "symbol name or part of it"},
                        "path": {"type": "string", "description": "limit to this dir or file"}
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
            ),
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
        ]
    }

    /// Execute one call. `bash` runs on the async path (tokio process +
    /// timeout); everything else is filesystem work on the blocking
    /// pool. Tool failures return `ok: false` content, never panic and
    /// never fail the turn.
    pub async fn execute(self: &Arc<Self>, call: &ToolCall) -> ToolOutput {
        if call.name.as_str() == "bash" {
            let out = self.bash(call).await;
            // A shell command can change anything — every memo is stale.
            self.note_mutation();
            return out;
        }
        let this = Arc::clone(self);
        let call = call.clone();
        match tokio::task::spawn_blocking(move || this.exec_memoized(&call)).await {
            Ok(out) => out,
            Err(e) => self.output_for(Err(format!("tool task failed: {e}"))),
        }
    }

    /// [`Self::exec`] behind the per-turn read memo: an identical
    /// read-only call whose inputs provably haven't changed returns a
    /// one-line stub pointing at the earlier result. `full_tokens`
    /// carries what the repeat would have cost, so the saving shows up
    /// in the same ledger column as truncation.
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

    /// (mtime, len) of a workspace file, `None` when it can't be read.
    fn file_stamp(&self, rel: &str) -> Option<(SystemTime, u64)> {
        let meta = std::fs::metadata(self.resolve(rel).ok()?).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }

    /// Policy gate: `Some((reason, detail))` means the call needs user
    /// approval before it runs. Reads are auto; mutations and shell are
    /// gated — "Run & remember" allowlists them for the session.
    pub fn gate_reason(&self, call: &ToolCall) -> Option<(String, Option<String>)> {
        match call.name.as_str() {
            "bash" => {
                let cmd: String = arg_str(call, "command")
                    .unwrap_or("")
                    .chars()
                    .take(200)
                    .collect();
                Some(("runs a shell command".to_string(), Some(cmd)))
            }
            "write_file" | "edit_file" => Some(("modifies files".to_string(), None)),
            _ => None,
        }
    }

    /// Error-shaped output for calls denied by policy — same accounting
    /// path as real results so the model sees an honest tool message.
    pub fn tool_error(&self, msg: String) -> ToolOutput {
        self.output_for(Err(msg))
    }

    /// Shared result → [`ToolOutput`] conversion: shape, count both
    /// sides, preview. Used by every tool including `bash`.
    fn output_for(&self, result: Result<String, String>) -> ToolOutput {
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
                let preview: String = content.chars().take(400).collect();
                let tokens = self.counter.count_sync(&content) as u32;
                ToolOutput {
                    full_tokens: tokens,
                    sent_tokens: tokens,
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
            "write_file" => self.mutating(|| self.write_file(call)),
            "edit_file" => self.mutating(|| self.edit_file(call)),
            "bash" => Err("bash runs on the async path".to_string()),
            other => Err(format!("unknown tool {other:?}")),
        };
        self.output_for(result)
    }

    /// Run a file mutation and, when it succeeded, advance the
    /// generation so stale read memos and symbols are never served.
    fn mutating(&self, f: impl FnOnce() -> Result<String, String>) -> Result<String, String> {
        let r = f();
        if r.is_ok() {
            self.note_mutation();
        }
        r
    }

    fn count_capped(&self, text: &str) -> u32 {
        // Slice on a char boundary instead of collecting chars — no
        // copy, and the cap stays a byte cap like every other one here.
        let capped = &text[..floor_char_boundary(text, FULL_COUNT_CAP)];
        self.counter.count_sync(capped) as u32
    }

    // ── path safety ──────────────────────────────────────────────────────

    /// Resolve a model-supplied path under the workspace root. Rejects
    /// `..` escapes lexically, then re-checks canonicalized targets so
    /// symlinks can't break out either.
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
        // Symlink check on anything that exists.
        if let Ok(canon) = norm.canonicalize()
            && !canon.starts_with(&self.root)
        {
            return Err(format!("path {rel:?} resolves outside the workspace"));
        }
        Ok(norm)
    }

    // ── tools ────────────────────────────────────────────────────────────

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
        // Binary sniff: NUL in the first 8KB means don't spew bytes.
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
                break; // enough for an exact-ish full count
            }
            // `N<tab>text`: measured with o200k on real source, the old
            // right-aligned `{:>6}  ` gutter cost ~40% on top of the raw
            // file; a bare number + tab costs ~15%.
            lines.push(format!("{}\t{}", i + 1, line));
        }
        // `total` only counts lines we iterated — if we broke early it
        // undercounts; note continuation honestly when we stopped early.
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
            let name = rel
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
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
                let name = rel
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default();
                if !glob_match(g, &rel_s) && !glob_match(g, &name) {
                    return;
                }
            }
            scanned += 1;
            let Ok(content) = std::fs::read_to_string(self.root.join(rel)) else {
                return; // binary or unreadable — skip silently
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
            // A file path searches just that file — no walk needed.
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

    /// `symbol` — definition lookup over the heuristic declaration
    /// index. One call answers "where is X defined" without the model
    /// grepping and reading its way there.
    fn symbol(&self, call: &ToolCall) -> Result<String, String> {
        let query = arg_str(call, "query")
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or("symbol: missing `query`")?;
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

        // Rank: exact, case-insensitive exact, prefix, substring. Ties
        // keep index order (path, then line) so output is deterministic.
        let mut ranked: Vec<(u8, &SymbolHit)> = hits
            .iter()
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

        // Group by file (first-appearance order) and attach the source
        // line, so the model gets the signature without a read.
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

    /// The cached definition index, rebuilt when a mutation happened
    /// since it was built or it has aged past [`SYMBOL_CACHE_TTL`].
    fn symbol_index(&self) -> Arc<Vec<SymbolHit>> {
        let generation = self.generation.load(Ordering::Relaxed);
        let mut cache = self.symbols.lock().expect("symbol cache poisoned");
        if let Some(c) = cache.as_ref()
            && c.generation == generation
            && c.built.elapsed() < SYMBOL_CACHE_TTL
        {
            return Arc::clone(&c.hits);
        }
        let hits = Arc::new(collect_workspace_symbols(&self.root));
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
        let matches = content.matches(old).count();
        if matches == 0 {
            return Err(format!("edit_file: `old_string` not found in {path}"));
        }
        if matches > 1 && !replace_all {
            return Err(format!(
                "edit_file: `old_string` matches {matches} times in {path} — add context or set replace_all"
            ));
        }
        let updated = if replace_all {
            content.replace(old, new)
        } else {
            content.replacen(old, new, 1)
        };
        self.capture_original(&fs_path);
        std::fs::write(&fs_path, &updated).map_err(|e| e.to_string())?;
        let delta = (new.lines().count() as i128 - old.lines().count() as i128) * matches as i128;
        Ok(format!(
            "edited {path}: {} replacement(s), {delta:+} lines",
            if replace_all { matches } else { 1 }
        ))
    }

    /// `bash` — the only gated-by-default tool. Runs through the
    /// workspace root with a hard timeout; output is capped before it
    /// can flood context.
    async fn bash(&self, call: &ToolCall) -> ToolOutput {
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
        cmd.current_dir(&self.root).kill_on_drop(true);

        let result =
            tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), cmd.output()).await;

        match result {
            Ok(Ok(out)) => {
                let code = out.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                let sent = format_command_output(code, &stdout, &stderr);
                // What an unshaped agent would have sent: the raw
                // streams. Counted (capped) so the saving is on record.
                let full_tokens = self
                    .count_capped(&stdout)
                    .saturating_add(self.count_capped(&stderr));
                self.output_shaped(sent, full_tokens, out.status.success())
            }
            Ok(Err(e)) => self.output_for(Err(format!("bash failed to start: {e}"))),
            Err(_) => self.output_for(Err(format!("bash timed out after {timeout_ms}ms (killed)"))),
        }
    }

    /// [`ToolOutput`] for text that was already shaped by its tool.
    /// Failures keep the `error: ` prefix every other tool error has.
    fn output_shaped(&self, sent: String, full_tokens: u32, ok: bool) -> ToolOutput {
        let content = if ok { sent } else { format!("error: {sent}") };
        let sent_tokens = self.counter.count_sync(&content) as u32;
        ToolOutput {
            preview: content.chars().take(400).collect(),
            full_tokens: full_tokens.max(sent_tokens),
            sent_tokens,
            content,
            ok,
        }
    }
}

// ── helpers ────────────────────────────────────────────────────────────────

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

/// `grep`'s line matcher. The pattern is tried as a regex AND as
/// literal text: models routinely search for code like `foo(bar)` or
/// `Vec<u8>` that is either an invalid regex or a valid one meaning
/// something else, and a silent zero-match there costs a wasted round.
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

/// Append one file's matches to `out` as a `path` heading followed by
/// `  line: text` rows (`  line- text` for context rows) — the path is
/// paid for once per file instead of once per match. Returns how many
/// matches were counted against the global budget.
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
            // A context row that is itself a match prints as a match.
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

/// First `max` chars of `line`, with an ellipsis when cut.
fn clip_line(line: &str, max: usize) -> std::borrow::Cow<'_, str> {
    if line.len() <= max {
        return line.into();
    }
    match line.char_indices().nth(max) {
        Some((idx, _)) => format!("{}…", &line[..idx]).into(),
        None => line.into(),
    }
}

/// Shape a finished command for the model: exit code first, then each
/// non-empty stream cleaned (ANSI, progress redraws, repeats) and
/// clipped head+tail. Empty streams cost nothing — no empty section
/// headers.
fn format_command_output(code: i32, stdout: &str, stderr: &str) -> String {
    let stdout = crate::shape::clean(stdout);
    let stderr = crate::shape::clean(stderr);
    let mut out = format!("exit code: {code}");
    // Both streams present → split the budget; otherwise one gets it all.
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

/// Lexically resolve `.`/`..` without touching the filesystem, so escape
/// attempts fail the `starts_with` check even for nonexistent paths.
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

/// Depth-first walk yielding each entry's path relative to `rel_root`,
/// whether it's a directory, and its size. Skips hidden dirs and
/// [`SKIP_DIRS`].
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

/// Glob match supporting `*` (within a segment), `**` (any depth), `?`.
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

/// `*`/`?` matcher within one path segment (no separators).
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

/// Truncate `text` to `cap` bytes on a char boundary, keeping the head
/// and tail so both the start (imports, signatures) and end (returns,
/// errors) of long outputs survive.
pub(crate) fn shape(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    let head = cap * 7 / 10;
    let tail = cap - head - 64; // room for the marker
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

/// Read the first usable project-instruction file under `root`.
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
        // AGENTS.md wins — first match in priority order.
        assert_eq!(t.project_context(), Some("use tabs"));
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
        // Regex alternation.
        let g = t.exec(&call("grep", json!({"pattern": "fn (parse|render)\\("})));
        assert!(
            g.content.contains("src/c.rs\n  2: fn parse"),
            "{}",
            g.content
        );
        assert!(g.content.contains("  3: fn render() {}"));
        // Code that is a broken/misleading regex still matches literally.
        let g = t.exec(&call("grep", json!({"pattern": "parse(input: Vec<u8>)"})));
        assert!(g.content.contains("  2: fn parse(input"), "{}", g.content);
        let g = t.exec(&call("grep", json!({"pattern": "main() {}"})));
        assert!(
            g.content.contains("src/b.rs\n  1: fn main() {}"),
            "{}",
            g.content
        );
        // ignore_case + glob filter + single-file path.
        let g = t.exec(&call(
            "grep",
            json!({"pattern": "PARSE", "ignore_case": true, "path": "src/c.rs"}),
        ));
        assert!(
            g.content.contains("  2:") && g.content.contains("  4:"),
            "{}",
            g.content
        );
        // Context rows use `-`, matches `:`; path printed once.
        let g = t.exec(&call(
            "grep",
            json!({"pattern": "render", "context": 1, "glob": "*.rs"}),
        ));
        assert_eq!(
            g.content,
            "src/c.rs\n  2- fn parse(input: Vec<u8>) {}\n  3: fn render() {}\n  4- fn Parse2() {}"
        );
        // Misses stay honest and cheap.
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
        // Prose in a non-source file must never be a definition.
        std::fs::write(dir.path().join("notes.md"), "class Parser is neat\n").unwrap();
        let out = t.exec(&call("symbol", json!({"query": "parse_expr"})));
        assert_eq!(
            out.content,
            "src/lib.rs\n  4: pub fn parse_expr(&self, depth: u8) -> u8 { depth }"
        );
        // Case-insensitive / partial queries rank below exact but still hit.
        let out = t.exec(&call("symbol", json!({"query": "parser"})));
        assert!(
            out.content.contains("  1: pub struct Parser;"),
            "{}",
            out.content
        );
        assert!(!out.content.contains("notes.md"));
        // Scope filter and honest miss.
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

        // Same call, nothing changed → stub, with the saving on record.
        let dup = t.exec_memoized(&read);
        assert!(
            dup.ok && dup.content.starts_with("[duplicate call"),
            "{}",
            dup.content
        );
        assert_eq!(dup.full_tokens, first.sent_tokens);
        assert!(dup.sent_tokens < 40);

        // Different args are a different call.
        let paged = t.exec_memoized(&call("read_file", json!({"path": "a.txt", "offset": 2})));
        assert!(paged.content.contains("2\tbeta"));

        // An agent edit invalidates → real content again.
        let e = t.exec_memoized(&call(
            "edit_file",
            json!({"path": "a.txt", "old_string": "beta", "new_string": "BETA"}),
        ));
        assert!(e.ok);
        let after = t.exec_memoized(&read);
        assert!(after.content.contains("2\tBETA"), "{}", after.content);

        // An edit from OUTSIDE the agent (no generation bump) is caught
        // by the file stamp.
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

        // A new turn (or compaction) forgets everything.
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

        // Failed calls are never memoized — the retry must really run.
        let miss = call("read_file", json!({"path": "missing.txt"}));
        assert!(!t.exec_memoized(&miss).ok);
        assert!(!t.exec_memoized(&miss).ok);
    }

    #[test]
    fn shape_keeps_head_and_tail_within_cap() {
        // Regression: the tail slice used to start at `len - tail_start`,
        // which re-sent most of the text (output grew past the cap) and
        // could split a multibyte char.
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
        // Colour + progress redraws + repeated warnings + a long middle.
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

        // Ambiguous match without replace_all is an honest error.
        let amb = t.exec(&call(
            "edit_file",
            json!({"path": "a.txt", "old_string": "a", "new_string": "x"}),
        ));
        assert!(!amb.ok);
        assert!(amb.content.contains("matches"));

        // Missing old_string is an honest error too.
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

        // Two edits to the same file — the FIRST original must win.
        t.exec(&call(
            "edit_file",
            json!({"path": "a.txt", "old_string": "beta", "new_string": "BETA"}),
        ));
        t.exec(&call(
            "edit_file",
            json!({"path": "a.txt", "old_string": "gamma", "new_string": "GAMMA"}),
        ));
        // A new file captures "didn't exist".
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
        // Simulate post-turn state (a.txt garbage, n.txt still present),
        // then rewind — restore must revert a.txt AND delete n.txt.
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
            .execute(&call("bash", json!({"command": "echo hi"})))
            .await;
        assert!(out.ok, "{}", out.content);
        assert!(out.content.contains("exit code: 0"));
        assert!(out.content.contains("hi"));
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
                .is_some()
        );
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
    }
}
