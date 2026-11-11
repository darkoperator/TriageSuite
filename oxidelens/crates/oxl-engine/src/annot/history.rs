//! The append-only change log, `ox.history`: written here for marks, tags
//! and notes, read back newest first. Nothing updates or deletes it.

use crate::actor::Actor;
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::query::MAX_LIMIT;
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
    conn.execute(
        "INSERT INTO ox.history \
         (id, \"at\", \"by\", target_kind, target_id, action, before_json, after_json) \
         VALUES (nextval('ox.history_seq'), ?, ?, ?, ?, ?, ?, ?)",
        params![
            at,
            actor.label(),
            change.target_kind,
            change.target_id,
            change.action,
            change.before.map(|v| v.to_string()),
            change.after.map(|v| v.to_string()),
        ],
    )?;
    Ok(())
}

/// Up to `limit` history rows from `offset`, newest first, of one target
/// kind or of all; the flag says more follow. `Invalid` for an unknown
/// target kind or a `limit` over `MAX_LIMIT`.
pub fn history(
    case: &Case,
    target_kind: Option<&str>,
    limit: u32,
    offset: u64,
) -> Result<(Vec<HistoryRow>, bool)> {
    if limit > MAX_LIMIT {
        return Err(EngineError::Invalid(format!(
            "a page is at most {MAX_LIMIT} rows, not {limit}"
        )));
    }
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
    // Fetching one more than asked says whether the page is the last one.
    params.push(SqlValue::BigInt(i64::from(limit) + 1));
    params.push(SqlValue::UBigInt(offset));
    let mut stmt = case.conn().prepare(&format!(
        "SELECT id, \"at\", \"by\", target_kind, target_id, action, before_json, after_json \
         FROM ox.history {filter} ORDER BY id DESC LIMIT ? OFFSET ?"
    ))?;
    let mut rows = stmt.query(params_from_iter(params))?;
    let mut out = Vec::new();
    let mut more = false;
    while let Some(row) = rows.next()? {
        if out.len() == limit as usize {
            more = true;
            break;
        }
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
    Ok((out, more))
}

fn json(id: i64, text: Option<String>) -> Result<Option<serde_json::Value>> {
    text.map(|text| {
        serde_json::from_str(&text).map_err(|e| {
            EngineError::Invalid(format!(
                "history row {id} holds JSON that does not parse: {e}"
            ))
        })
    })
    .transpose()
}
