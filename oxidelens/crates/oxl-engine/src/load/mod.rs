//! Loading sources into the case, and what the case records about them.
//!
//! Every load is one transaction: it either adds a source, its tables, its
//! inventory and its history row together, or leaves the case as it was.

mod file;
mod run;
mod sessions;
pub(crate) mod stage;
mod supersede;

pub use file::{load_file, relation_name_for_file, FileOptions};
pub(crate) use run::canonical_path;
pub use run::{load_run, load_run_with};
pub(crate) use sessions::backfill_sessions;
pub use sessions::{sessions, Session, SessionRelation};

use crate::actor::Actor;
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::time::now_iso;
use duckdb::{params, Connection};
use stage::{rebuild_relations, source_schema};
use supersede::reinstate;
use triage_core::output::duckdb::sql::quote_ident;

/// The most threads a load runs DuckDB on. Found on the second real run,
/// 5.5 M rows on a 32-thread machine: each thread holds its own CSV and sort
/// buffers, so on all 32 the load peaked at 7.2 GB; on 8 it peaked at 4.7 GB
/// and took 41 s instead of 62 s (12 and 16 threads were slower than 8, and
/// 4 no lighter). A memory limit was not the answer: under one, DuckDB 1.5
/// failed some of the same loads out of memory and crashed others.
pub(crate) const LOAD_THREADS: i64 = 8;

/// The database's threads, bounded at `LOAD_THREADS` for the length of a
/// load; `restore` puts the setting back. A load restores it before it
/// commits, so a failure to restore rolls the load back and the case is as
/// it was; a load that failed restores it after its rollback.
pub(super) struct LoadThreads {
    /// The setting to put back; `None` when the database was already on
    /// `LOAD_THREADS` or fewer and nothing was changed.
    previous: Option<i64>,
}

impl LoadThreads {
    pub(super) fn bound(conn: &Connection) -> Result<LoadThreads> {
        let current: i64 = conn.query_row("SELECT current_setting('threads')", [], |r| r.get(0))?;
        if current <= LOAD_THREADS {
            return Ok(LoadThreads { previous: None });
        }
        conn.execute_batch(&format!("SET threads = {LOAD_THREADS};"))?;
        Ok(LoadThreads {
            previous: Some(current),
        })
    }

    pub(super) fn restore(self, conn: &Connection) -> Result<()> {
        if let Some(threads) = self.previous {
            conn.execute_batch(&format!("SET threads = {threads};"))?;
        }
        Ok(())
    }
}

/// `name` as both loaders compare relation names when they look for a
/// collision: lower-cased with `str::to_lowercase`, so names that differ
/// only in case, ASCII or not, are the same name to either loader.
pub(super) fn fold_name(name: &str) -> String {
    name.to_lowercase()
}

/// One relation a load fed, and how much.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LoadedRelation {
    pub name: String,
    /// The hosts the load added rows for, sorted.
    pub hosts: Vec<String>,
    pub rows: u64,
}

/// Rows of an earlier source that a load superseded for one relation and host.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Superseded {
    pub relation: String,
    pub host: String,
    pub old_source: i64,
    /// Annotations (each mark, tag, note not deleted and evidence row on an
    /// entry not deleted) that moved to the new source's identical rows.
    pub moved: u64,
    /// Annotations left on the old rows: no identical row in the new
    /// source, or one that already had a mark, the same tag or was already
    /// evidence on the same entry.
    pub left_behind: u64,
}

/// What a load did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LoadReport {
    pub source_id: i64,
    pub relations: Vec<LoadedRelation>,
    pub superseded: Vec<Superseded>,
    pub warnings: Vec<String>,
    /// Datasets of the run whose files for some hosts were not loaded
    /// because they copy another (see `RunOptions::per_channel`), in
    /// inventory order.
    pub skipped: Vec<SkippedDataset>,
}

/// A dataset of a run a load left out, for some or all of its hosts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SkippedDataset {
    /// The relation it would have fed.
    pub relation: String,
    pub tool: String,
    pub dataset_id: String,
    /// The hosts whose files were left out, sorted. A host the dataset has
    /// a file for and that is not here was loaded.
    pub hosts: Vec<String>,
}

/// How a run is loaded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunOptions {
    /// Also load EvtxTriage's per-channel datasets (`Individual/<channel>`)
    /// when the run has its aggregate `events` dataset. The suite writes
    /// every event to both, so by default the copies are left out: they add
    /// no row, and each would show every event a second time.
    pub per_channel: bool,
}

/// One loaded source, as `ox.source` records it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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
    /// Evidence rows on entries not deleted. Each keeps its snapshot, and
    /// its entry lists it as "source removed".
    pub orphaned_evidence: u64,
}

/// Remove source `source_id` from the case: it is marked removed, its
/// schema is dropped, its datasets leave every view, and a relation it was
/// the last live source of is dropped with its catalog. For each relation
/// and host it was the active run for, the latest live run it had
/// superseded there is reinstated, and annotations move back to that run's
/// identical rows. Annotations with nowhere to go are kept, orphaned; when
/// the source's rows carry any annotation (a mark, a tag, a note or
/// evidence), `confirm_orphans` must be true or the removal is `Refused`
/// with their counts. Refused for an agent
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
    // Every annotation on the source's rows counts here, even one that will
    // move back to a reinstated run: the caller cannot know which will.
    let annotated = annotations_on(conn, source_id, &relations)?;
    let total = annotated.orphaned_marks
        + annotated.orphaned_tags
        + annotated.orphaned_notes
        + annotated.orphaned_evidence;
    if total > 0 && !confirm_orphans {
        return Err(EngineError::Refused(format!(
            "source {source_id} has annotations on its rows: {}, {}, {}, {}; removing it \
             may orphan them (they are kept, evidence with its snapshot). Confirm to \
             remove it anyway",
            counted(annotated.orphaned_marks, "mark"),
            counted(annotated.orphaned_tags, "tag"),
            counted(annotated.orphaned_notes, "note"),
            counted(annotated.orphaned_evidence, "evidence row"),
        )));
    }

    // Before the schema goes: rebinding reads the source's rows.
    let reinstated = reinstate(conn, source_id)?;
    let report = annotations_on(conn, source_id, &relations)?;

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
        quote_ident(&source_schema(source_id))
    ))?;

    let before = serde_json::json!({
        "kind": kind, "path": path, "run_id": run_id, "removed": false,
    });
    let after = serde_json::json!({
        "kind": kind, "path": path, "run_id": run_id, "removed": true,
        "relations_dropped": dropped,
        "reinstated": reinstated.iter().map(|r| r.history_json()).collect::<Vec<_>>(),
        "orphaned_marks": report.orphaned_marks,
        "orphaned_tags": report.orphaned_tags,
        "orphaned_notes": report.orphaned_notes,
        "orphaned_evidence": report.orphaned_evidence,
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

/// The marks, tags, (not deleted) notes and evidence (on entries not
/// deleted) on any row of `source_id`'s tables for `relations`, superseded
/// rows included: an annotation left behind on an old row is orphaned by
/// its removal too.
fn annotations_on(conn: &Connection, source_id: i64, relations: &[String]) -> Result<RemoveReport> {
    if relations.is_empty() {
        return Ok(RemoveReport {
            orphaned_marks: 0,
            orphaned_tags: 0,
            orphaned_notes: 0,
            orphaned_evidence: 0,
        });
    }
    let schema = source_schema(source_id);
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
    Ok(RemoveReport {
        orphaned_marks: count("ox.row_mark", "")?,
        orphaned_tags: count("ox.row_tag", "")?,
        orphaned_notes: count("ox.note", " AND NOT deleted")?,
        orphaned_evidence: if has_evidence(conn)? {
            count(
                "ox.evidence",
                " AND entry_id IN (SELECT id FROM ox.entry WHERE NOT deleted)",
            )?
        } else {
            0
        },
    })
}

/// Whether the case has `ox.evidence`. Every case the engine opens for
/// writing does (format 3 and later); only the test-built writable cases of
/// earlier formats (`Case::create_at_format_1`, `create_at_format_2`) lack
/// it, and supersession and removal read them as holding no evidence.
fn has_evidence(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM duckdb_tables() \
         WHERE database_name = current_database() AND schema_name = 'ox' \
           AND table_name = 'evidence'",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn threads(conn: &Connection) -> i64 {
        conn.query_row("SELECT current_setting('threads')", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn a_load_runs_on_at_most_load_threads_and_the_setting_comes_back() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("SET threads = 16;").unwrap();
        let bound = LoadThreads::bound(&conn).unwrap();
        assert_eq!(threads(&conn), LOAD_THREADS);
        bound.restore(&conn).unwrap();
        assert_eq!(threads(&conn), 16);
        // Fewer threads than the bound are kept, and nothing is put back.
        conn.execute_batch("SET threads = 4;").unwrap();
        let bound = LoadThreads::bound(&conn).unwrap();
        assert_eq!(threads(&conn), 4);
        conn.execute_batch("SET threads = 6;").unwrap();
        bound.restore(&conn).unwrap();
        assert_eq!(threads(&conn), 6);
    }
}
