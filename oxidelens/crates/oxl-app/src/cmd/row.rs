//! The detail pane's commands: one row with its provenance, one cell's
//! whole value split into its fields, the live counts on the ± pivot
//! buttons, and a merged list of rows across relations. Each is a read
//! under the window's query id and slot.

use super::At;
use crate::error::CmdError;
use crate::state::AppState;
use oxl_engine::{CellValue, Listing, MergedSource, MergedView, RowView, WindowTotal};

/// Row `row_id` for the detail pane, its UTC times in `zone` (`row_view`).
pub fn row_view(
    state: &AppState,
    at: At<'_>,
    row_id: i64,
    zone: &str,
) -> Result<RowView, CmdError> {
    at.read(state, |c| oxl_engine::row_view(c, row_id, zone))
}

/// The whole text of `column` in row `row_id`, split (`cell_value`).
pub fn cell_value(
    state: &AppState,
    at: At<'_>,
    row_id: i64,
    column: &str,
) -> Result<CellValue, CmdError> {
    at.read(state, |c| oxl_engine::cell_value(c, row_id, column))
}

/// For each of `windows`, the rows on the row's host within that many
/// seconds of it (`around_totals`).
pub fn around_totals(
    state: &AppState,
    at: At<'_>,
    row_id: i64,
    windows: &[u64],
) -> Result<Vec<WindowTotal>, CmdError> {
    at.read(state, |c| oxl_engine::around_totals(c, row_id, windows))
}

/// Up to `limit` rows of `source` from `offset`, across relations, each
/// time also in `zone` (`merged_view`).
pub fn merged_page(
    state: &AppState,
    at: At<'_>,
    source: &MergedSource,
    limit: u32,
    offset: u64,
    zone: &str,
) -> Result<Listing<MergedView>, CmdError> {
    at.read(state, |c| {
        oxl_engine::merged_view(c, source, limit, offset, zone)
    })
}

// Each wrapper's parameters are the command's IPC arguments, named as the
// window sends them; Tauri takes them one by one, not as a struct.
#[allow(clippy::too_many_arguments)]
pub mod commands {
    use crate::cmd::At;
    use crate::error::CmdError;
    use crate::state::AppState;
    use oxl_engine::{CellValue, Listing, MergedSource, MergedView, RowView, WindowTotal};
    use tauri::State;

    #[tauri::command(async)]
    pub fn row_view(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        row_id: i64,
        zone: String,
    ) -> Result<RowView, CmdError> {
        super::row_view(&app, At::new(&query_id, &slot), row_id, &zone)
    }

    #[tauri::command(async)]
    pub fn cell_value(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        row_id: i64,
        column: String,
    ) -> Result<CellValue, CmdError> {
        super::cell_value(&app, At::new(&query_id, &slot), row_id, &column)
    }

    #[tauri::command(async)]
    pub fn around_totals(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        row_id: i64,
        windows: Vec<u64>,
    ) -> Result<Vec<WindowTotal>, CmdError> {
        super::around_totals(&app, At::new(&query_id, &slot), row_id, &windows)
    }

    #[tauri::command(async)]
    pub fn merged_page(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        source: MergedSource,
        limit: u32,
        offset: u64,
        zone: String,
    ) -> Result<Listing<MergedView>, CmdError> {
        let at = At::new(&query_id, &slot);
        super::merged_page(&app, at, &source, limit, offset, &zone)
    }
}
