//! hate-app: the Tauri layer. This crate is deliberately thin — commands
//! and event forwarding only. Every decision about agent behavior lives in
//! `hate-core` and below; nothing here should ever grow logic.

#![deny(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod sessions_db;

use tauri::Manager;
use tauri_specta::{Builder, collect_commands};

/// Application entry point. Called by `src/main.rs`.
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
        commands::mcp_save,
        commands::sessions_list,
        commands::session_new,
        commands::session_open,
        commands::session_delete,
        commands::session_save_ui,
    ]);

    // Emit TypeScript bindings for the frontend's typed invoke wrapper.
    // Path is compile-time absolute so it works regardless of the runtime cwd.
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
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            app.manage(commands::Sessions::default());
            app.manage(commands::AuthState::default());
            // MCP servers connect once at startup; sessions share the
            // hub so a settings save reaches every session next turn.
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
