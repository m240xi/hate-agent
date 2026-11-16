#![deny(unsafe_code)]

mod antigravity;
mod commands;
mod images;
mod presence;
mod sessions_db;

use tauri::Manager;
use tauri_specta::{Builder, collect_commands};

pub fn run() {
    let builder = Builder::new().commands(collect_commands![
        commands::app_info,
        commands::session_send,
        commands::session_cancel,
        commands::session_compact,
        commands::session_answer,
        commands::session_respond_permission,
        commands::session_rewind_message,
        commands::session_set_options,
        commands::session_set_provider,
        commands::models_list,
        commands::provider_set_model,
        commands::provider_presets,
        commands::provider_preview_models,
        commands::prefs_get,
        commands::prefs_set,
        commands::usage_overview,
        commands::export_save,
        commands::session_rewind,
        commands::provider_upsert,
        commands::provider_remove,
        commands::skills_list,
        commands::skills_read,
        commands::skills_delete,
        commands::skills_auto_get,
        commands::skills_set_auto,
        commands::auth_nous_login,
        commands::auth_nous_cancel,
        commands::auth_claude_begin,
        commands::auth_claude_complete,
        commands::auth_status,
        commands::auth_set_key,
        commands::auth_logout,
        commands::providers_list,
        commands::session_set_workspace,
        commands::workspace_pick,
        commands::workspace_index,
        commands::workspace_read,
        commands::image_read,
        commands::drop_inspect,
        commands::image_save,
        commands::image_reveal,
        commands::workspace_changes,
        commands::workspace_diff,
        commands::mcp_list,
        commands::mcp_refresh,
        commands::mcp_save,
        commands::sessions_list,
        commands::session_new,
        commands::session_open,
        commands::session_delete,
        commands::session_rename,
        commands::session_save_ui,
        commands::update_check,
        commands::update_install,
        commands::presence_set_mode,
        antigravity::antigravity_status,
        antigravity::antigravity_install,
        antigravity::antigravity_refresh,
        antigravity::antigravity_login,
        antigravity::antigravity_login_cancel,
        antigravity::antigravity_logout,
        commands::presence_get,
        commands::presence_set,
    ]);

    // Regenerate the TypeScript bindings on every debug launch. A shape the
    // exporter refuses (e.g. a u64 field, which Specta forbids as BigInt) used
    // to take the whole app down before the window opened, which made
    // `tauri dev` unusable; warn loudly instead and let the app boot.
    #[cfg(debug_assertions)]
    if let Err(e) = builder.export(
        specta_typescript::Typescript::default(),
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../apps/desktop/src/bindings.ts"
        ),
    ) {
        eprintln!("warning: TypeScript bindings were not exported: {e}");
        eprintln!(
            "warning: apps/desktop/src/bindings.ts is now stale. Fix the offending export              (see the message above), then restart."
        );
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            app.manage(commands::Sessions::default());
            app.manage(commands::AuthState::default());
            app.manage(commands::UpdateState::default());
            app.manage(antigravity::AntigravityState::default());
            let discord_on =
                hate_auth::AuthContext::load().map_or(true, |c| c.config.discord_presence);
            let presence = presence::Presence::start(discord_on);
            app.manage(commands::PresenceState(presence));
            let mcp = commands::McpState::load();
            if !mcp.pending.is_empty() {
                let hub = mcp.hub.clone();
                let pending = mcp.pending.clone();
                tauri::async_runtime::spawn(async move {
                    hub.configure(pending).await;
                });
            }
            app.manage(mcp);
            builder.mount_events(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running hate-agent");
}
