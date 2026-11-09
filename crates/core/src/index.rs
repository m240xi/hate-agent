//! Workspace index for the UI: a real file tree plus heuristic symbol
//! hits. Read-only and capped — this feeds the `FileTree` pane, it is
//! not the agent's view of the world (`tools.rs` owns that).
//!
//! Symbol extraction is deliberately dumb: anchored declaration
//! keywords only, so call sites and references never pollute results.
//! It errs toward missing exotic syntax rather than emitting noise.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::tools::SKIP_DIRS;

/// Total tree entries (dirs + files) before we stop walking.
const MAX_ENTRIES: usize = 10_000;
/// Files actually opened for symbol scanning.
const MAX_SYMBOL_FILES: usize = 4_000;
/// Per-file read cap for symbol scanning.
const MAX_FILE_BYTES: u64 = 256 * 1024;
/// Lines scanned per file.
const MAX_FILE_LINES: usize = 4_000;
/// Per-file symbol cap.
const MAX_FILE_SYMBOLS: usize = 300;
/// Global symbol cap.
const MAX_SYMBOLS: usize = 20_000;
// Git status has no explicit timeout — `git status` on a normal repo
// returns in well under a second; a pathological repo just makes the
// index call slow once, it cannot hang the UI thread forever because
// commands run off the main thread.

/// Whether a tree node is a directory or a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    /// Directory.
    Dir,
    /// Regular file.
    File,
}

/// One node in the workspace file tree.
#[derive(Debug, Clone, Serialize)]
pub struct FileNode {
    /// Display name (final path segment).
    pub name: String,
    /// Workspace-relative path, `/`-joined — what `selectedPath` holds.
    pub path: String,
    /// Dir or file.
    pub kind: NodeKind,
    /// Git status dot (`M`/`A`/`D`/`?`), absent outside a repo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<char>,
    /// Child nodes; empty for files and leaf dirs.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub children: Vec<FileNode>,
}

/// Symbol classification — only `Def` is emitted today; `Ref`/`Call`
/// exist so the wire shape matches the UI's badge set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SymKind {
    /// Declaration site.
    Def,
    /// Reference site (unused).
    Ref,
    /// Call site (unused).
    Call,
}

/// One heuristic symbol hit (name + location).
#[derive(Debug, Clone, Serialize)]
pub struct SymbolHit {
    /// Declared identifier.
    pub name: String,
    /// Workspace-relative file path.
    pub path: String,
    /// 1-based line number.
    pub line: u32,
    /// Classification.
    pub kind: SymKind,
}

/// The built workspace index: tree + symbols + truncation flag.
#[derive(Debug, Serialize)]
pub struct WorkspaceIndex {
    /// Root-level tree nodes.
    pub tree: Vec<FileNode>,
    /// Symbol hits across scanned files.
    pub symbols: Vec<SymbolHit>,
    /// True when caps cut the walk short — UI may show a hint.
    pub truncated: bool,
}

/// Build the index for `root`. `root` must exist and be a directory;
/// callers get an `io::Error` otherwise.
pub fn build_index(root: &Path) -> io::Result<WorkspaceIndex> {
    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "workspace root is not a directory",
        ));
    }

    let statuses = git_status(&root);
    let mut entries: Vec<(String, bool)> = Vec::new(); // (rel path, is_dir)
    let mut truncated = false;
    walk(&root, &root, &mut entries, &mut truncated);

    let mut symbols = Vec::new();
    let mut scanned = 0usize;
    for (rel, is_dir) in &entries {
        if symbols.len() >= MAX_SYMBOLS || scanned >= MAX_SYMBOL_FILES {
            break;
        }
        if *is_dir {
            continue;
        }
        scanned += 1;
        collect_symbols(&root.join(rel), rel, &mut symbols);
    }

    Ok(WorkspaceIndex {
        tree: build_tree(&entries, &statuses),
        symbols,
        truncated,
    })
}

/// File extensions the agent-facing symbol scan opens. The UI index
/// scans everything text-like; the agent tool stays on source files so
/// prose ("class of problems…") never shows up as a definition.
const SOURCE_EXTS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "java", "kt", "kts", "cs", "rb",
    "php", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "swift", "scala", "lua", "svelte", "vue",
    "zig", "ex", "exs", "dart", "sh",
];

/// Definition sites across source files under `root` — no tree, no git
/// status. Feeds the agent's `symbol` tool, which caches the result and
/// rebuilds it after mutations. Same caps as [`build_index`].
pub fn collect_workspace_symbols(root: &Path) -> Vec<SymbolHit> {
    let mut entries: Vec<(String, bool)> = Vec::new();
    let mut truncated = false;
    walk(root, root, &mut entries, &mut truncated);

    let mut symbols = Vec::new();
    let mut scanned = 0usize;
    for (rel, is_dir) in &entries {
        if symbols.len() >= MAX_SYMBOLS || scanned >= MAX_SYMBOL_FILES {
            break;
        }
        let is_source = Path::new(rel)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| SOURCE_EXTS.contains(&e.to_ascii_lowercase().as_str()));
        if *is_dir || !is_source {
            continue;
        }
        scanned += 1;
        collect_symbols(&root.join(rel), rel, &mut symbols);
    }
    symbols
}

// ── walk ──────────────────────────────────────────────────────────────────

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, bool)>, truncated: &mut bool) {
    if out.len() >= MAX_ENTRIES {
        *truncated = true;
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    // Sort each level so the tree is deterministic across runs.
    let mut kids: Vec<(String, PathBuf, bool)> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let ft = e.file_type().ok()?;
            if ft.is_symlink() {
                return None; // never follow links out of the workspace
            }
            let name = e.file_name().to_string_lossy().into_owned();
            Some((name, e.path(), ft.is_dir()))
        })
        .collect();
    kids.sort_by(|a, b| (!a.2, &a.0).cmp(&(!b.2, &b.0))); // dirs first, then name

    for (name, path, is_dir) in kids {
        if out.len() >= MAX_ENTRIES {
            *truncated = true;
            return;
        }
        if is_dir {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel_s = rel.to_string_lossy().replace('\\', "/");
            out.push((rel_s.clone(), true));
            walk(root, &path, out, truncated);
        } else {
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel_s = rel.to_string_lossy().replace('\\', "/");
            out.push((rel_s, false));
        }
    }
}

// ── tree assembly ─────────────────────────────────────────────────────────

fn build_tree(entries: &[(String, bool)], statuses: &HashMap<String, char>) -> Vec<FileNode> {
    #[derive(Default)]
    struct Acc {
        children: BTreeMap<String, Acc>,
        is_file: bool,
        status: Option<char>,
    }

    fn emit(name: String, path: String, acc: Acc) -> FileNode {
        let kind = if acc.is_file {
            NodeKind::File
        } else {
            NodeKind::Dir
        };
        let children = if acc.children.is_empty() {
            Vec::new()
        } else {
            acc.children
                .into_iter()
                .map(|(n, c)| {
                    let child_path = format!("{path}/{n}");
                    emit(n, child_path, c)
                })
                .collect()
        };
        FileNode {
            name,
            path,
            kind,
            status: acc.status,
            children,
        }
    }

    let mut root = Acc::default();
    for (rel, is_dir) in entries {
        let mut node = &mut root;
        let segs: Vec<&str> = rel.split('/').collect();
        for (i, seg) in segs.iter().enumerate() {
            let last = i == segs.len() - 1;
            node = node.children.entry((*seg).to_string()).or_default();
            if last {
                node.is_file = !is_dir;
                if !is_dir {
                    node.status = statuses.get(rel).copied();
                }
            }
        }
    }

    root.children
        .into_iter()
        .map(|(n, c)| emit(n.clone(), n, c))
        .collect()
}

// ── git status ────────────────────────────────────────────────────────────

/// `git status --porcelain` → map of workspace-relative path → status
/// char. Absent git/repo just yields an empty map — the tree still
/// works, status dots simply don't appear.
fn git_status(root: &Path) -> HashMap<String, char> {
    let Ok(out) = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["status", "--porcelain"])
        .output()
    else {
        return HashMap::new();
    };
    if !out.status.success() {
        return HashMap::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut map = HashMap::new();
    for line in text.lines() {
        if line.len() < 4 {
            continue;
        }
        let xy = &line[..2];
        let mut path = line[3..].trim().to_string();
        // Renames: "orig -> new" — status belongs to the new path.
        if let Some((_, new)) = path.split_once(" -> ") {
            path = new.trim().to_string();
        }
        let path = path.trim_matches('"').replace('\\', "/");
        let status = if xy == "??" {
            '?'
        } else if xy.contains('D') {
            'D'
        } else if xy.contains('M') {
            'M'
        } else if xy.contains('A') {
            'A'
        } else {
            continue;
        };
        map.insert(path, status);
    }
    map
}

// ── symbol extraction ─────────────────────────────────────────────────────

/// Tokens allowed before the declaration keyword (`pub async fn` etc).
/// `static` is deliberately absent — it's a declaration keyword itself.
const MODIFIERS: &[&str] = &[
    "pub",
    "async",
    "export",
    "default",
    "extern",
    "unsafe",
    "abstract",
    "final",
    "open",
    "internal",
    "private",
    "protected",
    "public",
    "suspend",
    "override",
    "virtual",
    "sealed",
    "partial",
    "readonly",
    "declare",
    "inline",
    "constexpr",
    "mut",
    "ref",
    "lazy",
    "tailrec",
    "data",
    "inner",
    "outer",
    "companion",
    "expect",
    "actual",
    "crossinline",
    "noinline",
    "operator",
    "infix",
    "prefix",
    "postfix",
    "vararg",
];

/// Declaration keywords → symbol. `impl`/`mod` included: they name
/// real things a developer searches for.
const DEF_KEYWORDS: &[&str] = &[
    "fn",
    "def",
    "func",
    "function",
    "class",
    "struct",
    "enum",
    "trait",
    "interface",
    "impl",
    "type",
    "const",
    "static",
    "mod",
    "fun",
    "sub",
    "proc",
    "extension",
    "protocol",
    "actor",
    "object",
    "record",
    "union",
    "namespace",
    "module",
    "macro_rules",
];

/// After `enum`/`class` a language may repeat a kind word
/// (`enum class Color`) — skip it before reading the name.
const SECONDARY_KINDS: &[&str] = &["class", "struct", "interface", "enum"];

fn collect_symbols(path: &Path, rel: &str, out: &mut Vec<SymbolHit>) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.len() > MAX_FILE_BYTES || !looks_text(path) {
        return;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let mut found = 0usize;
    for (i, line) in text.lines().enumerate() {
        if i >= MAX_FILE_LINES || found >= MAX_FILE_SYMBOLS || out.len() >= MAX_SYMBOLS {
            break;
        }
        let Some(name) = decl_name(line) else {
            continue;
        };
        out.push(SymbolHit {
            name,
            path: rel.to_string(),
            line: (i + 1) as u32,
            kind: SymKind::Def,
        });
        found += 1;
    }
}

/// Cheap binary sniff: read the head, reject on NUL bytes.
fn looks_text(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 512];
    let n = f.read(&mut buf).unwrap_or(0);
    !buf[..n].contains(&0)
}

/// If `line` is a declaration, return the declared name.
fn decl_name(line: &str) -> Option<String> {
    fn bare(tok: &str) -> &str {
        tok.trim_end_matches(|c: char| ":;,!".contains(c))
    }
    let t = line.trim_start();
    if t.is_empty() || t.starts_with("//") || t.starts_with('#') || t.starts_with('*') {
        return None;
    }
    let toks: Vec<&str> = t.split_whitespace().collect();

    // Skip leading modifiers (`pub async fn`, `export default class`…).
    let mut i = 0;
    while i < toks.len() && MODIFIERS.contains(&bare(toks[i])) {
        i += 1;
    }
    if i >= toks.len() {
        return None;
    }
    // Generics attach to the keyword (`impl<T>`, `fn<T>`?) — cut at '<'.
    let kw_raw = bare(toks[i]);
    let kw_end = kw_raw.find('<').unwrap_or(kw_raw.len());
    let kw = &kw_raw[..kw_end];
    if !DEF_KEYWORDS.contains(&kw) {
        return None;
    }
    i += 1;

    // Post-keyword modifiers (`static mut FOO`).
    while i < toks.len() && MODIFIERS.contains(&bare(toks[i])) {
        i += 1;
    }

    // Go receiver: `func (r *T) Method(...)` — the receiver can span
    // tokens, so rescan the raw remainder for the closing paren.
    if kw == "func" && i < toks.len() && toks[i].starts_with('(') {
        let rest = toks[i..].join(" ");
        if let Some(close) = rest.find(')') {
            let after = rest[close + 1..].trim_start();
            return clean_name(after.split_whitespace().next()?);
        }
        return None;
    }

    // Skip secondary kind words (`enum class`, `struct class`…).
    while i < toks.len() && SECONDARY_KINDS.contains(&bare(toks[i])) {
        i += 1;
    }
    clean_name(toks.get(i).copied()?)
}

fn clean_name(raw: &str) -> Option<String> {
    let end = raw
        .find(|c: char| {
            matches!(
                c,
                '<' | '(' | '{' | '[' | ':' | '=' | ',' | ';' | '`' | '\'' | '"'
            )
        })
        .unwrap_or(raw.len());
    let name = raw[..end].trim_end_matches([':', ',', ';']);
    if name.is_empty()
        || !name.chars().any(|c| c.is_alphanumeric() || c == '_')
        || matches!(
            name,
            "self" | "Self" | "if" | "for" | "while" | "return" | "else"
        )
    {
        return None;
    }
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_common_decls() {
        assert_eq!(
            decl_name("pub async fn run_turn() -> X {").as_deref(),
            Some("run_turn")
        );
        assert_eq!(decl_name("fn main()").as_deref(), Some("main"));
        assert_eq!(decl_name("class Foo(Base):").as_deref(), Some("Foo"));
        assert_eq!(decl_name("  def handler(req):").as_deref(), Some("handler"));
        assert_eq!(decl_name("export const MAX = 3;").as_deref(), Some("MAX"));
        assert_eq!(decl_name("enum class Color {").as_deref(), Some("Color"));
        assert_eq!(
            decl_name("func (s *Server) Start() {}").as_deref(),
            Some("Start")
        );
        assert_eq!(decl_name("impl<T> Vec<T> {").as_deref(), Some("Vec"));
        assert_eq!(decl_name("trait Provider {").as_deref(), Some("Provider"));
        assert!(decl_name("// fn commented() {}").is_none());
        assert!(decl_name("let x = foo();").is_none());
        assert!(decl_name("x = compute(y)").is_none());
    }

    #[test]
    fn builds_nested_tree() {
        let entries = vec![
            ("src".to_string(), true),
            ("src/main.rs".to_string(), false),
            ("src/lib".to_string(), true),
            ("src/lib/util.rs".to_string(), false),
            ("README.md".to_string(), false),
        ];
        let mut statuses = HashMap::new();
        statuses.insert("src/main.rs".to_string(), 'M');
        let tree = build_tree(&entries, &statuses);
        assert_eq!(tree.len(), 2);
        let src = tree.iter().find(|n| n.name == "src").unwrap();
        assert_eq!(src.kind, NodeKind::Dir);
        let main = src.children.iter().find(|n| n.name == "main.rs").unwrap();
        assert_eq!(main.status, Some('M'));
        assert_eq!(main.path, "src/main.rs");
        let readme = tree.iter().find(|n| n.name == "README.md").unwrap();
        assert_eq!(readme.kind, NodeKind::File);
    }

    #[test]
    fn index_walk_skips_and_scans() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("src/main.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();
        fs::write(root.join("src/nested/deep.py"), "class Deep:\n    pass\n").unwrap();
        fs::create_dir_all(root.join("node_modules/dep")).unwrap();
        fs::write(root.join("node_modules/dep/x.js"), "function nope() {}").unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join(".hidden/h.rs"), "fn hidden() {}").unwrap();

        let idx = build_index(root).unwrap();
        let names: Vec<&str> = idx.tree.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"src"));
        assert!(!names.contains(&"node_modules"));
        assert!(!names.contains(&".hidden"));
        let sym_names: Vec<&str> = idx.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(sym_names.contains(&"alpha"));
        assert!(sym_names.contains(&"beta"));
        assert!(sym_names.contains(&"Deep"));
        assert!(!sym_names.contains(&"nope"));
        assert!(!sym_names.contains(&"hidden"));
    }
}
