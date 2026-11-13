//! Making, opening and closing the case the window works on. One case is
//! open at a time; opening another closes the first. Opening a step 1
//! (format 1) case upgrades it in place (`Case::open`). Opening or making a
//! case puts it first in the recent list.

use crate::error::CmdError;
use crate::state::AppState;
use oxl_engine::{Case, MarkCounts, RelationInfo, SourceInfo};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;

/// What the window shows about the open case.
#[derive(Debug, Clone, Serialize)]
pub struct CaseSummary {
    pub path: String,
    /// The name the case was made with.
    pub name: String,
    pub case_id: String,
    pub format: u32,
    pub display_zone: String,
    /// Every host with rows in a relation, sorted.
    pub hosts: Vec<String>,
    pub relations: Vec<RelationInfo>,
    pub sources: Vec<SourceInfo>,
    pub mark_counts: MarkCounts,
}

/// The summary of `case`, read on that connection.
pub fn summary(case: &Case) -> oxl_engine::Result<CaseSummary> {
    let info = case.info()?;
    let relations = oxl_engine::relations(case)?;
    let hosts: BTreeSet<String> = relations.iter().flat_map(|r| r.hosts.clone()).collect();
    Ok(CaseSummary {
        path: case.path().display().to_string(),
        name: info.name,
        case_id: case.case_id()?,
        format: case.format(),
        display_zone: oxl_engine::display_zone(case)?,
        hosts: hosts.into_iter().collect(),
        relations,
        sources: oxl_engine::sources(case)?,
        mark_counts: oxl_engine::mark_counts(case)?,
    })
}

/// Make a new case at `path` (a `.duckdb` name that does not exist yet)
/// called `name`, and open it.
pub fn case_new(state: &AppState, path: &Path, name: &str) -> Result<CaseSummary, CmdError> {
    let actor = state.actor();
    let summary = state.open_with(|| Case::create(path, name, &actor), summary)?;
    remember(state, &summary.path);
    Ok(summary)
}

/// Open the case at `path`, closing the one open now.
pub fn case_open(state: &AppState, path: &Path) -> Result<CaseSummary, CmdError> {
    let summary = state.open_with(|| Case::open(path), summary)?;
    remember(state, &summary.path);
    Ok(summary)
}

/// Close the open case, if there is one. Queries running on it answer
/// `interrupted`.
pub fn case_close(state: &AppState) -> Result<(), CmdError> {
    state.close()
}

/// Put `path` first in the recent list. The case is open either way: a
/// settings file that cannot be written is not worth failing the open over,
/// and the next save tries again.
fn remember(state: &AppState, path: &str) {
    let _ = state.settings.update(|s| s.add_recent(path));
}

pub mod commands {
    use super::CaseSummary;
    use crate::error::CmdError;
    use crate::state::AppState;
    use std::path::PathBuf;
    use tauri::State;

    #[tauri::command(async)]
    pub fn case_new(
        state: State<'_, AppState>,
        path: PathBuf,
        name: String,
    ) -> Result<CaseSummary, CmdError> {
        super::case_new(&state, &path, &name)
    }

    #[tauri::command(async)]
    pub fn case_open(state: State<'_, AppState>, path: PathBuf) -> Result<CaseSummary, CmdError> {
        super::case_open(&state, &path)
    }

    #[tauri::command(async)]
    pub fn case_close(state: State<'_, AppState>) -> Result<(), CmdError> {
        super::case_close(&state)
    }
}
