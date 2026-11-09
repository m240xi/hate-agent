//! Dynamic skills: markdown knowledge files the agent authors itself.
//!
//! A skill is a small markdown document with frontmatter (name, title,
//! trigger keywords) stored under `<config>/hate-agent/skills/`. The
//! session prompt carries a compact *index* of every skill plus the
//! full body of any skill whose triggers match the workspace signals;
//! the model can pull the rest on demand via `skill_view` and author
//! new ones via `skill_save`. A background reflection pass can also
//! write one automatically after enough conversation.
//!
//! Token discipline: the index is one line per skill (bounded count),
//! inlined bodies are byte-capped, and nothing is injected unless it
//! matches — an empty skill library costs ~40 tokens of guidance.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use hate_context::O200kCounter;
use serde_json::json;

use crate::provider::ToolSpec;
use crate::tools::{SENT_CAP, ToolOutput, shape};
use crate::types::ToolCall;

/// Skills included in the index, max.
const MAX_INDEX: usize = 40;
/// Bytes of matched skill bodies inlined into the prompt.
const MAX_INLINE_BYTES: usize = 6 * 1024;
/// Matched skills inlined, max.
const MAX_INLINE_SKILLS: usize = 2;
/// Stored skill body cap — a skill bigger than this is a document, not
/// a prompt injection.
pub const MAX_SKILL_BYTES: usize = 16 * 1024;
/// Minimum body length a reflection result must have to be saved —
/// guards against the model emitting a two-line non-skill.
pub const MIN_SKILL_BYTES: usize = 200;

/// One learned skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// Slug used as filename stem (`roblox-luau`).
    pub name: String,
    /// Human title (`Roblox Luau Development`).
    pub title: String,
    /// Lowercase trigger keywords matched against workspace signals.
    pub triggers: Vec<String>,
    /// Unix seconds of last write.
    pub updated: u64,
    /// Markdown body (no frontmatter).
    pub body: String,
}

impl Skill {
    /// Whether any trigger matches `signals` (both lowercase).
    pub fn matches(&self, signals: &[String]) -> bool {
        self.triggers.iter().any(|t| signals.iter().any(|s| s == t))
    }

    /// One-line index entry for the prompt.
    fn index_line(&self) -> String {
        if self.triggers.is_empty() {
            format!("- {} — {}", self.name, self.title)
        } else {
            format!(
                "- {} — {} (triggers: {})",
                self.name,
                self.title,
                self.triggers.join(", ")
            )
        }
    }

    fn to_file(&self) -> String {
        let mut out = String::with_capacity(self.body.len() + 256);
        out.push_str("---\n");
        let _ = writeln!(out, "name: {}", self.name);
        let _ = writeln!(out, "title: {}", self.title);
        let _ = writeln!(out, "triggers: {}", self.triggers.join(", "));
        let _ = writeln!(out, "updated: {}", self.updated);
        out.push_str("---\n\n");
        out.push_str(&self.body);
        out
    }

    /// Parse a skill file. Tolerant: unknown keys ignored, missing
    /// title falls back to the name, a body without frontmatter still
    /// loads (name from the filename).
    fn parse(name: &str, text: &str) -> Skill {
        let mut title = name.replace('-', " ");
        let mut triggers: Vec<String> = Vec::new();
        let mut updated = 0u64;
        let mut body = text.trim().to_string();

        if let Some(rest) = text.strip_prefix("---") {
            let rest = rest.strip_prefix("\r\n").unwrap_or(&rest[1..]);
            if let Some((head, tail)) = rest.split_once("\n---") {
                let tail = tail
                    .strip_prefix("\r\n")
                    .or_else(|| tail.strip_prefix('\n'))
                    .unwrap_or(tail);
                body = tail.trim().to_string();
                for line in head.lines() {
                    let line = line.trim_end_matches('\r');
                    let Some((k, v)) = line.split_once(':') else {
                        continue;
                    };
                    let v = v.trim();
                    match k.trim().to_ascii_lowercase().as_str() {
                        "title" if !v.is_empty() => title = v.to_string(),
                        "triggers" => {
                            triggers = v
                                .split(',')
                                .map(|t| t.trim().to_ascii_lowercase())
                                .filter(|t| !t.is_empty())
                                .collect();
                        }
                        "updated" => updated = v.parse().unwrap_or(0),
                        _ => {}
                    }
                }
            }
        }

        Skill {
            name: name.to_string(),
            title,
            triggers,
            updated,
            body,
        }
    }
}

/// Directory-backed skill library. All operations hit disk — the file
/// count is tiny and disk stays the single source of truth (the app
/// and CLI may both write).
pub struct SkillStore {
    dir: PathBuf,
    counter: O200kCounter,
}

impl SkillStore {
    /// Bind to `dir` (created lazily on first save).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            counter: O200kCounter::new().expect("o200k ranks load"),
        }
    }

    /// Default location: `<config>/hate-agent/skills`.
    pub fn default_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("hate-agent").join("skills"))
    }

    /// The directory this store writes to.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// All skills, sorted by name. Unreadable files are skipped.
    pub fn load(&self) -> Vec<Skill> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            out.push(Skill::parse(stem, &text));
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// One skill by slug.
    pub fn get(&self, name: &str) -> Option<Skill> {
        let path = self.dir.join(format!("{name}.md"));
        let text = std::fs::read_to_string(path).ok()?;
        Some(Skill::parse(name, &text))
    }

    /// Write a skill. Name is slugged and sanitized — it can never
    /// escape the skills directory.
    ///
    /// # Errors
    /// `Err` with the io message on write failure or an empty name.
    pub fn save(&self, skill: &Skill) -> Result<Skill, String> {
        let name = slugify(&skill.name);
        if name.is_empty() {
            return Err("skill name is empty after sanitizing".to_string());
        }
        let mut skill = skill.clone();
        skill.name.clone_from(&name);
        if skill.body.len() > MAX_SKILL_BYTES {
            skill.body = shape(&skill.body, MAX_SKILL_BYTES);
        }
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("mkdir {}: {e}", self.dir.display()))?;
        let path = self.dir.join(format!("{name}.md"));
        std::fs::write(&path, skill.to_file())
            .map_err(|e| format!("write {}: {e}", path.display()))?;
        Ok(skill)
    }

    /// Remove a skill. Returns whether a file was deleted.
    ///
    /// # Errors
    /// `Err` on io failure other than not-found.
    pub fn delete(&self, name: &str) -> Result<bool, String> {
        let path = self.dir.join(format!("{}.md", slugify(name)));
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(format!("delete {}: {e}", path.display())),
        }
    }

    /// The prompt section: index + bodies of skills matching `signals`.
    /// `None` only when the store is unusable (never — an empty index
    /// still advertises `skill_save`).
    pub fn render_prompt(&self, signals: &[String]) -> String {
        let skills = self.load();
        let mut out = String::with_capacity(2048);
        out.push_str("## Learned skills\n");
        out.push_str(
            "Skills authored by earlier sessions of this agent. Call `skill_view` with a \
             skill's name to load it before working in its domain. When this session teaches \
             durable domain knowledge (commands, pitfalls, conventions), call `skill_save`.\n",
        );
        if skills.is_empty() {
            out.push_str("- (none yet)\n");
            return out;
        }
        for skill in skills.iter().take(MAX_INDEX) {
            out.push_str(&skill.index_line());
            out.push('\n');
        }
        if skills.len() > MAX_INDEX {
            let _ = writeln!(out, "- (+{} more)", skills.len() - MAX_INDEX);
        }

        let matched: Vec<&Skill> = skills.iter().filter(|s| s.matches(signals)).collect();
        if matched.is_empty() {
            return out;
        }
        out.push_str("\nMatching this workspace:\n");
        let mut used = 0usize;
        for skill in matched.into_iter().take(MAX_INLINE_SKILLS) {
            let room = MAX_INLINE_BYTES.saturating_sub(used);
            if room < 512 {
                break;
            }
            let body = shape(&skill.body, room);
            used += body.len();
            let _ = write!(out, "\n### Skill: {}\n{}\n", skill.title, body);
        }
        out
    }

    /// Advertised schemas for the two skill tools.
    pub fn specs(&self) -> Vec<ToolSpec> {
        vec![
            spec(
                "skill_view",
                "Load a learned skill's full markdown by name (see the skills index in the prompt). Args: name (required).",
                json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "skill slug from the index"}
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }),
            ),
            spec(
                "skill_save",
                "Save a durable skill (best practices, commands, pitfalls) for future sessions. Args: title, triggers (keyword array), body (markdown). Overwrites an existing skill with the same name.",
                json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "triggers": {"type": "array", "items": {"type": "string"}},
                        "body": {"type": "string"},
                        "name": {"type": "string", "description": "optional slug; derived from title when omitted"}
                    },
                    "required": ["title", "triggers", "body"],
                    "additionalProperties": false
                }),
            ),
        ]
    }

    /// Execute one skill call. Never fails the turn — errors are
    /// `ok: false` tool content.
    pub fn execute(&self, call: &ToolCall) -> ToolOutput {
        let result = match call.name.as_str() {
            "skill_view" => self.view(call),
            "skill_save" => self.save_call(call),
            other => Err(format!("unknown skill tool {other:?}")),
        };
        self.output_for(result)
    }

    /// Error-shaped output for calls denied by policy — same accounting
    /// path as real results.
    pub fn tool_error(&self, msg: String) -> ToolOutput {
        self.output_for(Err(msg))
    }

    fn view(&self, call: &ToolCall) -> Result<String, String> {
        let name = arg_str(call, "name").ok_or("missing `name`")?;
        let skill = self
            .get(&slugify(&name))
            .ok_or_else(|| format!("no skill named {name:?} — check the index"))?;
        Ok(format!(
            "## Skill: {}\n\n{}",
            skill.title,
            shape(&skill.body, SENT_CAP)
        ))
    }

    fn save_call(&self, call: &ToolCall) -> Result<String, String> {
        let title = arg_str(call, "title").ok_or("missing `title`")?;
        let body = arg_str(call, "body").ok_or("missing `body`")?;
        if body.trim().len() < MIN_SKILL_BYTES {
            return Err(format!(
                "body too short ({} bytes, minimum {MIN_SKILL_BYTES}) — a skill needs real content",
                body.trim().len()
            ));
        }
        let triggers = match call.args.get("triggers") {
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_str())
                .map(|t| t.trim().to_ascii_lowercase())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>(),
            Some(serde_json::Value::String(s)) => s
                .split(',')
                .map(|t| t.trim().to_ascii_lowercase())
                .filter(|t| !t.is_empty())
                .collect(),
            _ => Vec::new(),
        };
        let name = arg_str(call, "name").unwrap_or_else(|| title.clone());
        let saved = self.save(&Skill {
            name,
            title,
            triggers,
            updated: now_secs(),
            body,
        })?;
        Ok(format!(
            "saved skill `{}` ({} bytes) — it joins the prompt index next turn",
            saved.name,
            saved.body.len()
        ))
    }

    fn output_for(&self, result: Result<String, String>) -> ToolOutput {
        match result {
            Ok(full) => {
                let sent = shape(&full, SENT_CAP);
                let preview: String = sent.chars().take(400).collect();
                ToolOutput {
                    full_tokens: self.counter.count_sync(&full) as u32,
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
}

/// Slug a skill name: lowercase alphanumerics and single dashes. Used
/// for filenames, so it is the traversal guard.
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Domain signals from a workspace: bounded walk collecting file
/// extensions and known config filenames, mapped to lowercase tokens.
pub fn workspace_signals(root: &Path) -> Vec<String> {
    const MAX_ENTRIES: usize = 3000;
    const MAX_DEPTH: usize = 3;
    /// Extension → tokens.
    const EXT_TOKENS: &[(&str, &[&str])] = &[
        ("lua", &["lua", "roblox"]),
        ("luau", &["luau", "roblox"]),
        ("rbxl", &["roblox"]),
        ("rbxlx", &["roblox"]),
        ("rbxm", &["roblox"]),
        ("rs", &["rust"]),
        ("ts", &["typescript", "web"]),
        ("tsx", &["typescript", "react", "web"]),
        ("js", &["javascript", "web"]),
        ("jsx", &["javascript", "react", "web"]),
        ("svelte", &["svelte", "web"]),
        ("vue", &["vue", "web"]),
        ("css", &["css", "web"]),
        ("html", &["html", "web"]),
        ("py", &["python"]),
        ("go", &["go"]),
        ("cs", &["dotnet", "csharp"]),
        ("java", &["java"]),
        ("kt", &["kotlin", "jvm"]),
        ("swift", &["swift"]),
        ("dart", &["dart", "flutter"]),
        ("sql", &["sql"]),
        ("sh", &["shell"]),
    ];
    /// Exact filenames → tokens.
    const FILE_TOKENS: &[(&str, &[&str])] = &[
        ("Cargo.toml", &["rust", "cargo"]),
        ("tauri.conf.json", &["tauri"]),
        ("package.json", &["node", "web"]),
        ("pyproject.toml", &["python"]),
        ("requirements.txt", &["python"]),
        ("go.mod", &["go"]),
        ("pubspec.yaml", &["dart", "flutter"]),
        ("default.project.json", &["roblox", "rojo"]),
        ("dockerfile", &["docker"]),
    ];

    let mut tokens: BTreeSet<String> = BTreeSet::new();
    let mut seen = 0usize;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return tokens.into_iter().collect();
            }
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let lower = name.to_ascii_lowercase();
            if lower == ".git" || lower == "node_modules" || lower == "target" || lower == "dist" {
                continue;
            }
            if path.is_dir() {
                stack.push((path, depth + 1));
                continue;
            }
            for (file, toks) in FILE_TOKENS {
                if name.eq_ignore_ascii_case(file) {
                    tokens.extend(toks.iter().map(|t| (*t).to_string()));
                }
            }
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let ext = ext.to_ascii_lowercase();
                for (e, toks) in EXT_TOKENS {
                    if *e == ext {
                        tokens.extend(toks.iter().map(|t| (*t).to_string()));
                    }
                }
            }
        }
    }
    tokens.into_iter().collect()
}

/// Keyword tokens from free text — lowercase words of 3+ chars.
pub fn text_signals(text: &str) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    let mut word = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            word.push(c.to_ascii_lowercase());
        } else if !word.is_empty() {
            if word.len() >= 3 {
                out.insert(std::mem::take(&mut word));
            } else {
                word.clear();
            }
            if out.len() >= 60 {
                break;
            }
        }
    }
    if word.len() >= 3 && out.len() < 60 {
        out.insert(word);
    }
    out.into_iter().collect()
}

/// System prompt for the background reflection call.
pub const REFLECTION_SYSTEM: &str = "\
You write compact skill files for an AI coding agent. Given a digest of a \
session, produce a durable skill capturing what was actually used: commands, \
APIs, conventions, and pitfalls. Be specific and actionable; skip filler. \
Output exactly three parts and nothing else:\n\
TITLE: <short human title>\n\
TRIGGERS: <comma-separated lowercase keywords for matching future workspaces>\n\
<body>\n\
The body is markdown, 40-120 lines.";

/// Parse a reflection result into `(title, triggers, body)`.
pub fn parse_reflection(text: &str) -> Option<(String, Vec<String>, String)> {
    let mut title = None;
    let mut triggers = Vec::new();
    let mut body_start = 0usize;
    let mut offset = 0usize;
    for line in text.lines() {
        let trimmed = line.trim();
        let upper = trimmed.to_ascii_uppercase();
        // `to_ascii_uppercase` preserves byte length, so fixed offsets
        // into the original line stay valid.
        if upper.starts_with("TITLE:") {
            title = Some(trimmed[6..].trim().to_string());
        } else if upper.starts_with("TRIGGERS:") {
            triggers = trimmed[9..]
                .split(',')
                .map(|t| t.trim().to_ascii_lowercase())
                .filter(|t| !t.is_empty())
                .collect();
        } else {
            body_start = offset;
            break;
        }
        offset += line.len() + 1;
    }
    let body = text.get(body_start..)?.trim().to_string();
    let title = title.filter(|t| !t.is_empty())?;
    if body.len() < MIN_SKILL_BYTES || triggers.is_empty() {
        return None;
    }
    Some((title, triggers, body))
}

/// Compact digest of a session for the reflection call: the user's
/// asks, tools used, and detected signals.
pub fn build_digest(messages: &[crate::provider::Message], signals: &[String]) -> String {
    let mut user_lines: Vec<String> = Vec::new();
    let mut tools: BTreeSet<String> = BTreeSet::new();
    for m in messages {
        match m {
            crate::provider::Message::User { content } => {
                user_lines.push(truncate_chars(content, 400));
            }
            crate::provider::Message::Assistant { tool_calls, .. } => {
                for call in tool_calls {
                    tools.insert(call.name.as_str().to_string());
                }
            }
            crate::provider::Message::Tool { .. } => {}
        }
    }
    let start = user_lines.len().saturating_sub(8);
    let mut out = String::with_capacity(2048);
    if !signals.is_empty() {
        let _ = writeln!(out, "Workspace signals: {}", signals.join(", "));
    }
    let _ = writeln!(
        out,
        "Tools used: {}",
        if tools.is_empty() {
            "(none)".to_string()
        } else {
            tools.into_iter().collect::<Vec<_>>().join(", ")
        }
    );
    out.push_str("User asks (oldest first):\n");
    for line in &user_lines[start..] {
        let _ = writeln!(out, "- {line}");
    }
    out
}

fn truncate_chars(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out.replace('\n', " ")
}

/// Unix seconds — shared with the session's reflection pass.
pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn spec(name: &'static str, description: &'static str, schema: serde_json::Value) -> ToolSpec {
    ToolSpec {
        name: crate::types::ToolName::new(name),
        description: description.to_string(),
        input_schema: schema,
    }
}

fn arg_str(call: &ToolCall, key: &str) -> Option<String> {
    call.args.get(key)?.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn temp_store() -> (SkillStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "hate-skills-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        (SkillStore::new(dir.clone()), dir)
    }

    fn sample(name: &str, triggers: &[&str]) -> Skill {
        Skill {
            name: name.to_string(),
            title: format!("{name} skill"),
            triggers: triggers.iter().map(|t| (*t).to_string()).collect(),
            updated: 1,
            body: format!("# {name}\n\n{}", "Useful convention. ".repeat(20)),
        }
    }

    #[test]
    fn frontmatter_roundtrips() {
        let (store, dir) = temp_store();
        let saved = store
            .save(&sample("roblox-luau", &["roblox", "luau"]))
            .unwrap();
        let loaded = store.get("roblox-luau").unwrap();
        assert_eq!(loaded.title, saved.title);
        assert_eq!(loaded.triggers, vec!["roblox", "luau"]);
        assert!(loaded.body.contains("Useful convention."));
        assert_eq!(loaded.updated, saved.updated);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn slugify_blocks_traversal() {
        assert_eq!(slugify("../../etc/passwd"), "etc-passwd");
        assert_eq!(slugify("Roblox Luau!"), "roblox-luau");
        assert_eq!(slugify("  "), "");
    }

    #[test]
    fn index_and_matching() {
        let (store, dir) = temp_store();
        store
            .save(&sample("roblox-luau", &["roblox", "luau"]))
            .unwrap();
        store
            .save(&sample("rust-tauri", &["rust", "tauri"]))
            .unwrap();

        let prompt = store.render_prompt(&["roblox".to_string(), "luau".to_string()]);
        assert!(prompt.contains("- roblox-luau — roblox-luau skill"));
        assert!(prompt.contains("### Skill: roblox-luau skill"));
        assert!(!prompt.contains("### Skill: rust-tauri skill"));
        // Index always lists both; only the match is inlined.
        assert!(prompt.contains("- rust-tauri — rust-tauri skill"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_store_advertises_save() {
        let (store, dir) = temp_store();
        let prompt = store.render_prompt(&[]);
        assert!(prompt.contains("skill_save"));
        assert!(prompt.contains("(none yet)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_signals_map_domains() {
        let dir = std::env::temp_dir().join(format!("hate-skills-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
        let signals = workspace_signals(&dir);
        assert!(signals.contains(&"rust".to_string()));
        assert!(signals.contains(&"cargo".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reflection_parse_rejects_short_and_missing_meta() {
        assert!(parse_reflection("TITLE: x\nTRIGGERS: a\ntoo short").is_none());
        assert!(parse_reflection(&format!("no meta\n{}", "x".repeat(300))).is_none());
        let ok = format!(
            "TITLE: Roblox Luau\nTRIGGERS: roblox, luau\n{}",
            "Do this. ".repeat(40)
        );
        let (title, triggers, body) = parse_reflection(&ok).unwrap();
        assert_eq!(title, "Roblox Luau");
        assert_eq!(triggers, vec!["roblox", "luau"]);
        assert!(body.starts_with("Do this."));
    }

    #[test]
    fn delete_removes_file() {
        let (store, dir) = temp_store();
        store.save(&sample("temp-skill", &["temp"])).unwrap();
        assert!(store.delete("temp-skill").unwrap());
        assert!(!store.delete("temp-skill").unwrap());
        assert!(store.get("temp-skill").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
