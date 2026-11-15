//! The commands, each a plain function over `AppState` (what the tests
//! call), wrapped for Tauri in each module's `commands`. Every wrapper is
//! `#[tauri::command(async)]`, so no command runs on the window's thread.
//!
//! A command that reads the case runs as a query (`AppState::read`) under
//! the query id and slot the window gives it (`At`), so a newer query on the
//! slot supersedes it and `query_cancel` stops it. A command that changes
//! the case is a write (`AppState::write`), and the window hears
//! `case-changed` with what it touched. Every rule is the engine's; a
//! command is the one call that reaches it.

pub mod case;
pub mod grid;
pub mod load;
pub mod record;
pub mod row;
pub mod settings;
pub mod views;

use crate::error::CmdError;
use crate::queries::check_name;
use crate::state::AppState;

/// Where a read runs: the window's id for the query, and the slot it fills
/// (`QueryRegistry`). Every read command takes both, as `queryId` and
/// `slot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct At<'a> {
    pub query_id: &'a str,
    pub slot: &'a str,
}

impl<'a> At<'a> {
    pub const fn new(query_id: &'a str, slot: &'a str) -> At<'a> {
        At { query_id, slot }
    }

    /// Run `work` on a reader as this query (`AppState::read`).
    pub fn read<T>(
        self,
        state: &AppState,
        work: impl FnOnce(&oxl_engine::Case) -> oxl_engine::Result<T>,
    ) -> Result<T, CmdError> {
        state.read(self.slot, self.query_id, work)
    }
}

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

/// An export checks its target against the loaded sources before it writes
/// and again, with the write gate held, before it renames its file into
/// place. These tests pause an export between the two (`Point::BeforeRename`)
/// and load a run whose folder holds the target meanwhile. A record export
/// holds the gate for all its work, so it is paused before it takes the gate
/// (`Point::BeforeGate`), and the load lands before any of its checks.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::NoSink;
    use crate::settings::SettingsStore;
    use crate::state::Point;
    use std::path::{Path, PathBuf};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    const LIMIT: Duration = Duration::from_secs(60);

    /// A case with the two-host run loaded, and the host-zones run built in
    /// `later/` but not loaded: its root, where the exports write.
    fn setup() -> (tempfile::TempDir, Arc<AppState>, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new(SettingsStore::in_memory(), Arc::new(NoSink)));
        case::case_new(&state, &d.path().join("case.duckdb"), "WS01 triage").unwrap();
        load::load_run(&state, &oxl_fixtures::two_hosts(d.path())).unwrap();
        let later = oxl_fixtures::host_zones_run(&d.path().join("later"));
        (d, state, later)
    }

    /// Run `export` on a thread, paused at `point` until a run whose root
    /// is `run` has been loaded; what it answered.
    fn export_while_loading(
        state: &Arc<AppState>,
        point: Point,
        run: &Path,
        export: impl FnOnce(&AppState) -> Result<(), CmdError> + Send + 'static,
    ) -> Result<(), CmdError> {
        let (reached_tx, reached) = mpsc::channel();
        let (go, go_rx) = mpsc::channel::<()>();
        state.on(point, move || {
            reached_tx.send(()).unwrap();
            go_rx.recv().unwrap();
        });
        let (done_tx, done) = mpsc::channel();
        {
            let state = state.clone();
            std::thread::spawn(move || {
                let _ = done_tx.send(export(&state));
            });
        }
        reached
            .recv_timeout(LIMIT)
            .expect("the export reached its pause");
        load::load_run(state, run).expect("the load ran while the export was paused");
        go.send(()).unwrap();
        done.recv_timeout(LIMIT).expect("the export answered")
    }

    fn partial(path: &Path) -> PathBuf {
        let mut name = path.as_os_str().to_os_string();
        name.push(".partial");
        PathBuf::from(name)
    }

    #[test]
    fn a_grid_export_into_a_run_loaded_meanwhile_is_refused() {
        let (_d, state, later) = setup();
        let target = later.join("events.csv");
        let grid: oxl_engine::GridState =
            serde_json::from_value(serde_json::json!({ "relation": "evtxtriage_events" })).unwrap();
        let answer = {
            let target = target.clone();
            export_while_loading(&state, Point::BeforeRename, &later, move |state| {
                grid::grid_export(state, At::new("e1", "export"), &grid, &target, None).map(|_| ())
            })
        };
        assert_eq!(answer.unwrap_err().code, "refused");
        assert!(!target.exists(), "the export landed inside the run");
        assert!(!partial(&target).exists(), "the partial file was left");
    }

    #[test]
    fn a_view_export_into_a_run_loaded_meanwhile_is_refused() {
        let (_d, state, later) = setup();
        let doc = oxl_engine::ViewDoc::default();
        let view = views::view_save(&state, "All", oxl_engine::ViewKind::View, "*", &doc).unwrap();
        let target = later.join("all.oxlview.json");
        let answer = {
            let target = target.clone();
            export_while_loading(&state, Point::BeforeRename, &later, move |state| {
                views::view_export(state, At::new("v1", "vexport"), view.id, &target)
            })
        };
        assert_eq!(answer.unwrap_err().code, "refused");
        assert!(!target.exists(), "the view landed inside the run");
        assert!(!partial(&target).exists(), "the partial file was left");
    }

    #[test]
    fn a_record_export_into_a_run_loaded_meanwhile_is_refused() {
        type Export = fn(&AppState, At<'_>, &Path) -> Result<oxl_engine::ExportReport, CmdError>;
        let exports: [(&str, Export); 3] = [
            ("timeline.csv", record::export_timeline),
            ("record.xlsx", record::export_workbook),
            ("record.md", record::export_report),
        ];
        for (name, export) in exports {
            let (_d, state, later) = setup();
            let target = later.join(name);
            let answer = {
                let target = target.clone();
                export_while_loading(&state, Point::BeforeGate, &later, move |state| {
                    export(state, At::new("r1", "rexport"), &target).map(|_| ())
                })
            };
            assert_eq!(answer.unwrap_err().code, "refused", "{name}");
            assert!(!target.exists(), "{name} landed inside the run");
            assert!(!partial(&target).exists(), "{name}'s partial file was left");
        }
    }
}
