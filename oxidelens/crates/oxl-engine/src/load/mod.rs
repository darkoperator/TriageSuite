//! Loading sources into the case, and what the case records about them.
//!
//! Every load is one transaction: it either adds a source, its tables, its
//! inventory and its history row together, or leaves the case as it was.

mod file;
mod run;
pub(crate) mod stage;

pub use file::{load_file, relation_name_for_file, FileOptions};
pub use run::load_run;

use crate::case::Case;
use crate::error::Result;
use duckdb::params;

/// One relation a load fed, and how much.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedRelation {
    pub name: String,
    /// The hosts the load added rows for, sorted.
    pub hosts: Vec<String>,
    pub rows: u64,
}

/// Rows of an earlier source that a load superseded for one relation and host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Superseded {
    pub relation: String,
    pub host: String,
    pub old_source: i64,
    /// Annotated rows whose annotations moved to the new source's rows.
    pub moved: u64,
    /// Annotated rows with no counterpart in the new source.
    pub left_behind: u64,
}

/// What a load did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadReport {
    pub source_id: i64,
    pub relations: Vec<LoadedRelation>,
    pub superseded: Vec<Superseded>,
    pub warnings: Vec<String>,
}

/// One loaded source, as `ox.source` records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    pub id: i64,
    /// `run` or `file`.
    pub kind: String,
    pub path: String,
    pub run_id: Option<String>,
    /// Every host the source has rows for, sorted.
    pub hosts: Vec<String>,
    pub loaded_at: String,
    pub loaded_by: String,
    pub removed: bool,
}

/// Every source ever loaded into the case, removed ones included, by id.
pub fn sources(case: &Case) -> Result<Vec<SourceInfo>> {
    let conn = case.conn();
    let mut hosts = conn
        .prepare("SELECT DISTINCT host FROM ox.source_dataset WHERE source_id = ? AND host IS NOT NULL ORDER BY host")?;
    let rows: Vec<SourceInfo> = conn
        .prepare(
            "SELECT id, kind, path, run_id, loaded_at, loaded_by, removed \
             FROM ox.source ORDER BY id",
        )?
        .query_map([], |r| {
            Ok(SourceInfo {
                id: r.get(0)?,
                kind: r.get(1)?,
                path: r.get(2)?,
                run_id: r.get(3)?,
                hosts: Vec::new(),
                loaded_at: r.get(4)?,
                loaded_by: r.get(5)?,
                removed: r.get(6)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    rows.into_iter()
        .map(|mut source| {
            source.hosts = hosts
                .query_map(params![source.id], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?;
            Ok(source)
        })
        .collect()
}
