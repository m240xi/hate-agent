//! Tauri commands. Each command is a thin adapter: validate input, call
//! into the core crates, return a serializable result. No agent logic
//! lives here.

use std::sync::Mutex;

use hate_auth::AuthContext;
use hate_core::{Command, Limits, SessionConfig, SessionHandle, spawn_session};
use hate_ledger::Ledger;
use hate_providers::{ProviderKind, ProviderSpec, build, warn_if_unknown};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Emitter, State};

/// Static facts about the running app — used by the settings pane and as a
/// smoke test that the IPC bridge works end to end.
#[derive(Debug, Clone, Serialize, Type)]
pub struct AppInfo {
    /// Crate version from Cargo.toml.
    pub version: String,
    /// OS data directory (`%APPDATA%` etc.) where config/ledger live.
    pub data_dir: String,
    /// `std::env::consts::OS`.
    pub platform: String,
    /// `std::env::consts::ARCH`.
    pub arch: String,
}

/// Return application metadata.
#[tauri::command]
#[specta::specta]
pub fn app_info() -> AppInfo {
    let data_dir = dirs::data_dir().map_or_else(std::env::temp_dir, |p| p.join("hate-agent"));
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: data_dir.display().to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

/// The live session, created lazily on first `session_send`. M1 keeps a
/// single session per window; multi-session lands with the workspace
/// model.
#[derive(Default)]
pub struct Sessions(pub Mutex<Option<SessionHandle>>);

/// Build (or reuse) the session and forward its events to the frontend
/// as `core-event` emissions — the store's only input (spec §5).
fn ensure_session(state: &Sessions, app: &AppHandle) -> Result<SessionHandle, String> {
    let mut guard = state
        .0
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    if let Some(h) = guard.as_ref() {
        return Ok(h.clone());
    }

    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    let mut config = ctx.config.clone();
    config.ensure_default_provider();
    let (name, pconf) = config
        .resolve_provider(None)
        .ok_or_else(|| "no provider configured — run `hate auth login`".to_string())?;

    warn_if_unknown(&pconf.kind);
    let credential = ctx.credential(name).map_err(|e| e.to_string())?;
    let base_url = ctx
        .base_url_override(name)
        .map_err(|e| e.to_string())?
        .unwrap_or_else(|| pconf.base_url.clone());
    let provider = build(ProviderSpec {
        kind: ProviderKind::parse(&pconf.kind),
        base_url,
        credential,
    });

    let ledger = ledger_handle();
    let handle = spawn_session(SessionConfig {
        provider,
        model: pconf.model.clone(),
        provider_name: name.to_string(),
        limits: Limits::default(),
        ledger,
    });

    let mut rx = handle.subscribe();
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            let _ = app2.emit("core-event", &ev);
        }
    });

    *guard = Some(handle.clone());
    Ok(handle)
}

fn ledger_handle() -> Option<std::sync::Arc<Ledger>> {
    let dir = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("hate-agent");
    std::fs::create_dir_all(&dir).ok()?;
    Ledger::open(&dir.join("ledger.db"))
        .ok()
        .map(std::sync::Arc::new)
}

/// Send a user message — creates the session on first use.
#[tauri::command]
#[specta::specta]
pub async fn session_send(
    state: State<'_, Sessions>,
    app: AppHandle,
    text: String,
) -> Result<(), String> {
    let handle = ensure_session(&state, &app)?;
    handle
        .command(Command::UserMessage(text))
        .map_err(|e| e.to_string())
}

/// Cancel the in-flight turn (composer Esc / cancel button).
#[tauri::command]
#[specta::specta]
pub fn session_cancel(state: State<'_, Sessions>) -> Result<(), String> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    if let Some(h) = guard.as_ref() {
        h.command(Command::Cancel).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Answer a pending `ask_user` prompt (accepted now; M2 wires it).
#[tauri::command]
#[specta::specta]
pub fn session_answer(state: State<'_, Sessions>, text: String) -> Result<(), String> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    if let Some(h) = guard.as_ref() {
        h.command(Command::AnswerPrompt(text))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Resolve a permission request (accepted now; M3 wires it).
#[tauri::command]
#[specta::specta]
pub fn session_respond_permission(
    state: State<'_, Sessions>,
    request_id: String,
    decision: String,
) -> Result<(), String> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    if let Some(h) = guard.as_ref() {
        h.command(Command::RespondPermission {
            request_id,
            decision,
        })
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
