//! Host records, as far as evidence needs them (ruling S7): attaching a row
//! from host `H` links its entry to the host record named `H`, creating
//! that record when the case has none.

use crate::actor::Actor;
use crate::annot::{record, record_all, write_rows, Change};
use crate::error::Result;
use duckdb::types::Value as SqlValue;
use duckdb::{params, Connection, OptionalExt};
use std::collections::HashSet;

/// The id of the live host record named `name` (ASCII case ignored, the
/// lowest id should there be more than one), creating it when the case has
/// none: `reviewed` for an analyst, `unreviewed` for an agent, with its
/// history row. Runs in the caller's transaction, stamped `at`.
pub(crate) fn ensure_host(conn: &Connection, name: &str, actor: &Actor, at: &str) -> Result<i64> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM ox.host WHERE NOT deleted AND lower(name) = lower(?) \
             ORDER BY id LIMIT 1",
            params![name],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let status = if actor.is_agent() {
        "unreviewed"
    } else {
        "reviewed"
    };
    let label = actor.label();
    let id: i64 = conn.query_row(
        "INSERT INTO ox.host \
         (id, name, role, note, status, created_by, created_at, modified_by, modified_at) \
         VALUES (nextval('ox.host_seq'), ?, '', '', ?, ?, ?, ?, ?) RETURNING id",
        params![name, status, label, at, label, at],
        |r| r.get(0),
    )?;
    record(
        conn,
        actor,
        at,
        Change {
            target_kind: "host",
            target_id: id.to_string(),
            action: "host_create",
            before: None,
            after: Some(serde_json::json!({
                "id": id, "name": name, "role": "", "note": "", "status": status,
                "created_by": label, "created_at": at, "deleted": false,
            })),
        },
    )?;
    Ok(id)
}

/// The history id of the link from entry `entry_id` to host record
/// `host_id`, as every `link` history row names it.
pub(crate) fn host_link_id(entry_id: i64, host_id: i64) -> String {
    format!("entry_host:{entry_id}:{host_id}")
}

/// Link each `(entry, host record)` pair not linked yet, with one history
/// row per new link, in the caller's transaction, stamped `at`. A pair
/// already linked, or named twice, is linked once.
pub(crate) fn link_hosts(
    conn: &Connection,
    pairs: &[(i64, i64)],
    actor: &Actor,
    at: &str,
) -> Result<()> {
    if pairs.is_empty() {
        return Ok(());
    }
    let entries: Vec<i64> = pairs
        .iter()
        .map(|(entry, _)| *entry)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut linked: HashSet<(i64, i64)> = HashSet::new();
    for chunk in entries.chunks(crate::annot::IDS_PER_STATEMENT) {
        let mut stmt = conn.prepare(&format!(
            "SELECT entry_id, record_id FROM ox.entry_host WHERE entry_id IN ({})",
            crate::annot::id_list(chunk)
        ))?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        for row in rows {
            linked.insert(row?);
        }
    }
    let label = actor.label();
    let mut rows = Vec::new();
    let mut history = Vec::new();
    for &(entry, host) in pairs {
        if !linked.insert((entry, host)) {
            continue;
        }
        rows.push(vec![
            SqlValue::BigInt(entry),
            SqlValue::BigInt(host),
            SqlValue::Text(label.clone()),
            SqlValue::Text(at.to_string()),
        ]);
        history.push(Change {
            target_kind: "link",
            target_id: host_link_id(entry, host),
            action: "link_add",
            before: None,
            after: Some(serde_json::json!({
                "kind": "host", "entry_id": entry, "record_id": host,
                "linked_by": label, "linked_at": at,
            })),
        });
    }
    write_rows(
        conn,
        "INSERT INTO ox.entry_host (entry_id, record_id, linked_by, linked_at) VALUES {values}",
        "(?, ?, ?, ?)",
        &rows,
    )?;
    record_all(conn, actor, at, history)
}
