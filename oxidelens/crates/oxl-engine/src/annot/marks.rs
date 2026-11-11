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

use super::history::{record, Change};
use super::{check_batch, find_row, no_row, unwritable, RowOutcome, Standing};
use crate::actor::Actor;
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::listing::{check_limit, fetch_limit, Listing};
use crate::query::MarkState;
use crate::time::now_iso;
use duckdb::types::Value as SqlValue;
use duckdb::{params, params_from_iter, Connection};

#[derive(Debug, Clone, serde::Serialize)]
pub struct MarkedRow {
    pub row_id: i64,
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
struct Mark {
    mark: String,
    marked_by: String,
    marked_at: String,
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
/// agent never changes a marked row: not done, "already marked". `Invalid`
/// for an empty list; `Refused` for an agent passing more than
/// `AGENT_ROWS_PER_CALL` rows.
pub fn mark_suspicious(case: &mut Case, rows: &[i64], actor: &Actor) -> Result<Vec<RowOutcome>> {
    check_batch(rows, actor)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let out = rows
        .iter()
        .map(|&row| mark_one(&tx, row, actor, &at))
        .collect::<Result<_>>()?;
    tx.commit()?;
    Ok(out)
}

fn mark_one(conn: &Connection, row: i64, actor: &Actor, at: &str) -> Result<RowOutcome> {
    if let Some(reason) = unwritable(conn, row)? {
        return Ok(RowOutcome::refused(row, reason));
    }
    let label = actor.label();
    let wanted = Mark {
        mark: MarkState::Suspicious.as_str().to_string(),
        marked_by: label.clone(),
        marked_at: at.to_string(),
    };
    let Some(existing) = mark_on(conn, row)? else {
        conn.execute(
            "INSERT INTO ox.row_mark (row_id, mark, marked_by, marked_at) VALUES (?, ?, ?, ?)",
            params![row, wanted.mark, wanted.marked_by, wanted.marked_at],
        )?;
        record(
            conn,
            actor,
            at,
            change(row, "mark_set", None, Some(&wanted)),
        )?;
        return Ok(RowOutcome::changed(row));
    };
    let already = format!("already marked by {}", existing.marked_by);
    if actor.is_agent() {
        return Ok(RowOutcome::refused(row, already));
    }
    if Actor::parse(&existing.marked_by).is_ok_and(|owner| owner.is_agent()) {
        conn.execute(
            "UPDATE ox.row_mark SET mark = ?, marked_by = ?, marked_at = ? WHERE row_id = ?",
            params![wanted.mark, wanted.marked_by, wanted.marked_at, row],
        )?;
        let take_over = change(row, "mark_take_over", Some(&existing), Some(&wanted));
        record(conn, actor, at, take_over)?;
        return Ok(RowOutcome::changed(row));
    }
    // Marked by an analyst, this one or another: it is already what was asked.
    Ok(RowOutcome::unchanged(row, already))
}

/// Clear the mark on each of `rows`. An analyst clears any mark; an agent
/// only one it set. A row with no mark is not done. A row out of the view
/// may still be cleared: the mark it carries was left behind there. The
/// whole-call checks are `mark_suspicious`'s.
pub fn clear_suspicious(case: &mut Case, rows: &[i64], actor: &Actor) -> Result<Vec<RowOutcome>> {
    check_batch(rows, actor)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let out = rows
        .iter()
        .map(|&row| clear_one(&tx, row, actor, &at))
        .collect::<Result<_>>()?;
    tx.commit()?;
    Ok(out)
}

fn clear_one(conn: &Connection, row: i64, actor: &Actor, at: &str) -> Result<RowOutcome> {
    let Some(existing) = mark_on(conn, row)? else {
        let reason = match find_row(conn, row)? {
            None => no_row(row),
            Some(_) => format!("row {row} is not marked"),
        };
        return Ok(RowOutcome::refused(row, reason));
    };
    if actor.is_agent() && existing.marked_by != actor.label() {
        return Ok(RowOutcome::refused(
            row,
            format!(
                "marked by {}; only an analyst can clear it",
                existing.marked_by
            ),
        ));
    }
    conn.execute("DELETE FROM ox.row_mark WHERE row_id = ?", params![row])?;
    record(
        conn,
        actor,
        at,
        change(row, "mark_clear", Some(&existing), None),
    )?;
    Ok(RowOutcome::changed(row))
}

/// The mark on `row`, if any.
fn mark_on(conn: &Connection, row: i64) -> Result<Option<Mark>> {
    Ok(conn
        .prepare(
            "SELECT mark, marked_by, marked_at FROM ox.row_mark WHERE row_id = ? \
             ORDER BY marked_at LIMIT 1",
        )?
        .query_map(params![row], |r| {
            Ok(Mark {
                mark: r.get(0)?,
                marked_by: r.get(1)?,
                marked_at: r.get(2)?,
            })
        })?
        .next()
        .transpose()?)
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
/// `active` false. `Invalid` when `limit` is 0 or over `MAX_LIMIT`.
pub fn marks(
    case: &Case,
    by: Option<&Actor>,
    limit: u32,
    offset: u64,
) -> Result<Listing<MarkedRow>> {
    check_limit(limit)?;
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
    let mut out = Vec::with_capacity(stored.items.len());
    for (row_id, mark, marked_by, marked_at) in stored.items {
        // Removing a source keeps its rows' ranges, so every mark locates.
        let row = find_row(conn, row_id)?.ok_or_else(|| {
            EngineError::Invalid(format!(
                "the case holds a mark on row {row_id}, which no source ever loaded"
            ))
        })?;
        let mark = MarkState::parse(&mark).ok_or_else(|| {
            EngineError::Invalid(format!("the case holds an unknown mark {mark:?}"))
        })?;
        out.push(MarkedRow {
            row_id,
            relation: row.relation,
            host: row.host,
            mark,
            marked_by,
            marked_at,
            active: row.standing == Standing::Active,
        });
    }
    Ok(Listing {
        items: out,
        truncated: stored.truncated,
    })
}
