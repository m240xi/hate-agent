use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::provider::Message;
use crate::tools::PROJECT_INSTRUCTIONS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandClass {
    Safe,
    Write,
    Network,
    Destructive,
}

impl CommandClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::Safe => "Safe",
            Self::Write => "Write",
            Self::Network => "Network",
            Self::Destructive => "Destructive",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandClassification {
    pub class: CommandClass,
    pub reason: String,
}

#[derive(Debug, Clone)]
struct ShellSegment {
    tokens: Vec<String>,
    has_write_redirect: bool,
    subshells: Vec<String>,
}

/// Classify a shell command string by parsing chains, pipes, quotes, redirections,
/// wrappers, and subshells into `Safe` / `Write` / `Network` / `Destructive`.
pub fn classify_bash(command: &str) -> CommandClassification {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return CommandClassification {
            class: CommandClass::Safe,
            reason: "empty command".to_string(),
        };
    }

    let segments = split_shell_segments(trimmed);
    if segments.is_empty() {
        return CommandClassification {
            class: CommandClass::Write,
            reason: "unparsed shell expression".to_string(),
        };
    }

    let mut worst = CommandClassification {
        class: CommandClass::Safe,
        reason: "read-only / check command".to_string(),
    };

    for seg in segments {
        for sub in &seg.subshells {
            let sub_cls = classify_bash(sub);
            if sub_cls.class > worst.class {
                worst = sub_cls;
            }
        }
        let seg_cls = classify_single_segment(&seg);
        if seg_cls.class > worst.class {
            worst = seg_cls;
        }
        if worst.class == CommandClass::Destructive {
            return worst;
        }
    }
    worst
}

fn split_shell_segments(input: &str) -> Vec<ShellSegment> {
    let chars: Vec<char> = input.chars().collect();
    let mut segments = Vec::new();
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut has_write_redirect = false;
    let mut subshells = Vec::new();
    let mut i = 0usize;
    let mut quote: Option<char> = None;

    let push_tok = |cur: &mut String, tokens: &mut Vec<String>| {
        if !cur.is_empty() {
            tokens.push(std::mem::take(cur));
        }
    };

    let flush_seg = |tokens: &mut Vec<String>,
                     has_write_redirect: &mut bool,
                     subshells: &mut Vec<String>,
                     segments: &mut Vec<ShellSegment>| {
        if !tokens.is_empty() || *has_write_redirect || !subshells.is_empty() {
            segments.push(ShellSegment {
                tokens: std::mem::take(tokens),
                has_write_redirect: *has_write_redirect,
                subshells: std::mem::take(subshells),
            });
            *has_write_redirect = false;
        }
    };

    while i < chars.len() {
        let ch = chars[i];
        if let Some(q) = quote {
            if ch == '\\' && q == '"' && i + 1 < chars.len() {
                cur.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if ch == q {
                quote = None;
                i += 1;
                continue;
            }
            if q == '"' && ch == '$' && i + 1 < chars.len() && chars[i + 1] == '(' {
                let (inner, next_i) = extract_paren_subshell(&chars, i + 2);
                subshells.push(inner);
                i = next_i;
                continue;
            }
            if q == '"' && ch == '`' {
                let (inner, next_i) = extract_backtick(&chars, i + 1);
                subshells.push(inner);
                i = next_i;
                continue;
            }
            cur.push(ch);
            i += 1;
            continue;
        }

        if ch == '\\' && i + 1 < chars.len() {
            cur.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            i += 1;
            continue;
        }
        if ch == '$' && i + 1 < chars.len() && chars[i + 1] == '(' {
            let (inner, next_i) = extract_paren_subshell(&chars, i + 2);
            subshells.push(inner);
            cur.push_str("$()");
            i = next_i;
            continue;
        }
        if ch == '`' {
            let (inner, next_i) = extract_backtick(&chars, i + 1);
            subshells.push(inner);
            cur.push_str("``");
            i = next_i;
            continue;
        }
        if ch.is_whitespace() {
            push_tok(&mut cur, &mut tokens);
            i += 1;
            continue;
        }
        if ch == ';' || ch == '\n' {
            push_tok(&mut cur, &mut tokens);
            flush_seg(
                &mut tokens,
                &mut has_write_redirect,
                &mut subshells,
                &mut segments,
            );
            i += 1;
            continue;
        }
        if ch == '|' || ch == '&' {
            if ch == '&' && i + 1 < chars.len() && chars[i + 1] == '>' {
                push_tok(&mut cur, &mut tokens);
                let (is_null, next_i) = peek_redirect_null(&chars, i + 2);
                if !is_null {
                    has_write_redirect = true;
                }
                i = next_i;
                continue;
            }
            if i + 1 < chars.len() && chars[i + 1] == ch {
                i += 2;
            } else {
                i += 1;
            }
            push_tok(&mut cur, &mut tokens);
            flush_seg(
                &mut tokens,
                &mut has_write_redirect,
                &mut subshells,
                &mut segments,
            );
            continue;
        }
        if ch == '>' {
            // Treat `1>` / `2>` as redirection too while preserving harmless `2>&1` or `>/dev/null` / `>nul`.
            if cur == "1" || cur == "2" {
                cur.clear();
            } else {
                push_tok(&mut cur, &mut tokens);
            }
            let after_op = if i + 1 < chars.len() && (chars[i + 1] == '>' || chars[i + 1] == '|') {
                i + 2
            } else {
                i + 1
            };
            let (is_null, next_i) = peek_redirect_null(&chars, after_op);
            if !is_null {
                has_write_redirect = true;
            }
            i = next_i;
            continue;
        }
        cur.push(ch);
        i += 1;
    }

    push_tok(&mut cur, &mut tokens);
    flush_seg(
        &mut tokens,
        &mut has_write_redirect,
        &mut subshells,
        &mut segments,
    );
    segments
}

fn extract_paren_subshell(chars: &[char], mut i: usize) -> (String, usize) {
    let mut depth = 1usize;
    let mut inner = String::new();
    while i < chars.len() {
        let ch = chars[i];
        if ch == '(' {
            depth += 1;
            inner.push(ch);
        } else if ch == ')' {
            depth -= 1;
            if depth == 0 {
                return (inner, i + 1);
            }
            inner.push(ch);
        } else {
            inner.push(ch);
        }
        i += 1;
    }
    (inner, i)
}

fn extract_backtick(chars: &[char], mut i: usize) -> (String, usize) {
    let mut inner = String::new();
    while i < chars.len() {
        if chars[i] == '`' {
            return (inner, i + 1);
        }
        inner.push(chars[i]);
        i += 1;
    }
    (inner, i)
}

fn peek_redirect_null(chars: &[char], mut i: usize) -> (bool, usize) {
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    let mut target = String::new();
    while i < chars.len() && !chars[i].is_whitespace() && !matches!(chars[i], ';' | '|' | '&') {
        target.push(chars[i]);
        i += 1;
    }
    let low = target.to_ascii_lowercase();
    let harmless = low == "/dev/null" || low == "nul" || low == "&1" || low == "&2";
    (harmless, i)
}

fn unwrap_command_tokens(mut toks: &[String]) -> &[String] {
    loop {
        if toks.is_empty() {
            return toks;
        }
        // Skip leading KEY=VALUE env assignments.
        if let Some(first) = toks.first()
            && let Some((k, _)) = first.split_once('=')
            && !k.is_empty()
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            toks = &toks[1..];
            continue;
        }
        let base = cmd_basename(&toks[0]);
        match base.as_str() {
            "env" | "nohup" => {
                let mut skip = 1;
                while skip < toks.len() && toks[skip].contains('=') {
                    skip += 1;
                }
                toks = &toks[skip..];
            }
            "nice" | "timeout" => {
                let mut skip = 1;
                if skip < toks.len()
                    && (toks[skip].starts_with('-')
                        || toks[skip].chars().all(|c| c.is_ascii_digit()))
                {
                    skip += 1;
                }
                toks = &toks[skip..];
            }
            _ => return toks,
        }
    }
}

fn cmd_basename(raw: &str) -> String {
    let file = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let low = file.to_ascii_lowercase();
    low.strip_suffix(".exe")
        .or_else(|| low.strip_suffix(".cmd"))
        .or_else(|| low.strip_suffix(".bat"))
        .unwrap_or(&low)
        .to_string()
}

fn classify_single_segment(seg: &ShellSegment) -> CommandClassification {
    let toks = unwrap_command_tokens(&seg.tokens);
    if toks.is_empty() {
        return if seg.has_write_redirect {
            CommandClassification {
                class: CommandClass::Write,
                reason: "writes via shell redirection".to_string(),
            }
        } else {
            CommandClassification {
                class: CommandClass::Safe,
                reason: "empty subcommand".to_string(),
            }
        };
    }

    let base = cmd_basename(&toks[0]);
    let args = &toks[1..];

    // Wrapper invocations carrying a nested command string: `cmd /c "..."`, `sh -c "..."`, `powershell -Command "..."`.
    if matches!(
        base.as_str(),
        "cmd" | "sh" | "bash" | "zsh" | "pwsh" | "powershell"
    ) {
        for (idx, a) in args.iter().enumerate() {
            let low = a.to_ascii_lowercase();
            if matches!(low.as_str(), "/c" | "-c" | "-command" | "-encodedcommand")
                && let Some(inner) = args.get(idx + 1)
            {
                let mut inner_cls = classify_bash(inner);
                if seg.has_write_redirect && inner_cls.class < CommandClass::Write {
                    inner_cls.class = CommandClass::Write;
                    inner_cls.reason = "shell redirection".to_string();
                }
                return inner_cls;
            }
        }
    }

    if is_destructive_cmd(&base, args) {
        return CommandClassification {
            class: CommandClass::Destructive,
            reason: format!("destructive `{base}` operation"),
        };
    }

    if is_network_cmd(&base, args) {
        return CommandClassification {
            class: CommandClass::Network,
            reason: format!("network-capable `{base}` operation"),
        };
    }

    let mut cls = if is_safe_cmd(&base, args) {
        CommandClassification {
            class: CommandClass::Safe,
            reason: format!("read-only `{base}` command"),
        }
    } else {
        CommandClassification {
            class: CommandClass::Write,
            reason: format!("command `{base}` may mutate workspace state"),
        }
    };

    if seg.has_write_redirect && cls.class < CommandClass::Write {
        cls.class = CommandClass::Write;
        cls.reason = "writes via shell redirection".to_string();
    }
    cls
}

fn has_short_flag(args: &[String], needle: char) -> bool {
    args.iter().any(|a| {
        if let Some(rest) = a.strip_prefix('-')
            && !rest.starts_with('-')
        {
            return rest.chars().any(|c| c == needle);
        }
        false
    })
}

fn is_destructive_cmd(base: &str, args: &[String]) -> bool {
    match base {
        "rm" => {
            has_short_flag(args, 'r')
                || has_short_flag(args, 'R')
                || has_short_flag(args, 'f')
                || args
                    .iter()
                    .any(|a| matches!(a.as_str(), "--recursive" | "--force" | "/" | "/*" | ".."))
        }
        "rmdir" | "shred" | "dd" | "fdisk" | "diskpart" | "format" | "shutdown" | "reboot"
        | "halt" | "poweroff" | "pkill" | "killall" => true,
        m if m.starts_with("mkfs") => true,
        "del" | "erase" | "rd" => args.iter().any(|a| {
            matches!(
                a.to_ascii_lowercase().as_str(),
                "/s" | "/f" | "/q" | "-recurse" | "-force"
            )
        }),
        "remove-item" | "ri" => args.iter().any(|a| {
            matches!(
                a.to_ascii_lowercase().as_str(),
                "-recurse" | "-force" | "-r" | "-f"
            )
        }),
        "taskkill" => args.iter().any(|a| a.eq_ignore_ascii_case("/f")),
        "kill" => args.iter().any(|a| a == "-9" || a == "-KILL"),
        "stop-process" => args.iter().any(|a| a.eq_ignore_ascii_case("-force")),
        "chmod" | "chown" => {
            has_short_flag(args, 'r')
                || has_short_flag(args, 'R')
                || args.iter().any(|a| a == "--recursive")
        }
        "git" => is_destructive_git(args),
        _ => false,
    }
}

fn is_destructive_git(args: &[String]) -> bool {
    let Some(sub) = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str)
    else {
        return false;
    };
    match sub {
        "reset" => args.iter().any(|a| a == "--hard"),
        "clean" => has_short_flag(args, 'f') || args.iter().any(|a| a == "--force"),
        "push" => args
            .iter()
            .any(|a| matches!(a.as_str(), "-f" | "--force" | "--force-with-lease")),
        "checkout" => args.iter().any(|a| a == "--"),
        "switch" => args.iter().any(|a| a == "--discard-changes"),
        "restore" => args.iter().any(|a| a == "--worktree" || a == "--"),
        "branch" => args.iter().any(|a| a == "-D"),
        _ => false,
    }
}

fn is_network_cmd(base: &str, args: &[String]) -> bool {
    match base {
        "curl" | "wget" | "xh" | "http" | "ssh" | "scp" | "sftp" | "nc" | "ncat" | "netcat"
        | "telnet" | "ftp" | "ping" | "invoke-webrequest" | "iwr" | "invoke-restmethod" | "irm"
        | "start-bitstransfer" | "gh" | "rustup" => true,
        "rsync" => args.iter().any(|a| a.contains(':')),
        "git" => matches!(
            args.iter()
                .find(|a| !a.starts_with('-'))
                .map(String::as_str),
            Some("push" | "pull" | "fetch" | "clone" | "ls-remote" | "remote")
        ),
        "npm" | "pnpm" | "yarn" => matches!(
            args.iter()
                .find(|a| !a.starts_with('-'))
                .map(String::as_str),
            Some("install" | "i" | "ci" | "add" | "update" | "publish" | "dlx")
        ),
        "cargo" => matches!(
            args.iter()
                .find(|a| !a.starts_with('-'))
                .map(String::as_str),
            Some("install" | "publish" | "update")
        ),
        "pip" | "pip3" | "uv" => args
            .iter()
            .any(|a| matches!(a.as_str(), "install" | "sync" | "download")),
        "go" => matches!(
            args.iter()
                .find(|a| !a.starts_with('-'))
                .map(String::as_str),
            Some("get")
        ),
        _ => false,
    }
}

fn is_safe_cmd(base: &str, args: &[String]) -> bool {
    match base {
        "ls" | "dir" | "pwd" | "echo" | "printf" | "which" | "where" | "whoami" | "hostname"
        | "uname" | "date" | "wc" | "sort" | "uniq" | "cut" | "tr" | "diff" | "head" | "tail"
        | "cat" | "type" | "less" | "more" | "rg" | "grep" | "findstr" | "select-string"
        | "get-childitem" | "get-content" | "get-location" | "tree" | "true" | "false" | "test"
        | "[" | "pytest" | "svelte-check" => true,
        "find" => !args.iter().any(|a| {
            matches!(
                a.as_str(),
                "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir"
            )
        }),
        "sed" => !has_short_flag(args, 'i') && !args.iter().any(|a| a.starts_with("--in-place")),
        "git" => {
            matches!(
                args.iter()
                    .find(|a| !a.starts_with('-'))
                    .map(String::as_str),
                Some(
                    "status"
                        | "diff"
                        | "log"
                        | "show"
                        | "rev-parse"
                        | "ls-files"
                        | "blame"
                        | "describe"
                        | "shortlog"
                        | "cat-file"
                )
            ) || (args
                .iter()
                .find(|a| !a.starts_with('-'))
                .map(String::as_str)
                == Some("branch")
                && !args
                    .iter()
                    .any(|a| matches!(a.as_str(), "-d" | "-D" | "-m" | "-M")))
        }
        "cargo" => match args
            .iter()
            .find(|a| !a.starts_with('-'))
            .map(String::as_str)
        {
            Some("check" | "test" | "clippy" | "metadata" | "tree" | "doc" | "version") => true,
            Some("fmt") => args.iter().any(|a| a == "--check"),
            None => args.iter().any(|a| a == "--version" || a == "-V"),
            _ => false,
        },
        "npm" | "pnpm" => {
            let sub = args
                .iter()
                .find(|a| !a.starts_with('-'))
                .map(String::as_str);
            match sub {
                Some("test") => true,
                Some("run") => args.iter().any(|a| {
                    matches!(
                        a.as_str(),
                        "test" | "check" | "lint" | "typecheck" | "format:check"
                    )
                }),
                _ => false,
            }
        }
        "go" => matches!(
            args.iter()
                .find(|a| !a.starts_with('-'))
                .map(String::as_str),
            Some("test" | "vet" | "version")
        ),
        "tsc" => args.iter().any(|a| a == "--noEmit"),
        "rustc" | "node" | "python" | "python3" => args
            .iter()
            .any(|a| a == "--version" || a == "-V" || a == "-v"),
        _ => false,
    }
}

struct RedactRule {
    re: Regex,
    replacement: &'static str,
}

fn secret_rules() -> &'static [RedactRule] {
    static RULES: OnceLock<Vec<RedactRule>> = OnceLock::new();
    RULES.get_or_init(|| {
        vec![
            RedactRule {
                re: Regex::new(
                    r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
                )
                .expect("private key regex"),
                replacement: "[REDACTED:PRIVATE_KEY]",
            },
            RedactRule {
                re: Regex::new(r"\bsk-(?:proj-|ant-|live-|test-)?[A-Za-z0-9_-]{20,}\b")
                    .expect("sk key regex"),
                replacement: "[REDACTED:API_KEY]",
            },
            RedactRule {
                re: Regex::new(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{30,}\b")
                    .expect("gh token regex"),
                replacement: "[REDACTED:GITHUB_TOKEN]",
            },
            RedactRule {
                re: Regex::new(r"\bgithub_pat_[A-Za-z0-9_]{22,}\b")
                    .expect("gh pat regex"),
                replacement: "[REDACTED:GITHUB_TOKEN]",
            },
            RedactRule {
                re: Regex::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b").expect("aws key regex"),
                replacement: "[REDACTED:AWS_ACCESS_KEY]",
            },
            RedactRule {
                re: Regex::new(r"\bxox[baprs]-[0-9A-Za-z-]{10,}\b").expect("slack token regex"),
                replacement: "[REDACTED:SLACK_TOKEN]",
            },
        ]
    })
}

#[derive(Debug, Clone, Default)]
pub struct SecretRedactor {
    pub allowlist: Vec<String>,
}

impl SecretRedactor {
    pub fn new(allowlist: Vec<String>) -> Self {
        Self { allowlist }
    }

    pub fn redact(&self, text: &str) -> (String, usize) {
        let mut cur = text.to_string();
        let mut total = 0usize;
        for rule in secret_rules() {
            if !rule.re.is_match(&cur) {
                continue;
            }
            let mut count = 0usize;
            let replaced = rule.re.replace_all(&cur, |caps: &regex::Captures<'_>| {
                let matched = caps.get(0).map_or("", |m| m.as_str());
                if self
                    .allowlist
                    .iter()
                    .any(|allow| !allow.is_empty() && matched.contains(allow))
                {
                    matched.to_string()
                } else {
                    count += 1;
                    rule.replacement.to_string()
                }
            });
            if count > 0 {
                cur = replaced.into_owned();
                total += count;
            }
        }
        (cur, total)
    }
}

pub fn redact_secrets(text: &str) -> (String, usize) {
    SecretRedactor::default().redact(text)
}

pub fn redact_chat_request(
    redactor: &SecretRedactor,
    system: &mut Option<String>,
    messages: &mut [Message],
) -> usize {
    let mut total = 0usize;
    if let Some(sys) = system {
        let (clean, n) = redactor.redact(sys);
        if n > 0 {
            *sys = clean;
            total += n;
        }
    }
    for msg in messages {
        let text = match msg {
            Message::User { content, .. }
            | Message::Assistant { content, .. }
            | Message::Tool { content, .. } => content,
        };
        let (clean, n) = redactor.redact(text);
        if n > 0 {
            *text = clean;
            total += n;
        }
    }
    total
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TrustFile {
    #[serde(default)]
    trusted_roots: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoTrustReport {
    pub trusted: bool,
    pub canonical_root: String,
    pub rule_files: Vec<String>,
    pub mcp_file: Option<String>,
    pub mcp_servers: Vec<String>,
}

impl RepoTrustReport {
    pub fn has_repo_config(&self) -> bool {
        !self.rule_files.is_empty() || self.mcp_file.is_some()
    }
}

pub struct TrustStore {
    path: PathBuf,
    roots: BTreeSet<String>,
}

impl TrustStore {
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("hate-agent")
            .join("trusted.json")
    }

    pub fn load() -> Self {
        Self::load_from(Self::default_path())
    }

    pub fn load_from(path: PathBuf) -> Self {
        let roots = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<TrustFile>(&s).ok())
            .map_or_default(|f| f.trusted_roots);
        Self { path, roots }
    }

    pub fn is_trusted(&self, root: &Path) -> bool {
        let key = canonical_key(root);
        self.roots.contains(&key)
    }

    pub fn set_trusted(&mut self, root: &Path, trusted: bool) -> Result<(), String> {
        let key = canonical_key(root);
        if trusted {
            self.roots.insert(key);
        } else {
            self.roots.remove(&key);
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let data = TrustFile {
            trusted_roots: self.roots.clone(),
        };
        let json = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, json).map_err(|e| e.to_string())
    }
}

pub fn inspect_repo_trust(root: &Path, store: &TrustStore) -> RepoTrustReport {
    let canonical = canonical_key(root);
    let mut rule_files = Vec::new();
    for rel in PROJECT_INSTRUCTIONS {
        let p = root.join(rel);
        if p.is_file() {
            rule_files.push((*rel).to_string());
        }
    }
    let mcp_path = root.join(".mcp.json");
    let (mcp_file, mcp_servers) = if mcp_path.is_file() {
        let servers = parse_repo_mcp_server_names(&mcp_path);
        (Some(".mcp.json".to_string()), servers)
    } else {
        (None, Vec::new())
    };
    let trusted = store.is_trusted(root) || (rule_files.is_empty() && mcp_file.is_none());
    RepoTrustReport {
        trusted,
        canonical_root: canonical,
        rule_files,
        mcp_file,
        mcp_servers,
    }
}

fn parse_repo_mcp_server_names(path: &Path) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    if let Some(obj) = val.get("mcpServers").and_then(|v| v.as_object()) {
        return obj.keys().cloned().collect();
    }
    if let Some(arr) = val.get("servers").and_then(|v| v.as_array()) {
        return arr
            .iter()
            .filter_map(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_string))
            .collect();
    }
    Vec::new()
}

fn canonical_key(root: &Path) -> String {
    let canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let s = canon.to_string_lossy().replace('\\', "/");
    s.strip_prefix("//?/").unwrap_or(&s).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_safe_write_network_and_destructive_commands() {
        assert_eq!(classify_bash("ls -la").class, CommandClass::Safe);
        assert_eq!(
            classify_bash("git status && git diff").class,
            CommandClass::Safe
        );
        assert_eq!(
            classify_bash("cargo test -p hate-core").class,
            CommandClass::Safe
        );
        assert_eq!(
            classify_bash("sed -n '1,10p' foo.rs").class,
            CommandClass::Safe
        );
        assert_eq!(classify_bash("echo hi >nul").class, CommandClass::Safe);

        assert_eq!(
            classify_bash("echo hi > out.txt").class,
            CommandClass::Write
        );
        assert_eq!(
            classify_bash("sed -i 's/a/b/' foo.rs").class,
            CommandClass::Write
        );
        assert_eq!(
            classify_bash("find . -name '*.tmp' -delete").class,
            CommandClass::Write
        );
        assert_eq!(classify_bash("touch foo.txt").class, CommandClass::Write);

        assert_eq!(
            classify_bash("ls && curl https://example.com").class,
            CommandClass::Network
        );
        assert_eq!(
            classify_bash("git pull origin main").class,
            CommandClass::Network
        );
        assert_eq!(
            classify_bash("npm install lodash").class,
            CommandClass::Network
        );

        assert_eq!(
            classify_bash("rm -rf build").class,
            CommandClass::Destructive
        );
        assert_eq!(
            classify_bash("echo ok; git reset --hard HEAD").class,
            CommandClass::Destructive
        );
        assert_eq!(
            classify_bash("echo $(rm -rf /tmp/x)").class,
            CommandClass::Destructive
        );
        assert_eq!(
            classify_bash("cmd /c \"git clean -fdx\"").class,
            CommandClass::Destructive
        );
    }

    #[test]
    fn redacts_common_secret_patterns_with_allowlist() {
        let sample = "key=sk-proj-abcdefghijklmnopqrstuvwxyz1234 gh=ghp_123456789012345678901234567890123456 aws=AKIAIOSFODNN7EXAMPLE";
        let (clean, count) = redact_secrets(sample);
        assert_eq!(count, 3);
        assert!(!clean.contains("sk-proj-"));
        assert!(!clean.contains("ghp_"));
        assert!(!clean.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(clean.contains("[REDACTED:API_KEY]"));
        assert!(clean.contains("[REDACTED:GITHUB_TOKEN]"));
        assert!(clean.contains("[REDACTED:AWS_ACCESS_KEY]"));

        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJ\n-----END RSA PRIVATE KEY-----";
        let (pem_clean, pem_n) = redact_secrets(pem);
        assert_eq!(pem_n, 1);
        assert_eq!(pem_clean, "[REDACTED:PRIVATE_KEY]");

        let allow = SecretRedactor::new(vec!["AKIAIOSFODNN7EXAMPLE".to_string()]);
        let (kept, n) = allow.redact("aws=AKIAIOSFODNN7EXAMPLE");
        assert_eq!(n, 0);
        assert!(kept.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn trust_store_roundtrips_and_inspects_repo_files() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("trusted.json");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("AGENTS.md"), "be careful").unwrap();
        std::fs::write(
            repo.join(".mcp.json"),
            r#"{"mcpServers":{"demo":{"command":"echo"}}}"#,
        )
        .unwrap();

        let mut store = TrustStore::load_from(store_path.clone());
        let before = inspect_repo_trust(&repo, &store);
        assert!(!before.trusted);
        assert!(before.has_repo_config());
        assert_eq!(before.rule_files, vec!["AGENTS.md".to_string()]);
        assert_eq!(before.mcp_servers, vec!["demo".to_string()]);

        store.set_trusted(&repo, true).unwrap();
        let reloaded = TrustStore::load_from(store_path);
        let after = inspect_repo_trust(&repo, &reloaded);
        assert!(after.trusted);
    }
}
