//! The OxideLens desktop app's backend: the Tauri commands over the case
//! engine, and the state they share.
//!
//! The commands are thin: every rule is the engine's. What the app adds is
//! how the engine is driven from a window: one writer connection that alone
//! changes the case (`state`), a pool of reader connections so reads never
//! wait for each other or for a write, and a registry (`queries`) under
//! which every read can be superseded by a newer one or cancelled. Every
//! command is a plain function over `AppState`, which is what the tests
//! call; `cmd::*::commands` wraps each for Tauri. `contract` is the
//! TypeScript the window is built against.

pub mod cmd;
pub mod contract;
pub mod error;
pub mod events;
pub mod queries;
pub mod settings;
pub mod state;

pub use error::CmdError;
pub use state::AppState;

/// The handler and the names it answers to, from one list, so the two
/// cannot differ: `[module path] command`, each command named as its
/// wrapper function is (which is the name Tauri routes by).
macro_rules! handlers {
    ($( [$($path:ident)::*] $name:ident ),* $(,)?) => {
        /// Every command the window may call.
        pub fn invoke_handler<R: tauri::Runtime>(
        ) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
            tauri::generate_handler![$( $($path::)* $name ),*]
        }

        /// The names `invoke_handler` answers to, in its order; the same as
        /// `contract::COMMANDS`.
        pub const HANDLERS: &[&str] = &[$( stringify!($name) ),*];
    };
}

handlers![
    [cmd::case::commands] case_new,
    [cmd::case::commands] case_open,
    [cmd::case::commands] case_close,
    [cmd::case::commands] case_summary,
    [cmd::load::commands] load_run,
    [cmd::load::commands] load_file,
    [cmd::case::commands] columns,
    [cmd::case::commands] sessions,
    [cmd::case::commands] host_zones,
    [cmd::case::commands] zone_names,
    [cmd::case::commands] display_zone_set,
    [cmd::grid::commands] grid_page,
    [cmd::grid::commands] grid_counts,
    [cmd::grid::commands] grid_groups,
    [cmd::grid::commands] grid_histogram,
    [cmd::grid::commands] grid_values,
    [cmd::grid::commands] grid_empty_columns,
    [cmd::grid::commands] grid_matches,
    [cmd::grid::commands] grid_match_count,
    [cmd::grid::commands] grid_row_offset,
    [cmd::grid::commands] grid_row_ids,
    [cmd::grid::commands] grid_row_positions,
    [cmd::grid::commands] grid_export,
    [cmd::grid::commands] date_range_chip,
    [cmd::grid::commands] default_colour_rules,
    [cmd::row::commands] row_view,
    [cmd::row::commands] cell_value,
    [cmd::row::commands] around_totals,
    [cmd::row::commands] merged_page,
    [cmd::views::commands] views_list,
    [cmd::views::commands] views_matching,
    [cmd::views::commands] view_save,
    [cmd::views::commands] view_delete,
    [cmd::views::commands] view_apply,
    [cmd::views::commands] view_export,
    [cmd::views::commands] view_import,
    [cmd::record::commands] mark_set,
    [cmd::record::commands] mark_clear,
    [cmd::record::commands] tag_add,
    [cmd::record::commands] tag_remove,
    [cmd::record::commands] tags_list,
    [cmd::record::commands] note_add,
    [cmd::record::commands] note_delete,
    [cmd::record::commands] notes_list,
    [cmd::record::commands] entries_list,
    [cmd::record::commands] entry_get,
    [cmd::record::commands] entry_new,
    [cmd::record::commands] entry_edit,
    [cmd::record::commands] entry_status,
    [cmd::record::commands] entry_delete,
    [cmd::record::commands] rows_promote,
    [cmd::record::commands] rows_attach,
    [cmd::record::commands] evidence_detach,
    [cmd::record::commands] evidence_list,
    [cmd::record::commands] records_list,
    [cmd::record::commands] record_get,
    [cmd::record::commands] record_upsert,
    [cmd::record::commands] record_delete,
    [cmd::record::commands] record_merge,
    [cmd::record::commands] record_status,
    [cmd::record::commands] record_link,
    [cmd::record::commands] record_unlink,
    [cmd::record::commands] record_entries,
    [cmd::record::commands] candidates,
    [cmd::record::commands] ioc_sightings,
    [cmd::record::commands] logon_summary,
    [cmd::record::commands] history_list,
    [cmd::record::commands] export_timeline,
    [cmd::record::commands] export_workbook,
    [cmd::record::commands] export_report,
    [cmd::settings::commands] settings_get,
    [cmd::settings::commands] settings_update,
    [cmd::settings::commands] recent_clear,
    [cmd::settings::commands] recent_remove,
    [cmd::commands] query_cancel,
];
