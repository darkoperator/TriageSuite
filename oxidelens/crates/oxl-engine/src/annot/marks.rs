//! Suspicious marks, and the marks listing.
//!
//! A row carries at most one Suspicious mark, and the engine keeps it so:
//! `ox.row_mark` has no uniqueness constraint (supersession repoints
//! `row_id`), so a mark is inserted only where none exists, a take-over
//! rewrites the one there, and rebinding never moves a mark onto a row that
//! already has one.
//!
//! An agent's mark is provisional: an analyst marking the row takes it
//! over, and an agent clears only a mark it set itself.
//!
//! A row that is evidence on an analyst's live entry is Malicious, derived
//! from the timeline (ruling S2): S and U leave it, naming the entry to
//! detach it from. A row that is evidence only on an agent's unreviewed
//! entries is Agent: malicious: an agent may not mark it (it is marked), and
//! U on it without a Suspicious mark names the entry. Everything that shows
//! a mark reads `ox.row_state` (`record::marks_relation`).

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
use crate::record::{detach_there, has_records, holders, marks_relation, Holder};
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
    /// Who the mark is from: its setter, or the creator of the entry that
    /// gives it (`ox.row_state."by"`).
    pub marked_by: String,
    /// When it was set, or when the row was first attached to a live entry
    /// by that creator.
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
/// A Malicious row is not done, the reason naming its entry; nor, for an
/// agent, an Agent: malicious row. An unmarked row is marked. A row an agent marked is taken over by an
/// analyst; a row an analyst marked stays theirs, done and unchanged. An
/// agent never changes a marked row: not done, "already marked". A row id
/// given more than once is one row, with one outcome, at its first place.
/// `Invalid` for an empty list; `Refused` for an agent passing more than
/// `AGENT_ROWS_PER_CALL` distinct rows.
pub fn mark_suspicious(case: &mut Case, rows: &[i64], actor: &Actor) -> Result<Vec<RowOutcome>> {
    let rows = &check_batch(rows, actor)?;
    let at = now_iso();
    let records = has_records(case);
    let tx = case.conn_mut().transaction()?;
    let mut batch = MarkBatch::new(&tx, rows, actor, &at, records)?;
    let out = rows.iter().map(|&row| batch.mark(row)).collect();
    batch.write()?;
    tx.commit()?;
    Ok(out)
}

/// Clear the Suspicious mark on each of `rows`. An analyst clears any; an
/// agent only one it set. A Malicious row is not done, the reason naming
/// its entry; nor is a row with no Suspicious mark (an Agent: malicious one
/// names its entry). A row out of the view
/// may still be cleared: the mark it carries was left behind there. The
/// whole-call checks are `mark_suspicious`'s.
pub fn clear_suspicious(case: &mut Case, rows: &[i64], actor: &Actor) -> Result<Vec<RowOutcome>> {
    let rows = &check_batch(rows, actor)?;
    let at = now_iso();
    let records = has_records(case);
    let tx = case.conn_mut().transaction()?;
    let mut batch = MarkBatch::new(&tx, rows, actor, &at, records)?;
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
    /// The entry each row that is evidence takes its derived mark from.
    held: HashMap<i64, Holder>,
    /// The mark on each row, as the batch has left it so far.
    marked: HashMap<i64, Mark>,
    inserts: Vec<(i64, Mark)>,
    take_overs: Vec<(i64, Mark)>,
    clears: Vec<i64>,
    history: Vec<Change<'static>>,
}

impl<'a> MarkBatch<'a> {
    /// `records`: the case has entries (format 3), so a row may be
    /// evidence.
    fn new(
        conn: &'a Connection,
        rows: &[i64],
        actor: &'a Actor,
        at: &'a str,
        records: bool,
    ) -> Result<Self> {
        Ok(MarkBatch {
            conn,
            actor,
            at,
            located: locate_rows(conn, rows)?,
            held: if records {
                holders(conn, rows)?
            } else {
                HashMap::new()
            },
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
        match self.held.get(&row) {
            Some(Holder::Analysts(entry)) => {
                return RowOutcome::refused(row, detach_there(*entry));
            }
            Some(Holder::Agents(entry)) if self.actor.is_agent() => {
                return RowOutcome::refused(
                    row,
                    format!("already marked Agent: malicious as evidence on entry E-{entry}"),
                );
            }
            _ => {}
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
        let held = self.held.get(&row);
        if let Some(Holder::Analysts(entry)) = held {
            return RowOutcome::refused(row, detach_there(*entry));
        }
        let Some(existing) = self.marked.get(&row).cloned() else {
            let reason = if let Some(holder) = held {
                detach_there(holder.entry())
            } else if self.located.contains_key(&row) {
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

/// Clear, in the caller's transaction, the Suspicious mark on each of
/// `rows` that `actor` may clear: an analyst any, an agent only its own.
/// A mark the actor may not clear stays, under whatever higher mark the
/// row has; a row with no mark is left. Returns the history of each clear
/// for the caller to record with the rest of its batch. M goes through
/// here, so promoting a row clears its mark by the rule U follows.
pub(crate) fn clear_where_allowed(
    conn: &Connection,
    rows: &[i64],
    actor: &Actor,
) -> Result<Vec<Change<'static>>> {
    let label = actor.label();
    let mut marked: Vec<(i64, Mark)> = marks_on(conn, rows)?
        .into_iter()
        .filter(|(_, mark)| !actor.is_agent() || mark.marked_by == label)
        .collect();
    marked.sort_by_key(|(row, _)| *row);
    let cleared: Vec<i64> = marked.iter().map(|(row, _)| *row).collect();
    per_id_chunk(
        conn,
        "DELETE FROM ox.row_mark WHERE row_id IN ({ids})",
        &[],
        &cleared,
    )?;
    Ok(marked
        .iter()
        .map(|(row, mark)| change(*row, "mark_clear", Some(mark), None))
        .collect())
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

/// When the mark on the listed row `s` was set: its Suspicious mark's
/// time, or, for a derived mark, when its giver (`s."by"`) first attached
/// the row to a live entry. Below format 3 there are no entries.
fn marked_at_sql(records: bool) -> &'static str {
    if records {
        "COALESCE(\
           (SELECT min(m.marked_at) FROM ox.row_mark AS m \
            WHERE m.row_id = s.row_id AND s.mark = 'suspicious'), \
           (SELECT min(v.attached_at) FROM ox.evidence AS v \
            JOIN ox.entry AS e ON e.id = v.entry_id \
            WHERE v.row_id = s.row_id AND NOT e.deleted AND e.created_by = s.\"by\"), '')"
    } else {
        "(SELECT min(m.marked_at) FROM ox.row_mark AS m WHERE m.row_id = s.row_id)"
    }
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

/// Up to `limit` marked rows from `offset`, as `ox.row_state` gives them,
/// every mark or only those from `by`, in the order they were marked. A row
/// out of the view is listed, with `active` false. `Invalid` when `limit`
/// is 0 or over `MAX_LIMIT`, or `offset` past `i64::MAX`.
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
            "WHERE s.\"by\" = ?"
        }
        None => "",
    };
    params.push(SqlValue::BigInt(fetch_limit(limit)));
    params.push(SqlValue::UBigInt(offset));
    let stored: Vec<(i64, String, String, String)> = conn
        .prepare(&format!(
            "SELECT s.row_id, s.mark, s.\"by\", {at} AS ox_at FROM {marks} AS s {filter} \
             ORDER BY ox_at, s.row_id LIMIT ? OFFSET ?",
            marks = marks_relation(case),
            at = marked_at_sql(has_records(case)),
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
