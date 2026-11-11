//! Loading sources into the case, and what the case records about them.
//!
//! Every load is one transaction: it either adds a source, its tables, its
//! inventory and its history row together, or leaves the case as it was.

mod file;
mod run;
pub(crate) mod stage;
mod supersede;

pub use file::{load_file, relation_name_for_file, FileOptions};
pub use run::load_run;

use crate::actor::Actor;
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::time::now_iso;
use duckdb::{params, Connection};
use stage::{rebuild_relations, source_schema};
use triage_core::output::duckdb::sql::quote_ident;

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

/// The annotations a removal left pointing at rows that are no longer in
/// the case. They are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveReport {
    pub orphaned_marks: u64,
    pub orphaned_tags: u64,
    /// Notes not deleted.
    pub orphaned_notes: u64,
}

/// Remove source `source_id` from the case: it is marked removed, its
/// schema is dropped, its datasets leave every view, and a relation it was
/// the last live source of is dropped with its catalog. Annotations on its
/// rows are kept, orphaned; when there are any, `confirm_orphans` must be
/// true or the removal is `Refused` with their counts. Refused for an agent
/// and for a source already removed; `NotFound` for an unknown id. A
/// removed run may be loaded again.
pub fn remove_source(
    case: &mut Case,
    source_id: i64,
    confirm_orphans: bool,
    actor: &Actor,
) -> Result<RemoveReport> {
    if actor.is_agent() {
        return Err(EngineError::Refused(
            "an agent may not remove a source; an analyst must".to_string(),
        ));
    }
    let tx = case.conn_mut().transaction()?;
    let report = remove_in(&tx, source_id, confirm_orphans, actor)?;
    tx.commit()?;
    Ok(report)
}

fn remove_in(
    conn: &Connection,
    source_id: i64,
    confirm_orphans: bool,
    actor: &Actor,
) -> Result<RemoveReport> {
    let (kind, path, run_id, removed): (String, String, Option<String>, bool) = conn
        .query_row(
            "SELECT kind, path, run_id, removed FROM ox.source WHERE id = ?",
            params![source_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|e| match e {
            duckdb::Error::QueryReturnedNoRows => {
                EngineError::NotFound(format!("no source {source_id}"))
            }
            other => other.into(),
        })?;
    if removed {
        return Err(EngineError::Refused(format!(
            "source {source_id} was already removed"
        )));
    }

    let relations: Vec<String> = conn
        .prepare("SELECT relation FROM ox.source_table WHERE source_id = ? ORDER BY relation")?
        .query_map(params![source_id], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let schema = source_schema(source_id);
    let report = if relations.is_empty() {
        RemoveReport {
            orphaned_marks: 0,
            orphaned_tags: 0,
            orphaned_notes: 0,
        }
    } else {
        // Every row the source holds, superseded ones included: an
        // annotation left behind on an old row is orphaned by its removal too.
        let rows = relations
            .iter()
            .map(|r| {
                format!(
                    "SELECT _ox_row FROM {}.{}",
                    quote_ident(&schema),
                    quote_ident(r)
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        let count = |table: &str, extra: &str| -> Result<u64> {
            let n: i64 = conn.query_row(
                &format!("SELECT count(*) FROM {table} WHERE row_id IN ({rows}){extra}"),
                [],
                |r| r.get(0),
            )?;
            Ok(u64::try_from(n).unwrap_or_default())
        };
        RemoveReport {
            orphaned_marks: count("ox.row_mark", "")?,
            orphaned_tags: count("ox.row_tag", "")?,
            orphaned_notes: count("ox.note", " AND NOT deleted")?,
        }
    };
    let orphans = report.orphaned_marks + report.orphaned_tags + report.orphaned_notes;
    if orphans > 0 && !confirm_orphans {
        return Err(EngineError::Refused(format!(
            "source {source_id} has annotations on its rows: {}, {}, {}; removing it \
             orphans them (they are kept). Confirm to remove it anyway",
            counted(report.orphaned_marks, "mark"),
            counted(report.orphaned_tags, "tag"),
            counted(report.orphaned_notes, "note"),
        )));
    }

    conn.execute(
        "UPDATE ox.source SET removed = true WHERE id = ?",
        params![source_id],
    )?;
    conn.execute(
        "UPDATE ox.source_dataset SET active = false WHERE source_id = ?",
        params![source_id],
    )?;
    // The views are rebuilt before the schema goes, so none still reads it;
    // a relation with no arm left loses its views here.
    rebuild_relations(conn)?;
    let mut dropped = Vec::new();
    for relation in &relations {
        let live: i64 = conn.query_row(
            "SELECT count(*) FROM ox.source_table st JOIN ox.source s ON s.id = st.source_id \
             WHERE st.relation = ? AND NOT s.removed",
            params![relation],
            |r| r.get(0),
        )?;
        if live == 0 {
            conn.execute(
                "DELETE FROM ox.column_catalog WHERE relation = ?",
                params![relation],
            )?;
            conn.execute("DELETE FROM ox.relation WHERE name = ?", params![relation])?;
            dropped.push(relation.clone());
        }
    }
    conn.execute_batch(&format!(
        "DROP SCHEMA IF EXISTS {} CASCADE;",
        quote_ident(&schema)
    ))?;

    let before = serde_json::json!({
        "kind": kind, "path": path, "run_id": run_id, "removed": false,
    });
    let after = serde_json::json!({
        "kind": kind, "path": path, "run_id": run_id, "removed": true,
        "relations_dropped": dropped,
        "orphaned_marks": report.orphaned_marks,
        "orphaned_tags": report.orphaned_tags,
        "orphaned_notes": report.orphaned_notes,
    });
    conn.execute(
        "INSERT INTO ox.history \
         (id, \"at\", \"by\", target_kind, target_id, action, before_json, after_json) \
         VALUES (nextval('ox.history_seq'), ?, ?, 'source', ?, 'source_remove', ?, ?)",
        params![
            now_iso(),
            actor.label(),
            source_id.to_string(),
            before.to_string(),
            after.to_string()
        ],
    )?;
    Ok(report)
}

/// `1 mark`, `2 marks`.
fn counted(n: u64, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
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
