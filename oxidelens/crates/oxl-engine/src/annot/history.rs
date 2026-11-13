//! The append-only change log, `ox.history`: written here for marks, tags
//! and notes, read back newest first. Nothing updates or deletes it.

use super::{text_or_null, write_rows};
use crate::actor::Actor;
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::listing::{check_limit, fetch_limit, Listing};
use crate::query::check_offset;
use duckdb::types::Value as SqlValue;
use duckdb::{params, params_from_iter, Connection};

/// Every target kind a history row names (ruling R2).
const TARGET_KINDS: [&str; 5] = ["source", "relation", "row_mark", "row_tag", "note"];

#[derive(Debug, Clone, serde::Serialize)]
pub struct HistoryRow {
    pub id: i64,
    pub at: String,
    pub by: String,
    pub target_kind: String,
    pub target_id: String,
    pub action: String,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
}

/// One change to record: what it touched, what was done, and the object
/// before and after (`None` when it did not exist, or no longer does).
pub(super) struct Change<'a> {
    pub target_kind: &'a str,
    pub target_id: String,
    pub action: &'a str,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
}

/// Write `change`'s history row in the caller's transaction, stamped `at`
/// and by `actor`.
pub(super) fn record(conn: &Connection, actor: &Actor, at: &str, change: Change<'_>) -> Result<()> {
    record_all(conn, actor, at, vec![change])
}

/// Write one history row per change, in the caller's transaction, stamped
/// `at` and by `actor`, their ids rising in the order given: the order a
/// batch made the changes, as one statement per change would have numbered
/// them.
pub(super) fn record_all(
    conn: &Connection,
    actor: &Actor,
    at: &str,
    changes: Vec<Change<'_>>,
) -> Result<()> {
    if changes.is_empty() {
        return Ok(());
    }
    let count = i64::try_from(changes.len()).unwrap_or(i64::MAX);
    let mut ids: Vec<i64> = conn
        .prepare("SELECT nextval('ox.history_seq') FROM range(?)")?
        .query_map(params![count], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    ids.sort_unstable();
    let label = actor.label();
    let rows: Vec<Vec<SqlValue>> = ids
        .into_iter()
        .zip(changes)
        .map(|(id, change)| {
            vec![
                SqlValue::BigInt(id),
                SqlValue::Text(at.to_string()),
                SqlValue::Text(label.clone()),
                SqlValue::Text(change.target_kind.to_string()),
                SqlValue::Text(change.target_id),
                SqlValue::Text(change.action.to_string()),
                text_or_null(change.before.map(|v| v.to_string())),
                text_or_null(change.after.map(|v| v.to_string())),
            ]
        })
        .collect();
    write_rows(
        conn,
        "INSERT INTO ox.history \
         (id, \"at\", \"by\", target_kind, target_id, action, before_json, after_json) \
         VALUES {values}",
        "(?, ?, ?, ?, ?, ?, ?, ?)",
        &rows,
    )
}

/// Up to `limit` history rows from `offset`, newest first, of one target
/// kind or of all. `Invalid` for an unknown target kind, a `limit` of 0 or
/// over `MAX_LIMIT`, or an `offset` past `i64::MAX`.
pub fn history(
    case: &Case,
    target_kind: Option<&str>,
    limit: u32,
    offset: u64,
) -> Result<Listing<HistoryRow>> {
    check_limit(limit)?;
    check_offset(offset)?;
    let mut params = Vec::new();
    let filter = match target_kind {
        None => "",
        Some(kind) if TARGET_KINDS.contains(&kind) => {
            params.push(SqlValue::Text(kind.to_string()));
            "WHERE target_kind = ?"
        }
        Some(kind) => {
            return Err(EngineError::Invalid(format!(
                "{kind:?} is not a target kind; one of {}",
                TARGET_KINDS.join(", ")
            )))
        }
    };
    params.push(SqlValue::BigInt(fetch_limit(limit)));
    params.push(SqlValue::UBigInt(offset));
    let mut stmt = case.conn().prepare(&format!(
        "SELECT id, \"at\", \"by\", target_kind, target_id, action, before_json, after_json \
         FROM ox.history {filter} ORDER BY id DESC LIMIT ? OFFSET ?"
    ))?;
    let mut rows = stmt.query(params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        out.push(HistoryRow {
            id,
            at: row.get(1)?,
            by: row.get(2)?,
            target_kind: row.get(3)?,
            target_id: row.get(4)?,
            action: row.get(5)?,
            before: json(id, row.get(6)?)?,
            after: json(id, row.get(7)?)?,
        });
    }
    Ok(Listing::from_fetched(out, limit))
}

fn json(id: i64, text: Option<String>) -> Result<Option<serde_json::Value>> {
    text.map(|text| {
        // The engine writes every history row's JSON itself.
        serde_json::from_str(&text).map_err(|e| {
            EngineError::Internal(format!(
                "history row {id} holds JSON that does not parse: {e}"
            ))
        })
    })
    .transpose()
}
