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
pub use row::{row_detail, row_place, RowDetail, RowPlace, RowStanding};
pub use tags::{add_tag, remove_tag, tags, TagInfo, AGENT_TAG_PREFIX, MAX_TAG};

use crate::actor::Actor;
use crate::error::{EngineError, Result};
use duckdb::types::Value as SqlValue;
use duckdb::{params_from_iter, Connection};
use std::collections::{BTreeMap, HashMap, HashSet};
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

/// The rows a batch works on: `rows` with each id kept once, at its first
/// appearance, so a row named twice gets one outcome and counts once. The
/// whole-call checks are made on them: at least one row, and no more than
/// `AGENT_ROWS_PER_CALL` from an agent.
fn check_batch(rows: &[i64], actor: &Actor) -> Result<Vec<i64>> {
    if rows.is_empty() {
        return Err(EngineError::Invalid("no row ids were given".to_string()));
    }
    let mut seen = HashSet::with_capacity(rows.len());
    let rows: Vec<i64> = rows.iter().copied().filter(|r| seen.insert(*r)).collect();
    if actor.is_agent() && rows.len() > AGENT_ROWS_PER_CALL {
        return Err(EngineError::Refused(format!(
            "an agent may change at most {AGENT_ROWS_PER_CALL} rows per call, not {}",
            rows.len()
        )));
    }
    Ok(rows)
}

/// Who may undo what `owner` (an actor label) marked or tagged, as a
/// refusal says it: an agent's, that agent or an analyst; anyone else's,
/// an analyst.
fn who_may_undo(owner: &str) -> String {
    match Actor::parse(owner) {
        Ok(actor) if actor.is_agent() => format!("{owner} or an analyst"),
        _ => "an analyst".to_string(),
    }
}

/// The source, relation and schema-qualified table holding row `row_id`;
/// `None` when no source ever loaded it. See `locate_rows`.
pub(crate) fn locate(conn: &Connection, row_id: i64) -> Result<Option<(i64, String, String)>> {
    Ok(find_row(conn, row_id)?.map(|r| (r.source_id, r.relation, r.table)))
}

/// The most ids one statement names. A batch has no limit of its own (an
/// analyst's has none); its ids are asked about this many at a time.
pub(crate) const IDS_PER_STATEMENT: usize = 2_048;

/// `ids` as the text of a SQL list. They are integers, so the text is
/// safe to put into a statement as it is.
pub(crate) fn id_list(ids: &[i64]) -> String {
    ids.iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The most rows one multi-row write carries.
const ROWS_PER_STATEMENT: usize = 512;

/// Run `sql` over `rows` a chunk at a time: its `{values}` becomes
/// `placeholder` (one row's `(?, ...)`) once per row of the chunk, and the
/// chunk's values are its parameters, in order. A batch decides every row
/// first and then writes them this way, so its cost does not grow by a
/// statement per row. Supersession's rebinding writes the same way.
pub(crate) fn write_rows(
    conn: &Connection,
    sql: &str,
    placeholder: &str,
    rows: &[Vec<SqlValue>],
) -> Result<()> {
    for chunk in rows.chunks(ROWS_PER_STATEMENT) {
        let values = vec![placeholder; chunk.len()].join(", ");
        let params: Vec<&SqlValue> = chunk.iter().flatten().collect();
        conn.execute(&sql.replace("{values}", &values), params_from_iter(params))?;
    }
    Ok(())
}

/// Run `sql` once per chunk of `ids`, its `{ids}` the chunk as a SQL list.
fn per_id_chunk(conn: &Connection, sql: &str, params: &[SqlValue], ids: &[i64]) -> Result<()> {
    for chunk in ids.chunks(IDS_PER_STATEMENT) {
        conn.execute(
            &sql.replace("{ids}", &id_list(chunk)),
            params_from_iter(params),
        )?;
    }
    Ok(())
}

/// Optional text as a statement parameter.
fn text_or_null(text: Option<String>) -> SqlValue {
    text.map_or(SqlValue::Null, SqlValue::Text)
}

/// Each of `ids` that a source ever loaded, located, with its host and
/// standing; an id no source loaded is absent. A row is found through the
/// row-id ranges `ox.source_table` records. Each table's rows take one
/// contiguous run of `ox.row_seq` in a single statement, so the ranges do
/// not overlap; a live table is still asked whether it holds the row. A
/// removed source's schema is gone and its range is all that remains of
/// it. The whole batch is read a few statements at a time: the ranges, then
/// each table the rows fall in once, then the catalog.
pub(crate) fn locate_rows(conn: &Connection, ids: &[i64]) -> Result<HashMap<i64, LocatedRow>> {
    let mut wanted: Vec<i64> = ids.to_vec();
    wanted.sort_unstable();
    wanted.dedup();

    // Every range each id falls in, in source then relation order.
    let mut candidates: Vec<(i64, i64, String, String, bool)> = Vec::new();
    for chunk in wanted.chunks(IDS_PER_STATEMENT) {
        let mut stmt = conn.prepare(&format!(
            "SELECT u.id, st.source_id, st.relation, st.table_schema, s.removed \
             FROM (SELECT unnest([{}]) AS id) AS u \
             JOIN ox.source_table st ON u.id BETWEEN st.first_row AND st.last_row \
             JOIN ox.source s ON s.id = st.source_id \
             ORDER BY u.id, st.source_id, st.relation",
            id_list(chunk)
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        for row in rows {
            candidates.push(row?);
        }
    }
    if candidates.is_empty() {
        return Ok(HashMap::new());
    }

    let host_columns: HashMap<String, Option<String>> = conn
        .prepare("SELECT name, host_column FROM ox.relation")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;

    // Which ids each live table holds, and each one's host.
    let mut by_table: BTreeMap<(String, String), Vec<i64>> = BTreeMap::new();
    for (id, _, relation, schema, removed) in &candidates {
        if !removed {
            by_table
                .entry((schema.clone(), relation.clone()))
                .or_default()
                .push(*id);
        }
    }
    let mut held: HashMap<(String, String, i64), Option<String>> = HashMap::new();
    for ((schema, relation), ids) in by_table {
        let table = format!("{}.{}", quote_ident(&schema), quote_ident(&relation));
        let host = match host_columns.get(&relation).cloned().flatten() {
            Some(column) => format!("CAST({} AS VARCHAR)", quote_ident(&column)),
            None => "CAST(NULL AS VARCHAR)".to_string(),
        };
        for chunk in ids.chunks(IDS_PER_STATEMENT) {
            let mut stmt = conn.prepare(&format!(
                "SELECT _ox_row, {host} FROM {table} WHERE _ox_row IN ({})",
                id_list(chunk)
            ))?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get(1)?)))?;
            for row in rows {
                let (id, host) = row?;
                held.insert((schema.clone(), relation.clone(), id), host);
            }
        }
    }

    // A loose file loaded without a host has one dataset row, its host NULL.
    let active: HashMap<(i64, String, Option<String>), bool> = conn
        .prepare(
            "SELECT source_id, relation, host, bool_or(active) FROM ox.source_dataset \
             GROUP BY source_id, relation, host",
        )?
        .query_map([], |r| {
            Ok(((r.get(0)?, r.get(1)?, r.get(2)?), r.get::<_, bool>(3)?))
        })?
        .collect::<std::result::Result<_, _>>()?;

    let mut out = HashMap::new();
    for (id, source_id, relation, schema, removed) in candidates {
        if out.contains_key(&id) {
            continue;
        }
        let table = format!("{}.{}", quote_ident(&schema), quote_ident(&relation));
        let (host, standing) = if removed {
            (None, Standing::Removed)
        } else {
            let Some(host) = held.get(&(schema, relation.clone(), id)) else {
                continue;
            };
            let is_active = active
                .get(&(source_id, relation.clone(), host.clone()))
                .copied()
                .unwrap_or(false);
            let standing = if is_active {
                Standing::Active
            } else {
                Standing::Superseded
            };
            (host.clone(), standing)
        };
        out.insert(
            id,
            LocatedRow {
                row_id: id,
                source_id,
                relation,
                table,
                host,
                standing,
            },
        );
    }
    Ok(out)
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
    Ok(locate_rows(conn, &[row_id])?.remove(&row_id))
}

/// Why row `row_id`, as `located` found it, may not take a new annotation,
/// or `None` when it may: it must exist and be in its relation's view.
fn unwritable(located: &HashMap<i64, LocatedRow>, row_id: i64) -> Option<String> {
    match located.get(&row_id) {
        None => Some(no_row(row_id)),
        Some(row) => inactive(row),
    }
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
