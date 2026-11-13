// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use oxl_app::events::TauriSink;
use oxl_app::state::{close_within, AppState, EXIT_CLOSE_LIMIT};
use std::sync::Arc;
use tauri::Manager;

fn main() {
    let builder = tauri::Builder::default();
    // Debug builds only: lets an agent drive the window through the Tauri MCP
    // server, over a WebSocket on the loopback interface.
    #[cfg(debug_assertions)]
    let builder = builder.plugin(
        tauri_plugin_mcp_bridge::Builder::new()
            .bind_address("127.0.0.1")
            .build(),
    );
    builder
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let events = Arc::new(TauriSink(app.handle().clone()));
            app.manage(AppState::with_settings_dir(&dir, events));
            Ok(())
        })
        .invoke_handler(oxl_app::invoke_handler())
        .build(tauri::generate_context!())
        .expect("the OxideLens window could not be built")
        .run(|app, event| {
            // Quitting closes the case (its queries interrupted, its readers
            // then its writer dropped) off the event loop's thread, and waits
            // for that at most EXIT_CLOSE_LIMIT: a reader held by work no
            // interrupt reaches must not hold up the exit.
            if let tauri::RunEvent::Exit = event {
                let app = app.clone();
                close_within(
                    move || {
                        if let Some(state) = app.try_state::<AppState>() {
                            let _ = state.close();
                        }
                    },
                    EXIT_CLOSE_LIMIT,
                );
            }
        });
}
