use std::sync::Mutex;
use std::time::Duration;

use hate_acp::agent::{self, Agent, GOOGLE_LOGIN, ModelChoice};
use hate_acp::runtime;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Emitter, State};

pub const KIND: &str = "acp";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Default)]
pub struct AntigravityState {
    login_cancel: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    busy: tokio::sync::Mutex<()>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cache {
    pub signed_in: bool,
    #[serde(default)]
    pub models: Vec<ModelChoice>,
    #[serde(default)]
    pub current: Option<String>,
}

fn cache_path() -> std::path::PathBuf {
    runtime::agent_home().join("hate-state.json")
}

pub fn load_cache() -> Cache {
    std::fs::read_to_string(cache_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_cache(c: &Cache) {
    let _ = std::fs::create_dir_all(runtime::agent_home());
    if let Ok(text) = serde_json::to_string_pretty(c) {
        let _ = std::fs::write(cache_path(), text);
    }
}

fn settings_path() -> std::path::PathBuf {
    runtime::agent_home()
        .join("antigravity-acp")
        .join("settings.json")
}

fn set_auth_type(kind: Option<&str>) -> Result<(), String> {
    let path = settings_path();
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    match kind {
        Some(k) => v["auth"] = serde_json::json!({"type": k}),
        None => {
            if let Some(o) = v.as_object_mut() {
                o.remove("auth");
            }
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap_or_default())
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct AntigravityStatus {
    pub supported: bool,
    pub platform: Option<String>,
    pub installed: Option<String>,
    pub signed_in: bool,
    pub models: u32,
}

fn status_now() -> AntigravityStatus {
    let cache = load_cache();
    AntigravityStatus {
        supported: runtime::platform_key().is_some(),
        platform: runtime::platform_key().map(str::to_string),
        installed: runtime::installed().map(|i| i.version),
        signed_in: cache.signed_in,
        models: u32::try_from(cache.models.len()).unwrap_or(u32::MAX),
    }
}

async fn probe() -> Result<Cache, String> {
    let inst = runtime::installed();
    let started = Agent::start(inst.as_ref())
        .await
        .map_err(|e| e.to_string())?;
    let agent = started.agent;
    let scratch = runtime::scratch_dir();
    let _ = std::fs::create_dir_all(&scratch);
    let res = agent
        .new_session(&scratch.to_string_lossy(), serde_json::json!([]))
        .await;
    agent.shutdown().await;
    let mut cache = load_cache();
    match res {
        Ok(s) => {
            let (models, current) = agent::models(&s.config_options);
            cache.signed_in = true;
            if !models.is_empty() {
                cache.models = models;
            }
            cache.current = current;
        }
        Err(e) if e.is_auth_required() => cache.signed_in = false,
        Err(e) => return Err(format!("Antigravity check failed: {}", e.detail())),
    }
    save_cache(&cache);
    Ok(cache)
}

#[tauri::command]
#[specta::specta]
pub fn antigravity_status() -> AntigravityStatus {
    status_now()
}

#[tauri::command]
#[specta::specta]
pub async fn antigravity_install(
    app: AppHandle,
    state: State<'_, AntigravityState>,
) -> Result<AntigravityStatus, String> {
    let _busy = state.busy.lock().await;
    let app2 = app.clone();
    let mut last_pct = u64::MAX;
    runtime::install(move |downloaded, total| {
        let pct = total.map_or(downloaded / (1024 * 1024), |t| downloaded * 100 / t.max(1));
        if pct != last_pct {
            last_pct = pct;
            let _ = app2.emit(
                "antigravity-event",
                serde_json::json!({"kind": "download", "downloaded": downloaded, "total": total}),
            );
        }
    })
    .await?;
    Ok(status_now())
}

#[tauri::command]
#[specta::specta]
pub async fn antigravity_refresh(
    state: State<'_, AntigravityState>,
) -> Result<AntigravityStatus, String> {
    let _busy = state.busy.lock().await;
    if runtime::installed().is_some() {
        probe().await?;
    }
    Ok(status_now())
}

#[tauri::command]
#[specta::specta]
pub async fn antigravity_login(
    app: AppHandle,
    state: State<'_, AntigravityState>,
) -> Result<AntigravityStatus, String> {
    let _busy = state.busy.lock().await;
    let inst = runtime::installed();
    let started = Agent::start(inst.as_ref())
        .await
        .map_err(|e| e.to_string())?;
    if !started.agent.supports_google_login() {
        started.agent.shutdown().await;
        return Err("this Antigravity build doesn't offer Google sign-in".to_string());
    }
    let mut stderr = started.stderr;
    let app2 = app.clone();
    tokio::spawn(async move {
        while let Some(line) = stderr.recv().await {
            if let Some(url) = agent::login_url(&line) {
                let _ = app2.emit(
                    "antigravity-event",
                    serde_json::json!({"kind": "login_url", "url": url}),
                );
            }
        }
    });
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    if let Ok(mut slot) = state.login_cancel.lock() {
        *slot = Some(cancel_tx);
    }
    let agent = started.agent;
    let outcome = tokio::select! {
        r = tokio::time::timeout(LOGIN_TIMEOUT, agent.authenticate(GOOGLE_LOGIN)) => match r {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(format!("Google sign-in failed: {}", e.detail())),
            Err(_) => Err("Google sign-in timed out".to_string()),
        },
        _ = cancel_rx => Err("sign-in cancelled".to_string()),
    };
    if let Ok(mut slot) = state.login_cancel.lock() {
        slot.take();
    }
    agent.shutdown().await;
    outcome?;
    set_auth_type(Some(GOOGLE_LOGIN))?;
    let cache = probe().await?;
    if !cache.signed_in {
        return Err("Google sign-in finished but Antigravity still reports no account".to_string());
    }
    let _ = app.emit(
        "antigravity-event",
        serde_json::json!({"kind": "signed_in"}),
    );
    Ok(status_now())
}

#[tauri::command]
#[specta::specta]
pub fn antigravity_login_cancel(state: State<'_, AntigravityState>) {
    if let Some(tx) = state.login_cancel.lock().ok().and_then(|mut s| s.take()) {
        let _ = tx.send(());
    }
}

#[tauri::command]
#[specta::specta]
pub async fn antigravity_logout(
    state: State<'_, AntigravityState>,
) -> Result<AntigravityStatus, String> {
    let _busy = state.busy.lock().await;
    if let Some(inst) = runtime::installed()
        && let Ok(started) = Agent::start(Some(&inst)).await
    {
        if started.agent.can_logout()
            && let Err(e) = started.agent.logout().await
        {
            tracing::warn!(error = %e, "antigravity logout");
        }
        started.agent.shutdown().await;
    }
    set_auth_type(None)?;
    save_cache(&Cache::default());
    Ok(status_now())
}

pub fn models() -> Vec<ModelChoice> {
    load_cache().models
}
