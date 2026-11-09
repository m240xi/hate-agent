use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::tools::SKIP_DIRS;

const MAX_ENTRIES: usize = 10_000;
const MAX_SYMBOL_FILES: usize = 4_000;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_FILE_LINES: usize = 4_000;
const MAX_FILE_SYMBOLS: usize = 300;
const MAX_SYMBOLS: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Dir,
    File,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub kind: NodeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<char>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub children: Vec<FileNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SymKind {
    Def,
    Ref,
    Call,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolHit {
    pub name: String,
    pub path: String,
    pub line: u32,
    pub kind: SymKind,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceIndex {
    pub tree: Vec<FileNode>,
    pub symbols: Vec<SymbolHit>,
    pub truncated: bool,
}

pub fn build_index(root: &Path) -> io::Result<WorkspaceIndex> {
    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "workspace root is not a directory",
        ));
    }

    let statuses = git_status(&root);
    let mut entries: Vec<(String, bool)> = Vec::new();
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

const SOURCE_EXTS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "go", "java", "kt", "kts", "cs", "rb",
    "php", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "swift", "scala", "lua", "svelte", "vue",
    "zig", "ex", "exs", "dart", "sh",
];

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

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, bool)>, truncated: &mut bool) {
    if out.len() >= MAX_ENTRIES {
        *truncated = true;
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut kids: Vec<(String, PathBuf, bool)> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let ft = e.file_type().ok()?;
            if ft.is_symlink() {
                return None;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            Some((name, e.path(), ft.is_dir()))
        })
        .collect();
    kids.sort_by(|a, b| (!a.2, &a.0).cmp(&(!b.2, &b.0)));

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

fn looks_text(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 512];
    let n = f.read(&mut buf).unwrap_or(0);
    !buf[..n].contains(&0)
}

fn decl_name(line: &str) -> Option<String> {
    fn bare(tok: &str) -> &str {
        tok.trim_end_matches(|c: char| ":;,!".contains(c))
    }
    let t = line.trim_start();
    if t.is_empty() || t.starts_with("//") || t.starts_with('#') || t.starts_with('*') {
        return None;
    }
    let toks: Vec<&str> = t.split_whitespace().collect();

    let mut i = 0;
    while i < toks.len() && MODIFIERS.contains(&bare(toks[i])) {
        i += 1;
    }
    if i >= toks.len() {
        return None;
    }
    let kw_raw = bare(toks[i]);
    let kw_end = kw_raw.find('<').unwrap_or(kw_raw.len());
    let kw = &kw_raw[..kw_end];
    if !DEF_KEYWORDS.contains(&kw) {
        return None;
    }
    i += 1;

    while i < toks.len() && MODIFIERS.contains(&bare(toks[i])) {
        i += 1;
    }

    if kw == "func" && i < toks.len() && toks[i].starts_with('(') {
        let rest = toks[i..].join(" ");
        if let Some(close) = rest.find(')') {
            let after = rest[close + 1..].trim_start();
            return clean_name(after.split_whitespace().next()?);
        }
        return None;
    }

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
