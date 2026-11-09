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

#[derive(Debug, Clone, Serialize, Type)]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
    pub platform: String,
    pub arch: String,
}

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

#[derive(Default)]
pub struct Sessions {
    inner: Mutex<SessionSlot>,
}

#[derive(Default)]
struct SessionSlot {
    handle: Option<SessionHandle>,
    key: Option<String>,
    mode: String,
    workspace: Option<String>,
    provider: Option<String>,
    restore: Option<PersistedSession>,
    model: Option<String>,
    thinking: Option<ThinkingLevel>,
    context_window: u64,
}

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

fn ensure_session(state: &Sessions, app: &AppHandle) -> Result<SessionHandle, String> {
    let mut guard = state
        .inner
        .lock()
        .map_err(|_| "session lock poisoned".to_string())?;
    if let Some(h) = guard.handle.as_ref() {
        return Ok(h.clone());
    }

    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    let provider_override = guard
        .provider
        .clone()
        .or_else(|| guard.restore.as_ref().map(|r| r.provider.clone()))
        .filter(|p| !p.is_empty());
    let (name, configured_model, provider) = resolve_target(&ctx, provider_override.as_deref())?;

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

    let tools = if guard.mode == "code" {
        guard
            .workspace
            .clone()
            .or_else(|| ctx.config.workspace_path.clone())
            .and_then(|p| match WorkspaceTools::new(&p) {
                Ok(t) => {
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
    let mcp = app.state::<McpState>().hub.clone();
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

pub struct McpState {
    pub hub: std::sync::Arc<hate_core::McpHub>,
    pub path: std::path::PathBuf,
    pub pending: Vec<hate_core::McpServerConfig>,
}

impl McpState {
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

fn mcp_config_path() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("hate-agent")
        .join("mcp.json")
}

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

#[tauri::command]
#[specta::specta]
pub fn session_rewind(state: State<'_, Sessions>, checkpoint_id: String) -> Result<(), String> {
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

#[tauri::command]
#[specta::specta]
pub fn session_set_options(
    state: State<'_, Sessions>,
    model: Option<String>,
    thinking: Option<String>,
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

#[derive(Debug, Clone, serde::Serialize, Type)]
pub struct NousLoginInfo {
    pub user_code: String,
    pub verification_url: String,
    pub verification_url_complete: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, Type)]
pub struct AuthStatus {
    pub kind: String,
    pub expires_at: Option<u32>,
}

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

#[derive(Default)]
pub struct AuthState {
    pub login_cancel: Mutex<Option<tokio_util::sync::CancellationToken>>,
    pub claude_pending: Mutex<Option<hate_auth::ClaudePending>>,
}

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

    let cancel = CancellationToken::new();
    if let Ok(mut slot) = state.login_cancel.lock()
        && let Some(old) = slot.replace(cancel.clone())
    {
        old.cancel();
    }

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

    match tokio::time::timeout(std::time::Duration::from_secs(30), info_rx).await {
        Ok(Ok(info)) => {
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

#[tauri::command]
#[specta::specta]
pub fn auth_nous_cancel(state: State<'_, AuthState>) {
    if let Ok(mut slot) = state.login_cancel.lock()
        && let Some(cancel) = slot.take()
    {
        cancel.cancel();
    }
}

#[tauri::command]
#[specta::specta]
pub fn auth_set_key(provider: String, key: String) -> Result<(), String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    ctx.set_api_key(&provider, &key).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn auth_logout(provider: String) -> Result<(), String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    ctx.logout(&provider).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, serde::Serialize, Type)]
pub struct ProviderInfo {
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub model: String,
    pub is_default: bool,
    pub auth_kind: String,
    pub auth_expires_at: Option<u32>,
}

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

#[derive(Debug, Clone, serde::Serialize, Type)]
pub struct ProviderModel {
    pub id: String,
    pub name: Option<String>,
    pub context_tokens: Option<u32>,
    pub price_in_mtok: Option<f64>,
    pub price_out_mtok: Option<f64>,
    pub price_cache_mtok: Option<f64>,
    pub reasoning_efforts: Vec<String>,
    pub default_effort: Option<String>,
    pub supports_tools: Option<bool>,
    pub supports_vision: Option<bool>,
}

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

#[derive(Debug, Clone, Serialize, Type)]
pub struct ProviderPreset {
    pub name: String,
    pub label: String,
    pub kind: String,
    pub base_url: String,
    pub model: String,
    pub auth: String,
    pub env_hint: Option<String>,
    pub key_url: Option<String>,
}

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

#[tauri::command]
#[specta::specta]
pub fn provider_presets() -> Vec<ProviderPreset> {
    preset_catalog()
}

#[tauri::command]
#[specta::specta]
pub fn provider_upsert(
    name: String,
    kind: String,
    base_url: String,
    model: String,
) -> Result<(), String> {
    let mut ctx = AuthContext::load().map_err(|e| e.to_string())?;
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

#[derive(Debug, Clone, Serialize, Type)]
pub struct ClaudeLoginInfo {
    pub authorize_url: String,
}

#[tauri::command]
#[specta::specta]
pub fn auth_claude_begin(state: State<'_, AuthState>) -> Result<ClaudeLoginInfo, String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    let auth = hate_auth::ClaudeAuth::new(std::sync::Arc::clone(&ctx.store));
    let login = auth.begin_login();
    if let Ok(mut slot) = state.claude_pending.lock() {
        *slot = Some(login.pending);
    }
    let _ = webbrowser::open(&login.authorize_url);
    Ok(ClaudeLoginInfo {
        authorize_url: login.authorize_url,
    })
}

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

#[derive(Debug, Clone, Serialize, Type)]
pub struct SkillInfo {
    pub name: String,
    pub title: String,
    pub triggers: Vec<String>,
    pub bytes: u32,
    pub updated: u32,
}

fn skill_store() -> Result<hate_core::SkillStore, String> {
    let dir = hate_core::SkillStore::default_dir()
        .ok_or_else(|| "no config directory available".to_string())?;
    Ok(hate_core::SkillStore::new(dir))
}

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

#[tauri::command]
#[specta::specta]
pub fn skills_read(name: String) -> Result<String, String> {
    let store = skill_store()?;
    store
        .get(&name)
        .map(|s| s.body)
        .ok_or_else(|| format!("no skill named {name:?}"))
}

#[tauri::command]
#[specta::specta]
pub fn skills_delete(name: String) -> Result<bool, String> {
    let store = skill_store()?;
    store.delete(&name)
}

#[tauri::command]
#[specta::specta]
pub fn skills_auto_get() -> Result<bool, String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    Ok(ctx.config.auto_skills)
}

#[tauri::command]
#[specta::specta]
pub fn skills_set_auto(enabled: bool) -> Result<(), String> {
    let mut ctx = AuthContext::load().map_err(|e| e.to_string())?;
    ctx.config.auto_skills = enabled;
    ctx.config.save().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn workspace_get() -> Result<Option<String>, String> {
    let ctx = AuthContext::load().map_err(|e| e.to_string())?;
    Ok(ctx.config.workspace_path.clone())
}

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

#[tauri::command]
#[specta::specta]
pub async fn workspace_pick(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;

    let picked =
        tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
            .await
            .map_err(|e| format!("picker task failed: {e}"))?;
    Ok(picked
        .and_then(|p| p.into_path().ok())
        .map(|p| dunce::simplified(&p).display().to_string()))
}

#[derive(Debug, Clone, serde::Serialize, Type)]
pub struct SessionMetaInfo {
    pub id: String,
    pub mode: String,
    pub title: String,
    pub provider: String,
    pub model: String,
    pub workspace: Option<String>,
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

#[derive(Debug, Clone, serde::Serialize, Type)]
pub struct SessionOpen {
    pub meta: SessionMetaInfo,
    pub ui_state: Option<String>,
}

fn db() -> Result<SessionDb, String> {
    SessionDb::open()
}

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

#[tauri::command]
#[specta::specta]
pub fn session_new(state: State<'_, Sessions>, mode: String) -> Result<SessionMetaInfo, String> {
    if mode != "chat" && mode != "code" {
        return Err(format!("unknown session mode {mode:?}"));
    }
    ensure_idle(&state)?;

    let key = uuid::Uuid::new_v4().to_string();
    db()?.create(&key, &mode)?;

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

#[tauri::command]
#[specta::specta]
pub fn session_save_ui(id: String, ui_state: String) -> Result<(), String> {
    if ui_state.len() > 8 * 1024 * 1024 {
        return Err("ui_state too large".to_string());
    }
    db()?.save_ui(&id, &ui_state)
}

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

#[tauri::command]
#[specta::specta]
pub fn workspace_index(path: String) -> Result<WorkspaceIndexOut, String> {
    let idx = hate_core::index::build_index(std::path::Path::new(&path))
        .map_err(|e| format!("index failed: {e}"))?;
    serde_json::from_value(serde_json::to_value(&idx).map_err(|e| e.to_string())?)
        .map_err(|e| format!("index shape mismatch: {e}"))
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct McpServerConfigDto {
    pub name: String,
    pub kind: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct McpServerInfo {
    pub name: String,
    pub kind: String,
    pub enabled: bool,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: std::collections::BTreeMap<String, String>,
    pub url: Option<String>,
    pub headers: std::collections::BTreeMap<String, String>,
    pub state: String,
    pub tools: u32,
    pub info: String,
}

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

#[tauri::command]
#[specta::specta]
pub fn mcp_list(state: State<'_, McpState>) -> Vec<McpServerInfo> {
    mcp_rows(&state)
}

#[tauri::command]
#[specta::specta]
pub async fn mcp_refresh(state: State<'_, McpState>) -> Result<Vec<McpServerInfo>, String> {
    state.hub.ensure_live(true).await;
    Ok(mcp_rows(&state))
}

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

#[derive(Default)]
pub struct UpdateState {
    pub pending: Mutex<Option<tauri_plugin_updater::Update>>,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: Option<String>,
    pub date: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub async fn update_check(
    app: AppHandle,
    state: State<'_, UpdateState>,
) -> Result<Option<UpdateInfo>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    let info = update.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        notes: u.body.clone(),
        date: u.date.map(|d| d.to_string()),
    });
    *state.pending.lock().map_err(|_| "update lock poisoned")? = update;
    Ok(info)
}

#[tauri::command]
#[specta::specta]
pub async fn update_install(app: AppHandle, state: State<'_, UpdateState>) -> Result<(), String> {
    let update = state
        .pending
        .lock()
        .map_err(|_| "update lock poisoned")?
        .take()
        .ok_or_else(|| "no pending update — run update_check first".to_string())?;
    let app2 = app.clone();
    update
        .download_and_install(
            move |downloaded, total| {
                let _ = app2.emit(
                    "update-event",
                    serde_json::json!({"kind": "downloading", "downloaded": downloaded, "total": total}),
                );
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}
