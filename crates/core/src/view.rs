//! Workspace views and safe workspace/git actions for the desktop UI.
//!
//! Every path here is workspace-relative and passes the same containment guard
//! the agent's own file tools use, so the UI can never escape the folder
//! the user picked.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write as _};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;

/// Refuse to render or write files larger than this; the UI shows a "too large" notice.
pub const MAX_VIEW_BYTES: u64 = 4 * 1024 * 1024;
/// Hard ceiling on lines returned in one peek.
pub const MAX_VIEW_LINES: usize = 4000;
/// Lines returned when the caller does not ask for a window.
pub const DEFAULT_VIEW_LINES: usize = 400;
/// Cap on a rendered diff body so a huge patch cannot stall the webview.
const MAX_DIFF_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct FileView {
    pub path: String,
    pub lines: Vec<String>,
    /// 1-based line number of `lines[0]`.
    pub start: u32,
    pub total_lines: u32,
    /// True when more lines exist after this window.
    pub truncated: bool,
    pub binary: bool,
    pub too_large: bool,
}

impl FileView {
    fn blocked(path: &str, total_lines: u32, binary: bool, too_large: bool) -> Self {
        Self {
            path: path.to_string(),
            lines: Vec::new(),
            start: 1,
            total_lines,
            truncated: false,
            binary,
            too_large,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDiff {
    pub path: String,
    pub unified: String,
    pub added: u32,
    pub removed: u32,
    pub tracked: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeStatus {
    pub is_git: bool,
    pub is_worktree: bool,
    pub branch: String,
    pub main_root: Option<String>,
    pub worktree_path: Option<String>,
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct MergeBackSummary {
    pub merged_branch: String,
    pub target_branch: String,
    pub main_root: String,
    pub summary: String,
}

/// Resolve a workspace-relative path, refusing anything that escapes the root.
///
/// Absolute paths are rejected outright: the UI only ever knows relative paths,
/// so accepting them would widen the attack surface for nothing.
pub fn resolve(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("workspace root: {e}"))?;
    let rel = rel.trim();
    if rel.is_empty() {
        return Err("empty path".to_string());
    }
    let candidate = Path::new(rel);
    if candidate.is_absolute() {
        return Err(format!("path {rel:?} must be workspace-relative"));
    }
    let joined = root.join(candidate);
    let norm = normalize_lexical(&joined);
    if !norm.starts_with(&root) {
        return Err(format!("path {rel:?} escapes the workspace root"));
    }
    if let Ok(canon) = norm.canonicalize()
        && !canon.starts_with(&root)
    {
        return Err(format!("path {rel:?} resolves outside the workspace"));
    }
    Ok(norm)
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

/// Read a window of a workspace file for display.
///
/// Returns `Ok` with `binary` / `too_large` set instead of failing, so the
/// viewer can explain itself rather than show a bare error.
pub fn read_view(root: &Path, rel: &str, offset: u32, limit: u32) -> Result<FileView, String> {
    let fs_path = resolve(root, rel)?;
    if !fs_path.is_file() {
        return Err(format!("{rel:?} is not a file"));
    }
    let meta = std::fs::metadata(&fs_path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_VIEW_BYTES {
        return Ok(FileView::blocked(rel, 0, false, true));
    }
    {
        use std::io::Read;
        let mut f = std::fs::File::open(&fs_path).map_err(|e| e.to_string())?;
        let mut head = [0u8; 8192];
        let n = f.read(&mut head).map_err(|e| e.to_string())?;
        if head[..n].contains(&0) {
            return Ok(FileView::blocked(rel, 0, true, false));
        }
    }

    let offset = offset.max(1) as usize;
    let limit = if limit == 0 {
        DEFAULT_VIEW_LINES
    } else {
        (limit as usize).min(MAX_VIEW_LINES)
    };

    let mut reader = BufReader::new(std::fs::File::open(&fs_path).map_err(|e| e.to_string())?);
    let mut buf = Vec::new();
    let mut lines: Vec<String> = Vec::with_capacity(limit.min(1024));
    let mut total = 0usize;
    loop {
        buf.clear();
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total += 1;
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        if total >= offset && lines.len() < limit {
            lines.push(String::from_utf8_lossy(&buf).into_owned());
        }
    }

    let shown_from = offset.min(total.max(1));
    let truncated = total > offset.saturating_sub(1) + lines.len();
    Ok(FileView {
        path: rel.to_string(),
        lines,
        start: shown_from as u32,
        total_lines: total as u32,
        truncated,
        binary: false,
        too_large: false,
    })
}

/// Write UTF-8 content to a workspace-relative file (for inline viewer edits).
pub fn write_file(root: &Path, rel: &str, content: &str) -> Result<u32, String> {
    if content.len() as u64 > MAX_VIEW_BYTES {
        return Err("file content exceeds the 4 MB editor limit".to_string());
    }
    let fs_path = resolve(root, rel)?;
    if fs_path.is_dir() {
        return Err(format!("{rel:?} is a directory"));
    }
    if let Some(parent) = fs_path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&fs_path, content.as_bytes()).map_err(|e| e.to_string())?;
    Ok(u32::try_from(content.lines().count()).unwrap_or(u32::MAX))
}

#[cfg(windows)]
fn quiet(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

#[cfg(not(windows))]
fn quiet(cmd: &mut Command) -> &mut Command {
    cmd
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).args(args);
    let out = quiet(&mut cmd)
        .output()
        .map_err(|e| format!("git unavailable: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        return Err(if err.is_empty() {
            "git exited with an error".to_string()
        } else {
            err.to_string()
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git_stdin(root: &Path, args: &[&str], stdin_bytes: &[u8]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = quiet(&mut cmd)
        .spawn()
        .map_err(|e| format!("git unavailable: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_bytes)
            .map_err(|e| format!("git stdin failed: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("git failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        return Err(if err.is_empty() {
            "git exited with an error".to_string()
        } else {
            err.to_string()
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// True when `root` is inside a git work tree. Cheap enough to call per open.
pub fn is_git_repo(root: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    git(&root, &["rev-parse", "--is-inside-work-tree"]).is_ok_and(|out| out.trim() == "true")
}

/// Revert an entire workspace file back to HEAD (or delete it if untracked).
pub fn revert_file(root: &Path, rel: &str) -> Result<(), String> {
    let fs_path = resolve(root, rel)?;
    let root_canon = root
        .canonicalize()
        .map_err(|e| format!("workspace root: {e}"))?;
    if !is_git_repo(&root_canon) {
        return Err("this folder is not a git repository".to_string());
    }
    let tracked = git(&root_canon, &["ls-files", "--error-unmatch", "--", rel]).is_ok();
    if tracked {
        let has_head = git(&root_canon, &["rev-parse", "--verify", "HEAD"]).is_ok();
        if has_head && git(&root_canon, &["cat-file", "-e", &format!("HEAD:{rel}")]).is_ok() {
            git(&root_canon, &["checkout", "HEAD", "--", rel])?;
        } else {
            let _ = git(&root_canon, &["reset", "HEAD", "--", rel]);
            if fs_path.is_file() {
                std::fs::remove_file(&fs_path).map_err(|e| e.to_string())?;
            }
        }
    } else if fs_path.is_file() {
        std::fs::remove_file(&fs_path).map_err(|e| e.to_string())?;
    } else {
        return Err(format!("{rel:?} does not exist"));
    }
    Ok(())
}

/// Revert a single unified diff hunk patch on `rel` using `git apply -R`.
pub fn revert_hunk(root: &Path, rel: &str, patch: &str) -> Result<(), String> {
    let _ = resolve(root, rel)?;
    let root_canon = root
        .canonicalize()
        .map_err(|e| format!("workspace root: {e}"))?;
    if !is_git_repo(&root_canon) {
        return Err("this folder is not a git repository".to_string());
    }
    if patch.trim().is_empty() {
        return Err("empty diff hunk".to_string());
    }
    let mut normalized = patch.replace("\r\n", "\n");
    if !normalized.ends_with('\n') {
        normalized.push('\n');
    }
    git_stdin(
        &root_canon,
        &["apply", "-R", "--unidiff-zero", "-"],
        normalized.as_bytes(),
    )?;
    Ok(())
}

/// Working-tree changes relative to HEAD, with line counts where git knows them.
///
/// Untracked files are counted by reading them, so a fresh file still shows a
/// useful `+N`. Sorting is stable (path) so the UI list does not jump around.
pub fn changes(root: &Path) -> Result<Vec<ChangedFile>, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("workspace root: {e}"))?;
    let status = git(&root, &["status", "--porcelain", "-z"])?;

    let mut counts: std::collections::HashMap<String, (u32, u32)> =
        std::collections::HashMap::new();
    let mut numstat = git(&root, &["diff", "--numstat", "HEAD", "--"]).unwrap_or_default();
    if numstat.trim().is_empty() {
        numstat = git(&root, &["diff", "--numstat", "--"]).unwrap_or_default();
        if let Ok(staged) = git(&root, &["diff", "--numstat", "--cached", "--"]) {
            numstat.push_str(&staged);
        }
    }
    for line in numstat.lines() {
        let mut it = line.splitn(3, '\t');
        let (Some(a), Some(r), Some(p)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let added = a.parse::<u32>().unwrap_or(0);
        let removed = r.parse::<u32>().unwrap_or(0);
        counts.insert(p.trim().to_string(), (added, removed));
    }

    let mut parts = status.split('\0').filter(|s| !s.is_empty());
    let mut out: Vec<ChangedFile> = Vec::new();
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let code = entry.get(..2).unwrap_or("??");
        let path = entry.get(3..).unwrap_or("");
        if code.starts_with('R') || code.starts_with('C') {
            // `-z` puts the original path in the following NUL-separated slot.
            let _old = parts.next();
        }
        if path.is_empty() {
            continue;
        }
        let status_ch = if code == "??" {
            "?"
        } else if code.contains('D') {
            "D"
        } else if code.contains('A') {
            "A"
        } else {
            "M"
        };
        let (mut added, removed) = counts.get(path).copied().unwrap_or((0, 0));
        if status_ch == "?" {
            added = count_lines(&root.join(path));
        }
        out.push(ChangedFile {
            path: path.to_string(),
            status: status_ch.to_string(),
            added,
            removed,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn count_lines(path: &Path) -> u32 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    if meta.len() > MAX_VIEW_BYTES {
        return 0;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return 0;
    };
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    let mut n = 0u32;
    while reader
        .read_until(b'\n', &mut buf)
        .is_ok_and(|read| read > 0)
    {
        n = n.saturating_add(1);
        buf.clear();
    }
    n
}

/// Unified diff for one workspace file.
///
/// Tracked files diff against HEAD; untracked files are rendered as a pure
/// addition so review still works before anything is staged.
pub fn diff(root: &Path, rel: &str) -> Result<FileDiff, String> {
    let fs_path = resolve(root, rel)?;
    let root_canon = root
        .canonicalize()
        .map_err(|e| format!("workspace root: {e}"))?;

    let tracked = git(&root_canon, &["ls-files", "--error-unmatch", "--", rel]).is_ok();

    if !fs_path.exists() && !tracked {
        return Err(format!("{rel:?} does not exist"));
    }

    let mut unified = String::new();
    if tracked {
        for args in [
            ["diff", "--no-color", "--no-ext-diff", "-U3", "HEAD", "--"].as_slice(),
            [
                "diff",
                "--no-color",
                "--no-ext-diff",
                "-U3",
                "--cached",
                "--",
            ]
            .as_slice(),
            ["diff", "--no-color", "--no-ext-diff", "-U3", "--"].as_slice(),
        ] {
            let mut full: Vec<&str> = args.to_vec();
            full.push(rel);
            let body = git(&root_canon, &full).unwrap_or_default();
            if !body.trim().is_empty() {
                unified = body;
                break;
            }
        }
    } else {
        let view = read_view(&root_canon, rel, 1, MAX_VIEW_LINES as u32)?;
        if view.too_large || view.binary {
            return Err(format!("{rel:?} is binary or too large to diff"));
        }
        let mut body = String::new();
        body.push_str("--- /dev/null\n");
        let _ = writeln!(body, "+++ b/{rel}");
        let _ = writeln!(body, "@@ -0,0 +1,{} @@", view.lines.len());
        for line in &view.lines {
            body.push('+');
            body.push_str(line);
            body.push('\n');
        }
        unified = body;
    }

    let (mut added, mut removed) = (0u32, 0u32);
    for line in unified.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added = added.saturating_add(1);
        } else if line.starts_with('-') {
            removed = removed.saturating_add(1);
        }
    }

    let truncated = unified.len() > MAX_DIFF_BYTES;
    if truncated {
        unified.truncate(floor_char_boundary(&unified, MAX_DIFF_BYTES));
        unified.push_str("\n[… diff truncated — open the file to see the rest]\n");
    }

    Ok(FileDiff {
        path: rel.to_string(),
        unified,
        added,
        removed,
        tracked,
        truncated,
    })
}

/// Combined repository diff (for `@git:diff` or `@git:staged` context attachment).
pub fn diff_all(root: &Path, staged_only: bool) -> Result<String, String> {
    let root_canon = root
        .canonicalize()
        .map_err(|e| format!("workspace root: {e}"))?;
    if !is_git_repo(&root_canon) {
        return Err("this folder is not a git repository".to_string());
    }
    let mut body = if staged_only {
        git(
            &root_canon,
            &["diff", "--no-color", "--no-ext-diff", "-U3", "--cached"],
        )?
    } else {
        let mut out = git(
            &root_canon,
            &["diff", "--no-color", "--no-ext-diff", "-U3", "HEAD"],
        )
        .unwrap_or_else(|_| {
            git(&root_canon, &["diff", "--no-color", "--no-ext-diff", "-U3"]).unwrap_or_default()
        });
        if let Ok(changed) = changes(&root_canon) {
            let untracked: Vec<_> = changed.into_iter().filter(|c| c.status == "?").collect();
            if !untracked.is_empty() {
                out.push_str("\n# Untracked files:\n");
                for f in untracked.iter().take(20) {
                    let _ = writeln!(out, "#   + {} ({} lines)", f.path, f.added);
                }
            }
        }
        out
    };
    if body.trim().is_empty() {
        return Ok(if staged_only {
            "(no staged changes)".to_string()
        } else {
            "(working tree is clean)".to_string()
        });
    }
    if body.len() > 96 * 1024 {
        body.truncate(floor_char_boundary(&body, 96 * 1024));
        body.push_str("\n[… diff truncated to 96 KB]\n");
    }
    Ok(body)
}

/// Concise listing of a workspace subdirectory for `@folder:<path>` context attachment.
pub fn list_dir_summary(root: &Path, rel: &str, max_entries: usize) -> Result<String, String> {
    let base = if rel.trim().is_empty() || rel.trim() == "." {
        root.canonicalize()
            .map_err(|e| format!("workspace root: {e}"))?
    } else {
        resolve(root, rel)?
    };
    if !base.is_dir() {
        return Err(format!("{rel:?} is not a directory"));
    }
    let root_canon = root
        .canonicalize()
        .map_err(|e| format!("workspace root: {e}"))?;
    let mut out = String::new();
    let mut stack = vec![base];
    let mut count = 0usize;
    let skip = [
        ".git",
        "node_modules",
        "target",
        "dist",
        "build",
        ".next",
        ".venv",
        "__pycache__",
    ];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut items: Vec<_> = entries.flatten().collect();
        items.sort_by_key(std::fs::DirEntry::file_name);
        let mut subdirs = Vec::new();
        for e in items {
            let name = e.file_name();
            let name_s = name.to_string_lossy();
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            if is_dir && (name_s.starts_with('.') || skip.contains(&name_s.as_ref())) {
                continue;
            }
            let p = e.path();
            let rel_p = p.strip_prefix(&root_canon).unwrap_or(&p);
            let rel_s = rel_p.to_string_lossy().replace('\\', "/");
            if count < max_entries {
                let _ = writeln!(out, "{rel_s}{}", if is_dir { "/" } else { "" });
                count += 1;
            }
            if is_dir && count < max_entries {
                subdirs.push(p);
            }
        }
        for d in subdirs.into_iter().rev() {
            stack.push(d);
        }
        if count >= max_entries {
            let _ = writeln!(out, "[… truncated at {max_entries} entries]");
            break;
        }
    }
    if out.is_empty() {
        out.push_str("(empty directory)\n");
    }
    Ok(out)
}

/// Inspect git & worktree state for a workspace path.
pub fn worktree_status(root: &Path) -> Result<WorktreeStatus, String> {
    let root_canon = root
        .canonicalize()
        .map_err(|e| format!("workspace root: {e}"))?;
    if !is_git_repo(&root_canon) {
        return Ok(WorktreeStatus {
            is_git: false,
            is_worktree: false,
            branch: String::new(),
            main_root: None,
            worktree_path: None,
            dirty: false,
            ahead: 0,
            behind: 0,
        });
    }
    let git_dir = PathBuf::from(git(&root_canon, &["rev-parse", "--git-dir"])?.trim());
    let common_dir = PathBuf::from(git(&root_canon, &["rev-parse", "--git-common-dir"])?.trim());
    let git_dir_abs = if git_dir.is_absolute() {
        git_dir
    } else {
        root_canon.join(git_dir)
    };
    let common_abs = if common_dir.is_absolute() {
        common_dir
    } else {
        root_canon.join(common_dir)
    };
    let git_canon = git_dir_abs.canonicalize().unwrap_or(git_dir_abs);
    let common_canon = common_abs.canonicalize().unwrap_or(common_abs);
    let is_worktree = git_canon != common_canon;
    let main_root = if is_worktree {
        common_canon.parent().map(dunce_str)
    } else {
        None
    };
    let branch = git(&root_canon, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let dirty = !git(&root_canon, &["status", "--porcelain"])
        .unwrap_or_default()
        .trim()
        .is_empty();
    let (ahead, behind) = if is_worktree {
        let base_branch = main_root
            .as_deref()
            .and_then(|mr| git(Path::new(mr), &["rev-parse", "--abbrev-ref", "HEAD"]).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "HEAD");
        if let Some(base) = base_branch {
            let range = format!("{base}...HEAD");
            let counts = git(
                &root_canon,
                &["rev-list", "--left-right", "--count", &range],
            )
            .unwrap_or_default();
            let mut nums = counts.split_whitespace();
            let b = nums.next().and_then(|x| x.parse::<u32>().ok()).unwrap_or(0);
            let a = nums.next().and_then(|x| x.parse::<u32>().ok()).unwrap_or(0);
            (a, b)
        } else {
            (0, 0)
        }
    } else {
        (0, 0)
    };
    Ok(WorktreeStatus {
        is_git: true,
        is_worktree,
        branch,
        main_root,
        worktree_path: is_worktree.then(|| dunce_str(&root_canon)),
        dirty,
        ahead,
        behind,
    })
}

fn dunce_str(path: &Path) -> String {
    let s = path.display().to_string();
    #[cfg(windows)]
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        return stripped.to_string();
    }
    s
}

/// Create an isolated git worktree at `dest` on a new branch.
pub fn worktree_create(root: &Path, dest: &Path, branch: &str) -> Result<String, String> {
    let root_canon = root
        .canonicalize()
        .map_err(|e| format!("workspace root: {e}"))?;
    if !is_git_repo(&root_canon) {
        return Err("this folder is not a git repository".to_string());
    }
    if git(&root_canon, &["rev-parse", "--verify", "HEAD"]).is_err() {
        return Err("repository has no commits yet — make an initial commit first".to_string());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("worktree dir: {e}"))?;
    }
    let branch_clean = sanitize_branch(branch);
    if branch_clean.is_empty() {
        return Err("invalid branch name".to_string());
    }
    let dest_s = dest.display().to_string();
    git(
        &root_canon,
        &["worktree", "add", "-b", &branch_clean, &dest_s, "HEAD"],
    )?;
    let canon = dest.canonicalize().unwrap_or_else(|_| dest.to_path_buf());
    Ok(dunce_str(&canon))
}

fn sanitize_branch(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches(['/', '.'])
        .to_string()
}

/// Commit any uncommitted changes in a worktree, then merge its branch back into the main repo.
pub fn worktree_merge_back(worktree_root: &Path) -> Result<MergeBackSummary, String> {
    let st = worktree_status(worktree_root)?;
    if !st.is_worktree {
        return Err("current workspace is not an isolated worktree".to_string());
    }
    let main_root = st
        .main_root
        .ok_or_else(|| "could not locate main repository root".to_string())?;
    let main_path = Path::new(&main_root);
    let wt_canon = worktree_root
        .canonicalize()
        .map_err(|e| format!("workspace root: {e}"))?;

    if !git(main_path, &["status", "--porcelain"])
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        return Err(
            "the main repository has uncommitted changes — commit or stash them before merging back"
                .to_string(),
        );
    }

    if st.dirty {
        git(&wt_canon, &["add", "-A"])?;
        git(
            &wt_canon,
            &[
                "-c",
                "user.name=hate-agent",
                "-c",
                "user.email=hate-agent@local",
                "commit",
                "-m",
                &format!("hate-agent worktree ({})", st.branch),
            ],
        )?;
    }

    let target_branch = git(main_path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map_or_else(|_| "main".to_string(), |s| s.trim().to_string());

    let merge_res = git(main_path, &["merge", "--no-edit", &st.branch]);
    match merge_res {
        Ok(out) => Ok(MergeBackSummary {
            merged_branch: st.branch,
            target_branch,
            main_root,
            summary: out.trim().to_string(),
        }),
        Err(e) => {
            let _ = git(main_path, &["merge", "--abort"]);
            Err(format!("merge failed (aborted cleanly): {e}"))
        }
    }
}

/// Remove an isolated worktree and return the main repository path.
pub fn worktree_remove(worktree_root: &Path, force: bool) -> Result<String, String> {
    let st = worktree_status(worktree_root)?;
    if !st.is_worktree {
        return Err("current workspace is not an isolated worktree".to_string());
    }
    let main_root = st
        .main_root
        .ok_or_else(|| "could not locate main repository root".to_string())?;
    let main_path = Path::new(&main_root);
    let wt_s = dunce_str(
        &worktree_root
            .canonicalize()
            .unwrap_or_else(|_| worktree_root.to_path_buf()),
    );

    if force {
        git(main_path, &["worktree", "remove", "--force", &wt_s])?;
    } else {
        git(main_path, &["worktree", "remove", &wt_s])?;
    }
    if st.branch.starts_with("hate/") {
        let flag = if force { "-D" } else { "-d" };
        let _ = git(main_path, &["branch", flag, &st.branch]);
    }
    Ok(main_root)
}

fn floor_char_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut end = i;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src").join("b.rs"), "fn main() {}\n").unwrap();
        let root = dir.path().to_path_buf();
        (dir, root)
    }

    #[test]
    fn resolves_relative_paths() {
        let (_d, root) = ws();
        let p = resolve(&root, "src/b.rs").unwrap();
        assert!(p.ends_with("b.rs"));
    }

    #[test]
    fn rejects_escape_attempts() {
        let (_d, root) = ws();
        assert!(resolve(&root, "../outside.txt").is_err());
        assert!(resolve(&root, "src/../../outside.txt").is_err());
        assert!(resolve(&root, "").is_err());
    }

    #[test]
    fn rejects_absolute_paths() {
        let (_d, root) = ws();
        let abs = root.join("a.txt");
        assert!(resolve(&root, &abs.display().to_string()).is_err());
    }

    #[test]
    fn reads_a_window() {
        let (_d, root) = ws();
        let v = read_view(&root, "a.txt", 2, 1).unwrap();
        assert_eq!(v.lines, vec!["two"]);
        assert_eq!(v.start, 2);
        assert_eq!(v.total_lines, 3);
        assert!(v.truncated);
        assert!(!v.binary);
    }

    #[test]
    fn writes_and_reads_file() {
        let (_d, root) = ws();
        let count = write_file(&root, "src/new.txt", "hello\nworld\n").unwrap();
        assert_eq!(count, 2);
        let v = read_view(&root, "src/new.txt", 1, 10).unwrap();
        assert_eq!(v.lines, vec!["hello", "world"]);
        assert!(write_file(&root, "../escape.txt", "nope").is_err());
    }

    #[test]
    fn reads_the_tail_without_truncation() {
        let (_d, root) = ws();
        let v = read_view(&root, "a.txt", 3, 10).unwrap();
        assert_eq!(v.lines, vec!["three"]);
        assert!(!v.truncated);
    }

    #[test]
    fn flags_binary_files() {
        let (d, root) = ws();
        std::fs::write(d.path().join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
        let v = read_view(&root, "blob.bin", 1, 10).unwrap();
        assert!(v.binary);
        assert_eq!(v.lines.len(), 0);
    }

    #[test]
    fn missing_file_is_an_error() {
        let (_d, root) = ws();
        assert!(read_view(&root, "nope.txt", 1, 10).is_err());
    }

    fn init_repo() -> Option<(tempfile::TempDir, PathBuf)> {
        let dir = tempfile::tempdir().ok()?;
        let root = dir.path().to_path_buf();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .is_ok_and(|o| o.status.success())
        };
        if !run(&["init", "-q"]) {
            return None;
        }
        let _ = run(&["config", "core.autocrlf", "false"]);
        std::fs::write(root.join("a.txt"), "one\n").ok()?;
        if !run(&["add", "."]) {
            return None;
        }
        if !run(&[
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "init",
        ]) {
            return None;
        }
        Some((dir, root))
    }

    #[test]
    fn diff_reports_a_modification() {
        let Some((_d, root)) = init_repo() else {
            return;
        };
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        let d = diff(&root, "a.txt").unwrap();
        assert!(d.tracked);
        assert!(d.unified.contains("+two"), "{}", d.unified);
        assert_eq!(d.added, 1);
        assert_eq!(d.removed, 0);

        let ch = changes(&root).unwrap();
        let entry = ch.iter().find(|c| c.path == "a.txt").expect("a.txt listed");
        assert_eq!(entry.status, "M");
        assert_eq!(entry.added, 1);
    }

    #[test]
    fn untracked_file_diffs_as_a_pure_addition() {
        let Some((_d, root)) = init_repo() else {
            return;
        };
        std::fs::write(root.join("fresh.txt"), "alpha\nbeta\n").unwrap();
        let d = diff(&root, "fresh.txt").unwrap();
        assert!(!d.tracked);
        assert_eq!(d.added, 2);
        assert_eq!(d.removed, 0);

        let ch = changes(&root).unwrap();
        let entry = ch
            .iter()
            .find(|c| c.path == "fresh.txt")
            .expect("fresh.txt listed");
        assert_eq!(entry.status, "?");
        assert_eq!(entry.added, 2);
    }

    #[test]
    fn revert_file_restores_tracked_and_removes_untracked() {
        let Some((_d, root)) = init_repo() else {
            return;
        };
        std::fs::write(root.join("a.txt"), "changed\n").unwrap();
        std::fs::write(root.join("extra.txt"), "temp\n").unwrap();
        revert_file(&root, "a.txt").unwrap();
        revert_file(&root, "extra.txt").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "one\n"
        );
        assert!(!root.join("extra.txt").exists());
    }

    #[test]
    fn revert_hunk_applies_reverse_patch() {
        let Some((_d, root)) = init_repo() else {
            return;
        };
        std::fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
        let d = diff(&root, "a.txt").unwrap();
        revert_hunk(&root, "a.txt", &d.unified).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "one\n"
        );
    }

    #[test]
    fn worktree_lifecycle_create_merge_and_remove() {
        let Some((_d, root)) = init_repo() else {
            return;
        };
        let wt_parent = tempfile::tempdir().unwrap();
        let wt_dest = wt_parent.path().join("wt1");
        let wt_path = worktree_create(&root, &wt_dest, "hate/test-wt").unwrap();
        let st = worktree_status(Path::new(&wt_path)).unwrap();
        assert!(st.is_worktree);
        assert_eq!(st.branch, "hate/test-wt");

        std::fs::write(Path::new(&wt_path).join("wt_file.txt"), "isolated\n").unwrap();
        let merged = worktree_merge_back(Path::new(&wt_path)).unwrap();
        assert_eq!(merged.merged_branch, "hate/test-wt");
        assert!(root.join("wt_file.txt").exists());

        let back = worktree_remove(Path::new(&wt_path), false).unwrap();
        assert!(Path::new(&back).join("wt_file.txt").exists());
    }

    #[test]
    fn diff_does_not_escape_the_workspace() {
        let Some((_d, root)) = init_repo() else {
            return;
        };
        assert!(diff(&root, "../a.txt").is_err());
    }
}
