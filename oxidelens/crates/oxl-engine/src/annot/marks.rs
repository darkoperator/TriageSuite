//! Suspicious marks.
//!
//! A row carries at most one mark, and the engine keeps it so:
//! `ox.row_mark` has no uniqueness constraint (supersession repoints
//! `row_id`), so a mark is inserted only where none exists, a take-over
//! rewrites the one there, and rebinding never moves a mark onto a row that
//! already has one.
//!
//! An agent's mark is provisional: an analyst marking the row takes it
//! over, and an agent clears only a mark it set itself.

use super::history::{record_all, Change};
use super::{
    check_batch, id_list, locate_rows, no_row, per_id_chunk, unwritable, who_may_undo, write_rows,
    LocatedRow, RowOutcome, Standing, IDS_PER_STATEMENT,
};
use crate::actor::Actor;
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::listing::{check_limit, fetch_limit, Listing};
use crate::query::{check_offset, read_mark, MarkState};
use crate::time::now_iso;
use duckdb::types::Value as SqlValue;
use duckdb::{params_from_iter, Connection};
use std::collections::HashMap;

#[derive(Debug, Clone, serde::Serialize)]
pub struct MarkedRow {
    pub row_id: i64,
    /// Empty only for a mark on a row id no source range covers, which a
    /// case the engine wrote never holds (as `RowDetail::relation`).
    pub relation: String,
    /// `None` for a relation without a host column, or a row of a removed
    /// source, whose data is gone.
    pub host: Option<String>,
    pub mark: MarkState,
    pub marked_by: String,
    pub marked_at: String,
    /// The row is in its relation's view, as `RowDetail::active` says: false
    /// once a later run superseded it or its source was removed.
    pub active: bool,
}

/// The mark on a row, as stored.
#[derive(Clone)]
pub(super) struct Mark {
    pub mark: String,
    pub marked_by: String,
    pub marked_at: String,
}

impl Mark {
    fn json(&self, row_id: i64) -> serde_json::Value {
        serde_json::json!({
            "row_id": row_id, "mark": self.mark,
            "marked_by": self.marked_by, "marked_at": self.marked_at,
        })
    }
}

/// Mark each of `rows` suspicious. An unknown or inactive row is not done.
/// An unmarked row is marked. A row an agent marked is taken over by an
/// analyst; a row an analyst marked stays theirs, done and unchanged. An
/// agent never changes a marked row: not done, "already marked". A row id
/// given more than once is one row, with one outcome, at its first place.
/// `Invalid` for an empty list; `Refused` for an agent passing more than
/// `AGENT_ROWS_PER_CALL` distinct rows.
pub fn mark_suspicious(case: &mut Case, rows: &[i64], actor: &Actor) -> Result<Vec<RowOutcome>> {
    let rows = &check_batch(rows, actor)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let mut batch = MarkBatch::new(&tx, rows, actor, &at)?;
    let out = rows.iter().map(|&row| batch.mark(row)).collect();
    batch.write()?;
    tx.commit()?;
    Ok(out)
}

/// Clear the mark on each of `rows`. An analyst clears any mark; an agent
/// only one it set. A row with no mark is not done. A row out of the view
/// may still be cleared: the mark it carries was left behind there. The
/// whole-call checks are `mark_suspicious`'s.
pub fn clear_suspicious(case: &mut Case, rows: &[i64], actor: &Actor) -> Result<Vec<RowOutcome>> {
    let rows = &check_batch(rows, actor)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let mut batch = MarkBatch::new(&tx, rows, actor, &at)?;
    let out = rows.iter().map(|&row| batch.clear(row)).collect();
    batch.write()?;
    tx.commit()?;
    Ok(out)
}

/// One call's mark change over its rows, each named once. It reads what it
/// needs about them up front, decides each row in order, and writes every
/// change at the end a few statements at a time.
struct MarkBatch<'a> {
    conn: &'a Connection,
    actor: &'a Actor,
    at: &'a str,
    located: HashMap<i64, LocatedRow>,
    /// The mark on each row, as the batch has left it so far.
    marked: HashMap<i64, Mark>,
    inserts: Vec<(i64, Mark)>,
    take_overs: Vec<(i64, Mark)>,
    clears: Vec<i64>,
    history: Vec<Change<'static>>,
}

impl<'a> MarkBatch<'a> {
    fn new(conn: &'a Connection, rows: &[i64], actor: &'a Actor, at: &'a str) -> Result<Self> {
        Ok(MarkBatch {
            conn,
            actor,
            at,
            located: locate_rows(conn, rows)?,
            marked: marks_on(conn, rows)?,
            inserts: Vec::new(),
            take_overs: Vec::new(),
            clears: Vec::new(),
            history: Vec::new(),
        })
    }

    fn mark(&mut self, row: i64) -> RowOutcome {
        if let Some(reason) = unwritable(&self.located, row) {
            return RowOutcome::refused(row, reason);
        }
        let wanted = Mark {
            mark: MarkState::Suspicious.as_str().to_string(),
            marked_by: self.actor.label(),
            marked_at: self.at.to_string(),
        };
        let Some(existing) = self.marked.get(&row).cloned() else {
            self.history
                .push(change(row, "mark_set", None, Some(&wanted)));
            self.inserts.push((row, wanted.clone()));
            self.marked.insert(row, wanted);
            return RowOutcome::changed(row);
        };
        let already = format!("already marked by {}", existing.marked_by);
        if self.actor.is_agent() {
            return RowOutcome::refused(row, already);
        }
        if Actor::parse(&existing.marked_by).is_ok_and(|owner| owner.is_agent()) {
            self.history.push(change(
                row,
                "mark_take_over",
                Some(&existing),
                Some(&wanted),
            ));
            self.take_overs.push((row, wanted.clone()));
            self.marked.insert(row, wanted);
            return RowOutcome::changed(row);
        }
        // Marked by an analyst, this one or another: it is already what was asked.
        RowOutcome::unchanged(row, already)
    }

    fn clear(&mut self, row: i64) -> RowOutcome {
        let Some(existing) = self.marked.get(&row).cloned() else {
            let reason = if self.located.contains_key(&row) {
                format!("row {row} is not marked")
            } else {
                no_row(row)
            };
            return RowOutcome::refused(row, reason);
        };
        if self.actor.is_agent() && existing.marked_by != self.actor.label() {
            return RowOutcome::refused(
                row,
                format!(
                    "marked by {}; only {} can clear it",
                    existing.marked_by,
                    who_may_undo(&existing.marked_by)
                ),
            );
        }
        self.history
            .push(change(row, "mark_clear", Some(&existing), None));
        self.clears.push(row);
        self.marked.remove(&row);
        RowOutcome::changed(row)
    }

    /// Write every change the batch decided, and its history.
    fn write(self) -> Result<()> {
        let conn = self.conn;
        per_id_chunk(
            conn,
            "DELETE FROM ox.row_mark WHERE row_id IN ({ids})",
            &[],
            &self.clears,
        )?;
        let values = |marks: &[(i64, Mark)]| -> Vec<Vec<SqlValue>> {
            marks
                .iter()
                .map(|(row, m)| {
                    vec![
                        SqlValue::BigInt(*row),
                        SqlValue::Text(m.mark.clone()),
                        SqlValue::Text(m.marked_by.clone()),
                        SqlValue::Text(m.marked_at.clone()),
                    ]
                })
                .collect()
        };
        write_rows(
            conn,
            "UPDATE ox.row_mark AS m \
             SET mark = v.mark, marked_by = v.marked_by, marked_at = v.marked_at \
             FROM (VALUES {values}) AS v(row_id, mark, marked_by, marked_at) \
             WHERE m.row_id = v.row_id",
            "(CAST(? AS BIGINT), CAST(? AS VARCHAR), CAST(? AS VARCHAR), CAST(? AS VARCHAR))",
            &values(&self.take_overs),
        )?;
        write_rows(
            conn,
            "INSERT INTO ox.row_mark (row_id, mark, marked_by, marked_at) VALUES {values}",
            "(?, ?, ?, ?)",
            &values(&self.inserts),
        )?;
        record_all(conn, self.actor, self.at, self.history)
    }
}

/// The mark on each of `rows` that has one: its earliest, should a row
/// carry more than one. Every reader of one row's mark goes through here.
pub(super) fn marks_on(conn: &Connection, rows: &[i64]) -> Result<HashMap<i64, Mark>> {
    let mut out = HashMap::new();
    for chunk in rows.chunks(IDS_PER_STATEMENT) {
        let mut stmt = conn.prepare(&format!(
            "SELECT row_id, mark, marked_by, marked_at FROM ox.row_mark \
             WHERE row_id IN ({}) \
             QUALIFY row_number() OVER (PARTITION BY row_id ORDER BY marked_at) = 1",
            id_list(chunk)
        ))?;
        let marks = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Mark {
                    mark: r.get(1)?,
                    marked_by: r.get(2)?,
                    marked_at: r.get(3)?,
                },
            ))
        })?;
        for mark in marks {
            let (row, mark) = mark?;
            out.insert(row, mark);
        }
    }
    Ok(out)
}

/// The history entry for a change to `row`'s mark.
fn change<'a>(
    row: i64,
    action: &'a str,
    before: Option<&Mark>,
    after: Option<&Mark>,
) -> Change<'a> {
    Change {
        target_kind: "row_mark",
        target_id: row.to_string(),
        action,
        before: before.map(|m| m.json(row)),
        after: after.map(|m| m.json(row)),
    }
}

/// Up to `limit` marked rows from `offset`, every mark or only `by`'s, in
/// the order they were marked. A row out of the view is listed, with
/// `active` false. `Invalid` when `limit` is 0 or over `MAX_LIMIT`, or
/// `offset` past `i64::MAX`.
pub fn marks(
    case: &Case,
    by: Option<&Actor>,
    limit: u32,
    offset: u64,
) -> Result<Listing<MarkedRow>> {
    check_limit(limit)?;
    check_offset(offset)?;
    let conn = case.conn();
    let mut params = Vec::new();
    let filter = match by {
        Some(actor) => {
            params.push(SqlValue::Text(actor.label()));
            "WHERE marked_by = ?"
        }
        None => "",
    };
    params.push(SqlValue::BigInt(fetch_limit(limit)));
    params.push(SqlValue::UBigInt(offset));
    let stored: Vec<(i64, String, String, String)> = conn
        .prepare(&format!(
            "SELECT row_id, mark, marked_by, marked_at FROM ox.row_mark {filter} \
             ORDER BY marked_at, row_id LIMIT ? OFFSET ?"
        ))?
        .query_map(params_from_iter(params), |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<std::result::Result<_, _>>()?;
    let stored = Listing::from_fetched(stored, limit);
    let ids: Vec<i64> = stored.items.iter().map(|(id, ..)| *id).collect();
    let located = locate_rows(conn, &ids)?;
    let mut out = Vec::with_capacity(stored.items.len());
    for (row_id, mark, marked_by, marked_at) in stored.items {
        let mark = read_mark(Some(mark))?.ok_or_else(|| {
            EngineError::Internal(format!("the mark on row {row_id} read back as none"))
        })?;
        // Removing a source keeps its rows' ranges, so a mark the engine
        // wrote always locates. One that does not (a case edited by hand)
        // is listed as out of every view, as `row_detail` shows it, rather
        // than failing the listing: its relation is empty and its host
        // unknown.
        let (relation, host, active) = match located.get(&row_id) {
            Some(row) => (
                row.relation.clone(),
                row.host.clone(),
                row.standing == Standing::Active,
            ),
            None => (String::new(), None, false),
        };
        out.push(MarkedRow {
            row_id,
            relation,
            host,
            mark,
            marked_by,
            marked_at,
            active,
        });
    }
    Ok(Listing {
        items: out,
        truncated: stored.truncated,
    })
}
