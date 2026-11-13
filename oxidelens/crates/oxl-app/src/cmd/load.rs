//! Loading a run or a loose file: each is a job. The window hears
//! `job-started`, then `job-finished` when the load ends, then (when it
//! loaded) `case-changed` with kinds `["sources"]`.

use crate::error::CmdError;
use crate::state::AppState;
use oxl_engine::{FileOptions, LoadReport};
use std::path::Path;

/// What changed when a load succeeds.
const KINDS: &[&str] = &["sources"];

/// The job's label: the name of the folder or file being loaded.
fn label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Load the TriageSuite run whose output folder is `path`.
pub fn load_run(state: &AppState, path: &Path) -> Result<LoadReport, CmdError> {
    let actor = state.actor();
    state.job("load_run", &label(path), KINDS, |case| {
        oxl_engine::load_run(case, path, &actor)
    })
}

/// Load one CSV or TSV file.
pub fn load_file(
    state: &AppState,
    path: &Path,
    options: FileOptions,
) -> Result<LoadReport, CmdError> {
    let actor = state.actor();
    state.job("load_file", &label(path), KINDS, |case| {
        oxl_engine::load_file(case, path, options, &actor)
    })
}

pub mod commands {
    use crate::error::CmdError;
    use crate::state::AppState;
    use oxl_engine::{FileOptions, LoadReport};
    use std::path::PathBuf;
    use tauri::State;

    #[tauri::command(async)]
    pub fn load_run(state: State<'_, AppState>, path: PathBuf) -> Result<LoadReport, CmdError> {
        super::load_run(&state, &path)
    }

    #[tauri::command(async)]
    pub fn load_file(
        state: State<'_, AppState>,
        path: PathBuf,
        options: Option<FileOptions>,
    ) -> Result<LoadReport, CmdError> {
        super::load_file(&state, &path, options.unwrap_or_default())
    }
}
