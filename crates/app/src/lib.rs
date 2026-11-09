#![deny(unsafe_code)]

mod commands;
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
        commands::session_rewind,
        commands::session_set_options,
        commands::session_set_provider,
        commands::models_list,
        commands::provider_set_model,
        commands::provider_presets,
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
        commands::workspace_get,
        commands::workspace_set,
        commands::workspace_pick,
        commands::workspace_index,
        commands::mcp_list,
        commands::mcp_refresh,
        commands::mcp_save,
        commands::sessions_list,
        commands::session_new,
        commands::session_open,
        commands::session_delete,
        commands::session_save_ui,
        commands::update_check,
        commands::update_install,
    ]);

    #[cfg(debug_assertions)]
    builder
        .export(
            specta_typescript::Typescript::default(),
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../apps/desktop/src/bindings.ts"
            ),
        )
        .expect("failed to export TypeScript bindings");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            app.manage(commands::Sessions::default());
            app.manage(commands::AuthState::default());
            app.manage(commands::UpdateState::default());
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
