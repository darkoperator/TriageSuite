//! A row's mark, as every reader shows it (ruling S2).
//!
//! `ox.row_state` holds one row per marked row: `malicious` when the row is
//! evidence on a live entry an analyst created or reviewed, else
//! `agent_malicious` when it is evidence only on live unreviewed agent
//! entries, else the mark `ox.row_mark` stores (`suspicious`; any other
//! value is a damaged case, which `read_mark` reports as `Internal`).
//! Every query that reads a mark (a page, the `_mark` filter, a group key, the Values tab, the
//! histogram's and a stack's marked counts, the merged list, the counts,
//! the marks listing, the row detail) reads it through `marks_relation`, so
//! they cannot disagree. Writers read who holds a row through `holders`,
//! which decides it as the view does.

use super::has_records;
use crate::annot::{id_list, IDS_PER_STATEMENT};
use crate::case::Case;
use crate::error::Result;
use duckdb::Connection;
use std::collections::HashMap;

/// The view of every row's mark: `(row_id, mark, "by")`.
const ROW_STATE: &str = "ox.row_state";

/// What stands in for `ox.row_state` in a case opened read-only below
/// format 3, which has neither the view nor any entry: its stored marks,
/// in the view's columns, passed through as the view passes them.
const STORED_ONLY: &str = "(SELECT row_id, mark, marked_by AS \"by\" FROM ox.row_mark)";

/// SQL naming the relation of marks to read in `case`, to place after
/// `FROM` with an alias: columns `row_id`, `mark` and `"by"`, one row per
/// marked row. `ox.row_state`, or for a case below format 3 its stored
/// marks alone.
pub(crate) fn marks_relation(case: &Case) -> &'static str {
    if has_records(case) {
        ROW_STATE
    } else {
        STORED_ONLY
    }
}

/// The current format's marks relation, for SQL compiled without a case at
/// hand (`compile_filter`).
pub(crate) const CURRENT_MARKS: &str = ROW_STATE;

/// An entry the row is evidence on, by the mark `ox.row_state` gives it.
pub(crate) enum Holder {
    /// The lowest-numbered live entry an analyst created or reviewed.
    Analysts(i64),
    /// The lowest-numbered live entry, every one an agent's unreviewed.
    Agents(i64),
}

impl Holder {
    /// The entry that gives the row its mark.
    pub(crate) fn entry(&self) -> i64 {
        match self {
            Holder::Analysts(entry) | Holder::Agents(entry) => *entry,
        }
    }
}

/// Why S or U leaves a row that is evidence alone: its mark comes from
/// entry `entry`, and only detaching it there changes that.
pub(crate) fn detach_there(entry: i64) -> String {
    format!("evidence on entry E-{entry}; detach it there")
}

/// For each of `rows` that is evidence on a live entry, the entry that
/// gives its mark, decided as `ox.row_state` decides it.
pub(crate) fn holders(conn: &Connection, rows: &[i64]) -> Result<HashMap<i64, Holder>> {
    let mut found: HashMap<i64, (Option<i64>, i64)> = HashMap::new();
    for chunk in rows.chunks(IDS_PER_STATEMENT) {
        let mut stmt = conn.prepare(&format!(
            "SELECT v.row_id, e.id, \
                    NOT starts_with(e.created_by, 'agent:') OR e.status = 'reviewed' \
             FROM ox.evidence v JOIN ox.entry e ON e.id = v.entry_id \
             WHERE NOT e.deleted AND v.row_id IN ({})",
            id_list(chunk)
        ))?;
        let read = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, bool>(2)?,
            ))
        })?;
        for row in read {
            let (row, entry, analysts) = row?;
            let held = found.entry(row).or_insert((None, entry));
            held.1 = held.1.min(entry);
            if analysts {
                held.0 = Some(held.0.map_or(entry, |e: i64| e.min(entry)));
            }
        }
    }
    Ok(found
        .into_iter()
        .map(|(row, (analysts, any))| {
            let holder = match analysts {
                Some(entry) => Holder::Analysts(entry),
                None => Holder::Agents(any),
            };
            (row, holder)
        })
        .collect())
}
