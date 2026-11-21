use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use hate_context::O200kCounter;
use serde::Serialize;

use crate::tools::SKIP_DIRS;

const MAX_ENTRIES: usize = 10_000;
const MAX_SYMBOL_FILES: usize = 4_000;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_FILE_LINES: usize = 4_000;
const MAX_FILE_SYMBOLS: usize = 300;
const MAX_SYMBOLS: usize = 20_000;
const MAX_SYMBOLS_ALL: usize = 50_000;
const ENTRY_POINT_NAMES: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "Makefile",
    "main.rs",
    "lib.rs",
    "index.ts",
    "index.tsx",
    "index.js",
    "main.ts",
    "main.go",
    "app.py",
    "main.py",
    "App.svelte",
];

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
        collect_symbols_mode(&root.join(rel), rel, false, MAX_SYMBOLS, &mut symbols);
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
    collect_workspace_symbols_mode(root, false)
}

pub fn collect_workspace_symbols_all(root: &Path) -> Vec<SymbolHit> {
    collect_workspace_symbols_mode(root, true)
}

fn collect_workspace_symbols_mode(root: &Path, include_refs_calls: bool) -> Vec<SymbolHit> {
    let mut entries: Vec<(String, bool)> = Vec::new();
    let mut truncated = false;
    walk(root, root, &mut entries, &mut truncated);

    let cap = if include_refs_calls {
        MAX_SYMBOLS_ALL
    } else {
        MAX_SYMBOLS
    };
    let mut symbols = Vec::new();
    let mut scanned = 0usize;
    for (rel, is_dir) in &entries {
        if symbols.len() >= cap || scanned >= MAX_SYMBOL_FILES {
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
        collect_symbols_mode(&root.join(rel), rel, include_refs_calls, cap, &mut symbols);
    }
    symbols
}

pub fn build_repo_map(root: &Path, max_tokens: usize) -> Option<String> {
    let mut entries: Vec<(String, bool)> = Vec::new();
    let mut truncated = false;
    walk(root, root, &mut entries, &mut truncated);
    if entries.is_empty() {
        return None;
    }

    let mut top_dirs: Vec<String> = Vec::new();
    let mut entry_points: Vec<String> = Vec::new();
    let mut file_defs: Vec<(String, usize)> = Vec::new();
    let mut scanned = 0usize;

    for (rel, is_dir) in &entries {
        if *is_dir {
            if !rel.contains('/') {
                top_dirs.push(rel.clone());
            }
            continue;
        }
        let fname = Path::new(rel)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if ENTRY_POINT_NAMES.contains(&fname) && entry_points.len() < 12 {
            entry_points.push(rel.clone());
        }
        let is_source = Path::new(rel)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| SOURCE_EXTS.contains(&e.to_ascii_lowercase().as_str()));
        if is_source && scanned < 400 {
            scanned += 1;
            let defs = collect_file_symbols(&root.join(rel), rel).len();
            if defs > 0 {
                file_defs.push((rel.clone(), defs));
            }
        }
    }

    file_defs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut out = String::from("[repo_map — workspace structure]");
    if !top_dirs.is_empty() {
        let shown: Vec<String> = top_dirs.iter().take(20).map(|d| format!("{d}/")).collect();
        let _ = write!(out, "\ndirs: {}", shown.join(", "));
    }
    if !entry_points.is_empty() {
        let _ = write!(out, "\nentry points: {}", entry_points.join(", "));
    }
    if !file_defs.is_empty() {
        out.push_str("\nkey files (defs):");
        let counter = O200kCounter::new().ok();
        for (path, defs) in file_defs.into_iter().take(40) {
            let line = format!("\n- {path} ({defs})");
            let next_tokens = counter.as_ref().map_or((out.len() + line.len()) / 4, |c| {
                c.count_sync(&(out.clone() + &line)) as usize
            });
            if next_tokens > max_tokens {
                out.push_str("\n- …");
                break;
            }
            out.push_str(&line);
        }
    }
    Some(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, bool)>, truncated: &mut bool) {
    let mut entries: Vec<(String, bool)> = Vec::new();
    let builder = ignore::WalkBuilder::new(dir)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|ent| {
            if ent.depth() == 0 {
                return true;
            }
            let name = ent.file_name().to_string_lossy();
            if name.starts_with('.') {
                return false;
            }
            if ent.file_type().is_some_and(|ft| ft.is_dir()) && SKIP_DIRS.contains(&name.as_ref()) {
                return false;
            }
            true
        })
        .build();

    for res in builder {
        let Ok(ent) = res else { continue };
        if ent.depth() == 0 {
            continue;
        }
        let is_dir = ent.file_type().is_some_and(|ft| ft.is_dir());
        let Ok(rel) = ent.path().strip_prefix(root) else {
            continue;
        };
        let rel_s = rel.to_string_lossy().replace('\\', "/");
        if rel_s.is_empty() {
            continue;
        }
        entries.push((rel_s, is_dir));
        if entries.len() >= MAX_ENTRIES {
            *truncated = true;
            break;
        }
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));
    out.extend(entries);
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
    let mut cmd = Command::new("git");
    cmd.args(["-C"]).arg(root).args(["status", "--porcelain"]);
    crate::process_util::hide_std_cmd(&mut cmd);
    let Ok(out) = cmd.output() else {
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

pub(crate) fn collect_file_symbols(path: &Path, rel: &str) -> Vec<SymbolHit> {
    let mut out = Vec::new();
    collect_symbols_mode(path, rel, false, MAX_SYMBOLS, &mut out);
    out
}

fn collect_symbols_mode(
    path: &Path,
    rel: &str,
    include_refs_calls: bool,
    cap: usize,
    out: &mut Vec<SymbolHit>,
) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.len() > MAX_FILE_BYTES || !looks_text(path) {
        return;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    if let Some(lang) = tree_sitter_language_for(path)
        && collect_tree_sitter_symbols(&lang, &text, rel, include_refs_calls, cap, out)
    {
        return;
    }
    let mut found = 0usize;
    for (i, line) in text.lines().enumerate() {
        if i >= MAX_FILE_LINES || found >= MAX_FILE_SYMBOLS || out.len() >= cap {
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

fn tree_sitter_language_for(path: &Path) -> Option<tree_sitter::Language> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "js" | "jsx" | "mjs" | "cjs" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        _ => None,
    }
}

fn collect_tree_sitter_symbols(
    lang: &tree_sitter::Language,
    text: &str,
    rel: &str,
    include_refs_calls: bool,
    cap: usize,
    out: &mut Vec<SymbolHit>,
) -> bool {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(lang).is_err() {
        return false;
    }
    let Some(tree) = parser.parse(text.as_bytes(), None) else {
        return false;
    };

    let bytes = text.as_bytes();
    let mut seen: HashSet<(String, u32, u8)> = HashSet::new();
    let mut def_count = 0usize;
    let mut ref_count = 0usize;
    let mut stack = vec![tree.root_node()];

    let mut push_hit = |name: &str, line: u32, kind: SymKind, out: &mut Vec<SymbolHit>| {
        if out.len() >= cap {
            return;
        }
        let tag = match kind {
            SymKind::Def => 0,
            SymKind::Call => 1,
            SymKind::Ref => 2,
        };
        if kind == SymKind::Def {
            if def_count >= MAX_FILE_SYMBOLS {
                return;
            }
            def_count += 1;
        } else {
            if !include_refs_calls || ref_count >= MAX_FILE_SYMBOLS {
                return;
            }
            ref_count += 1;
        }
        let clean = name.trim();
        if clean.is_empty() || !seen.insert((clean.to_string(), line, tag)) {
            return;
        }
        out.push(SymbolHit {
            name: clean.to_string(),
            path: rel.to_string(),
            line,
            kind,
        });
    };

    while let Some(node) = stack.pop() {
        if out.len() >= cap {
            break;
        }
        let kind_str = node.kind();
        let line = (node.start_position().row + 1) as u32;

        if is_def_node(kind_str) {
            if let Some(name) = extract_def_name(node, bytes) {
                push_hit(name, line, SymKind::Def, out);
            }
        } else if include_refs_calls && is_call_node(kind_str) {
            if let Some(name) = extract_call_name(node, bytes) {
                push_hit(name, line, SymKind::Call, out);
            }
        } else if include_refs_calls
            && matches!(kind_str, "type_identifier" | "field_identifier")
            && !node
                .parent()
                .is_some_and(|p| is_def_node(p.kind()) || is_call_node(p.kind()))
            && let Ok(txt) = node.utf8_text(bytes)
        {
            push_hit(txt, line, SymKind::Ref, out);
        }

        let mut cursor = node.walk();
        let children: Vec<_> = node.children(&mut cursor).collect();
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    true
}

fn is_def_node(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "impl_item"
            | "mod_item"
            | "type_item"
            | "const_item"
            | "static_item"
            | "macro_definition"
            | "function_declaration"
            | "method_definition"
            | "method_declaration"
            | "class_declaration"
            | "class_definition"
            | "function_definition"
            | "interface_declaration"
            | "type_alias_declaration"
            | "enum_declaration"
            | "type_spec"
            | "variable_declarator"
    )
}

fn is_call_node(kind: &str) -> bool {
    matches!(kind, "call_expression" | "call" | "macro_invocation")
}

fn extract_def_name<'a>(node: tree_sitter::Node<'a>, bytes: &'a [u8]) -> Option<&'a str> {
    if node.kind() == "impl_item" {
        let ty = node.child_by_field_name("type")?;
        return tail_ident_text(ty, bytes);
    }
    if node.kind() == "variable_declarator" {
        // Only treat variable_declarator as a top-level/export or function-valued definition.
        let val = node.child_by_field_name("value");
        let is_fn = val.is_some_and(|v| {
            matches!(
                v.kind(),
                "arrow_function" | "function_expression" | "function"
            )
        });
        let top = node
            .parent()
            .and_then(|p| p.parent())
            .is_some_and(|gp| matches!(gp.kind(), "program" | "export_statement"));
        if !is_fn && !top {
            return None;
        }
    }
    let name_node = node.child_by_field_name("name")?;
    tail_ident_text(name_node, bytes)
}

fn extract_call_name<'a>(node: tree_sitter::Node<'a>, bytes: &'a [u8]) -> Option<&'a str> {
    let target = if node.kind() == "macro_invocation" {
        node.child_by_field_name("macro")?
    } else {
        node.child_by_field_name("function")?
    };
    tail_ident_text(target, bytes)
}

fn tail_ident_text<'a>(node: tree_sitter::Node<'a>, bytes: &'a [u8]) -> Option<&'a str> {
    let txt = node.utf8_text(bytes).ok()?.trim();
    let head = txt.split('<').next().unwrap_or(txt).trim();
    let tail = head
        .rsplit(['.', ':', '>'])
        .find(|s| !s.is_empty())
        .unwrap_or(head)
        .trim();
    (!tail.is_empty()).then_some(tail)
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

    #[test]
    fn tree_sitter_extracts_defs_calls_refs_and_repo_map() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join(".gitignore"), "ignored_dir/\n").unwrap();
        fs::create_dir_all(root.join("ignored_dir")).unwrap();
        fs::write(root.join("ignored_dir/skip.rs"), "fn ignored_fn() {}\n").unwrap();
        fs::write(
            root.join("src/main.rs"),
            "struct Config { port: u16 }\nfn helper(cfg: Config) {}\nfn main() { helper(Config { port: 1 }); }\n",
        )
        .unwrap();
        fs::write(
            root.join("src/app.ts"),
            "export interface User { id: string }\nexport const greet = (u: User) => u.id;\n",
        )
        .unwrap();

        let all = collect_workspace_symbols_all(root);
        assert!(
            all.iter()
                .any(|h| h.name == "helper" && h.kind == SymKind::Def)
        );
        assert!(
            all.iter()
                .any(|h| h.name == "helper" && h.kind == SymKind::Call)
        );
        assert!(
            all.iter()
                .any(|h| h.name == "Config" && h.kind == SymKind::Ref)
        );
        assert!(
            all.iter()
                .any(|h| h.name == "User" && h.kind == SymKind::Def)
        );
        assert!(
            all.iter()
                .any(|h| h.name == "greet" && h.kind == SymKind::Def)
        );
        assert!(!all.iter().any(|h| h.name == "ignored_fn"));

        let map = build_repo_map(root, 800).expect("repo map built");
        assert!(map.contains("src/"));
        assert!(map.contains("src/main.rs"));
        let tokens = O200kCounter::new().unwrap().count_sync(&map);
        assert!(tokens <= 800, "repo_map was {tokens} tokens");
    }
}
