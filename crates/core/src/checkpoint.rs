use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde_json::json;

pub struct CheckpointBundle {
    pub id: String,
    pub root: PathBuf,
    pub files: BTreeMap<String, Option<Vec<u8>>>,
}

pub fn write_checkpoint(bundle: &CheckpointBundle) -> Result<(), String> {
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

pub fn restore_checkpoint(ckpt_dir: &Path, workspace_root: &Path) -> Result<usize, String> {
    let manifest_path = ckpt_dir.join("manifest.json");
    let raw =
        std::fs::read_to_string(&manifest_path).map_err(|e| format!("checkpoint manifest: {e}"))?;
    let meta: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("checkpoint manifest json: {e}"))?;
    let entries = meta["files"]
        .as_array()
        .ok_or("checkpoint manifest missing `files`")?;

    let ws = workspace_root
        .canonicalize()
        .map_err(|e| format!("workspace root: {e}"))?;
    let mut restored = 0usize;
    for entry in entries {
        let Some(path) = entry["path"].as_str() else {
            continue;
        };
        let rel = Path::new(path);
        if rel.is_absolute()
            || rel.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            continue;
        }
        let target = ws.join(rel);
        if entry["existed"].as_bool() == Some(true) {
            let blob = entry["blob"].as_u64().unwrap_or(u64::MAX);
            let src = ckpt_dir.join("files").join(blob.to_string());
            let Ok(data) = std::fs::read(&src) else {
                continue;
            };
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
