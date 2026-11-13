//! Making, opening and closing the case the window works on, and what the
//! window reads about the case as a whole: its summary, a relation's
//! columns, the run sessions, the hosts' own zones, and the display zone.
//! One case is open at a time; opening another closes the first. Opening a
//! step 1 (format 1) case upgrades it in place (`Case::open`). Opening or
//! making a case puts it first in the recent list.

use super::At;
use crate::error::CmdError;
use crate::state::AppState;
use oxl_engine::{Case, ColumnMeta, HostZone, MarkCounts, RelationInfo, Session, SourceInfo};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;

/// What `display_zone_set` changes.
const DISPLAY_ZONE: &[&str] = &["display_zone"];

/// What the window shows about the open case.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
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

/// The open case's summary, read as a query.
pub fn case_summary(state: &AppState, at: At<'_>) -> Result<CaseSummary, CmdError> {
    at.read(state, summary)
}

/// Every catalogued column of `relation`, in display order.
pub fn columns(state: &AppState, at: At<'_>, relation: &str) -> Result<Vec<ColumnMeta>, CmdError> {
    at.read(state, |c| {
        Ok(oxl_engine::columns(c, relation)?
            .iter()
            .map(ColumnMeta::from)
            .collect())
    })
}

/// The run sessions the case offers.
pub fn sessions(state: &AppState, at: At<'_>) -> Result<Vec<Session>, CmdError> {
    at.read(state, oxl_engine::sessions)
}

/// Each host's own time zone, as its registry recorded it.
pub fn host_zones(state: &AppState, at: At<'_>) -> Result<Vec<HostZone>, CmdError> {
    at.read(state, oxl_engine::host_zones)
}

/// Every display zone this build knows, `UTC` first, then the IANA names
/// sorted. A list compiled into the app: no case and no query.
pub fn zone_names() -> Vec<String> {
    triage_core::timestamp::zone::names()
        .iter()
        .map(|n| n.to_string())
        .collect()
}

/// Set the case's display zone; `case-changed` with `display_zone`.
pub fn display_zone_set(state: &AppState, zone: &str) -> Result<(), CmdError> {
    let actor = state.actor();
    state.write(DISPLAY_ZONE, |c| {
        oxl_engine::set_display_zone(c, zone, &actor)
    })
}

pub mod commands {
    use super::CaseSummary;
    use crate::cmd::At;
    use crate::error::CmdError;
    use crate::state::AppState;
    use oxl_engine::{ColumnMeta, HostZone, Session};
    use std::path::PathBuf;
    use tauri::State;

    #[tauri::command(async)]
    pub fn case_summary(
        state: State<'_, AppState>,
        query_id: String,
        slot: String,
    ) -> Result<CaseSummary, CmdError> {
        super::case_summary(&state, At::new(&query_id, &slot))
    }

    #[tauri::command(async)]
    pub fn columns(
        state: State<'_, AppState>,
        query_id: String,
        slot: String,
        relation: String,
    ) -> Result<Vec<ColumnMeta>, CmdError> {
        super::columns(&state, At::new(&query_id, &slot), &relation)
    }

    #[tauri::command(async)]
    pub fn sessions(
        state: State<'_, AppState>,
        query_id: String,
        slot: String,
    ) -> Result<Vec<Session>, CmdError> {
        super::sessions(&state, At::new(&query_id, &slot))
    }

    #[tauri::command(async)]
    pub fn host_zones(
        state: State<'_, AppState>,
        query_id: String,
        slot: String,
    ) -> Result<Vec<HostZone>, CmdError> {
        super::host_zones(&state, At::new(&query_id, &slot))
    }

    #[tauri::command(async)]
    pub fn zone_names() -> Vec<String> {
        super::zone_names()
    }

    #[tauri::command(async)]
    pub fn display_zone_set(state: State<'_, AppState>, zone: String) -> Result<(), CmdError> {
        super::display_zone_set(&state, &zone)
    }

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
