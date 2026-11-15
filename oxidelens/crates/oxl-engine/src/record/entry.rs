//! Timeline entries: made by hand or by M, edited, reviewed, deleted, and
//! listed in the Timeline's order.
//!
//! An entry's time is UTC text or nothing (ruling S8). One with none
//! "needs a time": it sorts after every dated entry and cannot be reviewed
//! until it has one. Only an analyst sets a status; an agent's entries are
//! `unreviewed` from the start and stay so until an analyst reviews them.

use super::hosts::host_link_id;
use super::{check_tactic, clip, has_records, is_technique_id, require_records};
use crate::actor::Actor;
use crate::annot::{record, record_all, Change};
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::listing::{check_limit, fetch_limit, Listing};
use crate::query::check_offset;
use crate::time::{now_iso, parse_utc_text};
use duckdb::types::Value as SqlValue;
use duckdb::{params, params_from_iter, Connection};

/// The most characters an entry's title holds. A title M takes from a row's
/// summary is cut to it.
pub const MAX_TITLE: usize = 200;
/// The most characters an entry's description holds.
pub const MAX_DESCRIPTION: usize = 10_000;
/// The most technique ids one entry carries.
pub const MAX_TECHNIQUES: usize = 64;
/// The most characters of a Timeline filter's text or host.
const MAX_FILTER: usize = 1_000;

/// An entry's review state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    /// An analyst's, not yet reviewed.
    Draft,
    /// An agent's (everything an agent makes starts here), not yet reviewed.
    Unreviewed,
    /// Reviewed by an analyst.
    Reviewed,
}

impl EntryStatus {
    /// The spelling stored in `ox.entry.status`.
    pub fn as_str(&self) -> &'static str {
        match self {
            EntryStatus::Draft => "draft",
            EntryStatus::Unreviewed => "unreviewed",
            EntryStatus::Reviewed => "reviewed",
        }
    }

    fn parse(text: &str) -> Result<EntryStatus> {
        match text {
            "draft" => Ok(EntryStatus::Draft),
            "unreviewed" => Ok(EntryStatus::Unreviewed),
            "reviewed" => Ok(EntryStatus::Reviewed),
            other => Err(EngineError::Internal(format!(
                "an entry holds the unknown status {other:?}"
            ))),
        }
    }

    /// What a new entry of `actor`'s starts as.
    pub(super) fn initial(actor: &Actor) -> EntryStatus {
        if actor.is_agent() {
            EntryStatus::Unreviewed
        } else {
            EntryStatus::Draft
        }
    }
}

/// One timeline entry, as the Timeline shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Entry {
    pub id: i64,
    /// When it happened, UTC text (`YYYY-MM-DDTHH:MM:SS.ffffffZ`); `None`
    /// until it is known.
    pub time: Option<String>,
    /// `row` (an evidence row's primary time) or `typed` (entered by hand);
    /// `None` with no time.
    pub time_source: Option<String>,
    /// No time yet: badged "needs a time", in the undated group at the end,
    /// and not reviewable until it has one.
    pub needs_time: bool,
    pub title: String,
    /// Empty when there is none.
    pub description: String,
    pub tactic: Option<String>,
    pub technique_ids: Vec<String>,
    pub status: EntryStatus,
    pub created_by: String,
    pub created_at: String,
    pub modified_by: String,
    pub modified_at: String,
    pub reviewed_by: Option<String>,
    /// An agent created it ("added by agent: <name>", from `created_by`);
    /// still true once an analyst has reviewed it.
    pub agent_added: bool,
    /// Rows attached to it as evidence.
    pub evidence_rows: u64,
    /// The names of the live host records it links, sorted.
    pub hosts: Vec<String>,
}

/// The fields a new entry or an edit sets; `None` leaves a field as it is
/// (a new entry's default: no description, time, tactic or technique).
/// `time` and `tactic` take `Some(None)` to clear them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryEdit {
    pub title: Option<String>,
    pub description: Option<String>,
    /// UTC text (`YYYY-MM-DDTHH:MM:SS[.f]Z`), typed by hand.
    pub time: Option<Option<String>>,
    /// One of `TACTICS`.
    pub tactic: Option<Option<String>>,
    /// Each `T1234` or `T1234.001`; a repeated id is kept once.
    pub technique_ids: Option<Vec<String>>,
}

/// Which entries the Timeline lists. Every field narrows; the default
/// lists every live entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntryQuery {
    /// Any of these statuses; empty for any status.
    pub status: Vec<EntryStatus>,
    /// Linked to the live host record of this name (ASCII case ignored).
    pub host: Option<String>,
    /// This tactic, one of `TACTICS`.
    pub tactic: Option<String>,
    /// In the title or description, case ignored.
    pub text: Option<String>,
}

/// An entry as stored, deleted or not: what history records and what a
/// change starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Stored {
    pub id: i64,
    pub time: Option<String>,
    pub time_source: Option<String>,
    pub title: String,
    pub description: String,
    pub tactic: Option<String>,
    pub technique_ids: Vec<String>,
    pub status: EntryStatus,
    pub created_by: String,
    pub created_at: String,
    pub modified_by: String,
    pub modified_at: String,
    pub reviewed_by: Option<String>,
    pub reviewed_at: Option<String>,
    pub deleted: bool,
}

impl Stored {
    /// A new entry of `actor`'s, made `at`, with nothing set but its title.
    pub(super) fn new(id: i64, title: String, actor: &Actor, at: &str) -> Stored {
        Stored {
            id,
            time: None,
            time_source: None,
            title,
            description: String::new(),
            tactic: None,
            technique_ids: Vec::new(),
            status: EntryStatus::initial(actor),
            created_by: actor.label(),
            created_at: at.to_string(),
            modified_by: actor.label(),
            modified_at: at.to_string(),
            reviewed_by: None,
            reviewed_at: None,
            deleted: false,
        }
    }

    /// The entry as its history rows record it.
    pub(super) fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "time": self.time,
            "time_source": self.time_source,
            "title": self.title,
            "description": self.description,
            "tactic": self.tactic,
            "technique_ids": self.technique_ids,
            "status": self.status.as_str(),
            "created_by": self.created_by,
            "created_at": self.created_at,
            "modified_by": self.modified_by,
            "modified_at": self.modified_at,
            "reviewed_by": self.reviewed_by,
            "reviewed_at": self.reviewed_at,
            "deleted": self.deleted,
        })
    }

    /// One `INSERT_SQL` row.
    pub(super) fn insert_values(&self) -> Vec<SqlValue> {
        let text = |t: &str| SqlValue::Text(t.to_string());
        let maybe = |t: &Option<String>| t.as_deref().map_or(SqlValue::Null, text);
        vec![
            SqlValue::BigInt(self.id),
            maybe(&self.time),
            maybe(&self.time_source),
            text(&self.title),
            text(&self.description),
            maybe(&self.tactic),
            text(&techniques_json(&self.technique_ids)),
            text(self.status.as_str()),
            text(&self.created_by),
            text(&self.created_at),
            text(&self.modified_by),
            text(&self.modified_at),
            maybe(&self.reviewed_by),
            maybe(&self.reviewed_at),
            SqlValue::Boolean(self.deleted),
        ]
    }

    /// The history of creating it.
    pub(super) fn created(&self) -> Change<'static> {
        Change {
            target_kind: "entry",
            target_id: self.id.to_string(),
            action: "entry_create",
            before: None,
            after: Some(self.json()),
        }
    }
}

/// Writes `Stored::insert_values` rows, through `write_rows`.
pub(super) const INSERT_SQL: &str = "INSERT INTO ox.entry \
     (id, \"time\", time_source, title, description, tactic, technique_ids, status, \
      created_by, created_at, modified_by, modified_at, reviewed_by, reviewed_at, deleted) \
     VALUES {values}";
pub(super) const INSERT_ROW: &str = "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

fn techniques_json(ids: &[String]) -> String {
    serde_json::Value::from(ids).to_string()
}

/// `count` new entry ids, rising.
pub(super) fn next_ids(conn: &Connection, count: usize) -> Result<Vec<i64>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let count = i64::try_from(count).unwrap_or(i64::MAX);
    let mut ids: Vec<i64> = conn
        .prepare("SELECT nextval('ox.entry_seq') FROM range(?)")?
        .query_map(params![count], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    ids.sort_unstable();
    Ok(ids)
}

/// What every read of an entry selects, in `read_entry_row`'s order: the
/// stored columns, then its evidence count and its live hosts' names as a
/// JSON list.
const ENTRY_SELECT: &str = "SELECT e.id, e.\"time\", e.time_source, e.title, e.description, \
     e.tactic, e.technique_ids, e.status, e.created_by, e.created_at, e.modified_by, \
     e.modified_at, e.reviewed_by, e.reviewed_at, e.deleted, \
     (SELECT count(*) FROM ox.evidence v WHERE v.entry_id = e.id), \
     (SELECT CAST(to_json(list(DISTINCT h.name ORDER BY h.name)) AS VARCHAR) \
      FROM ox.entry_host l JOIN ox.host h ON h.id = l.record_id \
      WHERE l.entry_id = e.id AND NOT h.deleted) \
     FROM ox.entry e";

/// An entry as `ENTRY_SELECT` reads it, before decoding.
type RawEntry = (
    (
        i64,
        Option<String>,
        Option<String>,
        String,
        String,
        Option<String>,
        String,
        String,
    ),
    (
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        bool,
        i64,
        Option<String>,
    ),
);

fn raw_entry(r: &duckdb::Row<'_>) -> duckdb::Result<RawEntry> {
    Ok((
        (
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
            r.get(7)?,
        ),
        (
            r.get(8)?,
            r.get(9)?,
            r.get(10)?,
            r.get(11)?,
            r.get(12)?,
            r.get(13)?,
            r.get(14)?,
            r.get(15)?,
            r.get(16)?,
        ),
    ))
}

/// The stored entry, its evidence count and its host names.
fn decode(raw: RawEntry) -> Result<(Stored, u64, Vec<String>)> {
    let ((id, time, time_source, title, description, tactic, techniques, status), rest) = raw;
    let (created_by, created_at, modified_by, modified_at, reviewed_by, reviewed_at, deleted) =
        (rest.0, rest.1, rest.2, rest.3, rest.4, rest.5, rest.6);
    let (evidence_rows, hosts) = (rest.7, rest.8);
    // The engine writes both lists itself.
    let technique_ids: Vec<String> = serde_json::from_str(&techniques).map_err(|e| {
        EngineError::Internal(format!(
            "entry {id} holds technique ids that do not parse: {e}"
        ))
    })?;
    let hosts: Vec<String> = match hosts {
        Some(json) => serde_json::from_str(&json).map_err(|e| {
            EngineError::Internal(format!("entry {id}'s host names do not parse: {e}"))
        })?,
        None => Vec::new(),
    };
    let stored = Stored {
        id,
        time,
        time_source,
        title,
        description,
        tactic,
        technique_ids,
        status: EntryStatus::parse(&status)?,
        created_by,
        created_at,
        modified_by,
        modified_at,
        reviewed_by,
        reviewed_at,
        deleted,
    };
    Ok((
        stored,
        u64::try_from(evidence_rows).unwrap_or_default(),
        hosts,
    ))
}

fn shown(stored: Stored, evidence_rows: u64, hosts: Vec<String>) -> Entry {
    Entry {
        id: stored.id,
        needs_time: stored.time.is_none(),
        time: stored.time,
        time_source: stored.time_source,
        title: stored.title,
        description: stored.description,
        tactic: stored.tactic,
        technique_ids: stored.technique_ids,
        status: stored.status,
        agent_added: Actor::parse(&stored.created_by).is_ok_and(|a| a.is_agent()),
        created_by: stored.created_by,
        created_at: stored.created_at,
        modified_by: stored.modified_by,
        modified_at: stored.modified_at,
        reviewed_by: stored.reviewed_by,
        evidence_rows,
        hosts,
    }
}

/// Entry `id` as stored, deleted or not; `NotFound` when there is none.
pub(super) fn read_stored(conn: &Connection, id: i64) -> Result<Stored> {
    Ok(read_full(conn, id)?.0)
}

fn read_full(conn: &Connection, id: i64) -> Result<(Stored, u64, Vec<String>)> {
    let raw = conn
        .prepare(&format!("{ENTRY_SELECT} WHERE e.id = ?"))?
        .query_map(params![id], raw_entry)?
        .next()
        .transpose()?
        .ok_or_else(|| EngineError::NotFound(format!("no entry E-{id}")))?;
    decode(raw)
}

/// Entry `id` as the Timeline shows it.
fn read_entry(conn: &Connection, id: i64) -> Result<Entry> {
    let (stored, evidence_rows, hosts) = read_full(conn, id)?;
    Ok(shown(stored, evidence_rows, hosts))
}

/// Entry `id`, which `actor` means to change: `Refused` once it is deleted,
/// and for an agent unless it is the agent's own and still unreviewed
/// (ruling S3: its changes to anything else arrive as proposals in step 5).
pub(super) fn changeable(conn: &Connection, id: i64, actor: &Actor) -> Result<Stored> {
    let stored = read_stored(conn, id)?;
    if stored.deleted {
        return Err(EngineError::Refused(format!("E-{id} was deleted")));
    }
    if actor.is_agent()
        && (stored.created_by != actor.label() || stored.status != EntryStatus::Unreviewed)
    {
        let owner = match &stored.reviewed_by {
            Some(reviewer) if stored.status == EntryStatus::Reviewed => {
                format!("{}, reviewed by {reviewer}", stored.created_by)
            }
            _ => stored.created_by.clone(),
        };
        return Err(EngineError::Refused(format!(
            "E-{id} is {owner}'s; an agent changes only its own unreviewed entries, and \
             its changes to others arrive as proposals with the Review tab"
        )));
    }
    Ok(stored)
}

/// `title`, checked: not blank, at most `MAX_TITLE` characters.
fn check_title(title: &str) -> Result<()> {
    if title.trim().is_empty() {
        return Err(EngineError::Invalid("an entry needs a title".to_string()));
    }
    let n = title.chars().count();
    if n > MAX_TITLE {
        return Err(EngineError::Invalid(format!(
            "a title is at most {MAX_TITLE} characters, not {n}"
        )));
    }
    Ok(())
}

/// `edit` applied to `stored`, each field checked; `Invalid` for the first
/// that does not hold, and nothing applied.
fn apply(stored: &Stored, edit: &EntryEdit) -> Result<Stored> {
    let mut next = stored.clone();
    if let Some(title) = &edit.title {
        check_title(title)?;
        next.title = title.clone();
    }
    if let Some(description) = &edit.description {
        let n = description.chars().count();
        if n > MAX_DESCRIPTION {
            return Err(EngineError::Invalid(format!(
                "a description is at most {MAX_DESCRIPTION} characters, not {n}"
            )));
        }
        next.description = description.clone();
    }
    match &edit.time {
        None => {}
        Some(None) => {
            next.time = None;
            next.time_source = None;
        }
        Some(Some(text)) => {
            let time = parse_utc_text(text).ok_or_else(|| {
                EngineError::Invalid(format!(
                    "{:?} is not UTC text (YYYY-MM-DDTHH:MM:SS[.f]Z); a time with no \
                     zone is never guessed",
                    clip(text, 64)
                ))
            })?;
            next.time = Some(time);
            next.time_source = Some("typed".to_string());
        }
    }
    match &edit.tactic {
        None => {}
        Some(None) => next.tactic = None,
        Some(Some(tactic)) => {
            check_tactic(tactic)?;
            next.tactic = Some(tactic.clone());
        }
    }
    if let Some(ids) = &edit.technique_ids {
        if ids.len() > MAX_TECHNIQUES {
            return Err(EngineError::Invalid(format!(
                "an entry carries at most {MAX_TECHNIQUES} technique ids, not {}",
                ids.len()
            )));
        }
        let mut kept: Vec<String> = Vec::with_capacity(ids.len());
        for id in ids {
            if !is_technique_id(id) {
                return Err(EngineError::Invalid(format!(
                    "{:?} is not an ATT&CK technique id (T1234 or T1234.001)",
                    clip(id, 64)
                )));
            }
            if !kept.contains(id) {
                kept.push(id.clone());
            }
        }
        next.technique_ids = kept;
    }
    Ok(next)
}

/// A new entry of `actor`'s from `edit`, which must give it a title: an
/// analyst's is a `draft`, an agent's `unreviewed`. With no time it needs
/// one; a typed time is `time_source = "typed"`.
pub fn new_entry(case: &mut Case, edit: &EntryEdit, actor: &Actor) -> Result<Entry> {
    require_records(case)?;
    let Some(title) = &edit.title else {
        return Err(EngineError::Invalid("an entry needs a title".to_string()));
    };
    let at = now_iso();
    // Checked before an id is taken, so a refused entry uses none.
    let draft = apply(&Stored::new(0, title.clone(), actor, &at), edit)?;
    let tx = case.conn_mut().transaction()?;
    let id = next_ids(&tx, 1)?[0];
    let stored = Stored { id, ..draft };
    crate::annot::write_rows(&tx, INSERT_SQL, INSERT_ROW, &[stored.insert_values()])?;
    record(&tx, actor, &at, stored.created())?;
    let entry = read_entry(&tx, id)?;
    tx.commit()?;
    Ok(entry)
}

/// Change entry `id` as `edit` says. An edit that changes nothing writes
/// nothing. `Refused` for an entry the actor may not change (`changeable`),
/// and for clearing a reviewed entry's time; `Invalid` for a field that
/// does not hold; `NotFound` for an unknown id.
pub fn edit_entry(case: &mut Case, id: i64, edit: &EntryEdit, actor: &Actor) -> Result<Entry> {
    require_records(case)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let before = changeable(&tx, id, actor)?;
    let mut after = apply(&before, edit)?;
    if after == before {
        return read_entry(&tx, id);
    }
    if after.status == EntryStatus::Reviewed && after.time.is_none() {
        return Err(EngineError::Refused(format!(
            "E-{id} is reviewed and keeps a time; set it back to draft to clear it"
        )));
    }
    after.modified_by = actor.label();
    after.modified_at = at.clone();
    tx.execute(
        "UPDATE ox.entry SET \"time\" = ?, time_source = ?, title = ?, description = ?, \
         tactic = ?, technique_ids = ?, modified_by = ?, modified_at = ? WHERE id = ?",
        params![
            after.time,
            after.time_source,
            after.title,
            after.description,
            after.tactic,
            techniques_json(&after.technique_ids),
            after.modified_by,
            after.modified_at,
            id
        ],
    )?;
    record(&tx, actor, &at, change(id, "entry_edit", &before, &after))?;
    let entry = read_entry(&tx, id)?;
    tx.commit()?;
    Ok(entry)
}

/// Set entry `id`'s status. Only an analyst does: an agent is `Refused`,
/// for its own entries too. `reviewed` is `Refused` for an entry with no
/// time (it needs one first); it records who reviewed it, which any other
/// status clears. Setting the status it has writes nothing.
pub fn set_entry_status(
    case: &mut Case,
    id: i64,
    status: EntryStatus,
    actor: &Actor,
) -> Result<Entry> {
    require_records(case)?;
    if actor.is_agent() {
        return Err(EngineError::Refused(
            "only an analyst sets an entry's status: an agent's entries stay unreviewed \
             until an analyst reviews them"
                .to_string(),
        ));
    }
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let before = changeable(&tx, id, actor)?;
    if before.status == status {
        return read_entry(&tx, id);
    }
    if status == EntryStatus::Reviewed && before.time.is_none() {
        return Err(EngineError::Refused(format!(
            "E-{id} needs a time before it can be reviewed"
        )));
    }
    let mut after = before.clone();
    after.status = status;
    (after.reviewed_by, after.reviewed_at) = if status == EntryStatus::Reviewed {
        (Some(actor.label()), Some(at.clone()))
    } else {
        (None, None)
    };
    after.modified_by = actor.label();
    after.modified_at = at.clone();
    tx.execute(
        "UPDATE ox.entry SET status = ?, reviewed_by = ?, reviewed_at = ?, modified_by = ?, \
         modified_at = ? WHERE id = ?",
        params![
            status.as_str(),
            after.reviewed_by,
            after.reviewed_at,
            after.modified_by,
            after.modified_at,
            id
        ],
    )?;
    record(&tx, actor, &at, change(id, "entry_status", &before, &after))?;
    let entry = read_entry(&tx, id)?;
    tx.commit()?;
    Ok(entry)
}

/// The record link tables an entry's links live in, by the kind each
/// `link` history row names.
const LINK_TABLES: [(&str, &str); 3] = [
    ("host", "ox.entry_host"),
    ("account", "ox.entry_account"),
    ("ioc", "ox.entry_ioc"),
];

/// Delete entry `id`: a tombstone. The entry is kept and flagged; its links
/// to host, account and IOC records are removed, each with its history row;
/// its evidence rows are kept with their snapshots, and a deleted entry
/// marks no row. `Refused` for an entry already deleted or one the actor
/// may not change (`changeable`); `NotFound` for an unknown id.
pub fn delete_entry(case: &mut Case, id: i64, actor: &Actor) -> Result<()> {
    require_records(case)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let before = changeable(&tx, id, actor)?;
    let mut history = Vec::new();
    let mut after = before.clone();
    after.deleted = true;
    after.modified_by = actor.label();
    after.modified_at = at.clone();
    tx.execute(
        "UPDATE ox.entry SET deleted = true, modified_by = ?, modified_at = ? WHERE id = ?",
        params![after.modified_by, after.modified_at, id],
    )?;
    history.push(change(id, "entry_delete", &before, &after));
    for (kind, table) in LINK_TABLES {
        let links: Vec<(i64, String, String)> = tx
            .prepare(&format!(
                "SELECT record_id, linked_by, linked_at FROM {table} \
                 WHERE entry_id = ? ORDER BY record_id"
            ))?
            .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        tx.execute(
            &format!("DELETE FROM {table} WHERE entry_id = ?"),
            params![id],
        )?;
        for (record_id, linked_by, linked_at) in links {
            let target_id = if kind == "host" {
                host_link_id(id, record_id)
            } else {
                format!("entry_{kind}:{id}:{record_id}")
            };
            history.push(Change {
                target_kind: "link",
                target_id,
                action: "link_remove",
                before: Some(serde_json::json!({
                    "kind": kind, "entry_id": id, "record_id": record_id,
                    "linked_by": linked_by, "linked_at": linked_at,
                })),
                after: None,
            });
        }
    }
    record_all(&tx, actor, &at, history)?;
    tx.commit()?;
    Ok(())
}

/// The history of a change to entry `id`.
fn change(id: i64, action: &'static str, before: &Stored, after: &Stored) -> Change<'static> {
    Change {
        target_kind: "entry",
        target_id: id.to_string(),
        action,
        before: Some(before.json()),
        after: Some(after.json()),
    }
}

/// Up to `limit` live entries from `offset` that `q` selects, in the
/// Timeline's order: by time, the undated after every dated one, then by
/// id. A case without the record tables has none. `Invalid` for a tactic
/// not in `TACTICS`, a text or host over 1,000 characters, a `limit` of 0
/// or over `MAX_LIMIT`, or an `offset` past `i64::MAX`.
pub fn entries(case: &Case, q: &EntryQuery, limit: u32, offset: u64) -> Result<Listing<Entry>> {
    check_limit(limit)?;
    check_offset(offset)?;
    let mut filters: Vec<String> = Vec::new();
    let mut params: Vec<SqlValue> = Vec::new();
    if !q.status.is_empty() {
        filters.push(format!(
            "e.status IN ({})",
            vec!["?"; q.status.len()].join(", ")
        ));
        params.extend(q.status.iter().map(|s| SqlValue::Text(s.as_str().into())));
    }
    if let Some(host) = &q.host {
        check_filter("host", host)?;
        filters.push(
            "EXISTS (SELECT 1 FROM ox.entry_host l JOIN ox.host h ON h.id = l.record_id \
             WHERE l.entry_id = e.id AND NOT h.deleted AND lower(h.name) = lower(?))"
                .to_string(),
        );
        params.push(SqlValue::Text(host.clone()));
    }
    if let Some(tactic) = &q.tactic {
        check_tactic(tactic)?;
        filters.push("e.tactic = ?".to_string());
        params.push(SqlValue::Text(tactic.clone()));
    }
    if let Some(text) = &q.text {
        check_filter("text", text)?;
        filters.push(
            "(contains(lower(e.title), lower(?)) OR contains(lower(e.description), lower(?)))"
                .to_string(),
        );
        params.push(SqlValue::Text(text.clone()));
        params.push(SqlValue::Text(text.clone()));
    }
    if !has_records(case) {
        return Ok(Listing {
            items: Vec::new(),
            truncated: false,
        });
    }
    list_entries(case, filters, params, limit, offset)
}

/// Up to `limit` live entries from `offset` that every one of `filters`
/// (SQL over `ox.entry e`, its values in `params`) selects, in the
/// Timeline's order. The caller has checked the case has the record
/// tables, `limit` and `offset`.
pub(super) fn list_entries(
    case: &Case,
    mut filters: Vec<String>,
    mut params: Vec<SqlValue>,
    limit: u32,
    offset: u64,
) -> Result<Listing<Entry>> {
    filters.insert(0, "NOT e.deleted".to_string());
    params.push(SqlValue::BigInt(fetch_limit(limit)));
    params.push(SqlValue::UBigInt(offset));
    let raw: Vec<RawEntry> = case
        .conn()
        .prepare(&format!(
            "{ENTRY_SELECT} WHERE {} ORDER BY e.\"time\" ASC NULLS LAST, e.id LIMIT ? OFFSET ?",
            filters.join(" AND ")
        ))?
        .query_map(params_from_iter(params), raw_entry)?
        .collect::<std::result::Result<_, _>>()?;
    let items = raw
        .into_iter()
        .map(|raw| decode(raw).map(|(s, n, h)| shown(s, n, h)))
        .collect::<Result<Vec<_>>>()?;
    Ok(Listing::from_fetched(items, limit))
}

fn check_filter(what: &str, text: &str) -> Result<()> {
    let n = text.chars().count();
    if n > MAX_FILTER {
        return Err(EngineError::Invalid(format!(
            "a Timeline {what} filter is at most {MAX_FILTER} characters, not {n}"
        )));
    }
    Ok(())
}
