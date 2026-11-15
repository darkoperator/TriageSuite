//! IOC records: a typed, validated value (ruling S4), the tool it belongs
//! to, ATT&CK ids and a note, with the columns computed from their linked
//! entries, and the sightings computed from the loaded data.

use super::entry::MAX_TECHNIQUES;
use super::records::{
    agent_added, changeable_record, duplicate, next_id, non_empty, order_by, record_change,
    refuse_duplicate, row_json, seen_columns, RawSeen, RecordKind, RecordOrder, RecordStatus, Seen,
};
use super::validate::{
    check_line, check_text, validate_attack_id, validate_ioc, IocType, MAX_RECORD_LABEL,
};
use super::{has_records, require_records};
use crate::actor::Actor;
use crate::annot::record;
use crate::case::{stored_zone, Case};
use crate::catalog::TimeSemantics;
use crate::error::{EngineError, Result};
use crate::listing::{check_limit, fetch_limit, Listing};
use crate::query::{check_offset, find, FindMode, FindRequest};
use crate::time::now_iso;
use duckdb::types::Value as SqlValue;
use duckdb::{params, params_from_iter, Connection};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

/// The most characters of an IOC sheet filter's tool or text.
const MAX_FILTER: usize = 1_000;

/// An IOC record, as the IOCs sheet shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct IocRecord {
    pub id: i64,
    pub ioc_type: IocType,
    /// Validated for its type and stored canonical (`validate_ioc`).
    pub value: String,
    /// The tool it belongs to; `None` when none.
    pub tool: Option<String>,
    /// ATT&CK technique ids, each once, in the order given.
    pub attack_ids: Vec<String>,
    /// Empty when there is none.
    pub note: String,
    pub status: RecordStatus,
    pub created_by: String,
    pub created_at: String,
    pub modified_by: String,
    pub modified_at: String,
    /// An agent created it; still true once an analyst has reviewed it.
    pub agent_added: bool,
    /// Deleted (a tombstone); only `ioc_record` returns one.
    pub deleted: bool,
    /// Live entries linked to it.
    pub entries: u64,
    /// The earliest time among them, undated entries left out.
    pub first_seen: Option<String>,
    /// The latest time among them.
    pub last_seen: Option<String>,
    /// `first_seen` as the case's display zone shows it (`YYYY-MM-DD
    /// HH:MM:SS` and the fraction as stored), what the sheet shows; `None`
    /// with no dated entry.
    pub first_seen_shown: Option<String>,
    /// `last_seen` as the case's display zone shows it.
    pub last_seen_shown: Option<String>,
    /// The live host records those entries link, by name.
    pub hosts_seen: Vec<String>,
}

/// What `upsert_ioc` sets: a new record with `id: None`, else every field
/// of record `id`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct IocSpec {
    pub id: Option<i64>,
    pub ioc_type: IocType,
    /// Validated for `ioc_type` (`validate_ioc`); with it, unique among live
    /// IOCs once canonical, ASCII case ignored.
    pub value: String,
    /// At most `MAX_RECORD_LABEL` characters, no control character; empty
    /// is `None`.
    pub tool: Option<String>,
    /// Each `T1234` or `T1234.001`, at most `MAX_TECHNIQUES`; a repeated id
    /// is kept once.
    pub attack_ids: Vec<String>,
    /// At most `MAX_RECORD_NOTE` characters.
    pub note: String,
}

/// Which IOCs the sheet lists. Every field narrows; the default lists every
/// live IOC.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct IocQuery {
    pub ioc_type: Option<IocType>,
    /// This tool, case ignored.
    pub tool: Option<String>,
    /// In the value, tool or note, case ignored.
    pub text: Option<String>,
    /// The sheet's order (by a column an IOC has, `RecordOrderBy`, then
    /// id); absent or `null`, the closest values to `text` first, then by
    /// type and value.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub order: Option<RecordOrder>,
}

/// The fields `spec` stores, checked: `(value, tool, attack_ids)`.
fn checked(spec: &IocSpec) -> Result<(String, Option<String>, Vec<String>)> {
    let value = validate_ioc(spec.ioc_type, &spec.value)?;
    let tool = non_empty(&spec.tool);
    if let Some(tool) = &tool {
        check_line("tool", tool, MAX_RECORD_LABEL, false)?;
    }
    if spec.attack_ids.len() > MAX_TECHNIQUES {
        return Err(EngineError::Invalid(format!(
            "an IOC carries at most {MAX_TECHNIQUES} ATT&CK ids, not {}",
            spec.attack_ids.len()
        )));
    }
    let mut attack_ids: Vec<String> = Vec::with_capacity(spec.attack_ids.len());
    for raw in &spec.attack_ids {
        let id = validate_attack_id(raw)?;
        if !attack_ids.contains(&id) {
            attack_ids.push(id);
        }
    }
    check_text("note", &spec.note)?;
    Ok((value, tool, attack_ids))
}

/// The id of the live IOC of type `t` whose value is `value` (canonical),
/// ASCII case ignored; `None` when there is none.
pub(crate) fn find_ioc(conn: &Connection, t: IocType, value: &str) -> Result<Option<i64>> {
    duplicate(
        conn,
        "SELECT id, value FROM ox.ioc WHERE NOT deleted AND type = ? AND lower(value) = lower(?)",
        &[
            SqlValue::Text(t.as_str().to_string()),
            SqlValue::Text(value.to_string()),
        ],
        |r| Ok((r.get(0)?, r.get::<_, String>(1)?)),
        |other| other.eq_ignore_ascii_case(value),
    )
}

fn ids_json(ids: &[String]) -> String {
    serde_json::Value::from(ids).to_string()
}

/// Make or change an IOC record, as `spec` says: a new one (`reviewed` for
/// an analyst, `unreviewed` for an agent) when `spec.id` is `None`, else
/// every field of record `spec.id`. The value is stored canonical. An edit
/// that changes nothing writes nothing. `Invalid` for a field that does not
/// hold; `Refused` for a type and value another live IOC has (ASCII case
/// ignored), a deleted record, or one the actor may not change (S3);
/// `NotFound` for an unknown id.
pub fn upsert_ioc(case: &mut Case, spec: &IocSpec, actor: &Actor) -> Result<IocRecord> {
    require_records(case)?;
    let (value, tool, attack_ids) = checked(spec)?;
    let kind = RecordKind::Ioc;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    if let Some(id) = spec.id {
        changeable_record(&tx, kind, id, actor)?;
    }
    if let Some(existing) = find_ioc(&tx, spec.ioc_type, &value)?.filter(|&e| Some(e) != spec.id) {
        return Err(refuse_duplicate(kind, existing, "holds that value"));
    }
    let label = actor.label();
    let id = match spec.id {
        None => {
            let id = next_id(&tx, kind)?;
            tx.execute(
                "INSERT INTO ox.ioc \
                 (id, type, value, tool, attack_ids, note, status, \
                  created_by, created_at, modified_by, modified_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    id,
                    spec.ioc_type.as_str(),
                    value,
                    tool,
                    ids_json(&attack_ids),
                    spec.note,
                    RecordStatus::initial(actor).as_str(),
                    label,
                    at,
                    label,
                    at
                ],
            )?;
            let after = row_json(&tx, kind, id)?;
            let action = kind.action("create");
            record(
                &tx,
                actor,
                &at,
                record_change(kind, id, &action, None, Some(after)),
            )?;
            id
        }
        Some(id) => {
            let current = read_ioc(&tx, id)?;
            let unchanged = current.ioc_type == spec.ioc_type
                && current.value == value
                && current.tool == tool
                && current.attack_ids == attack_ids
                && current.note == spec.note;
            if unchanged {
                return Ok(current);
            }
            let before = row_json(&tx, kind, id)?;
            tx.execute(
                "UPDATE ox.ioc SET type = ?, value = ?, tool = ?, attack_ids = ?, note = ?, \
                 modified_by = ?, modified_at = ? WHERE id = ?",
                params![
                    spec.ioc_type.as_str(),
                    value,
                    tool,
                    ids_json(&attack_ids),
                    spec.note,
                    label,
                    at,
                    id
                ],
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
    let out = read_ioc(&tx, id)?;
    tx.commit()?;
    Ok(out)
}

/// What every read of an IOC selects, in `raw_ioc`'s order.
fn ioc_select() -> String {
    format!(
        "SELECT r.id, r.type, r.value, r.tool, r.attack_ids, r.note, r.status, \
         r.created_by, r.created_at, r.modified_by, r.modified_at, r.deleted, {} \
         FROM ox.ioc r",
        seen_columns(RecordKind::Ioc)
    )
}

/// An IOC as `ioc_select` reads it, before decoding.
type RawIoc = (
    (i64, String, String, Option<String>, String, String, String),
    (String, String, String, String, bool),
    RawSeen,
);

fn raw_ioc(r: &duckdb::Row<'_>) -> duckdb::Result<RawIoc> {
    Ok((
        (
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
        ),
        (r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?, r.get(11)?),
        Seen::raw(r, 12)?,
    ))
}

fn decode_ioc(raw: RawIoc, zone: &str) -> Result<IocRecord> {
    let ((id, ioc_type, value, tool, attack_ids, note, status), stamps, seen) = raw;
    let (created_by, created_at, modified_by, modified_at, deleted) = stamps;
    let seen = Seen::decode(RecordKind::Ioc, id, seen, zone)?;
    // The engine writes both itself.
    let ioc_type = IocType::parse(&ioc_type)
        .map_err(|_| EngineError::Internal(format!("I-{id} holds the type {ioc_type:?}")))?;
    let attack_ids: Vec<String> = serde_json::from_str(&attack_ids).map_err(|e| {
        EngineError::Internal(format!("I-{id} holds ATT&CK ids that do not parse: {e}"))
    })?;
    Ok(IocRecord {
        id,
        ioc_type,
        value,
        tool,
        attack_ids,
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
        first_seen_shown: seen.first_seen_shown,
        last_seen_shown: seen.last_seen_shown,
        hosts_seen: seen.hosts_seen,
    })
}

fn read_ioc(conn: &Connection, id: i64) -> Result<IocRecord> {
    let raw = conn
        .prepare(&format!("{} WHERE r.id = ?", ioc_select()))?
        .query_map(params![id], raw_ioc)?
        .next()
        .transpose()?
        .ok_or_else(|| EngineError::NotFound(format!("no {}", RecordKind::Ioc.shown(id))))?;
    decode_ioc(raw, &stored_zone(conn)?)
}

/// IOC record `id`, deleted or not. `NotFound` for an unknown id, and in a
/// case without the record tables.
pub fn ioc_record(case: &Case, id: i64) -> Result<IocRecord> {
    if !has_records(case) {
        return Err(EngineError::NotFound(format!(
            "no {}",
            RecordKind::Ioc.shown(id)
        )));
    }
    read_ioc(case.conn(), id)
}

fn check_filter(what: &str, text: &str) -> Result<()> {
    let n = text.chars().count();
    if n > MAX_FILTER {
        return Err(EngineError::Invalid(format!(
            "an IOC {what} filter is at most {MAX_FILTER} characters, not {n}"
        )));
    }
    Ok(())
}

/// Up to `limit` live IOC records from `offset` that `q` selects, the
/// values closest to `q.text` first (`match_rank`), then by type
/// (as spelled), then value (case ignored), then id. A case without the
/// record tables has none. `Invalid` for a tool or text over 1,000
/// characters, a `limit` of 0 or over `MAX_LIMIT`, or an `offset` past
/// `i64::MAX`.
pub fn iocs(case: &Case, q: &IocQuery, limit: u32, offset: u64) -> Result<Listing<IocRecord>> {
    check_limit(limit)?;
    check_offset(offset)?;
    let explicit = q
        .order
        .map(|o| order_by(RecordKind::Ioc, o, "r.value"))
        .transpose()?;
    let mut filters = vec!["NOT r.deleted".to_string()];
    let mut params: Vec<SqlValue> = Vec::new();
    if let Some(t) = q.ioc_type {
        filters.push("r.type = ?".to_string());
        params.push(SqlValue::Text(t.as_str().to_string()));
    }
    if let Some(tool) = &q.tool {
        check_filter("tool", tool)?;
        filters.push("lower(r.tool) = lower(?)".to_string());
        params.push(SqlValue::Text(tool.clone()));
    }
    if let Some(text) = &q.text {
        check_filter("text", text)?;
        filters.push(
            "(contains(lower(r.value), lower(?)) OR contains(lower(coalesce(r.tool, '')), \
             lower(?)) OR contains(lower(r.note), lower(?)))"
                .to_string(),
        );
        params.extend(std::iter::repeat_n(SqlValue::Text(text.clone()), 3));
    }
    if !has_records(case) {
        return Ok(Listing {
            items: Vec::new(),
            truncated: false,
        });
    }
    // The sheet's order when it asked for one; else the closest values to the
    // text first (`match_rank`), then by type and value.
    let ordering = match explicit {
        Some(o) => o,
        None => {
            let rank_text = q.text.clone().map_or(SqlValue::Null, SqlValue::Text);
            params.extend(std::iter::repeat_n(rank_text, super::MATCH_RANK_PARAMS));
            format!(
                "{}, r.type, lower(r.value), r.id",
                super::match_rank("r.value")
            )
        }
    };
    params.push(SqlValue::BigInt(fetch_limit(limit)));
    params.push(SqlValue::UBigInt(offset));
    let raw: Vec<RawIoc> = case
        .conn()
        .prepare(&format!(
            "{} WHERE {} ORDER BY {ordering} LIMIT ? OFFSET ?",
            ioc_select(),
            filters.join(" AND "),
        ))?
        .query_map(params_from_iter(params), raw_ioc)?
        .collect::<std::result::Result<_, _>>()?;
    let zone = stored_zone(case.conn())?;
    let items = raw
        .into_iter()
        .map(|r| decode_ioc(r, &zone))
        .collect::<Result<Vec<_>>>()?;
    Ok(Listing::from_fetched(items, limit))
}

/// Where an IOC's value occurs in the loaded data (ruling S5): Find
/// everywhere, exact and without regard to case, for the value. Computed
/// each time it is asked for, never stored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Sightings {
    /// Matching cells, as Find everywhere counts them: a row whose value is
    /// in two columns counts twice. 0 is "not found".
    pub hits: u64,
    /// The relations with a hit and the hits in each, by relation name.
    pub relations: Vec<(String, u64)>,
    /// The hosts a hit came from, sorted; a hit with no host adds none.
    pub hosts: Vec<String>,
    /// The earliest and latest primary time among the hits of relations
    /// whose primary time is UTC, as Find renders it. A time whose zone is
    /// not known is never compared with a UTC one, so it is left out.
    pub first: Option<String>,
    pub last: Option<String>,
}

/// The sightings of live IOC `ioc_id`: `find` (`FindMode::Exact`) for its
/// value across every relation and host. `NotFound` for an IOC the case
/// does not have or has deleted, and in a case without the record tables;
/// `Invalid` with the message `cancelled` when `cancel` is set, read before
/// the search and after each relation; `Invalid` for a value longer than
/// Find takes (`MAX_VALUE`).
pub fn ioc_sightings(case: &Case, ioc_id: i64, cancel: &AtomicBool) -> Result<Sightings> {
    sightings(case, ioc_id, cancel, &mut |_| {})
}

/// `ioc_sightings`, calling `pause` with each relation's name as its search
/// is done, before the cancel flag is read for the next: a test holds the
/// sweep there, mid-search, and counts the relations it reached. Built only
/// with `test-support`.
#[cfg(feature = "test-support")]
pub fn ioc_sightings_paused(
    case: &Case,
    ioc_id: i64,
    cancel: &AtomicBool,
    pause: &mut dyn FnMut(&str),
) -> Result<Sightings> {
    sightings(case, ioc_id, cancel, pause)
}

fn sightings(
    case: &Case,
    ioc_id: i64,
    cancel: &AtomicBool,
    pause: &mut dyn FnMut(&str),
) -> Result<Sightings> {
    let ioc = ioc_record(case, ioc_id)?;
    if ioc.deleted {
        return Err(EngineError::NotFound(format!(
            "no {}: it was deleted",
            RecordKind::Ioc.shown(ioc_id)
        )));
    }
    let cancelled = || EngineError::Invalid("cancelled".to_string());
    if cancel.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    let found = find(
        case,
        &FindRequest {
            value: ioc.value,
            mode: FindMode::Exact,
            hosts: Vec::new(),
            from: None,
            to: None,
        },
        &mut |relation, _| {
            pause(relation);
            !cancel.load(Ordering::Relaxed)
        },
    )?;
    if found.cancelled {
        return Err(cancelled());
    }
    let mut relations: BTreeMap<String, u64> = BTreeMap::new();
    let mut hosts: BTreeSet<String> = BTreeSet::new();
    let mut out = Sightings {
        hits: 0,
        relations: Vec::new(),
        hosts: Vec::new(),
        first: None,
        last: None,
    };
    for hit in found.hits {
        out.hits += hit.count;
        *relations.entry(hit.relation).or_default() += hit.count;
        hosts.extend(hit.host);
        if hit.time_semantics == Some(TimeSemantics::Utc) {
            // Find renders every UTC time in one fixed-width form, so text
            // order is time order.
            out.first = out.first.into_iter().chain(hit.first).min();
            out.last = out.last.into_iter().chain(hit.last).max();
        }
    }
    out.relations = relations.into_iter().collect();
    out.hosts = hosts.into_iter().collect();
    Ok(out)
}

/// Texts bound in one statement of `ioc_texts`.
const TEXTS_PER_QUERY: usize = 1_000;

/// Which of `texts` (each ASCII lower-cased, as the caller gives them) are
/// a live IOC's value, ASCII case ignored, for flagging the grid cells
/// that hold one (`GridCell::ioc`). Only the IOCs that can match are read:
/// SQL keeps those whose `lower` value is one of the texts' (Unicode
/// lowering, which every ASCII-case match passes), bound in chunks of
/// `TEXTS_PER_QUERY`, and each is then checked here in ASCII case. So what
/// is read is bounded by `texts`, a page's cells, not by the case's IOCs.
/// Empty in a case without the record tables, which has no IOCs.
pub(crate) fn ioc_texts(case: &Case, texts: &BTreeSet<String>) -> Result<HashSet<String>> {
    let mut out = HashSet::new();
    if !has_records(case) {
        return Ok(out);
    }
    let list: Vec<&String> = texts.iter().collect();
    for chunk in list.chunks(TEXTS_PER_QUERY) {
        let sql = format!(
            "SELECT DISTINCT value FROM ox.ioc WHERE NOT deleted AND lower(value) IN ({})",
            vec!["lower(?)"; chunk.len()].join(", ")
        );
        let params = chunk.iter().map(|t| SqlValue::Text((*t).clone()));
        let mut stmt = case.conn().prepare(&sql)?;
        let mut rows = stmt.query(params_from_iter(params))?;
        while let Some(row) = rows.next()? {
            let value: String = row.get(0)?;
            let folded = value.to_ascii_lowercase();
            if texts.contains(&folded) {
                out.insert(folded);
            }
        }
    }
    Ok(out)
}
