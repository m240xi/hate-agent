use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use hate_core::{TerminalBridge, TerminalTabSummary};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Emitter, State};

const MAX_BUFFER_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Type)]
pub struct ShellInfo {
    pub id: String,
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct TerminalTabInfo {
    pub id: String,
    pub title: String,
    pub shell: String,
    pub cwd: String,
    pub alive: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalOutputEvent {
    pub id: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalExitEvent {
    pub id: String,
    pub exit_code: Option<i32>,
}

struct TerminalEntry {
    id: String,
    title: String,
    shell: String,
    cwd: String,
    alive: Arc<AtomicBool>,
    exit_code: Arc<Mutex<Option<i32>>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    buffer: Arc<Mutex<VecDeque<u8>>>,
    created_seq: u64,
}

#[derive(Default)]
pub struct TerminalManager {
    tabs: Mutex<HashMap<String, Arc<TerminalEntry>>>,
    seq: std::sync::atomic::AtomicU64,
}

#[derive(Clone, Default)]
pub struct TerminalState(pub Arc<TerminalManager>);

impl TerminalState {
    pub fn shutdown_all(&self) {
        self.0.shutdown_all();
    }

    pub fn bridge(&self) -> Arc<dyn TerminalBridge> {
        Arc::clone(&self.0) as Arc<dyn TerminalBridge>
    }
}

impl TerminalManager {
    pub fn shutdown_all(&self) {
        let entries: Vec<Arc<TerminalEntry>> = {
            let mut map = self.tabs.lock().expect("terminal map poisoned");
            map.drain().map(|(_, v)| v).collect()
        };
        for entry in entries {
            entry.alive.store(false, Ordering::Relaxed);
            if let Ok(mut k) = entry.killer.lock() {
                let _ = k.kill();
            }
        }
    }

    fn resolve_tab(&self, id: Option<&str>) -> Result<Arc<TerminalEntry>, String> {
        let map = self.tabs.lock().expect("terminal map poisoned");
        if let Some(id) = id.filter(|s| !s.trim().is_empty()) {
            return map
                .get(id)
                .cloned()
                .ok_or_else(|| format!("unknown terminal tab {id:?}"));
        }
        map.values()
            .max_by_key(|e| (e.alive.load(Ordering::Relaxed), e.created_seq))
            .cloned()
            .ok_or_else(|| "no terminal tabs open".to_string())
    }

    pub fn read_tab_buffer(
        &self,
        id: Option<&str>,
        raw: bool,
        tail_lines: usize,
    ) -> Result<String, String> {
        let entry = self.resolve_tab(id)?;
        let text = {
            let buf = entry.buffer.lock().expect("terminal buffer poisoned");
            let bytes: Vec<u8> = buf.iter().copied().collect();
            String::from_utf8_lossy(&bytes).into_owned()
        };
        if raw {
            return Ok(text);
        }
        let cleaned = strip_ansi(&text);
        let lines: Vec<&str> = cleaned.lines().collect();
        let start = lines.len().saturating_sub(tail_lines);
        Ok(lines[start..].join("\n"))
    }

    pub fn list_infos(&self) -> Vec<TerminalTabInfo> {
        let mut entries: Vec<Arc<TerminalEntry>> = self
            .tabs
            .lock()
            .expect("terminal map poisoned")
            .values()
            .cloned()
            .collect();
        entries.sort_by_key(|e| e.created_seq);
        entries
            .into_iter()
            .map(|e| TerminalTabInfo {
                id: e.id.clone(),
                title: e.title.clone(),
                shell: e.shell.clone(),
                cwd: e.cwd.clone(),
                alive: e.alive.load(Ordering::Relaxed),
                exit_code: *e.exit_code.lock().expect("exit_code poisoned"),
            })
            .collect()
    }
}

impl TerminalBridge for TerminalManager {
    fn list_terminals(&self) -> Vec<TerminalTabSummary> {
        self.list_infos()
            .into_iter()
            .map(|t| TerminalTabSummary {
                id: t.id,
                title: t.title,
                shell: t.shell,
                cwd: t.cwd,
                alive: t.alive,
            })
            .collect()
    }

    fn read_output(&self, id: Option<&str>, tail_lines: usize) -> Result<String, String> {
        self.read_tab_buffer(id, false, tail_lines)
    }

    fn send_input(&self, id: Option<&str>, input: &str) -> Result<String, String> {
        let entry = self.resolve_tab(id)?;
        if !entry.alive.load(Ordering::Relaxed) {
            return Err(format!("terminal {} has exited", entry.title));
        }
        let mut w = entry
            .writer
            .lock()
            .map_err(|_| "terminal writer lock poisoned".to_string())?;
        w.write_all(input.as_bytes())
            .and_then(|()| w.flush())
            .map_err(|e| format!("failed writing to terminal: {e}"))?;
        Ok(format!(
            "sent {} byte(s) to terminal {} ({})",
            input.len(),
            entry.title,
            entry.id
        ))
    }
}

pub fn available_shells() -> Vec<ShellInfo> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        if which("pwsh.exe") {
            out.push(ShellInfo {
                id: "pwsh".to_string(),
                label: "PowerShell 7".to_string(),
                program: "pwsh.exe".to_string(),
                args: vec!["-NoLogo".to_string()],
            });
        }
        out.push(ShellInfo {
            id: "powershell".to_string(),
            label: "PowerShell".to_string(),
            program: "powershell.exe".to_string(),
            args: vec!["-NoLogo".to_string()],
        });
        out.push(ShellInfo {
            id: "cmd".to_string(),
            label: "Command Prompt".to_string(),
            program: "cmd.exe".to_string(),
            args: Vec::new(),
        });
        let git_bash = Path::new(r"C:\Program Files\Git\bin\bash.exe");
        if git_bash.is_file() {
            out.push(ShellInfo {
                id: "git-bash".to_string(),
                label: "Git Bash".to_string(),
                program: git_bash.display().to_string(),
                args: vec!["--login".to_string(), "-i".to_string()],
            });
        }
        if which("wsl.exe") {
            out.push(ShellInfo {
                id: "wsl".to_string(),
                label: "WSL".to_string(),
                program: "wsl.exe".to_string(),
                args: Vec::new(),
            });
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(sh) = std::env::var("SHELL")
            && !sh.trim().is_empty()
        {
            let label = Path::new(&sh)
                .file_name()
                .map_or_else(|| "Shell".to_string(), |n| n.to_string_lossy().into_owned());
            out.push(ShellInfo {
                id: "default".to_string(),
                label,
                program: sh,
                args: Vec::new(),
            });
        }
        for (id, label, prog) in [
            ("zsh", "zsh", "/bin/zsh"),
            ("bash", "bash", "/bin/bash"),
            ("sh", "sh", "/bin/sh"),
        ] {
            if Path::new(prog).is_file() && !out.iter().any(|s| s.program == prog) {
                out.push(ShellInfo {
                    id: id.to_string(),
                    label: label.to_string(),
                    program: prog.to_string(),
                    args: Vec::new(),
                });
            }
        }
    }
    out
}

#[cfg(windows)]
fn which(name: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| dir.join(name).is_file())
}

fn default_cwd(cwd: Option<String>) -> PathBuf {
    if let Some(p) = cwd.filter(|s| !s.trim().is_empty()) {
        let pb = PathBuf::from(&p);
        if pb.is_dir() {
            return pb;
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

#[tauri::command]
#[specta::specta]
pub fn terminal_list_shells() -> Vec<ShellInfo> {
    available_shells()
}

#[tauri::command]
#[specta::specta]
pub fn terminal_list(state: State<'_, TerminalState>) -> Vec<TerminalTabInfo> {
    state.0.list_infos()
}

#[tauri::command]
#[specta::specta]
pub fn terminal_create(
    app: AppHandle,
    state: State<'_, TerminalState>,
    cwd: Option<String>,
    shell: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<TerminalTabInfo, String> {
    let shells = available_shells();
    let chosen = shell
        .as_deref()
        .and_then(|s| shells.iter().find(|sh| sh.id == s || sh.program == s))
        .or_else(|| shells.first())
        .cloned()
        .ok_or_else(|| "no shell found on this system".to_string())?;

    let cwd_path = default_cwd(cwd);
    let cwd_display = dunce::simplified(&cwd_path).display().to_string();

    let cols = cols.unwrap_or(100).clamp(20, 500);
    let rows = rows.unwrap_or(24).clamp(5, 200);

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("failed to open PTY: {e}"))?;

    let mut cmd = CommandBuilder::new(&chosen.program);
    for a in &chosen.args {
        cmd.arg(a);
    }
    cmd.cwd(&cwd_path);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("failed to launch {}: {e}", chosen.label))?;
    drop(pair.slave);

    let killer = child.clone_killer();
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("failed to read PTY: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("failed to write PTY: {e}"))?;

    let id = uuid::Uuid::new_v4().to_string();
    let seq = state.0.seq.fetch_add(1, Ordering::Relaxed) + 1;
    let title = format!("{} #{}", chosen.label, seq);
    let alive = Arc::new(AtomicBool::new(true));
    let exit_code = Arc::new(Mutex::new(None));
    let buffer = Arc::new(Mutex::new(VecDeque::with_capacity(16 * 1024)));

    let entry = Arc::new(TerminalEntry {
        id: id.clone(),
        title: title.clone(),
        shell: chosen.id.clone(),
        cwd: cwd_display.clone(),
        alive: Arc::clone(&alive),
        exit_code: Arc::clone(&exit_code),
        master: Mutex::new(pair.master),
        writer: Mutex::new(writer),
        killer: Mutex::new(killer),
        buffer: Arc::clone(&buffer),
        created_seq: seq,
    });

    state
        .0
        .tabs
        .lock()
        .expect("terminal map poisoned")
        .insert(id.clone(), entry);

    let id_read = id.clone();
    let app_read = app.clone();
    let buf_read = Arc::clone(&buffer);
    std::thread::Builder::new()
        .name(format!("pty-read-{seq}"))
        .spawn(move || {
            let mut raw = [0u8; 4096];
            let mut pending_utf8 = Vec::<u8>::new();
            while let Ok(n) = reader.read(&mut raw) {
                if n == 0 {
                    break;
                }
                let slice = &raw[..n];
                {
                    let mut b = buf_read.lock().expect("terminal buffer poisoned");
                    for &byte in slice {
                        if b.len() >= MAX_BUFFER_BYTES {
                            b.pop_front();
                        }
                        b.push_back(byte);
                    }
                }
                pending_utf8.extend_from_slice(slice);
                let text = drain_valid_utf8(&mut pending_utf8);
                if !text.is_empty() {
                    let _ = app_read.emit(
                        "terminal-output",
                        TerminalOutputEvent {
                            id: id_read.clone(),
                            data: text,
                        },
                    );
                }
            }
            if !pending_utf8.is_empty() {
                let tail = String::from_utf8_lossy(&pending_utf8).into_owned();
                let _ = app_read.emit(
                    "terminal-output",
                    TerminalOutputEvent {
                        id: id_read.clone(),
                        data: tail,
                    },
                );
            }
        })
        .map_err(|e| format!("failed spawning PTY reader: {e}"))?;

    let id_wait = id.clone();
    let alive_wait = Arc::clone(&alive);
    let code_wait = Arc::clone(&exit_code);
    std::thread::Builder::new()
        .name(format!("pty-wait-{seq}"))
        .spawn(move || {
            let code = child
                .wait()
                .ok()
                .and_then(|s| i32::try_from(s.exit_code()).ok());
            *code_wait.lock().expect("exit_code poisoned") = code;
            alive_wait.store(false, Ordering::Relaxed);
            let _ = app.emit(
                "terminal-exit",
                TerminalExitEvent {
                    id: id_wait,
                    exit_code: code,
                },
            );
        })
        .map_err(|e| format!("failed spawning PTY waiter: {e}"))?;

    Ok(TerminalTabInfo {
        id,
        title,
        shell: chosen.id,
        cwd: cwd_display,
        alive: true,
        exit_code: None,
    })
}

fn drain_valid_utf8(buf: &mut Vec<u8>) -> String {
    match std::str::from_utf8(buf) {
        Ok(s) => {
            let out = s.to_string();
            buf.clear();
            out
        }
        Err(err) => {
            let valid_up_to = err.valid_up_to();
            if err.error_len().is_none() {
                let out = std::str::from_utf8(&buf[..valid_up_to])
                    .unwrap_or("")
                    .to_string();
                buf.drain(..valid_up_to);
                out
            } else {
                let out = String::from_utf8_lossy(buf).into_owned();
                buf.clear();
                out
            }
        }
    }
}

#[tauri::command]
#[specta::specta]
pub fn terminal_write(
    state: State<'_, TerminalState>,
    id: String,
    data: String,
) -> Result<(), String> {
    let entry = state.0.resolve_tab(Some(&id))?;
    let mut w = entry
        .writer
        .lock()
        .map_err(|_| "terminal writer lock poisoned".to_string())?;
    w.write_all(data.as_bytes())
        .and_then(|()| w.flush())
        .map_err(|e| format!("terminal write failed: {e}"))
}

#[tauri::command]
#[specta::specta]
pub fn terminal_resize(
    state: State<'_, TerminalState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let entry = state.0.resolve_tab(Some(&id))?;
    let m = entry
        .master
        .lock()
        .map_err(|_| "terminal master lock poisoned".to_string())?;
    m.resize(PtySize {
        rows: rows.clamp(5, 200),
        cols: cols.clamp(20, 500),
        pixel_width: 0,
        pixel_height: 0,
    })
    .map_err(|e| format!("terminal resize failed: {e}"))
}

#[tauri::command]
#[specta::specta]
pub fn terminal_close(state: State<'_, TerminalState>, id: String) {
    let entry = state
        .0
        .tabs
        .lock()
        .expect("terminal map poisoned")
        .remove(&id);
    if let Some(e) = entry {
        e.alive.store(false, Ordering::Relaxed);
        if let Ok(mut k) = e.killer.lock() {
            let _ = k.kill();
        }
    }
}

#[tauri::command]
#[specta::specta]
pub fn terminal_read_buffer(
    state: State<'_, TerminalState>,
    id: String,
    raw: Option<bool>,
    tail_lines: Option<u32>,
) -> Result<String, String> {
    state.0.read_tab_buffer(
        Some(&id),
        raw.unwrap_or(false),
        tail_lines.unwrap_or(120) as usize,
    )
}

/// Strip ANSI/VT escape sequences for clean AI / `@terminal` context text.
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    for ch in chars.by_ref() {
                        if ('@'..='~').contains(&ch) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(ch) = chars.next() {
                        if ch == '\x07' {
                            break;
                        }
                        if ch == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else if !matches!(c, '\r' | '\x08' | '\x07') {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_csi_and_osc_sequences() {
        let s = "\x1b[32mhello\x1b[0m \x1b]0;title\x07world\r\n";
        assert_eq!(strip_ansi(s), "hello world\n");
    }

    #[test]
    fn utf8_carryover_handles_split_multibyte_char() {
        let mut buf = "hi ".as_bytes().to_vec();
        // First 2 bytes of 4-byte emoji '🚀' (F0 9F 9A 80)
        buf.extend_from_slice(&[0xF0, 0x9F]);
        let part1 = drain_valid_utf8(&mut buf);
        assert_eq!(part1, "hi ");
        assert_eq!(buf, vec![0xF0, 0x9F]);
        buf.extend_from_slice(&[0x9A, 0x80, b'!']);
        let part2 = drain_valid_utf8(&mut buf);
        assert_eq!(part2, "🚀!");
        assert_eq!(buf, [] as [u8; 0]);
    }
}
