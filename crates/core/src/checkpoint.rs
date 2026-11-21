use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use serde_json::{Map, json};
use uuid::Uuid;

use crate::process_util::hide_std_cmd;

/// 500 MiB total budget across `<data>/hate-agent/checkpoints/*`.
pub const DEFAULT_CHECKPOINT_MAX_BYTES: u64 = 500 * 1024 * 1024;

pub struct CheckpointBundle {
    pub id: String,
    pub root: PathBuf,
    pub files: BTreeMap<String, Option<Vec<u8>>>,
}

fn is_safe_rel_path(path: &str) -> bool {
    let rel = Path::new(path);
    !path.is_empty()
        && !rel.is_absolute()
        && !rel.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn validate_checkpoint_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        return Err("invalid checkpoint id".to_string());
    }
    Ok(())
}

fn git_cmd(git_dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    hide_std_cmd(&mut cmd);
    cmd.arg("--git-dir")
        .arg(git_dir)
        .env("GIT_AUTHOR_NAME", "hate-agent")
        .env("GIT_AUTHOR_EMAIL", "checkpoint@local")
        .env("GIT_COMMITTER_NAME", "hate-agent")
        .env("GIT_COMMITTER_EMAIL", "checkpoint@local")
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

fn run_git_capture(mut cmd: Command, context: &str) -> Result<Vec<u8>, String> {
    let out = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("{context}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{context}: {}", err.trim()));
    }
    Ok(out.stdout)
}

fn ensure_shadow_repo(repo_git: &Path) -> Result<(), String> {
    if repo_git.exists() {
        return Ok(());
    }
    if let Some(parent) = repo_git.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create ckpt root: {e}"))?;
    }
    let mut init = Command::new("git");
    hide_std_cmd(&mut init);
    init.args(["init", "--bare"])
        .arg(repo_git)
        .env("GIT_TERMINAL_PROMPT", "0");
    run_git_capture(init, "git init --bare")?;

    let mut cfg = git_cmd(repo_git);
    cfg.args(["config", "gc.auto", "0"]);
    let _ = run_git_capture(cfg, "git config gc.auto");
    Ok(())
}

fn hash_blob_bytes(repo_git: &Path, bytes: &[u8]) -> Result<String, String> {
    let mut cmd = git_cmd(repo_git);
    cmd.args(["hash-object", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("git hash-object: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(bytes)
            .map_err(|e| format!("git hash-object stdin: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("git hash-object wait: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git hash-object failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn write_shadow_git(bundle: &CheckpointBundle) -> Result<(), String> {
    let dir = bundle.root.join(&bundle.id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create ckpt dir: {e}"))?;
    let repo_git = bundle.root.join("repo.git");
    ensure_shadow_repo(&repo_git)?;

    let idx_file = dir.join(".idx");
    if idx_file.exists() {
        let _ = std::fs::remove_file(&idx_file);
    }

    let mut present = Map::new();
    let mut absent = Vec::new();

    for (path, bytes_opt) in &bundle.files {
        let norm = path.replace('\\', "/");
        if !is_safe_rel_path(&norm) {
            continue;
        }
        if let Some(bytes) = bytes_opt {
            let oid = hash_blob_bytes(&repo_git, bytes)?;
            let cacheinfo = format!("100644,{oid},{norm}");
            let mut upd = git_cmd(&repo_git);
            upd.env("GIT_INDEX_FILE", &idx_file).args([
                "update-index",
                "--add",
                "--cacheinfo",
                &cacheinfo,
            ]);
            run_git_capture(upd, "git update-index")?;
            present.insert(norm, serde_json::Value::String(oid));
        } else {
            absent.push(serde_json::Value::String(norm));
        }
    }

    let mut write_tree = git_cmd(&repo_git);
    write_tree
        .env("GIT_INDEX_FILE", &idx_file)
        .arg("write-tree");
    let tree_raw = run_git_capture(write_tree, "git write-tree");
    let _ = std::fs::remove_file(&idx_file);
    let tree_oid = String::from_utf8_lossy(&tree_raw?).trim().to_string();

    let mut rev_head = git_cmd(&repo_git);
    rev_head.args(["rev-parse", "--verify", "refs/heads/main"]);
    let parent_oid = run_git_capture(rev_head, "rev-parse main")
        .ok()
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .filter(|s| !s.is_empty());

    let mut commit_cmd = git_cmd(&repo_git);
    commit_cmd.args(["commit-tree", &tree_oid]);
    if let Some(parent) = &parent_oid {
        commit_cmd.args(["-p", parent]);
    }
    commit_cmd.args(["-m", &bundle.id]);
    let commit_oid = String::from_utf8_lossy(&run_git_capture(commit_cmd, "git commit-tree")?)
        .trim()
        .to_string();

    let ref_name = format!("refs/checkpoints/{}", bundle.id);
    let mut upd_ref = git_cmd(&repo_git);
    upd_ref.args(["update-ref", &ref_name, &commit_oid]);
    run_git_capture(upd_ref, "git update-ref checkpoint")?;

    let mut upd_main = git_cmd(&repo_git);
    upd_main.args(["update-ref", "refs/heads/main", &commit_oid]);
    let _ = run_git_capture(upd_main, "git update-ref main");

    let meta = json!({
        "version": 2,
        "commit": commit_oid,
        "ref": ref_name,
        "present": present,
        "absent": absent,
    });
    std::fs::write(
        dir.join("shadow.json"),
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("write shadow.json: {e}"))?;
    Ok(())
}

fn write_legacy_v1(bundle: &CheckpointBundle) -> Result<(), String> {
    let dir = bundle.root.join(&bundle.id);
    let files_dir = dir.join("files");
    let mut entries = Vec::with_capacity(bundle.files.len());
    for (i, (path, bytes)) in bundle.files.iter().enumerate() {
        if let Some(data) = bytes {
            std::fs::create_dir_all(&files_dir).map_err(|e| e.to_string())?;
            std::fs::write(files_dir.join(i.to_string()), data).map_err(|e| e.to_string())?;
        }
        entries.push(json!({
            "path": path,
            "existed": bytes.is_some(),
            "blob": i,
        }));
    }
    let meta = json!({ "version": 1, "files": entries });
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn write_checkpoint(bundle: &CheckpointBundle) -> Result<(), String> {
    validate_checkpoint_id(&bundle.id)?;
    if let Err(err) = write_shadow_git(bundle) {
        tracing::debug!(error = %err, "shadow-git checkpoint unavailable, falling back to v1 manifest");
        write_legacy_v1(bundle)?;
    }

    if let Some(parent) = bundle.root.parent()
        && parent.file_name() == Some(OsStr::new("checkpoints"))
    {
        let _ = evict_old_checkpoints(parent, DEFAULT_CHECKPOINT_MAX_BYTES, Some(&bundle.root));
    }
    Ok(())
}

fn read_shadow_snapshot(ckpt_dir: &Path) -> Result<BTreeMap<String, Option<Vec<u8>>>, String> {
    let raw = std::fs::read_to_string(ckpt_dir.join("shadow.json"))
        .map_err(|e| format!("checkpoint shadow manifest: {e}"))?;
    let meta: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("checkpoint shadow json: {e}"))?;
    let repo_git = ckpt_dir
        .parent()
        .ok_or_else(|| "checkpoint dir missing parent".to_string())?
        .join("repo.git");

    let mut out = BTreeMap::new();
    if let Some(present) = meta["present"].as_object() {
        for (path, oid_val) in present {
            let Some(oid) = oid_val.as_str() else {
                continue;
            };
            if !is_safe_rel_path(path) {
                continue;
            }
            let mut cat = git_cmd(&repo_git);
            cat.args(["cat-file", "blob", oid]);
            let bytes = run_git_capture(cat, "git cat-file blob")?;
            out.insert(path.clone(), Some(bytes));
        }
    }
    if let Some(absent) = meta["absent"].as_array() {
        for p in absent {
            if let Some(path) = p.as_str()
                && is_safe_rel_path(path)
            {
                out.insert(path.to_string(), None);
            }
        }
    }
    Ok(out)
}

fn read_v1_snapshot(ckpt_dir: &Path) -> Result<BTreeMap<String, Option<Vec<u8>>>, String> {
    let manifest_path = ckpt_dir.join("manifest.json");
    let raw =
        std::fs::read_to_string(&manifest_path).map_err(|e| format!("checkpoint manifest: {e}"))?;
    let meta: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("checkpoint manifest json: {e}"))?;
    let entries = meta["files"]
        .as_array()
        .ok_or("checkpoint manifest missing `files`")?;

    let mut out = BTreeMap::new();
    for entry in entries {
        let Some(path) = entry["path"].as_str() else {
            continue;
        };
        if !is_safe_rel_path(path) {
            continue;
        }
        if entry["existed"].as_bool() == Some(true) {
            let blob = entry["blob"].as_u64().unwrap_or(u64::MAX);
            let src = ckpt_dir.join("files").join(blob.to_string());
            if let Ok(data) = std::fs::read(&src) {
                out.insert(path.to_string(), Some(data));
            }
        } else {
            out.insert(path.to_string(), None);
        }
    }
    Ok(out)
}

fn read_snapshot_any(ckpt_dir: &Path) -> Result<BTreeMap<String, Option<Vec<u8>>>, String> {
    if ckpt_dir.join("shadow.json").is_file() {
        read_shadow_snapshot(ckpt_dir)
    } else {
        read_v1_snapshot(ckpt_dir)
    }
}

pub fn restore_checkpoint(ckpt_dir: &Path, workspace_root: &Path) -> Result<usize, String> {
    let snapshot = read_snapshot_any(ckpt_dir)?;
    let ws = workspace_root
        .canonicalize()
        .map_err(|e| format!("workspace root: {e}"))?;
    let mut restored = 0usize;
    for (path, opt_data) in snapshot {
        if !is_safe_rel_path(&path) {
            continue;
        }
        let target = ws.join(&path);
        if let Some(data) = opt_data {
            if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&target, &data).is_ok() {
                restored += 1;
            }
        } else if target.exists() && std::fs::remove_file(&target).is_ok() {
            restored += 1;
        }
    }
    Ok(restored)
}

fn git_diff_blobs(path: &str, old: Option<&[u8]>, new: Option<&[u8]>) -> Result<String, String> {
    if old == new {
        return Ok(String::new());
    }
    let tmp = std::env::temp_dir().join(format!("hate-ckpt-diff-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("temp diff dir: {e}"))?;
    let old_path = tmp.join("old");
    let new_path = tmp.join("new");
    let res = (|| -> Result<String, String> {
        std::fs::write(&old_path, old.unwrap_or_default()).map_err(|e| e.to_string())?;
        std::fs::write(&new_path, new.unwrap_or_default()).map_err(|e| e.to_string())?;

        let mut cmd = Command::new("git");
        hide_std_cmd(&mut cmd);
        cmd.args(["diff", "--no-index", "--no-color", "--"])
            .arg(&old_path)
            .arg(&new_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let out = cmd.output().map_err(|e| format!("git diff: {e}"))?;
        // git diff --no-index exits with 1 when files differ.
        if !matches!(out.status.code(), Some(0 | 1)) {
            return Err(format!(
                "git diff failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let raw = String::from_utf8_lossy(&out.stdout);
        let old_header = match old {
            Some(_) => format!("a/{path}"),
            None => "/dev/null".to_string(),
        };
        let new_header = match new {
            Some(_) => format!("b/{path}"),
            None => "/dev/null".to_string(),
        };
        let mut normalized = Vec::new();
        for line in raw.lines() {
            if line.starts_with("diff --git ") {
                normalized.push(format!("diff --git a/{path} b/{path}"));
            } else if line.starts_with("--- ") {
                normalized.push(format!("--- {old_header}"));
            } else if line.starts_with("+++ ") {
                normalized.push(format!("+++ {new_header}"));
            } else if !line.starts_with("index ") {
                normalized.push(line.to_string());
            }
        }
        Ok(normalized.join("\n"))
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    res
}

/// Produce a unified diff between `from_id` and `to_id` (or the live workspace when `to_id` is `None`).
pub fn checkpoint_diff(
    ckpt_root: &Path,
    from_id: &str,
    to_id: Option<&str>,
    workspace_root: &Path,
) -> Result<String, String> {
    validate_checkpoint_id(from_id)?;
    if let Some(tid) = to_id {
        validate_checkpoint_id(tid)?;
    }
    let from_snap = read_snapshot_any(&ckpt_root.join(from_id))?;
    let to_snap = if let Some(tid) = to_id {
        read_snapshot_any(&ckpt_root.join(tid))?
    } else {
        let mut live = BTreeMap::new();
        for path in from_snap.keys() {
            if is_safe_rel_path(path) {
                live.insert(path.clone(), std::fs::read(workspace_root.join(path)).ok());
            }
        }
        live
    };

    let all_paths: BTreeSet<&str> = from_snap
        .keys()
        .chain(to_snap.keys())
        .map(String::as_str)
        .collect();
    let mut chunks = Vec::new();
    for path in all_paths {
        let old_bytes = from_snap.get(path).and_then(|v| v.as_deref());
        let new_bytes = to_snap.get(path).and_then(|v| v.as_deref());
        let diff = git_diff_blobs(path, old_bytes, new_bytes)?;
        if !diff.is_empty() {
            chunks.push(diff);
        }
    }
    Ok(chunks.join("\n"))
}

fn dir_size_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let p = entry.path();
        if let Ok(meta) = entry.metadata() {
            if meta.is_dir() {
                total = total.saturating_add(dir_size_bytes(&p));
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

/// Evict oldest session checkpoint directories under `parent` until total disk usage <= `max_bytes`.
pub fn evict_old_checkpoints(parent: &Path, max_bytes: u64, keep: Option<&Path>) -> usize {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return 0;
    };
    let mut dirs: Vec<(PathBuf, SystemTime, u64)> = Vec::new();
    let mut total = 0u64;

    for entry in entries.flatten() {
        let p = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let sz = dir_size_bytes(&p);
        total = total.saturating_add(sz);
        if keep.is_some_and(|k| k == p) {
            continue;
        }
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        dirs.push((p, mtime, sz));
    }

    if total <= max_bytes {
        return 0;
    }
    dirs.sort_by_key(|(_, t, _)| *t);

    let mut removed = 0usize;
    for (p, _, sz) in dirs {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_dir_all(&p).is_ok() {
            total = total.saturating_sub(sz);
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_git_snapshot_diff_and_restore_work() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let ck_root = dir.path().join("ckpts");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("a.txt"), "hello\nworld\n").unwrap();

        let mut files = BTreeMap::new();
        files.insert("a.txt".to_string(), Some(b"hello\nworld\n".to_vec()));
        files.insert("created.txt".to_string(), None);

        let bundle = CheckpointBundle {
            id: "cp1".to_string(),
            root: ck_root.clone(),
            files,
        };
        write_checkpoint(&bundle).unwrap();
        assert!(ck_root.join("cp1").join("shadow.json").is_file());
        assert!(ck_root.join("repo.git").is_dir());

        std::fs::write(ws.join("a.txt"), "hello\nchanged\n").unwrap();
        std::fs::write(ws.join("created.txt"), "new file\n").unwrap();

        let diff = checkpoint_diff(&ck_root, "cp1", None, &ws).unwrap();
        assert!(
            diff.contains("-world") && diff.contains("+changed"),
            "{diff}"
        );
        assert!(diff.contains("+new file"), "{diff}");

        let restored = restore_checkpoint(&ck_root.join("cp1"), &ws).unwrap();
        assert_eq!(restored, 2);
        assert_eq!(
            std::fs::read_to_string(ws.join("a.txt")).unwrap(),
            "hello\nworld\n"
        );
        assert!(!ws.join("created.txt").exists());
    }

    #[test]
    fn legacy_v1_manifest_still_restores_and_diffs() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let ck_root = dir.path().join("ckpts");
        std::fs::create_dir_all(&ws).unwrap();
        let bundle = CheckpointBundle {
            id: "legacy1".to_string(),
            root: ck_root.clone(),
            files: BTreeMap::from([("x.txt".to_string(), Some(b"old\n".to_vec()))]),
        };
        write_legacy_v1(&bundle).unwrap();

        std::fs::write(ws.join("x.txt"), "new\n").unwrap();
        let diff = checkpoint_diff(&ck_root, "legacy1", None, &ws).unwrap();
        assert!(diff.contains("-old") && diff.contains("+new"), "{diff}");

        let restored = restore_checkpoint(&ck_root.join("legacy1"), &ws).unwrap();
        assert_eq!(restored, 1);
        assert_eq!(std::fs::read_to_string(ws.join("x.txt")).unwrap(), "old\n");
    }

    #[test]
    fn lru_eviction_removes_oldest_dirs_until_under_budget() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("checkpoints");
        std::fs::create_dir_all(parent.join("old")).unwrap();
        std::fs::create_dir_all(parent.join("new")).unwrap();

        std::fs::write(parent.join("old").join("blob"), vec![b'a'; 200]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(parent.join("new").join("blob"), vec![b'b'; 200]).unwrap();

        let removed = evict_old_checkpoints(&parent, 250, Some(&parent.join("new")));
        assert_eq!(removed, 1);
        assert!(!parent.join("old").exists());
        assert!(parent.join("new").exists());
    }
}
