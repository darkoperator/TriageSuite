//! The analyst's and the agent's writes beside the rows: suspicious marks,
//! tags and notes, the append-only history every change writes, and the row
//! detail view that gathers them.
//!
//! The rules on who may change what live here, not in the CLI: an agent is
//! capped per call, may not take over or clear another owner's mark, tags
//! only under its own prefix and removes or deletes only what it wrote. A
//! batch never fails as a whole for one row's sake; each row gets a
//! `RowOutcome`.
//!
//! An annotation names a row by `_ox_row`. A row of a superseded run or a
//! removed source is out of every view, so it takes no new mark, tag or
//! note; what it already carries can still be cleared.

mod history;
mod marks;
mod notes;
mod row;
mod tags;

pub use history::{history, HistoryRow};
pub use marks::{clear_suspicious, mark_suspicious, marks, MarkedRow};
pub use notes::{add_note, delete_note, notes, Note, MAX_NOTE};
pub use row::{row_detail, RowDetail};
pub use tags::{add_tag, remove_tag, tags, TagInfo, AGENT_TAG_PREFIX, MAX_TAG};

use crate::actor::Actor;
use crate::error::{EngineError, Result};
use crate::load::stage::host_column;
use duckdb::{params, Connection};
use triage_core::output::duckdb::sql::quote_ident;

/// The most row ids an agent may pass to one mark or tag call.
pub const AGENT_ROWS_PER_CALL: usize = 1_000;

/// What a batch call did to one of the rows it was given.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RowOutcome {
    pub row_id: i64,
    /// The row is now as asked, whether or not this call changed it.
    pub done: bool,
    /// Why not, or why nothing changed; `None` for a plain change.
    pub reason: Option<String>,
}

impl RowOutcome {
    fn changed(row_id: i64) -> RowOutcome {
        RowOutcome {
            row_id,
            done: true,
            reason: None,
        }
    }

    fn unchanged(row_id: i64, reason: String) -> RowOutcome {
        RowOutcome {
            row_id,
            done: true,
            reason: Some(reason),
        }
    }

    fn refused(row_id: i64, reason: String) -> RowOutcome {
        RowOutcome {
            row_id,
            done: false,
            reason: Some(reason),
        }
    }
}

/// The whole-call checks every batch makes: at least one row, and no more
/// than `AGENT_ROWS_PER_CALL` from an agent.
fn check_batch(rows: &[i64], actor: &Actor) -> Result<()> {
    if rows.is_empty() {
        return Err(EngineError::Invalid("no row ids were given".to_string()));
    }
    if actor.is_agent() && rows.len() > AGENT_ROWS_PER_CALL {
        return Err(EngineError::Refused(format!(
            "an agent may change at most {AGENT_ROWS_PER_CALL} rows per call, not {}",
            rows.len()
        )));
    }
    Ok(())
}

/// The source, relation and schema-qualified table holding row `row_id`,
/// found through the row-id ranges `ox.source_table` records; `None` when
/// no source ever loaded it. Each table's rows take one contiguous run of
/// `ox.row_seq` in a single statement, so the ranges do not overlap; a live
/// table is still asked whether it holds the row. A removed source's
/// schema is gone and its range is all that remains of it.
pub(crate) fn locate(conn: &Connection, row_id: i64) -> Result<Option<(i64, String, String)>> {
    let candidates: Vec<(i64, String, String, bool)> = conn
        .prepare(
            "SELECT st.source_id, st.relation, st.table_schema, s.removed \
             FROM ox.source_table st JOIN ox.source s ON s.id = st.source_id \
             WHERE ? BETWEEN st.first_row AND st.last_row ORDER BY st.source_id, st.relation",
        )?
        .query_map(params![row_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<std::result::Result<_, _>>()?;
    for (source, relation, schema, removed) in candidates {
        let table = format!("{}.{}", quote_ident(&schema), quote_ident(&relation));
        if !removed {
            let held: i64 = conn.query_row(
                &format!("SELECT count(*) FROM {table} WHERE _ox_row = ?"),
                params![row_id],
                |r| r.get(0),
            )?;
            if held == 0 {
                continue;
            }
        }
        return Ok(Some((source, relation, table)));
    }
    Ok(None)
}

/// Whether a located row is in its relation's view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standing {
    Active,
    /// A later run of its host replaced its dataset.
    Superseded,
    /// Its source was removed; its data is no longer in the case.
    Removed,
}

/// A row, located, with its host and standing.
pub(crate) struct LocatedRow {
    pub row_id: i64,
    pub source_id: i64,
    pub relation: String,
    /// Schema-qualified and quoted.
    pub table: String,
    /// `None` for a relation without a host column, or a removed source's
    /// row, whose host is gone with its data.
    pub host: Option<String>,
    pub standing: Standing,
}

/// Row `row_id`, located; `None` when no source ever loaded it.
pub(crate) fn find_row(conn: &Connection, row_id: i64) -> Result<Option<LocatedRow>> {
    let Some((source_id, relation, table)) = locate(conn, row_id)? else {
        return Ok(None);
    };
    let removed: bool = conn.query_row(
        "SELECT removed FROM ox.source WHERE id = ?",
        params![source_id],
        |r| r.get(0),
    )?;
    if removed {
        return Ok(Some(LocatedRow {
            row_id,
            source_id,
            relation,
            table,
            host: None,
            standing: Standing::Removed,
        }));
    }
    let host: Option<String> = match host_column(conn, &relation)? {
        Some(column) => conn.query_row(
            &format!(
                "SELECT CAST({} AS VARCHAR) FROM {table} WHERE _ox_row = ?",
                quote_ident(&column)
            ),
            params![row_id],
            |r| r.get(0),
        )?,
        None => None,
    };
    // A loose file loaded without a host has one dataset row, its host NULL.
    let active: Option<bool> = conn.query_row(
        "SELECT bool_or(active) FROM ox.source_dataset \
         WHERE source_id = ? AND relation = ? AND host IS NOT DISTINCT FROM ?",
        params![source_id, relation, host],
        |r| r.get(0),
    )?;
    let standing = if active == Some(true) {
        Standing::Active
    } else {
        Standing::Superseded
    };
    Ok(Some(LocatedRow {
        row_id,
        source_id,
        relation,
        table,
        host,
        standing,
    }))
}

/// Why row `row_id` may not take a new annotation, or `None` when it may:
/// it must exist and be in its relation's view.
fn unwritable(conn: &Connection, row_id: i64) -> Result<Option<String>> {
    Ok(match find_row(conn, row_id)? {
        None => Some(no_row(row_id)),
        Some(row) => inactive(&row),
    })
}

/// Why located row `row` is out of its relation's view, or `None` when it
/// is in it.
fn inactive(row: &LocatedRow) -> Option<String> {
    let row_id = row.row_id;
    match row.standing {
        Standing::Active => None,
        Standing::Superseded => Some(format!(
            "row {row_id} is from a superseded run: a later run of its host replaced \
             it, so the row is no longer in the view"
        )),
        Standing::Removed => Some(format!(
            "row {row_id} is from a removed source (source {}); its data is no longer \
             in the case",
            row.source_id
        )),
    }
}

fn no_row(row_id: i64) -> String {
    format!("no row {row_id} in the case")
}
