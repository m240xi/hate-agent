//! Read-only workspace views for the desktop UI: file peeks and git diffs.
//!
//! Every path here is workspace-relative and passes the same containment guard
//! the agent's own file tools use, so the UI can never read outside the folder
//! the user picked. Nothing in this module writes.

use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde::Serialize;

/// Refuse to render files larger than this; the UI shows a "too large" notice.
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

/// True when `root` is inside a git work tree. Cheap enough to call per open.
pub fn is_git_repo(root: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    git(&root, &["rev-parse", "--is-inside-work-tree"]).is_ok_and(|out| out.trim() == "true")
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
    use std::fmt::Write as _;

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
    fn diff_does_not_escape_the_workspace() {
        let Some((_d, root)) = init_repo() else {
            return;
        };
        assert!(diff(&root, "../a.txt").is_err());
    }
}
