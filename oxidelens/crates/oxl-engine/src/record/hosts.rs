//! Host records: what an analyst or agent keeps about a host, and the link
//! evidence makes (ruling S7): attaching a row from host `H` links its
//! entry to the host record named `H`, creating that record when the case
//! has none.

use super::records::{
    agent_added, changeable_record, duplicate, next_id, record_change, refuse_duplicate, row_json,
    seen_columns, RawSeen, RecordKind, RecordStatus, Seen,
};
use super::validate::{check_line, check_text, MAX_RECORD_LABEL, MAX_RECORD_NAME};
use super::{has_records, require_records};
use crate::actor::Actor;
use crate::annot::{record, record_all, write_rows, Change};
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::listing::{check_limit, fetch_limit, Listing};
use crate::query::check_offset;
use crate::time::now_iso;
use duckdb::types::Value as SqlValue;
use duckdb::{params, Connection, OptionalExt};
use std::collections::HashSet;

/// A host record, as the Hosts sheet shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HostRecord {
    pub id: i64,
    /// The host's name, as the data names it.
    pub name: String,
    /// Empty when not set.
    pub role: String,
    /// Empty when there is none.
    pub note: String,
    pub status: RecordStatus,
    pub created_by: String,
    pub created_at: String,
    pub modified_by: String,
    pub modified_at: String,
    /// An agent created it ("added by agent: <name>", from `created_by`);
    /// still true once an analyst has reviewed it.
    pub agent_added: bool,
    /// Deleted (a tombstone); only `host_record` returns one.
    pub deleted: bool,
    /// Live entries linked to it.
    pub entries: u64,
    /// The earliest time among them, undated entries left out; `None` with
    /// no dated entry.
    pub first_seen: Option<String>,
    /// The latest time among them.
    pub last_seen: Option<String>,
}

/// What `upsert_host` sets: a new record with `id: None`, else every field
/// of record `id`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostSpec {
    pub id: Option<i64>,
    /// 1 to `MAX_RECORD_NAME` characters, not blank, no control character;
    /// unique among live hosts, ASCII case ignored.
    pub name: String,
    /// At most `MAX_RECORD_LABEL` characters, no control character.
    pub role: String,
    /// At most `MAX_RECORD_NOTE` characters.
    pub note: String,
}

/// The id of the live host record named `name`, ASCII case ignored (the
/// lowest id should there be more than one); `None` when there is none.
pub(crate) fn find_host(conn: &Connection, name: &str) -> Result<Option<i64>> {
    duplicate(
        conn,
        "SELECT id, name FROM ox.host WHERE NOT deleted AND lower(name) = lower(?)",
        &[SqlValue::Text(name.to_string())],
        |r| Ok((r.get(0)?, r.get::<_, String>(1)?)),
        |other| other.eq_ignore_ascii_case(name),
    )
}

/// The first live entry linked to host record `host_id` that holds evidence
/// from a host named `name` (ASCII case ignored); `None` when none does.
/// While one does, the record keeps that name (ruling S7): renaming it, or
/// merging it into a host of another name, is refused.
pub(crate) fn evidence_holder(conn: &Connection, host_id: i64, name: &str) -> Result<Option<i64>> {
    // `translate` folds ASCII letters only, as `eq_ignore_ascii_case` does;
    // `lower()` would fold more.
    Ok(conn
        .query_row(
            "SELECT l.entry_id FROM ox.entry_host l \
             JOIN ox.entry e ON e.id = l.entry_id \
             JOIN ox.evidence v ON v.entry_id = e.id \
             WHERE l.record_id = ? AND NOT e.deleted \
             AND translate(v.host, 'ABCDEFGHIJKLMNOPQRSTUVWXYZ', \
             'abcdefghijklmnopqrstuvwxyz') = ? \
             ORDER BY l.entry_id LIMIT 1",
            params![host_id, name.to_ascii_lowercase()],
            |r| r.get(0),
        )
        .optional()?)
}

/// `Refused`: host record `host_id`, named `name`, keeps that name while
/// entry `entry_id` holds evidence from it; `doing` says what was refused.
pub(crate) fn refuse_evidence_name(
    entry_id: i64,
    host_id: i64,
    name: &str,
    doing: &str,
) -> EngineError {
    EngineError::Refused(format!(
        "E-{entry_id} holds evidence from {}; {} keeps that name while it does, so it cannot \
         be {doing}; detach the evidence first",
        super::clip(name, 64),
        RecordKind::Host.shown(host_id)
    ))
}

/// The id of the live host record named `name` (`find_host`), creating it
/// when the case has none: `reviewed` for an analyst, `unreviewed` for an
/// agent, with its history row. Runs in the caller's transaction, stamped
/// `at`.
pub(crate) fn ensure_host(conn: &Connection, name: &str, actor: &Actor, at: &str) -> Result<i64> {
    if let Some(id) = find_host(conn, name)? {
        return Ok(id);
    }
    insert_host(conn, name, "", "", actor, at)
}

/// A new host record of `actor`'s, with its history row.
fn insert_host(
    conn: &Connection,
    name: &str,
    role: &str,
    note: &str,
    actor: &Actor,
    at: &str,
) -> Result<i64> {
    let kind = RecordKind::Host;
    let id = next_id(conn, kind)?;
    let label = actor.label();
    conn.execute(
        "INSERT INTO ox.host \
         (id, name, role, note, status, created_by, created_at, modified_by, modified_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            id,
            name,
            role,
            note,
            RecordStatus::initial(actor).as_str(),
            label,
            at,
            label,
            at
        ],
    )?;
    let after = row_json(conn, kind, id)?;
    let action = kind.action("create");
    record(
        conn,
        actor,
        at,
        record_change(kind, id, &action, None, Some(after)),
    )?;
    Ok(id)
}

/// Make or change a host record, as `spec` says: a new one (`reviewed` for
/// an analyst, `unreviewed` for an agent) when `spec.id` is `None`, else
/// every field of record `spec.id`. An edit that changes nothing writes
/// nothing. `Invalid` for a field that does not hold; `Refused` for a name
/// another live host has (ASCII case ignored), a deleted record, or one the
/// actor may not change (S3); `NotFound` for an unknown id.
pub fn upsert_host(case: &mut Case, spec: &HostSpec, actor: &Actor) -> Result<HostRecord> {
    require_records(case)?;
    check_line("host name", &spec.name, MAX_RECORD_NAME, true)?;
    check_line("host role", &spec.role, MAX_RECORD_LABEL, false)?;
    check_text("note", &spec.note)?;
    let kind = RecordKind::Host;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    if let Some(id) = spec.id {
        changeable_record(&tx, kind, id, actor)?;
    }
    if let Some(existing) = find_host(&tx, &spec.name)?.filter(|&e| Some(e) != spec.id) {
        return Err(refuse_duplicate(kind, existing, "names that host"));
    }
    let id = match spec.id {
        None => insert_host(&tx, &spec.name, &spec.role, &spec.note, actor, &at)?,
        Some(id) => {
            let current = read_host(&tx, id)?;
            if (&current.name, &current.role, &current.note) == (&spec.name, &spec.role, &spec.note)
            {
                return Ok(current);
            }
            if !current.name.eq_ignore_ascii_case(&spec.name) {
                if let Some(entry) = evidence_holder(&tx, id, &current.name)? {
                    return Err(refuse_evidence_name(entry, id, &current.name, "renamed"));
                }
            }
            let before = row_json(&tx, kind, id)?;
            tx.execute(
                "UPDATE ox.host SET name = ?, role = ?, note = ?, modified_by = ?, \
                 modified_at = ? WHERE id = ?",
                params![spec.name, spec.role, spec.note, actor.label(), at, id],
            )?;
            let after = row_json(&tx, kind, id)?;
            let action = kind.action("edit");
            record(
                &tx,
                actor,
                &at,
                record_change(kind, id, &action, Some(before), Some(after)),
            )?;
            id
        }
    };
    let out = read_host(&tx, id)?;
    tx.commit()?;
    Ok(out)
}

/// What every read of a host selects, in `raw_host`'s order.
fn host_select() -> String {
    format!(
        "SELECT r.id, r.name, r.role, r.note, r.status, r.created_by, r.created_at, \
         r.modified_by, r.modified_at, r.deleted, {} FROM ox.host r",
        seen_columns(RecordKind::Host)
    )
}

/// A host as `host_select` reads it, before decoding.
type RawHost = (
    (i64, String, String, String, String),
    (String, String, String, String, bool),
    RawSeen,
);

fn raw_host(r: &duckdb::Row<'_>) -> duckdb::Result<RawHost> {
    Ok((
        (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?),
        (r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?),
        Seen::raw(r, 10)?,
    ))
}

fn decode_host(raw: RawHost) -> Result<HostRecord> {
    let ((id, name, role, note, status), stamps, seen) = raw;
    let (created_by, created_at, modified_by, modified_at, deleted) = stamps;
    let seen = Seen::decode(RecordKind::Host, id, seen)?;
    Ok(HostRecord {
        id,
        name,
        role,
        note,
        status: RecordStatus::stored(&status)?,
        agent_added: agent_added(&created_by),
        created_by,
        created_at,
        modified_by,
        modified_at,
        deleted,
        entries: seen.entries,
        first_seen: seen.first_seen,
        last_seen: seen.last_seen,
    })
}

fn read_host(conn: &Connection, id: i64) -> Result<HostRecord> {
    let raw = conn
        .prepare(&format!("{} WHERE r.id = ?", host_select()))?
        .query_map(params![id], raw_host)?
        .next()
        .transpose()?
        .ok_or_else(|| EngineError::NotFound(format!("no {}", RecordKind::Host.shown(id))))?;
    decode_host(raw)
}

/// Host record `id`, deleted or not. `NotFound` for an unknown id, and in a
/// case without the record tables.
pub fn host_record(case: &Case, id: i64) -> Result<HostRecord> {
    if !has_records(case) {
        return Err(EngineError::NotFound(format!(
            "no {}",
            RecordKind::Host.shown(id)
        )));
    }
    read_host(case.conn(), id)
}

/// Up to `limit` live host records from `offset`, by name (case ignored),
/// then id. A case without the record tables has none. `Invalid` for a
/// `limit` of 0 or over `MAX_LIMIT`, or an `offset` past `i64::MAX`.
pub fn hosts(case: &Case, limit: u32, offset: u64) -> Result<Listing<HostRecord>> {
    check_limit(limit)?;
    check_offset(offset)?;
    if !has_records(case) {
        return Ok(Listing {
            items: Vec::new(),
            truncated: false,
        });
    }
    let raw: Vec<RawHost> = case
        .conn()
        .prepare(&format!(
            "{} WHERE NOT r.deleted ORDER BY lower(r.name), r.id LIMIT ? OFFSET ?",
            host_select()
        ))?
        .query_map(params![fetch_limit(limit), offset], raw_host)?
        .collect::<std::result::Result<_, _>>()?;
    let items = raw
        .into_iter()
        .map(decode_host)
        .collect::<Result<Vec<_>>>()?;
    Ok(Listing::from_fetched(items, limit))
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
