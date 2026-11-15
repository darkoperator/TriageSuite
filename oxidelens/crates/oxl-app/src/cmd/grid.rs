//! The grid's commands: its page, counts, groups, histogram, a column's
//! values, the empty columns, the find positions, the selection's row ids
//! and their places after a re-read, and the chips and colour
//! rules the window builds. Each takes the grid's whole `GridState` and is
//! a read under the window's query id and slot; the CSV export is also a
//! job (`grid_export`).

use super::At;
use crate::error::CmdError;
use crate::state::{AppState, Point};
use oxl_engine::{
    Chip, ColourRule, ExportReport, GridCounts, GridPage, GridState, GroupView, Histogram, Listing,
    MatchPos, PageOptions, ValueCount, ValueOrder,
};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// The kind `job-started` and `job-finished` carry for an export.
pub const EXPORT_JOB: &str = "export";

/// Up to `limit` rows of what `grid` shows from `offset`, rendered in
/// `options.zone` with `options.rules` matched (`page_view`).
pub fn grid_page(
    state: &AppState,
    at: At<'_>,
    grid: &GridState,
    limit: u32,
    offset: u64,
    options: &PageOptions,
) -> Result<GridPage, CmdError> {
    at.read(state, |c| {
        oxl_engine::page_view(c, grid, limit, offset, options)
    })
}

/// The status bar's counts (`count_view`).
pub fn grid_counts(state: &AppState, at: At<'_>, grid: &GridState) -> Result<GridCounts, CmdError> {
    at.read(state, |c| oxl_engine::count_view(c, grid))
}

/// The group bar's groups (`groups_view`).
// The command's own arguments, each one part of the group bar's request
// (`groups_view` takes the same), after the state and where the read runs.
#[allow(clippy::too_many_arguments)]
pub fn grid_groups(
    state: &AppState,
    at: At<'_>,
    grid: &GridState,
    by: &[String],
    parent: &[Option<String>],
    limit: u32,
    offset: u64,
    zone: &str,
) -> Result<Listing<GroupView>, CmdError> {
    at.read(state, |c| {
        oxl_engine::groups_view(c, grid, by, parent, limit, offset, zone)
    })
}

/// The histogram strip over `column`, or the primary time
/// (`histogram_view`).
pub fn grid_histogram(
    state: &AppState,
    at: At<'_>,
    grid: &GridState,
    column: Option<&str>,
    max_buckets: u32,
    zone: &str,
) -> Result<Histogram, CmdError> {
    at.read(state, |c| {
        oxl_engine::histogram_view(c, grid, column, max_buckets, zone)
    })
}

/// A column's values and their counts, the chips `except_chips` names set
/// aside (`values`).
// The command's own arguments, each one input of the Values tab's request
// (`values` takes the same), after the state and where the read runs.
#[allow(clippy::too_many_arguments)]
pub fn grid_values(
    state: &AppState,
    at: At<'_>,
    grid: &GridState,
    column: &str,
    except_chips: &[String],
    contains: Option<&str>,
    order: ValueOrder,
    limit: u32,
    offset: u64,
) -> Result<Listing<ValueCount>, CmdError> {
    at.read(state, |c| {
        oxl_engine::values(
            c,
            grid,
            column,
            except_chips,
            contains,
            order,
            limit,
            offset,
        )
    })
}

/// The columns with no text among the rows `grid` selects
/// (`empty_columns`).
pub fn grid_empty_columns(
    state: &AppState,
    at: At<'_>,
    grid: &GridState,
) -> Result<Vec<String>, CmdError> {
    at.read(state, |c| oxl_engine::empty_columns(c, grid))
}

/// The positions of the rows a find-mode search selects (`matches`).
pub fn grid_matches(
    state: &AppState,
    at: At<'_>,
    grid: &GridState,
    from: Option<u64>,
    backward: bool,
    limit: u32,
) -> Result<Listing<MatchPos>, CmdError> {
    at.read(state, |c| {
        oxl_engine::matches(c, grid, from, backward, limit)
    })
}

/// How many rows a find-mode search selects (`match_count`).
pub fn grid_match_count(state: &AppState, at: At<'_>, grid: &GridState) -> Result<u64, CmdError> {
    at.read(state, |c| oxl_engine::match_count(c, grid))
}

/// Where row `row_id` sits in what `grid` shows (`row_offset`): its offset in
/// the grid's order, or `None` when the grid leaves it out.
pub fn grid_row_offset(
    state: &AppState,
    at: At<'_>,
    grid: &GridState,
    row_id: i64,
) -> Result<Option<u64>, CmdError> {
    at.read(state, |c| oxl_engine::row_offset(c, grid, row_id))
}

/// The row ids at the inclusive index `ranges` of what `grid` shows, in its
/// order (`row_ids_at`): what an action on the selection acts on. At most
/// `MAX_ACTION_ROWS` indexes, or `invalid` naming the count.
pub fn grid_row_ids(
    state: &AppState,
    at: At<'_>,
    grid: &GridState,
    ranges: &[(u64, u64)],
) -> Result<Vec<i64>, CmdError> {
    at.read(state, |c| oxl_engine::row_ids_at(c, grid, ranges))
}

/// Each of `ids` with its index in what `grid` shows, or `None` when it is no
/// longer there (`row_positions`): what the selection is rebuilt from after a
/// re-read.
pub fn grid_row_positions(
    state: &AppState,
    at: At<'_>,
    grid: &GridState,
    ids: &[i64],
) -> Result<Vec<(i64, Option<u64>)>, CmdError> {
    at.read(state, |c| oxl_engine::row_positions(c, grid, ids))
}

/// Write what `grid` shows to `path` as CSV (`export_csv`), as a job: the
/// window hears `job-started` and `job-finished` (kind `export`, the file's
/// name as the label), and no `case-changed`, since the case is unchanged.
///
/// The rows are written while writes go on; the target is checked against
/// the loaded sources again, and the file renamed into place, with the write
/// gate held (`export_csv_guarded`, `AppState::wait_for_gate`), so an
/// export never lands inside a run a load made a source while it was
/// writing: it answers `refused` and its partial file is removed.
///
/// The export runs on a reader under `at`, with its cancel flag registered
/// with the query: `query_cancel` with its id, a newer query on its slot,
/// or the case closing sets the flag and interrupts DuckDB. Stopped before
/// its file was renamed into place, it answers `interrupted` (or
/// `superseded`) and the engine has removed the partial file; stopped after,
/// it answers its report, because the file is there.
pub fn grid_export(
    state: &AppState,
    at: At<'_>,
    grid: &GridState,
    path: &Path,
    zone_times: Option<&str>,
) -> Result<ExportReport, CmdError> {
    state.current()?;
    state.announced(EXPORT_JOB, &job_label(path), || {
        let cancel = Arc::new(AtomicBool::new(false));
        state.read_to_completion(at.slot, at.query_id, cancel.clone(), |c| {
            // The rows are written beside any write; the target's second
            // check and the rename wait for the write gate, so a load that
            // made the target part of a source meanwhile is seen.
            oxl_engine::export_csv_guarded(c, grid, path, zone_times, &cancel, |_| {
                state.hook(Point::BeforeRename);
                state.wait_for_gate(&cancel)
            })
        })
    })
}

/// What the status bar names an export by: the file's name, or the whole
/// path when it has none.
pub(crate) fn job_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A date-range chip on `relation`'s timestamp `column`, its bounds on
/// `zone`'s clock (`date_range_chip`).
// The command's own arguments (`date_range_chip` takes the same), after
// where the read runs.
#[allow(clippy::too_many_arguments)]
pub fn date_range_chip(
    state: &AppState,
    at: At<'_>,
    relation: &str,
    column: &str,
    from: Option<&str>,
    to: Option<&str>,
    zone: &str,
) -> Result<Chip, CmdError> {
    at.read(state, |c| {
        oxl_engine::date_range_chip(c, relation, column, from, to, zone)
    })
}

/// The colour rules `relation` starts with (`default_colour_rules`).
pub fn default_colour_rules(
    state: &AppState,
    at: At<'_>,
    relation: &str,
) -> Result<Vec<ColourRule>, CmdError> {
    at.read(state, |c| oxl_engine::default_colour_rules(c, relation))
}

/// The Tauri wrappers. The app's state is `app`, because the window's
/// argument `state` is the grid's `GridState`.
// Each wrapper's parameters are the command's IPC arguments, named as the
// window sends them; Tauri takes them one by one, not as a struct.
#[allow(clippy::too_many_arguments)]
pub mod commands {
    use crate::cmd::At;
    use crate::error::CmdError;
    use crate::state::AppState;
    use oxl_engine::{
        Chip, ColourRule, ExportReport, GridCounts, GridPage, GridState, GroupView, Histogram,
        Listing, MatchPos, PageOptions, ValueCount, ValueOrder,
    };
    use std::path::PathBuf;
    use tauri::State;

    #[tauri::command(async)]
    pub fn grid_page(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        state: GridState,
        limit: u32,
        offset: u64,
        options: PageOptions,
    ) -> Result<GridPage, CmdError> {
        let at = At::new(&query_id, &slot);
        super::grid_page(&app, at, &state, limit, offset, &options)
    }

    #[tauri::command(async)]
    pub fn grid_counts(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        state: GridState,
    ) -> Result<GridCounts, CmdError> {
        super::grid_counts(&app, At::new(&query_id, &slot), &state)
    }

    #[tauri::command(async)]
    pub fn grid_groups(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        state: GridState,
        by: Vec<String>,
        parent: Vec<Option<String>>,
        limit: u32,
        offset: u64,
        zone: String,
    ) -> Result<Listing<GroupView>, CmdError> {
        let at = At::new(&query_id, &slot);
        super::grid_groups(&app, at, &state, &by, &parent, limit, offset, &zone)
    }

    #[tauri::command(async)]
    pub fn grid_histogram(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        state: GridState,
        column: Option<String>,
        max_buckets: u32,
        zone: String,
    ) -> Result<Histogram, CmdError> {
        let at = At::new(&query_id, &slot);
        super::grid_histogram(&app, at, &state, column.as_deref(), max_buckets, &zone)
    }

    #[tauri::command(async)]
    pub fn grid_values(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        state: GridState,
        column: String,
        except_chips: Vec<String>,
        contains: Option<String>,
        order: ValueOrder,
        limit: u32,
        offset: u64,
    ) -> Result<Listing<ValueCount>, CmdError> {
        super::grid_values(
            &app,
            At::new(&query_id, &slot),
            &state,
            &column,
            &except_chips,
            contains.as_deref(),
            order,
            limit,
            offset,
        )
    }

    #[tauri::command(async)]
    pub fn grid_empty_columns(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        state: GridState,
    ) -> Result<Vec<String>, CmdError> {
        super::grid_empty_columns(&app, At::new(&query_id, &slot), &state)
    }

    #[tauri::command(async)]
    pub fn grid_matches(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        state: GridState,
        from: Option<u64>,
        backward: bool,
        limit: u32,
    ) -> Result<Listing<MatchPos>, CmdError> {
        super::grid_matches(
            &app,
            At::new(&query_id, &slot),
            &state,
            from,
            backward,
            limit,
        )
    }

    #[tauri::command(async)]
    pub fn grid_match_count(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        state: GridState,
    ) -> Result<u64, CmdError> {
        super::grid_match_count(&app, At::new(&query_id, &slot), &state)
    }

    #[tauri::command(async)]
    pub fn grid_row_offset(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        state: GridState,
        row_id: i64,
    ) -> Result<Option<u64>, CmdError> {
        super::grid_row_offset(&app, At::new(&query_id, &slot), &state, row_id)
    }

    #[tauri::command(async)]
    pub fn grid_row_ids(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        state: GridState,
        ranges: Vec<(u64, u64)>,
    ) -> Result<Vec<i64>, CmdError> {
        super::grid_row_ids(&app, At::new(&query_id, &slot), &state, &ranges)
    }

    #[tauri::command(async)]
    pub fn grid_row_positions(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        state: GridState,
        ids: Vec<i64>,
    ) -> Result<Vec<(i64, Option<u64>)>, CmdError> {
        super::grid_row_positions(&app, At::new(&query_id, &slot), &state, &ids)
    }

    #[tauri::command(async)]
    pub fn grid_export(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        state: GridState,
        path: PathBuf,
        zone_times: Option<String>,
    ) -> Result<ExportReport, CmdError> {
        let at = At::new(&query_id, &slot);
        super::grid_export(&app, at, &state, &path, zone_times.as_deref())
    }

    #[tauri::command(async)]
    pub fn date_range_chip(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        relation: String,
        column: String,
        from: Option<String>,
        to: Option<String>,
        zone: String,
    ) -> Result<Chip, CmdError> {
        super::date_range_chip(
            &app,
            At::new(&query_id, &slot),
            &relation,
            &column,
            from.as_deref(),
            to.as_deref(),
            &zone,
        )
    }

    #[tauri::command(async)]
    pub fn default_colour_rules(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        relation: String,
    ) -> Result<Vec<ColourRule>, CmdError> {
        super::default_colour_rules(&app, At::new(&query_id, &slot), &relation)
    }
}
