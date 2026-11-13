//! The commands, each a plain function over `AppState` (what the tests
//! call), wrapped for Tauri in each module's `commands`. Every wrapper is
//! `#[tauri::command(async)]`, so no command runs on the window's thread.

pub mod case;
pub mod load;
pub mod settings;

use crate::error::CmdError;
use crate::queries::check_name;
use crate::state::AppState;

/// Query ids one `query_cancel` may name.
pub const MAX_CANCEL: usize = 256;

/// Stop the queries `query_ids` names (see `QueryRegistry::cancel`).
pub fn query_cancel(state: &AppState, query_ids: &[String]) -> Result<(), CmdError> {
    if query_ids.len() > MAX_CANCEL {
        return Err(CmdError::invalid(format!(
            "at most {MAX_CANCEL} query ids per cancel"
        )));
    }
    for id in query_ids {
        check_name("query id", id)?;
    }
    state.queries.cancel(query_ids);
    Ok(())
}

pub mod commands {
    use crate::error::CmdError;
    use crate::state::AppState;
    use tauri::State;

    #[tauri::command(async)]
    pub fn query_cancel(
        state: State<'_, AppState>,
        query_ids: Vec<String>,
    ) -> Result<(), CmdError> {
        super::query_cancel(&state, &query_ids)
    }
}
