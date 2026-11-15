use std::io::Read as _;
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const AGENT_ID: &str = "antigravity-acp";
const REGISTRY_URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Installed {
    pub version: String,
    pub exe: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BinaryDist {
    pub archive: String,
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub dist: BinaryDist,
}

pub fn platform_key() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("windows-x86_64"),
        ("windows", "aarch64") => Some("windows-aarch64"),
        ("macos", "aarch64") => Some("darwin-aarch64"),
        ("macos", "x86_64") => Some("darwin-x86_64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("linux", "aarch64") => Some("linux-aarch64"),
        _ => None,
    }
}

pub fn data_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("hate-agent")
}

pub fn agent_dir() -> PathBuf {
    data_root().join("agents").join(AGENT_ID)
}

pub fn agent_home() -> PathBuf {
    data_root().join("antigravity")
}

pub fn scratch_dir() -> PathBuf {
    data_root().join("antigravity-chat")
}

fn manifest_path(root: &Path) -> PathBuf {
    root.join("installed.json")
}

pub fn installed() -> Option<Installed> {
    installed_in(&agent_dir())
}

fn installed_in(root: &Path) -> Option<Installed> {
    let text = std::fs::read_to_string(manifest_path(root)).ok()?;
    let inst: Installed = serde_json::from_str(&text).ok()?;
    inst.exe.is_file().then_some(inst)
}

pub fn select_release(registry: &serde_json::Value, platform: &str) -> Result<Release, String> {
    let agent = registry
        .get("agents")
        .and_then(|a| a.as_array())
        .and_then(|a| {
            a.iter()
                .find(|x| x.get("id").and_then(|i| i.as_str()) == Some(AGENT_ID))
        })
        .ok_or("Antigravity is not listed in the ACP registry")?;
    let version = agent
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let dist = agent
        .pointer(&format!("/distribution/binary/{platform}"))
        .cloned()
        .ok_or_else(|| format!("Antigravity has no build for {platform}"))?;
    let dist: BinaryDist =
        serde_json::from_value(dist).map_err(|e| format!("registry entry malformed: {e}"))?;
    if !dist.archive.starts_with("https://") {
        return Err("registry archive URL is not https".to_string());
    }
    Ok(Release { version, dist })
}

pub async fn latest_release() -> Result<Release, String> {
    let platform = platform_key().ok_or("this platform has no Antigravity build")?;
    let registry: serde_json::Value = reqwest::get(REGISTRY_URL)
        .await
        .map_err(|e| format!("registry fetch: {e}"))?
        .error_for_status()
        .map_err(|e| format!("registry fetch: {e}"))?
        .json()
        .await
        .map_err(|e| format!("registry decode: {e}"))?;
    select_release(&registry, platform)
}

pub async fn install(
    mut progress: impl FnMut(u64, Option<u64>) + Send,
) -> Result<Installed, String> {
    let release = latest_release().await?;
    if let Some(cur) = installed()
        && cur.version == release.version
    {
        return Ok(cur);
    }
    let resp = reqwest::get(&release.dist.archive)
        .await
        .map_err(|e| format!("download: {e}"))?
        .error_for_status()
        .map_err(|e| format!("download: {e}"))?;
    let total = resp.content_length();
    let mut stream = resp.bytes_stream();
    let mut bytes: Vec<u8> = Vec::with_capacity(usize::try_from(total.unwrap_or(0)).unwrap_or(0));
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("download: {e}"))?;
        bytes.extend_from_slice(&chunk);
        progress(bytes.len() as u64, total);
    }
    let root = agent_dir();
    tokio::task::spawn_blocking(move || unpack(&root, &release, &bytes))
        .await
        .map_err(|e| format!("install task: {e}"))?
}

pub fn verify_sha256(bytes: &[u8], expected: Option<&str>) -> Result<(), String> {
    let Some(expected) = expected else {
        return Err("registry entry has no checksum; refusing to install".to_string());
    };
    let got = hex(&Sha256::digest(bytes));
    if got.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch (expected {expected}, got {got})"
        ))
    }
}

fn unpack(root: &Path, release: &Release, bytes: &[u8]) -> Result<Installed, String> {
    verify_sha256(bytes, release.dist.sha256.as_deref())?;
    let dest = root.join(&release.version);
    let staging = root.join(format!(".{}-staging", release.version));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("create {}: {e}", staging.display()))?;

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("open archive: {e}"))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("archive entry: {e}"))?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(format!(
                "archive entry escapes its folder: {}",
                entry.name().unwrap_or_default()
            ));
        };
        let out = staging.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| format!("mkdir: {e}"))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("extract: {e}"))?;
        std::fs::write(&out, &buf).map_err(|e| format!("write {}: {e}", out.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = entry.unix_mode().unwrap_or(0o644) | 0o111;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
        }
    }

    let cmd = release.dist.cmd.trim_start_matches("./");
    if !staging.join(cmd).is_file() {
        return Err(format!("archive is missing the agent executable `{cmd}`"));
    }
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&staging, &dest).map_err(|e| format!("finalize install: {e}"))?;

    let installed = Installed {
        version: release.version.clone(),
        exe: dest.join(cmd),
        args: release.dist.args.clone(),
    };
    let manifest = serde_json::to_string_pretty(&installed).map_err(|e| e.to_string())?;
    std::fs::write(manifest_path(root), manifest).map_err(|e| format!("write manifest: {e}"))?;

    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() && p != dest {
                let _ = std::fs::remove_dir_all(p);
            }
        }
    }
    Ok(installed)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

pub fn command(inst: &Installed) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(&inst.exe);
    cmd.args(&inst.args);
    if let Some(dir) = inst.exe.parent() {
        cmd.current_dir(dir);
    }
    let home = agent_home();
    let _ = std::fs::create_dir_all(&home);
    cmd.env("GEMINI_HOME", home);
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn registry(sha: &str, archive: &str) -> serde_json::Value {
        serde_json::json!({
            "version": "1.0.0",
            "agents": [
                {"id": "someone-else", "version": "9", "distribution": {}},
                {"id": AGENT_ID, "version": "1.5.0", "distribution": {"binary": {
                    "windows-x86_64": {"archive": archive, "cmd": "./agy_acp_server.exe", "sha256": sha},
                    "linux-x86_64": {"archive": archive, "cmd": "./agy_acp_server.par", "args": ["--uid="], "sha256": sha}
                }}}
            ]
        })
    }

    fn zip_with(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            for (name, data) in files {
                w.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    #[test]
    fn picks_this_agent_and_platform() {
        let reg = registry("ab", "https://dl.google.com/x.zip");
        let r = select_release(&reg, "linux-x86_64").unwrap();
        assert_eq!(r.version, "1.5.0");
        assert_eq!(r.dist.cmd, "./agy_acp_server.par");
        assert_eq!(r.dist.args, vec!["--uid=".to_string()]);
        assert!(select_release(&reg, "darwin-x86_64").is_err());
        let bad = registry("ab", "http://insecure/x.zip");
        assert!(select_release(&bad, "linux-x86_64").is_err());
    }

    #[test]
    fn checksum_is_mandatory_and_enforced() {
        let data = b"payload";
        assert!(verify_sha256(data, None).is_err());
        assert!(verify_sha256(data, Some("00")).is_err());
        assert!(verify_sha256(data, Some(&sha(data).to_uppercase())).is_ok());
    }

    #[test]
    fn unpacks_verifies_and_records_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = zip_with(&[("agy_acp_server.exe", b"exe"), ("helper.exe", b"h")]);
        let release = Release {
            version: "1.5.0".to_string(),
            dist: BinaryDist {
                archive: "https://x".to_string(),
                cmd: "./agy_acp_server.exe".to_string(),
                args: vec![],
                sha256: Some(sha(&bytes)),
            },
        };
        let inst = unpack(tmp.path(), &release, &bytes).unwrap();
        assert!(
            inst.exe.ends_with("1.5.0/agy_acp_server.exe")
                || inst.exe.ends_with("1.5.0\\agy_acp_server.exe")
        );
        assert_eq!(installed_in(tmp.path()), Some(inst.clone()));

        let tampered = Release {
            dist: BinaryDist {
                sha256: Some("0".repeat(64)),
                ..release.dist.clone()
            },
            ..release.clone()
        };
        assert!(unpack(tmp.path(), &tampered, &bytes).is_err());
        assert_eq!(installed_in(tmp.path()), Some(inst));
    }

    #[test]
    fn rejects_archives_without_the_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let bytes = zip_with(&[("readme.txt", b"hi")]);
        let release = Release {
            version: "2.0.0".to_string(),
            dist: BinaryDist {
                archive: "https://x".to_string(),
                cmd: "./agy_acp_server.exe".to_string(),
                args: vec![],
                sha256: Some(sha(&bytes)),
            },
        };
        assert!(unpack(tmp.path(), &release, &bytes).is_err());
        assert!(installed_in(tmp.path()).is_none());
    }
}
