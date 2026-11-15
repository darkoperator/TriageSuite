//! What host, account and IOC records share: their kind and status, who
//! may change one (ruling S3), the columns computed from their linked
//! entries, and the changes that work the same for every kind: delete,
//! merge, status, link and unlink.
//!
//! A record's history rows carry the record as stored (`row_json`), before
//! and after; a link's carry the link, as `entry.rs` writes them.

use super::entry::{changeable, list_entries};
use super::hosts::link_hosts;
use super::{clip, has_records, require_records, Entry};
use crate::actor::Actor;
use crate::annot::{record, record_all, Change};
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::listing::{check_limit, Listing};
use crate::query::check_offset;
use crate::time::now_iso;
use duckdb::types::Value as SqlValue;
use duckdb::{params, Connection, OptionalExt};

/// Which kind of record: the three sheets beside the Timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Host,
    Account,
    Ioc,
}

impl RecordKind {
    pub const ALL: [RecordKind; 3] = [RecordKind::Host, RecordKind::Account, RecordKind::Ioc];

    /// `host`, `account` or `ioc`: the history target kind, and the kind a
    /// link's history row names.
    pub fn as_str(&self) -> &'static str {
        match self {
            RecordKind::Host => "host",
            RecordKind::Account => "account",
            RecordKind::Ioc => "ioc",
        }
    }

    /// The kind `as_str` spells `text`; `Invalid` otherwise.
    pub fn parse(text: &str) -> Result<RecordKind> {
        RecordKind::ALL
            .into_iter()
            .find(|k| k.as_str() == text)
            .ok_or_else(|| {
                EngineError::Invalid(format!(
                    "{:?} is not a record kind; one of host, account, ioc",
                    clip(text, 64)
                ))
            })
    }

    /// The letter its ids are shown with: `H-3`, `A-7`, `I-40` (ruling S1).
    pub fn prefix(&self) -> &'static str {
        match self {
            RecordKind::Host => "H",
            RecordKind::Account => "A",
            RecordKind::Ioc => "I",
        }
    }

    /// Record `id` as it is shown.
    pub(super) fn shown(&self, id: i64) -> String {
        format!("{}-{id}", self.prefix())
    }

    pub(super) fn table(&self) -> &'static str {
        match self {
            RecordKind::Host => "ox.host",
            RecordKind::Account => "ox.account",
            RecordKind::Ioc => "ox.ioc",
        }
    }

    pub(super) fn link_table(&self) -> &'static str {
        match self {
            RecordKind::Host => "ox.entry_host",
            RecordKind::Account => "ox.entry_account",
            RecordKind::Ioc => "ox.entry_ioc",
        }
    }

    pub(super) fn sequence(&self) -> &'static str {
        match self {
            RecordKind::Host => "ox.host_seq",
            RecordKind::Account => "ox.account_seq",
            RecordKind::Ioc => "ox.ioc_seq",
        }
    }

    /// The history action `<kind>_<verb>`: `host_create`, `ioc_merge`.
    pub(super) fn action(&self, verb: &str) -> String {
        format!("{}_{verb}", self.as_str())
    }
}

/// A record's review state. An analyst's records are `reviewed` from the
/// start; an agent's are `unreviewed` until an analyst reviews them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    Unreviewed,
    Reviewed,
}

impl RecordStatus {
    /// The spelling stored in the record's `status`.
    pub fn as_str(&self) -> &'static str {
        match self {
            RecordStatus::Unreviewed => "unreviewed",
            RecordStatus::Reviewed => "reviewed",
        }
    }

    /// The status `as_str` spells `text`; `Invalid` otherwise.
    pub fn parse(text: &str) -> Result<RecordStatus> {
        match text {
            "unreviewed" => Ok(RecordStatus::Unreviewed),
            "reviewed" => Ok(RecordStatus::Reviewed),
            other => Err(EngineError::Invalid(format!(
                "{:?} is not a record status; unreviewed or reviewed",
                clip(other, 64)
            ))),
        }
    }

    /// What a new record of `actor`'s starts as.
    pub(super) fn initial(actor: &Actor) -> RecordStatus {
        if actor.is_agent() {
            RecordStatus::Unreviewed
        } else {
            RecordStatus::Reviewed
        }
    }

    /// The status a stored record holds; `Internal` for one the engine
    /// never writes.
    pub(super) fn stored(text: &str) -> Result<RecordStatus> {
        RecordStatus::parse(text)
            .map_err(|_| EngineError::Internal(format!("a record holds the status {text:?}")))
    }
}

/// Whether the actor `created_by` names is an agent: the "added by agent"
/// label, which review does not remove.
pub(super) fn agent_added(created_by: &str) -> bool {
    Actor::parse(created_by).is_ok_and(|a| a.is_agent())
}

/// What every change to a record checks first.
pub(super) struct Head {
    pub status: RecordStatus,
    pub created_by: String,
    pub deleted: bool,
}

/// Record `id` of `kind`'s head; `NotFound` when there is none.
pub(super) fn read_head(conn: &Connection, kind: RecordKind, id: i64) -> Result<Head> {
    let row: Option<(String, String, bool)> = conn
        .query_row(
            &format!(
                "SELECT status, created_by, deleted FROM {} WHERE id = ?",
                kind.table()
            ),
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (status, created_by, deleted) =
        row.ok_or_else(|| EngineError::NotFound(format!("no {}", kind.shown(id))))?;
    Ok(Head {
        status: RecordStatus::stored(&status)?,
        created_by,
        deleted,
    })
}

/// Record `id` of `kind`, which `actor` means to change: `Refused` once it
/// is deleted, and for an agent unless it is the agent's own and still
/// unreviewed (ruling S3: its changes to anything else arrive as proposals
/// in step 5).
pub(super) fn changeable_record(
    conn: &Connection,
    kind: RecordKind,
    id: i64,
    actor: &Actor,
) -> Result<Head> {
    let head = read_head(conn, kind, id)?;
    let shown = kind.shown(id);
    if head.deleted {
        return Err(EngineError::Refused(format!("{shown} was deleted")));
    }
    if actor.is_agent()
        && (head.created_by != actor.label() || head.status != RecordStatus::Unreviewed)
    {
        let reviewed = if head.status == RecordStatus::Reviewed {
            ", reviewed"
        } else {
            ""
        };
        return Err(EngineError::Refused(format!(
            "{shown} is {}'s{reviewed}; an agent changes only its own unreviewed records, \
             and its changes to others arrive as proposals with the Review tab",
            head.created_by
        )));
    }
    Ok(head)
}

/// Record `id` of `kind` as stored, every column, as its history rows
/// carry it.
pub(super) fn row_json(conn: &Connection, kind: RecordKind, id: i64) -> Result<serde_json::Value> {
    let text: String = conn.query_row(
        &format!(
            "SELECT CAST(to_json(r) AS VARCHAR) FROM {} r WHERE r.id = ?",
            kind.table()
        ),
        params![id],
        |r| r.get(0),
    )?;
    serde_json::from_str(&text).map_err(|e| {
        EngineError::Internal(format!(
            "{} does not read back as JSON: {e}",
            kind.shown(id)
        ))
    })
}

/// The history of a change to record `id` of `kind`.
pub(super) fn record_change(
    kind: RecordKind,
    id: i64,
    action: &str,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
) -> Change<'_> {
    Change {
        target_kind: kind.as_str(),
        target_id: id.to_string(),
        action,
        before,
        after,
    }
}

/// The history id of the link from entry `entry_id` to record `record_id`.
fn link_id(kind: RecordKind, entry_id: i64, record_id: i64) -> String {
    format!("entry_{}:{entry_id}:{record_id}", kind.as_str())
}

/// The history of a link appearing (`after`) or going (`before`).
fn link_change(
    kind: RecordKind,
    entry_id: i64,
    record_id: i64,
    (linked_by, linked_at): (&str, &str),
    added: bool,
) -> Change<'static> {
    let link = serde_json::json!({
        "kind": kind.as_str(), "entry_id": entry_id, "record_id": record_id,
        "linked_by": linked_by, "linked_at": linked_at,
    });
    let (action, before, after) = if added {
        ("link_add", None, Some(link))
    } else {
        ("link_remove", Some(link), None)
    };
    Change {
        target_kind: "link",
        target_id: link_id(kind, entry_id, record_id),
        action,
        before,
        after,
    }
}

/// Record `record_id`'s links: `(entry_id, linked_by, linked_at)`, by entry.
fn links_of(
    conn: &Connection,
    kind: RecordKind,
    record_id: i64,
) -> Result<Vec<(i64, String, String)>> {
    Ok(conn
        .prepare(&format!(
            "SELECT entry_id, linked_by, linked_at FROM {} WHERE record_id = ? \
             ORDER BY entry_id",
            kind.link_table()
        ))?
        .query_map(params![record_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<std::result::Result<_, _>>()?)
}

/// Why a record is tombstoned, which names its history row.
enum Tombstone {
    /// `<kind>_delete`.
    Delete,
    /// `<kind>_merge`, its after naming the record it went into as
    /// `merged_into`.
    MergedInto(i64),
}

/// Tombstone record `id` of `kind`, whose head `changeable_record` gave:
/// flagged deleted, its links removed, each with its history row, then the
/// record's own row, as `why` says.
fn tombstone(
    conn: &Connection,
    kind: RecordKind,
    id: i64,
    why: Tombstone,
    actor: &Actor,
    at: &str,
) -> Result<()> {
    let before = row_json(conn, kind, id)?;
    let label = actor.label();
    conn.execute(
        &format!(
            "UPDATE {} SET deleted = true, modified_by = ?, modified_at = ? WHERE id = ?",
            kind.table()
        ),
        params![label, at, id],
    )?;
    let mut after = row_json(conn, kind, id)?;
    let verb = match why {
        Tombstone::Delete => "delete",
        Tombstone::MergedInto(into) => {
            if let Some(object) = after.as_object_mut() {
                object.insert("merged_into".to_string(), serde_json::Value::from(into));
            }
            "merge"
        }
    };
    let mut history: Vec<Change<'_>> = Vec::new();
    for (entry_id, linked_by, linked_at) in links_of(conn, kind, id)? {
        history.push(link_change(
            kind,
            entry_id,
            id,
            (&linked_by, &linked_at),
            false,
        ));
    }
    conn.execute(
        &format!("DELETE FROM {} WHERE record_id = ?", kind.link_table()),
        params![id],
    )?;
    let action = kind.action(verb);
    history.push(record_change(kind, id, &action, Some(before), Some(after)));
    record_all(conn, actor, at, history)
}

/// Delete record `id` of `kind`: a tombstone. The record is kept and
/// flagged; its links to entries are removed, each with its history row.
/// A deleted record's name is free for a new one. `Refused` for a record
/// already deleted or one the actor may not change (S3); `NotFound` for an
/// unknown id.
pub fn delete_record(case: &mut Case, kind: RecordKind, id: i64, actor: &Actor) -> Result<()> {
    require_records(case)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    changeable_record(&tx, kind, id, actor)?;
    tombstone(&tx, kind, id, Tombstone::Delete, actor, &at)?;
    tx.commit()?;
    Ok(())
}

/// Merge record `from` into record `into`, both of `kind`: every entry
/// linked to `from` ends linked to `into` once (a link `into` already has
/// is kept, `from`'s dropped; any other is repointed, keeping who linked it
/// and when), and `from` is tombstoned with `merged_into` in its history.
/// `into`'s fields are left as they are. `Invalid` for a record merged
/// into itself; `Refused` for a deleted record or one the actor may not
/// change (S3: an agent merges only its own unreviewed records, either
/// side); `NotFound` for an unknown id.
pub fn merge_records(
    case: &mut Case,
    kind: RecordKind,
    from: i64,
    into: i64,
    actor: &Actor,
) -> Result<()> {
    require_records(case)?;
    if from == into {
        return Err(EngineError::Invalid(format!(
            "{} cannot be merged into itself",
            kind.shown(from)
        )));
    }
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    changeable_record(&tx, kind, from, actor)?;
    changeable_record(&tx, kind, into, actor)?;
    let kept: std::collections::HashSet<i64> = links_of(&tx, kind, into)?
        .into_iter()
        .map(|(entry, _, _)| entry)
        .collect();
    let mut history: Vec<Change<'_>> = Vec::new();
    for (entry_id, linked_by, linked_at) in links_of(&tx, kind, from)? {
        let by_at = (linked_by.as_str(), linked_at.as_str());
        history.push(link_change(kind, entry_id, from, by_at, false));
        if kept.contains(&entry_id) {
            tx.execute(
                &format!(
                    "DELETE FROM {} WHERE entry_id = ? AND record_id = ?",
                    kind.link_table()
                ),
                params![entry_id, from],
            )?;
        } else {
            tx.execute(
                &format!(
                    "UPDATE {} SET record_id = ? WHERE entry_id = ? AND record_id = ?",
                    kind.link_table()
                ),
                params![into, entry_id, from],
            )?;
            history.push(link_change(kind, entry_id, into, by_at, true));
        }
    }
    record_all(&tx, actor, &at, history)?;
    tombstone(&tx, kind, from, Tombstone::MergedInto(into), actor, &at)?;
    tx.commit()?;
    Ok(())
}

/// Set record `id`'s status. Only an analyst does: an agent is `Refused`,
/// naming the owner when the record is not its own (S3). Setting the
/// status it has writes nothing. `Refused` for a deleted record;
/// `NotFound` for an unknown id.
pub fn set_record_status(
    case: &mut Case,
    kind: RecordKind,
    id: i64,
    status: RecordStatus,
    actor: &Actor,
) -> Result<()> {
    require_records(case)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let head = changeable_record(&tx, kind, id, actor)?;
    if actor.is_agent() {
        return Err(EngineError::Refused(format!(
            "only an analyst sets a record's status: {} stays unreviewed until an analyst \
             reviews it",
            kind.shown(id)
        )));
    }
    if head.status == status {
        return Ok(());
    }
    let before = row_json(&tx, kind, id)?;
    tx.execute(
        &format!(
            "UPDATE {} SET status = ?, modified_by = ?, modified_at = ? WHERE id = ?",
            kind.table()
        ),
        params![status.as_str(), actor.label(), at, id],
    )?;
    let after = row_json(&tx, kind, id)?;
    let action = kind.action("status");
    record(
        &tx,
        actor,
        &at,
        record_change(kind, id, &action, Some(before), Some(after)),
    )?;
    tx.commit()?;
    Ok(())
}

/// Link entry `entry_id` to record `record_id` of `kind`, with its history
/// row; a link already there writes nothing. The entry must be one the
/// actor may change (an agent links only from its own unreviewed entries,
/// S3); the record may be anyone's, but not deleted (`Refused`). `NotFound`
/// for an unknown entry or record.
pub fn link(
    case: &mut Case,
    entry_id: i64,
    kind: RecordKind,
    record_id: i64,
    actor: &Actor,
) -> Result<()> {
    require_records(case)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    changeable(&tx, entry_id, actor)?;
    if read_head(&tx, kind, record_id)?.deleted {
        return Err(EngineError::Refused(format!(
            "{} was deleted",
            kind.shown(record_id)
        )));
    }
    if kind == RecordKind::Host {
        // The same writer attaching evidence uses (S7).
        link_hosts(&tx, &[(entry_id, record_id)], actor, &at)?;
    } else if link_of(&tx, kind, entry_id, record_id)?.is_none() {
        let label = actor.label();
        tx.execute(
            &format!(
                "INSERT INTO {} (entry_id, record_id, linked_by, linked_at) VALUES (?, ?, ?, ?)",
                kind.link_table()
            ),
            params![entry_id, record_id, label, at],
        )?;
        record(
            &tx,
            actor,
            &at,
            link_change(kind, entry_id, record_id, (&label, &at), true),
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Who linked entry `entry_id` to record `record_id`, and when; `None`
/// when they are not linked.
fn link_of(
    conn: &Connection,
    kind: RecordKind,
    entry_id: i64,
    record_id: i64,
) -> Result<Option<(String, String)>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT linked_by, linked_at FROM {} WHERE entry_id = ? AND record_id = ? \
                 LIMIT 1",
                kind.link_table()
            ),
            params![entry_id, record_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Remove the link from entry `entry_id` to record `record_id` of `kind`,
/// with its history row. The entry must be one the actor may change (S3).
/// A host is `Refused` while the entry holds evidence from it (ruling S7:
/// the evidence keeps it linked). `NotFound` for an unknown entry, or a
/// record the entry is not linked to.
pub fn unlink(
    case: &mut Case,
    entry_id: i64,
    kind: RecordKind,
    record_id: i64,
    actor: &Actor,
) -> Result<()> {
    require_records(case)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    changeable(&tx, entry_id, actor)?;
    let Some((linked_by, linked_at)) = link_of(&tx, kind, entry_id, record_id)? else {
        return Err(EngineError::NotFound(format!(
            "E-{entry_id} is not linked to {}",
            kind.shown(record_id)
        )));
    };
    if kind == RecordKind::Host {
        let name: String = tx.query_row(
            "SELECT name FROM ox.host WHERE id = ?",
            params![record_id],
            |r| r.get(0),
        )?;
        let hosts: Vec<String> = tx
            .prepare(
                "SELECT DISTINCT host FROM ox.evidence WHERE entry_id = ? AND host IS NOT NULL",
            )?
            .query_map(params![entry_id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        if hosts.iter().any(|h| h.eq_ignore_ascii_case(&name)) {
            return Err(EngineError::Refused(format!(
                "E-{entry_id} holds evidence from {}; detach it before unlinking {}",
                clip(&name, 64),
                kind.shown(record_id)
            )));
        }
    }
    tx.execute(
        &format!(
            "DELETE FROM {} WHERE entry_id = ? AND record_id = ?",
            kind.link_table()
        ),
        params![entry_id, record_id],
    )?;
    record(
        &tx,
        actor,
        &at,
        link_change(kind, entry_id, record_id, (&linked_by, &linked_at), false),
    )?;
    tx.commit()?;
    Ok(())
}

/// Up to `limit` live entries from `offset` linked to record `id` of
/// `kind`, in the Timeline's order. A case without the record tables has
/// none. `NotFound` for an unknown record; `Invalid` for a `limit` of 0 or
/// over `MAX_LIMIT`, or an `offset` past `i64::MAX`.
pub fn record_entries(
    case: &Case,
    kind: RecordKind,
    id: i64,
    limit: u32,
    offset: u64,
) -> Result<Listing<Entry>> {
    check_limit(limit)?;
    check_offset(offset)?;
    if !has_records(case) {
        return Ok(Listing {
            items: Vec::new(),
            truncated: false,
        });
    }
    read_head(case.conn(), kind, id)?;
    list_entries(
        case,
        vec![format!(
            "EXISTS (SELECT 1 FROM {} l WHERE l.entry_id = e.id AND l.record_id = ?)",
            kind.link_table()
        )],
        vec![SqlValue::BigInt(id)],
        limit,
        offset,
    )
}

/// What every record's computed columns are: its live linked entries, the
/// earliest and latest of their times (undated entries left out), and the
/// names of the live host records those entries link, as a JSON list.
/// Four select expressions over a record aliased `r`, read by `Seen::read`.
pub(super) fn seen_columns(kind: RecordKind) -> String {
    let linked = format!(
        "FROM {} l JOIN ox.entry e ON e.id = l.entry_id WHERE l.record_id = r.id AND NOT e.deleted",
        kind.link_table()
    );
    format!(
        "(SELECT count(*) {linked}), (SELECT min(e.\"time\") {linked}), \
         (SELECT max(e.\"time\") {linked}), \
         (SELECT CAST(to_json(list(DISTINCT h.name ORDER BY h.name)) AS VARCHAR) \
          FROM {} l JOIN ox.entry e ON e.id = l.entry_id \
          JOIN ox.entry_host eh ON eh.entry_id = e.id JOIN ox.host h ON h.id = eh.record_id \
          WHERE l.record_id = r.id AND NOT e.deleted AND NOT h.deleted)",
        kind.link_table()
    )
}

/// A record's computed columns, as `seen_columns` selects them.
pub(super) struct Seen {
    pub entries: u64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub hosts_seen: Vec<String>,
}

/// `seen_columns`' four values as a row holds them, before decoding.
pub(super) type RawSeen = (i64, Option<String>, Option<String>, Option<String>);

impl Seen {
    /// The four columns from `at` on.
    pub(super) fn raw(r: &duckdb::Row<'_>, at: usize) -> duckdb::Result<RawSeen> {
        Ok((r.get(at)?, r.get(at + 1)?, r.get(at + 2)?, r.get(at + 3)?))
    }

    pub(super) fn decode(kind: RecordKind, id: i64, raw: RawSeen) -> Result<Seen> {
        let (entries, first_seen, last_seen, hosts) = raw;
        let hosts_seen = match hosts {
            Some(json) => serde_json::from_str(&json).map_err(|e| {
                EngineError::Internal(format!("{}'s host names do not parse: {e}", kind.shown(id)))
            })?,
            None => Vec::new(),
        };
        Ok(Seen {
            entries: u64::try_from(entries).unwrap_or_default(),
            first_seen,
            last_seen,
            hosts_seen,
        })
    }
}

/// The id of the live record of `kind` that `same` says is a duplicate,
/// among the live records `sql` selects as `(id, key…)` with `params`; the
/// lowest such id. `sql` narrows with SQL's `lower()`, which folds more
/// than ASCII; `same` decides, ignoring ASCII case only.
pub(super) fn duplicate<T>(
    conn: &Connection,
    sql: &str,
    params: &[SqlValue],
    read: impl Fn(&duckdb::Row<'_>) -> duckdb::Result<(i64, T)>,
    same: impl Fn(&T) -> bool,
) -> Result<Option<i64>> {
    let rows: Vec<(i64, T)> = conn
        .prepare(sql)?
        .query_map(duckdb::params_from_iter(params), |r| read(r))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows
        .into_iter()
        .filter(|(_, key)| same(key))
        .map(|(id, _)| id)
        .min())
}

/// `Refused` naming the live record `existing` of `kind` that already has
/// what `what` describes.
pub(super) fn refuse_duplicate(kind: RecordKind, existing: i64, what: &str) -> EngineError {
    EngineError::Refused(format!(
        "{} already {what}; edit or merge that one",
        kind.shown(existing)
    ))
}

/// A new id for a record of `kind`.
pub(super) fn next_id(conn: &Connection, kind: RecordKind) -> Result<i64> {
    Ok(
        conn.query_row(&format!("SELECT nextval('{}')", kind.sequence()), [], |r| {
            r.get(0)
        })?,
    )
}

/// `Some(text)`, or `None` for an empty one: how an optional field the
/// analyst left blank is stored.
pub(super) fn non_empty(text: &Option<String>) -> Option<String> {
    text.as_ref().filter(|t| !t.is_empty()).cloned()
}
