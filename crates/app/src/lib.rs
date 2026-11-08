//! hate-app: the Tauri layer. This crate is deliberately thin — commands
//! and event forwarding only. Every decision about agent behavior lives in
//! `hate-core` and below; nothing here should ever grow logic.

#![deny(unsafe_code)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use tauri::Manager;
use tauri_specta::{Builder, collect_commands};

/// Application entry point. Called by `src/main.rs`.
pub fn run() {
    let builder = Builder::new().commands(collect_commands![
        commands::app_info,
        commands::session_send,
        commands::session_cancel,
        commands::session_answer,
        commands::session_respond_permission,
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
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            app.manage(commands::Sessions::default());
            builder.mount_events(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running hate-agent");
}
