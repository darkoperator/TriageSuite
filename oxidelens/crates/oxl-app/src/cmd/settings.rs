//! The settings file's commands. Each answers a `SettingsReply`: the
//! settings, and the damaged settings file the app found at start, if any,
//! until the analyst next saves the settings.

use crate::error::CmdError;
use crate::settings::{Settings, SettingsReply};
use crate::state::AppState;

/// The settings now.
pub fn settings_get(state: &AppState) -> Result<SettingsReply, CmdError> {
    Ok(SettingsReply {
        settings: state.settings.get(),
        damaged: state.settings.damaged(),
        actor: state.actor().label(),
    })
}

/// Check `settings`, save them, and answer what was saved; `invalid`
/// (nothing saved) for a value out of range. The `recent` list sent is
/// ignored and the stored one kept: the backend owns it (opening a case adds
/// to it, `recent_remove` and `recent_clear` take from it).
pub fn settings_update(state: &AppState, settings: Settings) -> Result<SettingsReply, CmdError> {
    let settings = state.settings.save_keeping_recent(&settings)?;
    Ok(SettingsReply {
        settings,
        damaged: state.settings.damaged(),
        actor: state.actor().label(),
    })
}

/// Forget the recent cases: empty the list, save, and answer the settings.
pub fn recent_clear(state: &AppState) -> Result<SettingsReply, CmdError> {
    let settings = state.settings.update(|s| s.recent.clear())?;
    Ok(SettingsReply {
        settings,
        damaged: state.settings.damaged(),
        actor: state.actor().label(),
    })
}

/// Forget one recent case, `path` (a path not in the list changes
/// nothing), save, and answer the settings.
pub fn recent_remove(state: &AppState, path: &str) -> Result<SettingsReply, CmdError> {
    let settings = state.settings.update(|s| s.recent.retain(|p| p != path))?;
    Ok(SettingsReply {
        settings,
        damaged: state.settings.damaged(),
        actor: state.actor().label(),
    })
}

pub mod commands {
    use crate::error::CmdError;
    use crate::settings::{Settings, SettingsReply};
    use crate::state::AppState;
    use tauri::State;

    #[tauri::command(async)]
    pub fn settings_get(state: State<'_, AppState>) -> Result<SettingsReply, CmdError> {
        super::settings_get(&state)
    }

    #[tauri::command(async)]
    pub fn settings_update(
        state: State<'_, AppState>,
        settings: Settings,
    ) -> Result<SettingsReply, CmdError> {
        super::settings_update(&state, settings)
    }

    #[tauri::command(async)]
    pub fn recent_clear(state: State<'_, AppState>) -> Result<SettingsReply, CmdError> {
        super::recent_clear(&state)
    }

    #[tauri::command(async)]
    pub fn recent_remove(
        state: State<'_, AppState>,
        path: String,
    ) -> Result<SettingsReply, CmdError> {
        super::recent_remove(&state, &path)
    }
}
