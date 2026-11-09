//! Tauri commands. Each command is a thin adapter: validate input, call
//! into the core crates, return a serializable result. No agent logic
//! lives here.

use std::sync::Mutex;

use hate_auth::AuthContext;
use hate_core::{
    Command, Limits, PermissionHub, PersistCtx, PersistedSession, SessionConfig, SessionHandle,
    SessionMeta, ThinkingLevel, WorkspaceTools, spawn_session,
};
use hate_ledger::Ledger;
use hate_providers::{ProviderKind, ProviderSpec, build, warn_if_unknown};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Emitter, State};

use crate::sessions_db::{SessionDb, SqliteJournal, StoredSession};

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

/// The live session plus options picked before it spawned. One active
/// session at a time; switching sessions persists the old one (the actor
/// journals at every turn end) and restores the new one's history.
#[derive(Default)]
pub struct Sessions {
    inner: Mutex<SessionSlot>,
}

#[derive(Default)]
struct SessionSlot {
    handle: Option<SessionHandle>,
    /// Persisted session row key; `None` = ephemeral (pre-open sends).
    key: Option<String>,
    /// `chat` | `code` — captured at open/new, journaled with the row.
    mode: String,
    /// Workspace captured for code sessions.
    workspace: Option<String>,
    /// Provider override restored from the session; `None` = default.
    provider: Option<String>,
    /// State to seed the next spawn (set by `session_open`, consumed
    /// by `ensure_session`).
    restore: Option<PersistedSession>,
    /// Model/thinking chosen via `session_set_options` before the first
    /// send — applied at spawn, then superseded by live `SetOptions`.
    model: Option<String>,
    thinking: Option<ThinkingLevel>,
    /// Selected model's catalog context window; `0` = unknown.
    context_window: u64,
}

/// Resolve provider name + config + built adapter — shared by session
/// spawn and `models_list` so they can't drift apart.
fn resolve_target(
    ctx: &AuthContext,
    name_override: Option<&str>,
) -> Result<(String, String, std::sync::Arc<dyn hate_core::Provider>), String> {
    let mut config = ctx.config.clone();
    config.ensure_default_provider();
    let (name, pconf) = config
        .resolve_provider(name_override)
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
    Ok((name.to_string(), pconf.model.clone(), provider))
}

/// Build (or reuse) the session and forward its events to the frontend
/// as `core-event` emissions — the store's only input (spec §5).
fn ensure_session(state: &Sessions, app: &AppHandle) -> Result<SessionHandle, String> {
    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    if let Some(h) = guard.handle.as_ref() {
        return Ok(h.clone());
    }

    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    // A restored session keeps the provider it was created under;
    // everything else resolves the configured default.
    let provider_override = guard
        .provider
        .clone()
        .or_else(|| guard.restore.as_ref().map(|r| r.provider.clone()))
        .filter(|p| !p.is_empty());
    let (name, configured_model, provider) = resolve_target(&ctx, provider_override.as_deref())?;

    // Restored sessions run their own model/thinking; pending
    // `session_set_options` values still win (user's latest pick).
    let (model, thinking) = match guard.restore.as_ref() {
        Some(r) => (
            guard.model.clone().unwrap_or_else(|| r.model.clone()),
            guard.thinking.unwrap_or(r.thinking),
        ),
        None => (
            guard.model.clone().unwrap_or(configured_model),
            guard.thinking.unwrap_or_default(),
        ),
    };

    let persist = match guard.key.clone() {
        Some(key) => Some(PersistCtx {
            key,
            mode: guard.mode.clone(),
            workspace: guard.workspace.clone(),
            journal: std::sync::Arc::new(SqliteJournal::new()?),
            restore: guard.restore.take(),
        }),
        None => None,
    };

    // Code sessions get the read-only workspace tool surface; chat stays
    // text-only. A workspace that can't be canonicalized degrades to no
    // tools rather than failing the session.
    let tools = if guard.mode == "code" {
        guard
            .workspace
            .clone()
            .or_else(|| ctx.config.workspace_path.clone())
            .and_then(|p| match WorkspaceTools::new(&p) {
                Ok(t) => {
                    // Persisted sessions get restorable checkpoints;
                    // keyless (ephemeral) sessions skip capture.
                    if let Some(key) = guard.key.as_deref() {
                        t.enable_checkpoints(checkpoint_root(key));
                    }
                    Some(std::sync::Arc::new(t))
                }
                Err(e) => {
                    tracing::warn!(error = %e, "workspace tools unavailable");
                    None
                }
            })
    } else {
        None
    };

    let ledger = ledger_handle();
    let handle = spawn_session(SessionConfig {
        provider,
        model,
        provider_name: name,
        thinking,
        limits: Limits::default(),
        ledger,
        persist,
        tools,
        permissions: std::sync::Arc::new(PermissionHub::default()),
        context_window: guard.context_window,
    });

    let mut rx = handle.subscribe();
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            let _ = app2.emit("core-event", &ev);
        }
    });

    guard.handle = Some(handle.clone());
    Ok(handle)
}

/// `<data>/hate-agent/checkpoints/<session-key>` — where per-turn file
/// snapshots land for rewind. Must match what `enable_checkpoints` got.
fn checkpoint_root(key: &str) -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("hate-agent")
        .join("checkpoints")
        .join(key)
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
        .inner
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    if let Some(h) = guard.handle.as_ref() {
        h.command(Command::Cancel).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Answer a pending `ask_user` prompt (accepted now; M2 wires it).
#[tauri::command]
#[specta::specta]
pub fn session_answer(state: State<'_, Sessions>, text: String) -> Result<(), String> {
    let guard = state
        .inner
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    if let Some(h) = guard.handle.as_ref() {
        h.command(Command::AnswerPrompt(text))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Restore the workspace to a checkpoint's pre-turn file state
/// (M4 rewind). Exact for `write_file`/`edit_file` mutations; `bash`
/// side effects are not captured.
#[tauri::command]
#[specta::specta]
pub fn session_rewind(state: State<'_, Sessions>, checkpoint_id: String) -> Result<(), String> {
    // Ids are uuids we generated — reject anything path-like anyway.
    if checkpoint_id.is_empty()
        || checkpoint_id.contains(['/', '\\'])
        || checkpoint_id.contains("..")
    {
        return Err("invalid checkpoint id".to_string());
    }
    let guard = state
        .inner
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    let key = guard.key.clone().ok_or("session has no checkpoint store")?;
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    let ws = guard
        .workspace
        .clone()
        .or_else(|| ctx.config.workspace_path.clone())
        .ok_or("no workspace folder")?;
    let ckpt_dir = checkpoint_root(&key).join(&checkpoint_id);
    let restored =
        hate_core::restore_checkpoint(std::path::Path::new(&ckpt_dir), std::path::Path::new(&ws))?;
    tracing::info!(checkpoint = %checkpoint_id, restored, "workspace rewound");
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
        .inner
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    if let Some(h) = guard.handle.as_ref() {
        h.command(Command::RespondPermission {
            request_id,
            decision,
        })
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Change model and/or thinking level. Applies to the live session
/// immediately (next turn onward) and is remembered for the session's
/// first spawn when called before any message was sent.
#[tauri::command]
#[specta::specta]
pub fn session_set_options(
    state: State<'_, Sessions>,
    model: Option<String>,
    thinking: Option<String>,
    context_window: Option<u64>,
) -> Result<(), String> {
    let thinking = thinking.map(|t| ThinkingLevel::parse(&t)).transpose()?;
    let mut slot = state
        .inner
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    if let Some(m) = &model {
        slot.model = Some(m.clone());
    }
    if let Some(t) = thinking {
        slot.thinking = Some(t);
    }
    if let Some(w) = context_window {
        slot.context_window = w;
    }
    if let Some(h) = slot.handle.as_ref() {
        h.command(Command::SetOptions {
            model,
            thinking,
            context_window,
        })
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// What the frontend shows while the user approves device login.
#[derive(Debug, Clone, serde::Serialize, Type)]
pub struct NousLoginInfo {
    /// Code to type at the Portal page.
    pub user_code: String,
    /// Page to open.
    pub verification_url: String,
    /// URL with the code pre-filled, when offered.
    pub verification_url_complete: Option<String>,
}

/// Credential state for the connect UI (`kind`: `none`|`api_key`|`oauth`).
#[derive(Debug, Clone, serde::Serialize, Type)]
pub struct AuthStatus {
    /// Which credential kind is stored.
    pub kind: String,
    /// Access-token expiry (OAuth), unix seconds. `u32` because
    /// specta-typescript refuses 64-bit ints (JS precision).
    pub expires_at: Option<u32>,
}

/// Report the stored credential for a provider (default `nous`).
#[tauri::command]
#[specta::specta]
pub fn auth_status(provider: Option<String>) -> Result<AuthStatus, String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    let name = provider.unwrap_or_else(|| "nous".to_string());
    let status = ctx.status(&name).map_err(|e| e.to_string())?;
    Ok(AuthStatus {
        kind: status.kind.to_string(),
        expires_at: status
            .expires_at
            .map(|e| u32::try_from(e).unwrap_or(u32::MAX)),
    })
}

/// In-flight device login, cancellable from the UI.
#[derive(Default)]
pub struct AuthState {
    /// Cancel token for the running poll; `None` when idle.
    pub login_cancel: Mutex<Option<tokio_util::sync::CancellationToken>>,
}

/// Start Nous Portal device login. Returns the user code + URL as soon
/// as the Portal issues them; approval polling continues in the
/// background and reports on the `auth-event` channel
/// (`{"type":"nous_login","ok":bool,"error"?:string}`).
#[tauri::command]
#[specta::specta]
pub async fn auth_nous_login(
    state: State<'_, AuthState>,
    app: AppHandle,
) -> Result<NousLoginInfo, String> {
    use hate_auth::PortalAuth;
    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;

    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    let auth = std::sync::Arc::new(PortalAuth::new(std::sync::Arc::clone(&ctx.store)));

    // One login at a time: cancel any previous poll first.
    let cancel = CancellationToken::new();
    if let Ok(mut slot) = state.login_cancel.lock()
        && let Some(old) = slot.replace(cancel.clone())
    {
        old.cancel();
    }

    // `login` reports flow info via callback *before* polling — relay it
    // through a oneshot so this command can return promptly.
    let (info_tx, info_rx) = oneshot::channel::<NousLoginInfo>();
    let auth2 = std::sync::Arc::clone(&auth);
    let app2 = app.clone();
    let cancel2 = cancel.clone();
    tauri::async_runtime::spawn(async move {
        let result = auth2
            .login(&cancel2, |info| {
                let _ = info_tx.send(NousLoginInfo {
                    user_code: info.user_code.clone(),
                    verification_url: info.verification_url.clone(),
                    verification_url_complete: info.verification_url_complete.clone(),
                });
            })
            .await;
        let payload = match result {
            Ok(_) => serde_json::json!({"type": "nous_login", "ok": true}),
            Err(e) => {
                serde_json::json!({"type": "nous_login", "ok": false, "error": e.to_string()})
            }
        };
        let _ = app2.emit("auth-event", payload);
    });

    // The device-code request is quick; bound the wait so a hung Portal
    // doesn't park the command forever.
    match tokio::time::timeout(std::time::Duration::from_secs(30), info_rx).await {
        Ok(Ok(info)) => {
            // Hand the approval page to the system browser — prefer the
            // code-pre-filled URL when the Portal offers one. Failure to
            // open is non-fatal; the UI still shows the link.
            let url = info
                .verification_url_complete
                .clone()
                .unwrap_or_else(|| info.verification_url.clone());
            let _ = webbrowser::open(&url);
            Ok(info)
        }
        Ok(Err(_)) => Err("login task ended before device code".to_string()),
        Err(_) => Err("timed out requesting device code".to_string()),
    }
}

/// Cancel an in-flight Nous device login.
#[tauri::command]
#[specta::specta]
pub fn auth_nous_cancel(state: State<'_, AuthState>) {
    if let Ok(mut slot) = state.login_cancel.lock()
        && let Some(cancel) = slot.take()
    {
        cancel.cancel();
    }
}

/// Store an API key for a provider (Settings → provider key entry).
/// The key travels frontend→backend once and is never returned.
#[tauri::command]
#[specta::specta]
pub fn auth_set_key(provider: String, key: String) -> Result<(), String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    ctx.set_api_key(&provider, &key).map_err(|e| e.to_string())
}

/// Remove stored credentials for a provider.
#[tauri::command]
#[specta::specta]
pub fn auth_logout(provider: String) -> Result<(), String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    ctx.logout(&provider).map_err(|e| e.to_string())
}

/// One row of the Settings → Providers list.
#[derive(Debug, Clone, serde::Serialize, Type)]
pub struct ProviderInfo {
    /// Config key (`nous`, `openrouter`, …).
    pub name: String,
    /// Adapter family (`openai_compat`, `anthropic`).
    pub kind: String,
    /// API base URL.
    pub base_url: String,
    /// Default model id.
    pub model: String,
    /// Whether this is the session's default provider.
    pub is_default: bool,
    /// Credential state: `none` | `api_key` | `oauth`.
    pub auth_kind: String,
    /// OAuth access-token expiry, unix seconds.
    pub auth_expires_at: Option<u32>,
}

/// List configured providers with their credential state — drives the
/// Settings → Providers section. Secrets are never included.
#[tauri::command]
#[specta::specta]
pub fn providers_list() -> Result<Vec<ProviderInfo>, String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    let mut config = ctx.config.clone();
    config.ensure_default_provider();
    let default = config.default_provider.clone();
    let mut out = Vec::new();
    for (name, p) in &config.providers {
        let status = ctx.status(name).map_err(|e| e.to_string())?;
        out.push(ProviderInfo {
            name: name.clone(),
            kind: p.kind.clone(),
            base_url: p.base_url.clone(),
            model: p.model.clone(),
            is_default: default.as_deref() == Some(name.as_str()),
            auth_kind: status.kind.to_string(),
            auth_expires_at: status
                .expires_at
                .map(|e| u32::try_from(e).unwrap_or(u32::MAX)),
        });
    }
    Ok(out)
}

/// One model entry for the composer picker — catalog metadata
/// flattened so the frontend gets a flat row shape.
#[derive(Debug, Clone, serde::Serialize, Type)]
pub struct ProviderModel {
    /// Provider-side model id (`moonshotai/kimi-k3`, …).
    pub id: String,
    /// Display name when the provider supplies one.
    pub name: Option<String>,
    /// Context window in tokens, when published.
    pub context_tokens: Option<u32>,
    /// USD per 1M input tokens, when priced.
    pub price_in_mtok: Option<f64>,
    /// USD per 1M output tokens, when priced.
    pub price_out_mtok: Option<f64>,
    /// USD per 1M cached-input tokens, when priced.
    pub price_cache_mtok: Option<f64>,
    /// Reasoning effort strings the model accepts.
    pub reasoning_efforts: Vec<String>,
    /// Provider's default reasoning effort.
    pub default_effort: Option<String>,
    /// Endpoint says tool calls are supported.
    pub supports_tools: Option<bool>,
    /// Endpoint says image input is supported.
    pub supports_vision: Option<bool>,
}

/// List models the resolved provider's credential can access — drives
/// the composer model picker. `provider` names a configured provider;
/// `None` uses the default (same rule as sessions).
#[tauri::command]
#[specta::specta]
pub async fn models_list(provider: Option<String>) -> Result<Vec<ProviderModel>, String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    let (_name, _model, provider) = resolve_target(&ctx, provider.as_deref())?;
    let models = provider.list_models().await.map_err(|e| e.to_string())?;
    Ok(models
        .into_iter()
        .map(|m| {
            let meta = m.meta.unwrap_or_default();
            ProviderModel {
                id: m.id,
                name: m.name,
                context_tokens: meta.context_tokens,
                price_in_mtok: meta.price_in_mtok,
                price_out_mtok: meta.price_out_mtok,
                price_cache_mtok: meta.price_cache_mtok,
                reasoning_efforts: meta.reasoning_efforts,
                default_effort: meta.default_effort,
                supports_tools: meta.supports_tools,
                supports_vision: meta.supports_vision,
            }
        })
        .collect())
}

/// Persist the default model for a configured provider — the picker's
/// selection survives restarts and applies to the CLI too.
#[tauri::command]
#[specta::specta]
pub fn provider_set_model(provider: String, model: String) -> Result<(), String> {
    let mut ctx = AuthContext::load().map_err(|e| e.to_string())?;
    let p = ctx
        .config
        .providers
        .get_mut(&provider)
        .ok_or_else(|| format!("unknown provider {provider:?}"))?;
    p.model = model;
    ctx.config.save().map_err(|e| e.to_string())
}

/// Current workspace folder for code sessions (`None` = app data dir).
#[tauri::command]
#[specta::specta]
pub fn workspace_get() -> Result<Option<String>, String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    Ok(ctx.config.workspace_path.clone())
}

/// Persist the workspace folder (`None` clears it). An active code
/// session adopts the new folder so its journal entry stays truthful.
#[tauri::command]
#[specta::specta]
pub fn workspace_set(state: State<'_, Sessions>, path: Option<String>) -> Result<(), String> {
    let mut ctx = AuthContext::load().map_err(|e| e.to_string())?;
    ctx.config.workspace_path.clone_from(&path);
    ctx.config.save().map_err(|e| e.to_string())?;
    if let Ok(mut guard) = state.inner.lock()
        && guard.mode == "code"
    {
        guard.workspace = path;
    }
    Ok(())
}

/// Open the native folder picker; returns the chosen path or `None`.
#[tauri::command]
#[specta::specta]
pub async fn workspace_pick(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let picked =
        tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
            .await
            .map_err(|e| format!("picker task failed: {e}"))?;
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| {
        // `into_path` can yield \\?\ verbatim paths on Windows — simplify
        // for display/storage.
        dunce::simplified(&p).display().to_string()
    }))
}

// ── session persistence ────────────────────────────────────────────────────

/// Wire shape for one session row — mirrors `hate_core::SessionMeta`
/// (core types don't carry specta derives).
#[derive(Debug, Clone, serde::Serialize, Type)]
pub struct SessionMetaInfo {
    /// App-level session id.
    pub id: String,
    /// `chat` | `code`.
    pub mode: String,
    /// Derived title (first user message).
    pub title: String,
    /// Provider config name.
    pub provider: String,
    /// Model id.
    pub model: String,
    /// Workspace folder for code sessions.
    pub workspace: Option<String>,
    /// Last save, unix seconds.
    pub updated_at: u32,
}

fn meta_info(m: SessionMeta) -> SessionMetaInfo {
    SessionMetaInfo {
        id: m.id,
        mode: m.mode,
        title: m.title,
        provider: m.provider,
        model: m.model,
        workspace: m.workspace,
        updated_at: m.updated_at,
    }
}

/// `session_open` payload: metadata plus the frontend's UI blob as raw
/// JSON text (kept as a string so specta doesn't have to model arbitrary
/// JSON — the store parses it).
#[derive(Debug, Clone, serde::Serialize, Type)]
pub struct SessionOpen {
    /// Row metadata.
    pub meta: SessionMetaInfo,
    /// Serialized UI state; `None` for sessions that never saved one.
    pub ui_state: Option<String>,
}

fn db() -> Result<SessionDb, String> {
    SessionDb::open()
}

/// Fail when a turn is in flight — switching sessions mid-stream would
/// silently orphan the turn, so the UI disables it instead.
fn ensure_idle(state: &Sessions) -> Result<(), String> {
    let guard = state
        .inner
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    if let Some(h) = guard.handle.as_ref()
        && h.snapshot().running
    {
        return Err("session is running — cancel the turn first".to_string());
    }
    Ok(())
}

/// Reset the slot to a fresh/other session: drop the live actor (its
/// state is already journaled) and prime the next spawn.
fn prime_slot(guard: &mut SessionSlot, stored: Option<StoredSession>) {
    guard.handle = None;
    guard.restore = None;
    guard.model = None;
    guard.thinking = None;
    if let Some(s) = stored {
        guard.key = Some(s.meta.id.clone());
        guard.mode.clone_from(&s.meta.mode);
        guard.workspace = s
            .persisted
            .as_ref()
            .and_then(|p| p.workspace.clone())
            .or_else(|| s.meta.workspace.clone());
        guard.provider = (!s.meta.provider.is_empty()).then(|| s.meta.provider.clone());
        guard.restore = s.persisted;
    } else {
        guard.key = None;
        guard.provider = None;
    }
}

/// List persisted sessions for the browser; `mode` filters chat/code.
#[tauri::command]
#[specta::specta]
pub fn sessions_list(mode: Option<String>) -> Result<Vec<SessionMetaInfo>, String> {
    let mode = mode.filter(|m| m == "chat" || m == "code");
    Ok(db()?
        .list(mode.as_deref())?
        .into_iter()
        .map(meta_info)
        .collect())
}

/// Create a new persisted session and make it active. Returns its row.
#[tauri::command]
#[specta::specta]
pub fn session_new(state: State<'_, Sessions>, mode: String) -> Result<SessionMetaInfo, String> {
    if mode != "chat" && mode != "code" {
        return Err(format!("unknown session mode {mode:?}"));
    }
    ensure_idle(&state)?;

    let key = uuid::Uuid::new_v4().to_string();
    db()?.create(&key, &mode)?;

    // Code sessions capture the current workspace; chat sessions don't
    // need one (the transcript is workspace-agnostic).
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    let workspace = (mode == "code")
        .then(|| ctx.config.workspace_path.clone())
        .flatten();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u32::try_from(d.as_secs()).unwrap_or(u32::MAX));

    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    prime_slot(&mut guard, None);
    guard.key = Some(key.clone());
    guard.mode.clone_from(&mode);
    guard.workspace.clone_from(&workspace);

    Ok(SessionMetaInfo {
        id: key,
        mode,
        title: "New session".to_string(),
        provider: String::new(),
        model: String::new(),
        workspace,
        updated_at: now,
    })
}

/// Open a persisted session: primes the slot for the next spawn and
/// returns its metadata + UI blob for the frontend to restore.
#[tauri::command]
#[specta::specta]
pub fn session_open(state: State<'_, Sessions>, id: String) -> Result<SessionOpen, String> {
    ensure_idle(&state)?;
    let stored = db()?
        .load(&id)?
        .ok_or_else(|| format!("unknown session {id:?}"))?;
    let meta = meta_info(stored.meta.clone());
    let ui_state = stored.ui_state.clone();

    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    prime_slot(&mut guard, Some(stored));

    Ok(SessionOpen { meta, ui_state })
}

/// Delete a session permanently. If it's the active one, the slot resets
/// to ephemeral — the frontend then shows the session browser.
#[tauri::command]
#[specta::specta]
pub fn session_delete(state: State<'_, Sessions>, id: String) -> Result<(), String> {
    ensure_idle(&state)?;
    db()?.delete(&id)?;
    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    if guard.key.as_deref() == Some(id.as_str()) {
        prime_slot(&mut guard, None);
    }
    Ok(())
}

/// Persist the frontend's UI blob for a session (transcript blocks,
/// draft, stats). Raw JSON text — stored verbatim, never parsed here.
#[tauri::command]
#[specta::specta]
pub fn session_save_ui(id: String, ui_state: String) -> Result<(), String> {
    if ui_state.len() > 8 * 1024 * 1024 {
        return Err("ui_state too large".to_string());
    }
    db()?.save_ui(&id, &ui_state)
}

// ── workspace index ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum IndexNodeKind {
    Dir,
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct IndexNode {
    pub name: String,
    pub path: String,
    pub kind: IndexNodeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<IndexNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum IndexSymKind {
    Def,
    Ref,
    Call,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct IndexSymbol {
    pub name: String,
    pub path: String,
    pub line: u32,
    pub kind: IndexSymKind,
}

#[derive(Debug, Serialize, Deserialize, Type)]
pub struct WorkspaceIndexOut {
    pub tree: Vec<IndexNode>,
    pub symbols: Vec<IndexSymbol>,
    pub truncated: bool,
}

/// Build the workspace index (file tree + heuristic symbols) for the
/// Code view's left pane. Read-only; caps keep huge repos responsive.
#[tauri::command]
#[specta::specta]
pub fn workspace_index(path: String) -> Result<WorkspaceIndexOut, String> {
    let idx = hate_core::index::build_index(std::path::Path::new(&path))
        .map_err(|e| format!("index failed: {e}"))?;
    // Core types are serde-only; the app DTOs mirror them for specta.
    serde_json::from_value(serde_json::to_value(&idx).map_err(|e| e.to_string())?)
        .map_err(|e| format!("index shape mismatch: {e}"))
}
