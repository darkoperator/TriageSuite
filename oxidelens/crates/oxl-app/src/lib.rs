//! The OxideLens desktop app's backend: the Tauri commands over the case
//! engine, and the state they share.
//!
//! The commands are thin: every rule is the engine's. What the app adds is
//! how the engine is driven from a window: one writer connection that alone
//! changes the case (`state`), a pool of reader connections so reads never
//! wait for each other or for a write, and a registry (`queries`) under
//! which every read can be superseded by a newer one or cancelled. Every
//! command is a plain function over `AppState`, which is what the tests
//! call; `cmd::*::commands` wraps each for Tauri.

pub mod cmd;
pub mod error;
pub mod events;
pub mod queries;
pub mod settings;
pub mod state;

pub use error::CmdError;
pub use state::AppState;

/// Every command the window may call.
pub fn invoke_handler<R: tauri::Runtime>(
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        cmd::case::commands::case_new,
        cmd::case::commands::case_open,
        cmd::case::commands::case_close,
        cmd::load::commands::load_run,
        cmd::load::commands::load_file,
        cmd::settings::commands::settings_get,
        cmd::settings::commands::settings_update,
        cmd::commands::query_cancel,
    ]
}
