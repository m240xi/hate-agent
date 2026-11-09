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
use tauri::{AppHandle, Emitter, Manager, State};

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
/// as `core-event` emissions — the store's only input.
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
    // MCP tools are workspace-independent — every session (chat
    // included) sees whatever servers are currently connected.
    let mcp = app.state::<McpState>().hub.clone();
    // Learned skills live in the shared config dir; chat sessions get
    // them too. `auto_skills` gates the background reflection pass.
    let skills = hate_core::SkillStore::default_dir()
        .map(|d| std::sync::Arc::new(hate_core::SkillStore::new(d)));
    let auto_skills = ctx.config.auto_skills;
    let handle = spawn_session(SessionConfig {
        provider,
        model,
        provider_name: name,
        thinking,
        limits: Limits::default(),
        ledger,
        persist,
        tools,
        mcp: Some(mcp),
        skills,
        auto_skills,
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

/// Managed MCP hub + its config file. One hub for the whole app —
/// sessions share it, so a settings save reaches every session's next
/// turn without respawning anything.
pub struct McpState {
    /// Live server connections + advertised tools.
    pub hub: std::sync::Arc<hate_core::McpHub>,
    /// `<data>/hate-agent/mcp.json` — persisted server configs.
    pub path: std::path::PathBuf,
    /// Configs loaded at startup, applied by a background `configure`.
    pub pending: Vec<hate_core::McpServerConfig>,
}

impl McpState {
    /// Read `mcp.json` synchronously (cheap, once at startup); the
    /// actual server connects happen in `setup` on the async runtime.
    pub fn load() -> Self {
        let path = mcp_config_path();
        let pending = hate_core::McpConfigFile::load(&path);
        Self {
            hub: std::sync::Arc::new(hate_core::McpHub::new()),
            path,
            pending,
        }
    }
}

/// `<data>/hate-agent/mcp.json` — same dir as `sessions.db`/`ledger.db`.
fn mcp_config_path() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("hate-agent")
        .join("mcp.json")
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

/// `/compact` — force a context compaction on the live session.
/// No-op-ish error when a turn is running (the turn owns its convo).
#[tauri::command]
#[specta::specta]
pub fn session_compact(state: State<'_, Sessions>) -> Result<(), String> {
    let guard = state
        .inner
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    if let Some(h) = guard.handle.as_ref() {
        h.command(Command::Compact).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Answer a pending `ask_user` prompt. Reserved — no tool emits one
/// yet, so the session just warns.
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

/// Restore the workspace to a checkpoint's pre-turn file state.
/// Exact for `write_file`/`edit_file` mutations; `bash`
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

/// Resolve a pending permission request: `allow`, `allow_session`,
/// anything else denies.
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
    // u32 at the IPC boundary — specta forbids u64 exports; context
    // windows fit comfortably.
    context_window: Option<u32>,
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
        slot.context_window = u64::from(w);
    }
    if let Some(h) = slot.handle.as_ref() {
        h.command(Command::SetOptions {
            model,
            thinking,
            context_window: context_window.map(u64::from),
        })
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Switch the live session to another provider (cross-provider model
/// pick). Resolves the adapter from config and hands it to the actor —
/// adapters are stateless, so the next turn runs on the new provider
/// immediately. Also makes `provider` the configured default so new
/// sessions and the CLI agree with the pick.
#[tauri::command]
#[specta::specta]
pub fn session_set_provider(
    state: State<'_, Sessions>,
    provider: String,
    model: Option<String>,
) -> Result<(), String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    let (name, configured_model, adapter) = resolve_target(&ctx, Some(&provider))?;
    let model = model.unwrap_or(configured_model);
    let mut slot = state
        .inner
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    slot.provider = Some(name.clone());
    slot.model = Some(model.clone());
    if let Some(h) = slot.handle.as_ref() {
        h.command(Command::SetProvider {
            provider: adapter,
            provider_name: name.clone(),
            model: Some(model),
        })
        .map_err(|e| e.to_string())?;
    }
    let mut config = ctx.config.clone();
    config.default_provider = Some(name);
    config.save().map_err(|e| e.to_string())?;
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

/// In-flight logins, cancellable/finishable from the UI.
#[derive(Default)]
pub struct AuthState {
    /// Cancel token for the running Nous poll; `None` when idle.
    pub login_cancel: Mutex<Option<tokio_util::sync::CancellationToken>>,
    /// PKCE material for an in-progress Claude browser login.
    pub claude_pending: Mutex<Option<hate_auth::ClaudePending>>,
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

/// One row of the provider catalog — a known endpoint the UI can add
/// with one click. Secrets still come from keychain/env/OAuth; presets
/// only carry non-secret connection facts.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ProviderPreset {
    /// Config key this provider is stored under.
    pub name: String,
    /// Human-facing label.
    pub label: String,
    /// Adapter family (`openai_compat`, `anthropic`).
    pub kind: String,
    /// API base URL.
    pub base_url: String,
    /// Suggested default model id.
    pub model: String,
    /// `api_key` | `oauth` | `none` — how credentials are obtained.
    pub auth: String,
    /// Conventional env var that also authenticates, when one exists.
    pub env_hint: Option<String>,
    /// Where to get a key, for the UI hint line.
    pub key_url: Option<String>,
}

/// Well-known provider catalog. Adding a provider later is one row —
/// the upsert path is generic.
fn preset_catalog() -> Vec<ProviderPreset> {
    let p = |name: &str,
             label: &str,
             kind: &str,
             base_url: &str,
             model: &str,
             auth: &str,
             env_hint: Option<&str>,
             key_url: Option<&str>| ProviderPreset {
        name: name.to_string(),
        label: label.to_string(),
        kind: kind.to_string(),
        base_url: base_url.to_string(),
        model: model.to_string(),
        auth: auth.to_string(),
        env_hint: env_hint.map(str::to_string),
        key_url: key_url.map(str::to_string),
    };
    vec![
        p(
            "nous",
            "Nous Portal",
            "openai_compat",
            hate_auth::DEFAULT_INFERENCE_URL,
            "moonshotai/kimi-k3",
            "oauth",
            Some("NOUS_API_KEY"),
            Some("https://portal.nousresearch.com"),
        ),
        p(
            "claude",
            "Claude",
            "anthropic",
            "https://api.anthropic.com",
            "claude-sonnet-4-5",
            "oauth",
            Some("ANTHROPIC_API_KEY"),
            Some("https://console.anthropic.com/settings/keys"),
        ),
        p(
            "openai",
            "OpenAI",
            "openai_compat",
            "https://api.openai.com/v1",
            "gpt-5",
            "api_key",
            Some("OPENAI_API_KEY"),
            Some("https://platform.openai.com/api-keys"),
        ),
        p(
            "openrouter",
            "OpenRouter",
            "openai_compat",
            "https://openrouter.ai/api/v1",
            "anthropic/claude-sonnet-4.5",
            "api_key",
            Some("OPENROUTER_API_KEY"),
            Some("https://openrouter.ai/keys"),
        ),
        p(
            "deepseek",
            "DeepSeek",
            "openai_compat",
            "https://api.deepseek.com/v1",
            "deepseek-chat",
            "api_key",
            Some("DEEPSEEK_API_KEY"),
            Some("https://platform.deepseek.com/api_keys"),
        ),
        p(
            "xai",
            "xAI",
            "openai_compat",
            "https://api.x.ai/v1",
            "grok-4",
            "api_key",
            Some("XAI_API_KEY"),
            Some("https://console.x.ai"),
        ),
        p(
            "gemini",
            "Gemini",
            "openai_compat",
            "https://generativelanguage.googleapis.com/v1beta/openai/",
            "gemini-2.5-pro",
            "api_key",
            Some("GEMINI_API_KEY"),
            Some("https://aistudio.google.com/apikey"),
        ),
        p(
            "groq",
            "Groq",
            "openai_compat",
            "https://api.groq.com/openai/v1",
            "llama-3.3-70b-versatile",
            "api_key",
            Some("GROQ_API_KEY"),
            Some("https://console.groq.com/keys"),
        ),
        p(
            "mistral",
            "Mistral",
            "openai_compat",
            "https://api.mistral.ai/v1",
            "mistral-large-latest",
            "api_key",
            Some("MISTRAL_API_KEY"),
            Some("https://console.mistral.ai/api-keys"),
        ),
        p(
            "ollama",
            "Ollama",
            "openai_compat",
            "http://localhost:11434/v1",
            "llama3.1",
            "none",
            None,
            None,
        ),
    ]
}

/// The provider catalog for the Settings "add provider" grid.
#[tauri::command]
#[specta::specta]
pub fn provider_presets() -> Vec<ProviderPreset> {
    preset_catalog()
}

/// Create or update a provider entry in `config.toml`. Presets call
/// this with their catalog values; custom providers can too.
#[tauri::command]
#[specta::specta]
pub fn provider_upsert(
    name: String,
    kind: String,
    base_url: String,
    model: String,
) -> Result<(), String> {
    let mut ctx = AuthContext::load().map_err(|e| e.to_string())?;
    // Normalize through the parser so typos can't create dead kinds.
    let kind = ProviderKind::parse(&kind).name().to_string();
    ctx.config.providers.insert(
        name,
        hate_auth::ProviderConfig {
            kind,
            base_url,
            model,
        },
    );
    ctx.config.save().map_err(|e| e.to_string())
}

/// Remove a provider entry. Refuses the last provider so the app never
/// strands itself with zero configured backends.
#[tauri::command]
#[specta::specta]
pub fn provider_remove(name: String) -> Result<(), String> {
    let mut ctx = AuthContext::load().map_err(|e| e.to_string())?;
    if !ctx.config.providers.contains_key(&name) {
        return Err(format!("unknown provider {name:?}"));
    }
    if ctx.config.providers.len() <= 1 {
        return Err("can't remove the last provider".to_string());
    }
    ctx.config.providers.remove(&name);
    if ctx.config.default_provider.as_deref() == Some(name.as_str()) {
        ctx.config.default_provider = ctx.config.providers.keys().next().cloned();
    }
    ctx.config.save().map_err(|e| e.to_string())
}

/// Claude authorize URL for the Settings "Sign in with Claude" button.
/// PKCE material is stashed in [`AuthState`]; the browser opens here and
/// the user pastes the callback code into [`auth_claude_complete`].
#[derive(Debug, Clone, Serialize, Type)]
pub struct ClaudeLoginInfo {
    /// Full `claude.ai/oauth/authorize` URL (also opened in browser).
    pub authorize_url: String,
}

/// Begin Claude subscription login (Pro/Max OAuth). Stashes the PKCE
/// material, opens the authorize URL in the system browser, and returns
/// it for display; completion is [`auth_claude_complete`].
#[tauri::command]
#[specta::specta]
pub fn auth_claude_begin(state: State<'_, AuthState>) -> Result<ClaudeLoginInfo, String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    let auth = hate_auth::ClaudeAuth::new(std::sync::Arc::clone(&ctx.store));
    let login = auth.begin_login();
    if let Ok(mut slot) = state.claude_pending.lock() {
        *slot = Some(login.pending);
    }
    // Hand the approval page to the browser; the UI still shows the
    // link in case the open fails.
    let _ = webbrowser::open(&login.authorize_url);
    Ok(ClaudeLoginInfo {
        authorize_url: login.authorize_url,
    })
}

/// Finish Claude login with the code the callback page displayed
/// (bare code or `code#state` form). Persists tokens, makes sure a
/// `claude` provider entry exists, and reports on `auth-event`
/// (`{"type":"claude_login","ok":bool,"error"?:string}`).
#[tauri::command]
#[specta::specta]
pub async fn auth_claude_complete(
    state: State<'_, AuthState>,
    app: AppHandle,
    code: String,
) -> Result<(), String> {
    let pending = state
        .claude_pending
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .ok_or_else(|| "no Claude login in progress — start again".to_string())?;

    let mut ctx = AuthContext::load().map_err(|e| e.to_string())?;
    let auth = hate_auth::ClaudeAuth::new(std::sync::Arc::clone(&ctx.store));
    let result = auth.complete_login(&pending, &code).await;

    let payload = match &result {
        Ok(_) => {
            // Self-heal: a login with no matching config entry would be
            // unreachable — register the canonical provider.
            if !ctx.config.providers.contains_key("claude") {
                ctx.config.providers.insert(
                    "claude".to_string(),
                    hate_auth::ProviderConfig {
                        kind: "anthropic".to_string(),
                        base_url: "https://api.anthropic.com".to_string(),
                        model: "claude-sonnet-4-5".to_string(),
                    },
                );
                let _ = ctx.config.save();
            }
            serde_json::json!({"type": "claude_login", "ok": true})
        }
        Err(e) => {
            serde_json::json!({"type": "claude_login", "ok": false, "error": e.to_string()})
        }
    };
    let _ = app.emit("auth-event", payload);
    result.map(|_| ()).map_err(|e| e.to_string())
}

/// One row of the Settings → Skills list — metadata only; bodies load
/// on demand via [`skills_read`].
#[derive(Debug, Clone, Serialize, Type)]
pub struct SkillInfo {
    /// Slug (filename stem).
    pub name: String,
    /// Human title.
    pub title: String,
    /// Trigger keywords.
    pub triggers: Vec<String>,
    /// Body size in bytes.
    pub bytes: u32,
    /// Last-write unix seconds.
    pub updated: u32,
}

fn skill_store() -> Result<hate_core::SkillStore, String> {
    let dir = hate_core::SkillStore::default_dir()
        .ok_or_else(|| "no config directory available".to_string())?;
    Ok(hate_core::SkillStore::new(dir))
}

/// Learned skills, sorted by name.
#[tauri::command]
#[specta::specta]
pub fn skills_list() -> Result<Vec<SkillInfo>, String> {
    let store = skill_store()?;
    Ok(store
        .load()
        .into_iter()
        .map(|s| SkillInfo {
            name: s.name,
            title: s.title,
            triggers: s.triggers,
            bytes: u32::try_from(s.body.len()).unwrap_or(u32::MAX),
            updated: u32::try_from(s.updated).unwrap_or(u32::MAX),
        })
        .collect())
}

/// A skill's markdown body.
#[tauri::command]
#[specta::specta]
pub fn skills_read(name: String) -> Result<String, String> {
    let store = skill_store()?;
    store
        .get(&name)
        .map(|s| s.body)
        .ok_or_else(|| format!("no skill named {name:?}"))
}

/// Delete a skill file. Returns whether anything was removed.
#[tauri::command]
#[specta::specta]
pub fn skills_delete(name: String) -> Result<bool, String> {
    let store = skill_store()?;
    store.delete(&name)
}

/// Whether automatic skill authoring is on (config `auto_skills`).
#[tauri::command]
#[specta::specta]
pub fn skills_auto_get() -> Result<bool, String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    Ok(ctx.config.auto_skills)
}

/// Toggle automatic skill authoring (`config.toml` → `auto_skills`).
#[tauri::command]
#[specta::specta]
pub fn skills_set_auto(enabled: bool) -> Result<(), String> {
    let mut ctx = AuthContext::load().map_err(|e| e.to_string())?;
    ctx.config.auto_skills = enabled;
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

// ── MCP servers ─────────────────────────────────────────────────────────

/// Wire shape for one configured MCP server — mirrors
/// `hate_core::McpServerConfig` for specta/TS bindings.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct McpServerConfigDto {
    /// Unique display name → `mcp__<name>__` wire prefix.
    pub name: String,
    /// `"stdio"` or `"http"`.
    pub kind: String,
    /// Disabled servers stay configured but never connect.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// stdio: executable (`npx`, `uvx`, binary path).
    #[serde(default)]
    pub command: Option<String>,
    /// stdio: arguments after the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// stdio: extra environment on top of inherited vars.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// http: streamable-HTTP endpoint.
    #[serde(default)]
    pub url: Option<String>,
    /// http: headers sent on every request.
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
}

fn default_true() -> bool {
    true
}

/// One settings row: full config (editable) + live connection state.
#[derive(Debug, Clone, Serialize, Type)]
pub struct McpServerInfo {
    /// Configured name.
    pub name: String,
    /// `"stdio"` | `"http"`.
    pub kind: String,
    /// Config `enabled` flag.
    pub enabled: bool,
    /// stdio: executable.
    pub command: Option<String>,
    /// stdio: arguments.
    pub args: Vec<String>,
    /// stdio: extra environment.
    pub env: std::collections::BTreeMap<String, String>,
    /// http: endpoint.
    pub url: Option<String>,
    /// http: headers.
    pub headers: std::collections::BTreeMap<String, String>,
    /// `connected` | `connecting` | `error` | `disabled`.
    pub state: String,
    /// Tools the server advertised (0 until connected).
    /// u32 at the IPC boundary — specta forbids usize/u64 exports.
    pub tools: u32,
    /// Server name/version from the handshake.
    pub info: String,
}

/// Merge `hub.configs()` (editable fields) with `hub.status()` (live
/// state) into settings rows, keyed by name.
fn mcp_rows(state: &McpState) -> Vec<McpServerInfo> {
    let status = state.hub.status();
    state
        .hub
        .configs()
        .into_iter()
        .map(|c| {
            let st = status.iter().find(|s| s.name == c.name);
            McpServerInfo {
                name: c.name,
                kind: c.kind,
                enabled: c.enabled,
                command: c.command,
                args: c.args,
                env: c.env,
                url: c.url,
                headers: c.headers,
                state: st.map_or_else(|| "error".to_string(), |s| s.state.clone()),
                tools: st.map_or(0, |s| u32::try_from(s.tools).unwrap_or(u32::MAX)),
                info: st.map_or_else(String::new, |s| s.info.clone()),
            }
        })
        .collect()
}

/// Validate + normalize one DTO into a core config. Names must be
/// unique-ish (duplicates get suffixed) and each kind needs its
/// required field — bad rows are rejected, not silently dropped.
fn validate_mcp_config(
    dto: &McpServerConfigDto,
    seen: &mut Vec<String>,
) -> Result<hate_core::McpServerConfig, String> {
    let name = dto.name.trim().to_string();
    if name.is_empty() {
        return Err("MCP server needs a name".to_string());
    }
    if seen.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
        return Err(format!("duplicate MCP server name `{name}`"));
    }
    seen.push(name.clone());
    match dto.kind.as_str() {
        "stdio" => {
            let command = dto
                .command
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .ok_or("stdio server needs a command")?
                .to_string();
            Ok(hate_core::McpServerConfig {
                name,
                kind: "stdio".to_string(),
                enabled: dto.enabled,
                command: Some(command),
                args: dto.args.clone(),
                env: dto.env.clone(),
                url: None,
                headers: std::collections::BTreeMap::new(),
            })
        }
        "http" => {
            let url = dto
                .url
                .as_deref()
                .map(str::trim)
                .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
                .ok_or("http server needs an http(s) URL")?
                .to_string();
            Ok(hate_core::McpServerConfig {
                name,
                kind: "http".to_string(),
                enabled: dto.enabled,
                command: None,
                args: Vec::new(),
                env: std::collections::BTreeMap::new(),
                url: Some(url),
                headers: dto.headers.clone(),
            })
        }
        other => Err(format!("unknown MCP kind `{other}` (stdio|http)")),
    }
}

/// Configured servers + live connection state for the settings pane.
#[tauri::command]
#[specta::specta]
pub fn mcp_list(state: State<'_, McpState>) -> Vec<McpServerInfo> {
    mcp_rows(&state)
}

/// Replace the whole server list: validate → persist `mcp.json` →
/// reconnect the hub. Existing sessions pick the new tool set up on
/// their next turn; in-flight turns are untouched.
#[tauri::command]
#[specta::specta]
pub async fn mcp_save(
    state: State<'_, McpState>,
    servers: Vec<McpServerConfigDto>,
) -> Result<Vec<McpServerInfo>, String> {
    let mut seen = Vec::with_capacity(servers.len());
    let mut configs = Vec::with_capacity(servers.len());
    for dto in &servers {
        configs.push(validate_mcp_config(dto, &mut seen)?);
    }
    hate_core::McpHub::save_config(&state.path, &configs)?;
    state.hub.configure(configs).await;
    Ok(mcp_rows(&state))
}
