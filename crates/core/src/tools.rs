//! Read-only workspace tools (M2): `read_file`, `list_dir`, `glob`,
//! `grep`. Every path resolves under a canonicalized workspace root —
//! `..` escapes and symlink breakouts are rejected before any I/O.
//!
//! Outputs are *shaped* before re-entering model context: files are
//! line-numbered and capped, listings and searches are entry-capped, and
//! every result reports `full_tokens` vs `sent_tokens` so the UI can
//! show what truncation saved. Tool *errors* (missing file, bad args)
//! come back as error text the model can read and recover from — they
//! are never turn failures.

use std::fmt::Write as _;
use std::io::BufRead;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use hate_context::O200kCounter;
use serde_json::json;

use crate::provider::ToolSpec;
use crate::types::{ToolCall, ToolName};

/// Bytes of formatted output we allow into model context per call.
/// ~48KB ≈ 12k tokens — enough for a large file view, small enough that
/// a tool-happy loop can't flood the window.
const SENT_CAP: usize = 48 * 1024;
/// Bytes of *unshaped* output we bother counting exactly. Beyond this
/// `full_tokens` saturates — the sent count (what actually costs) is
/// always exact.
const FULL_COUNT_CAP: usize = 256 * 1024;
/// Hard caps on collected results before shaping.
const MAX_LIST_ENTRIES: usize = 500;
const MAX_GLOB_MATCHES: usize = 200;
const MAX_GREP_MATCHES: usize = 100;
const MAX_GREP_FILE_BYTES: u64 = 512 * 1024;
/// Directories never descended into during walks.
const SKIP_DIRS: &[&str] = &[
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

/// The M2 tool surface, bound to one canonicalized workspace root.
pub struct WorkspaceTools {
    root: PathBuf,
    counter: O200kCounter,
}

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
            root: canon,
            counter: O200kCounter::new().map_err(|e| e.to_string())?,
        })
    }

    /// Advertised schemas — rebuilt per request; cheap and always fresh.
    pub fn specs(&self) -> Vec<ToolSpec> {
        vec![
            spec(
                "read_file",
                "Read a file with line numbers. Args: path (required), offset (1-based start line), limit (max lines, default 400).",
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
                "List a directory tree. Args: path (default '.'), depth (levels, default 1, max 3).",
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
                "Find files by pattern. Args: pattern (required, supports * ** ?), path (dir to search, default '.').",
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
                "Search file contents (literal substring). Args: pattern (required), path (default '.'), glob (optional filename filter).",
                json!({
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string"},
                        "path": {"type": "string"},
                        "glob": {"type": "string"}
                    },
                    "required": ["pattern"],
                    "additionalProperties": false
                }),
            ),
        ]
    }

    /// Execute one call off the async executor. Tool failures return
    /// `ok: false` content, never panic and never fail the turn.
    pub async fn execute(self: &Arc<Self>, call: &ToolCall) -> ToolOutput {
        let this = Arc::clone(self);
        let call = call.clone();
        match tokio::task::spawn_blocking(move || this.exec(&call)).await {
            Ok(out) => out,
            Err(e) => {
                let content = format!("error: tool task failed: {e}");
                let tokens = self.counter.count_sync(&content) as u32;
                ToolOutput {
                    full_tokens: tokens,
                    sent_tokens: tokens,
                    preview: content.chars().take(400).collect(),
                    content,
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
            other => Err(format!("unknown tool {other:?}")),
        };
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

    fn count_capped(&self, text: &str) -> u32 {
        let capped: String = text.chars().take(FULL_COUNT_CAP).collect();
        self.counter.count_sync(&capped) as u32
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
            lines.push(format!("{:>6}  {}", i + 1, line));
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
            let _ = write!(
                out,
                "\n[… listing truncated at {MAX_LIST_ENTRIES} entries]"
            );
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
        let base = self.resolve(arg_str(call, "path").unwrap_or("."))?;
        let glob_filter = arg_str(call, "glob");
        let mut out = String::new();
        let mut count = 0usize;
        let mut scanned = 0usize;
        walk(&base, usize::MAX, &self.root, &mut |rel, is_dir, size| {
            if is_dir || count >= MAX_GREP_MATCHES {
                return;
            }
            if size > MAX_GREP_FILE_BYTES {
                return;
            }
            if let Some(g) = glob_filter {
                let s = rel.to_string_lossy().replace('\\', "/");
                let name = rel
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default();
                if !glob_match(g, &s) && !glob_match(g, &name) {
                    return;
                }
            }
            scanned += 1;
            let full = self.root.join(&rel);
            let Ok(content) = std::fs::read_to_string(&full) else {
                return; // binary or unreadable — skip silently
            };
            if content.contains('\0') {
                return;
            }
            let rel_s = rel.to_string_lossy().replace('\\', "/");
            for (i, line) in content.lines().enumerate() {
                if count >= MAX_GREP_MATCHES {
                    break;
                }
                if line.contains(pattern) {
                    count += 1;
                    let trimmed: String = line.trim().chars().take(200).collect();
                    let _ = writeln!(out, "{rel_s}:{}: {trimmed}", i + 1);
                }
            }
        });
        if out.is_empty() {
            out = format!("(no matches for {pattern:?} in {scanned} files)");
        } else if count >= MAX_GREP_MATCHES {
            let _ = write!(out, "\n[… truncated at {MAX_GREP_MATCHES} matches]");
        }
        Ok(out)
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
fn shape(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_string();
    }
    let head = cap * 7 / 10;
    let tail = cap - head - 64; // room for the marker
    let mut out = String::with_capacity(cap + 128);
    out.push_str(&text[..floor_char_boundary(text, head)]);
    let _ = write!(
        out,
        "\n\n[… {} bytes omitted …]\n\n",
        text.len() - head - tail
    );
    out.push_str(&text[text.len() - floor_char_boundary_rev(text, tail)..]);
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(out.content.contains("     1  alpha"));
        assert!(out.content.contains("     3  gamma"));
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
        assert!(g.content.contains("a.txt:1: alpha"));
    }

    #[test]
    fn unknown_tool_errors_not_panics() {
        let (_d, t) = tools();
        let out = t.exec(&call("rm_rf", json!({})));
        assert!(!out.ok);
        assert!(out.content.contains("unknown tool"));
    }
}
