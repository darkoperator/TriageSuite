//! The settings file's commands. Both answer a `SettingsReply`: the
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
/// (nothing saved) for a value out of range.
pub fn settings_update(state: &AppState, settings: Settings) -> Result<SettingsReply, CmdError> {
    let settings = state.settings.save(&settings)?;
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
}
