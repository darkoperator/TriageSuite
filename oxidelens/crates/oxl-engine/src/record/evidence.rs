//! Rows attached to entries as evidence: M (promote: a new entry per row),
//! A (attach to an entry), detach, and the evidence list with its re-check.
//!
//! Attaching a row stores, beside its id, the row's hash and a snapshot of
//! its cells (catalogued column name to text), read from the row's own
//! source table. Supersession moves the evidence to the identical row of a
//! newer run by that hash; the snapshot stays through supersession and the
//! removal of the row's source, so the evidence can always be quoted, and
//! re-check compares it with the row as the case holds it now.

use super::entry::{changeable, next_ids, read_stored, Stored, INSERT_ROW, INSERT_SQL};
use super::hosts::{ensure_host, link_hosts};
use super::{has_records, require_records, MAX_DESCRIPTION, MAX_TITLE};
use crate::actor::Actor;
use crate::annot::{
    check_batch, clear_where_allowed, id_list, inactive, locate_rows, no_row, record_all,
    write_rows, Change, LocatedRow, RowOutcome, Standing, IDS_PER_STATEMENT,
};
use crate::case::Case;
use crate::catalog::appmap::summary_for;
use crate::catalog::{physical_columns, read_columns, read_relation, ColumnRole, TimeSemantics};
use crate::error::{EngineError, Result};
use crate::load::stage::source_schema;
use crate::query::{summary_sql, ISO_FORMAT, ROW_ALIAS};
use crate::time::now_iso;
use duckdb::types::Value as SqlValue;
use duckdb::{params, params_from_iter, Connection};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use triage_core::output::duckdb::sql::quote_ident;

/// What ends an undated entry's description whose written time had to be
/// cut to fit `MAX_DESCRIPTION`.
const CUT_NOTE: &str = " ... (cut to fit the description)";
/// Characters of a summary made from a snapshot, as a merged row's.
const SNAPSHOT_SUMMARY_CAP: usize = 300;
/// Values a snapshot's summary takes when its relation has no summary entry.
const SNAPSHOT_SUMMARY_VALUES: usize = 4;

/// What M did with one row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PromoteOutcome {
    pub row_id: i64,
    /// The entry made for the row, or the one it is already evidence on;
    /// `None` when the row could not be promoted.
    pub entry_id: Option<i64>,
    /// A new entry was made.
    pub created: bool,
    /// Why nothing was made, when that needs saying: the row is unknown or
    /// out of the view, or it is evidence on an agent's entry to review.
    pub reason: Option<String>,
}

/// How an evidence row compares with the row the case holds now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStanding {
    /// In the view, and its cells and hash are as they were attached.
    Unchanged,
    /// In the view, but its cells or hash differ from the snapshot.
    Changed,
    /// A newer run of its host replaced it and has no identical row: it is
    /// "from a superseded run", for the analyst to resolve.
    Superseded,
    /// Its source was removed; the snapshot is all that is left of it.
    SourceRemoved,
}

/// One row attached to an entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Evidence {
    pub row_id: i64,
    pub relation: String,
    /// The host the row describes; `None` for a row with no host.
    pub host: Option<String>,
    /// Re-checked on every read.
    pub standing: EvidenceStanding,
    /// The row's one-line summary, as a merged row's: read from the row
    /// while its table is in the case, else made from the snapshot.
    pub summary: String,
    pub attached_by: String,
    pub attached_at: String,
}

/// What attaching a row records, and what re-check compares, read from the
/// row's own source table.
struct RowFacts {
    hash: String,
    /// JSON object of catalogued column name to cell text (null for NULL).
    snapshot: String,
    summary: String,
    /// The primary time as UTC text, when it gives a UTC instant.
    time: Option<String>,
    /// The primary time column's name and its text as written, when it has
    /// text: what an undated entry's description quotes.
    written: Option<(String, String)>,
    /// The columns re-check leaves out: the run id and output file, which
    /// differ between two runs' identical rows (as the hash ignores them).
    provenance: Vec<String>,
}

/// The facts of each of `rows`, read a statement per table (and per
/// `IDS_PER_STATEMENT` rows) plus the catalog of each relation, however
/// many rows there are. A row whose source was removed is skipped: its
/// table is gone.
fn read_facts(conn: &Connection, rows: &[&LocatedRow]) -> Result<HashMap<i64, RowFacts>> {
    let mut tables: BTreeMap<(i64, String, String), Vec<i64>> = BTreeMap::new();
    for row in rows {
        if row.standing != Standing::Removed {
            tables
                .entry((row.source_id, row.relation.clone(), row.table.clone()))
                .or_default()
                .push(row.row_id);
        }
    }
    let mut out = HashMap::new();
    for ((source_id, relation, table), ids) in tables {
        let info = read_relation(conn, &relation)?;
        let columns = read_columns(conn, &relation)?;
        let present = physical_columns(conn, &source_schema(source_id), &relation)?;
        let has = |column: &str| present.contains(&column.to_lowercase());

        let kept: Vec<_> = columns.iter().filter(|c| has(&c.text_column)).collect();
        let snapshot = if kept.is_empty() {
            "CAST('{}' AS VARCHAR)".to_string()
        } else {
            let pairs: Vec<String> = kept
                .iter()
                .map(|c| {
                    format!(
                        "CAST(? AS VARCHAR), CAST({ROW_ALIAS}.{} AS VARCHAR)",
                        quote_ident(&c.text_column)
                    )
                })
                .collect();
            format!("CAST(json_object({}) AS VARCHAR)", pairs.join(", "))
        };
        let names: Vec<SqlValue> = kept
            .iter()
            .map(|c| SqlValue::Text(c.name.clone()))
            .collect();
        let provenance: Vec<String> = columns
            .iter()
            .filter(|c| matches!(c.role, ColumnRole::RunId | ColumnRole::OutputFile))
            .map(|c| c.name.clone())
            .collect();

        let primary = info
            .primary_time
            .as_deref()
            .and_then(|name| columns.iter().find(|c| c.name == name));
        let time = match primary {
            Some(c) if c.time_semantics == Some(TimeSemantics::Utc) => c
                .typed_column
                .as_deref()
                .filter(|t| has(t))
                .map(|t| format!("strftime({ROW_ALIAS}.{}, '{ISO_FORMAT}')", quote_ident(t))),
            _ => None,
        }
        .unwrap_or_else(|| "CAST(NULL AS VARCHAR)".to_string());
        let written = primary
            .filter(|c| has(&c.text_column))
            .map(|c| {
                format!(
                    "CAST({ROW_ALIAS}.{} AS VARCHAR)",
                    quote_ident(&c.text_column)
                )
            })
            .unwrap_or_else(|| "CAST(NULL AS VARCHAR)".to_string());
        let summary = summary_sql(&relation, &columns, Some(&present));

        for chunk in ids.chunks(IDS_PER_STATEMENT) {
            let mut stmt = conn.prepare(&format!(
                "SELECT {ROW_ALIAS}._ox_row, CAST({ROW_ALIAS}._ox_hash AS VARCHAR), {snapshot}, \
                        {summary}, {time}, {written} \
                 FROM {table} AS {ROW_ALIAS} WHERE {ROW_ALIAS}._ox_row IN ({})",
                id_list(chunk)
            ))?;
            let read = stmt.query_map(params_from_iter(&names), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })?;
            for row in read {
                let (id, hash, snapshot, summary, time, written) = row?;
                let written = primary
                    .zip(written.filter(|w| !w.is_empty()))
                    .map(|(c, w)| (c.name.clone(), w));
                out.insert(
                    id,
                    RowFacts {
                        hash,
                        snapshot,
                        summary: summary.unwrap_or_default(),
                        time,
                        written,
                        provenance: provenance.clone(),
                    },
                );
            }
        }
    }
    Ok(out)
}

/// An entry the row is evidence on, by the mark `ox.row_state` gives it.
enum Holder {
    /// The lowest-numbered live entry an analyst created or reviewed.
    Analysts(i64),
    /// The lowest-numbered live entry, every one an agent's unreviewed.
    Agents(i64),
}

/// For each of `rows` that is evidence on a live entry, the entry that
/// gives its mark, decided as `ox.row_state` decides it.
fn holders(conn: &Connection, rows: &[i64]) -> Result<HashMap<i64, Holder>> {
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

/// One evidence row to insert, and its history.
struct Attach {
    entry_id: i64,
    row_id: i64,
    relation: String,
    host: Option<String>,
    hash: String,
    snapshot: String,
}

impl Attach {
    fn new(entry_id: i64, row: &LocatedRow, facts: &RowFacts) -> Attach {
        Attach {
            entry_id,
            row_id: row.row_id,
            relation: row.relation.clone(),
            host: row.host.clone().filter(|h| !h.is_empty()),
            hash: facts.hash.clone(),
            snapshot: facts.snapshot.clone(),
        }
    }

    fn values(&self, actor: &str, at: &str) -> Vec<SqlValue> {
        vec![
            SqlValue::BigInt(self.entry_id),
            SqlValue::BigInt(self.row_id),
            SqlValue::Text(self.relation.clone()),
            self.host.clone().map_or(SqlValue::Null, SqlValue::Text),
            SqlValue::Text(self.hash.clone()),
            SqlValue::Text(self.snapshot.clone()),
            SqlValue::Text(actor.to_string()),
            SqlValue::Text(at.to_string()),
        ]
    }

    fn change(&self, actor: &str, at: &str) -> Change<'static> {
        Change {
            target_kind: "evidence",
            target_id: evidence_id(self.entry_id, self.row_id),
            action: "evidence_attach",
            before: None,
            after: Some(serde_json::json!({
                "entry_id": self.entry_id, "row_id": self.row_id,
                "relation": self.relation, "host": self.host, "row_hash": self.hash,
                "attached_by": actor, "attached_at": at,
            })),
        }
    }
}

/// The history id of row `row_id` as evidence on entry `entry_id`.
fn evidence_id(entry_id: i64, row_id: i64) -> String {
    format!("{entry_id}:{row_id}")
}

/// Insert `attaches` with their history, then link each entry to the host
/// record of each of its rows' hosts (ruling S7), creating a record the
/// case lacks.
fn write_attaches(conn: &Connection, attaches: &[Attach], actor: &Actor, at: &str) -> Result<()> {
    let label = actor.label();
    let rows: Vec<Vec<SqlValue>> = attaches.iter().map(|a| a.values(&label, at)).collect();
    write_rows(
        conn,
        "INSERT INTO ox.evidence \
         (entry_id, row_id, relation, host, row_hash, snapshot_json, attached_by, attached_at) \
         VALUES {values}",
        "(?, ?, ?, ?, ?, ?, ?, ?)",
        &rows,
    )?;
    record_all(
        conn,
        actor,
        at,
        attaches.iter().map(|a| a.change(&label, at)).collect(),
    )?;
    let names: BTreeSet<&str> = attaches.iter().filter_map(|a| a.host.as_deref()).collect();
    let mut records: HashMap<String, i64> = HashMap::new();
    for name in names {
        records.insert(name.to_string(), ensure_host(conn, name, actor, at)?);
    }
    let pairs: Vec<(i64, i64)> = attaches
        .iter()
        .filter_map(|a| Some((a.entry_id, *records.get(a.host.as_deref()?)?)))
        .collect();
    link_hosts(conn, &pairs, actor, at)
}

/// The title M gives the entry for a row: its summary cut to `MAX_TITLE`
/// characters, or the relation and row id when the summary is empty.
fn title_for(row: &LocatedRow, facts: &RowFacts) -> String {
    let summary = facts.summary.trim();
    if summary.is_empty() {
        return format!("{} row {}", row.relation, row.row_id);
    }
    cut(summary, MAX_TITLE)
}

/// An undated entry's description: the primary time column and its value
/// as written, whole while it fits `MAX_DESCRIPTION`; a longer value is cut
/// so that the description, ending in `CUT_NOTE`, is exactly that long.
fn written_description(column: &str, text: &str) -> String {
    let whole = format!("{column} as written: {text}");
    if whole.chars().count() <= MAX_DESCRIPTION {
        return whole;
    }
    let keep = MAX_DESCRIPTION.saturating_sub(CUT_NOTE.chars().count());
    format!("{}{CUT_NOTE}", cut(&whole, keep))
}

/// The first `max` characters of `text`.
fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => text[..end].to_string(),
        None => text.to_string(),
    }
}

/// M: a new entry for each of `rows`, with the row attached as its
/// evidence. An analyst's entry is a `draft`, an agent's `unreviewed`. The
/// entry's title is the row's summary (cut to `MAX_TITLE`), its host the
/// row's (linked by S7), and its time the row's primary time when that is
/// a UTC instant (`time_source = "row"`). When it is not (no primary time,
/// a column with no zone, a blank cell, a value that did not convert) the
/// entry has no time and needs one, and its description quotes the time as
/// written, when there is text. The row's Suspicious mark is cleared when
/// the actor may clear it (an analyst any, an agent only its own). One
/// transaction for the batch.
///
/// A row already evidence on a live entry gets no new one: `created` false
/// and that entry's id, the analysts' entry for a Malicious row, or for an
/// Agent: malicious row the agent's entry with the reason "agent entry;
/// review it". An unknown or inactive row gets no entry and says why. A row
/// id given more than once is one row. `Invalid` for an empty list;
/// `Refused` for an agent passing more than `AGENT_ROWS_PER_CALL` rows.
pub fn promote_rows(case: &mut Case, rows: &[i64], actor: &Actor) -> Result<Vec<PromoteOutcome>> {
    let rows = check_batch(rows, actor)?;
    require_records(case)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    let located = locate_rows(&tx, &rows)?;
    let held = holders(&tx, &rows)?;

    let mut out = Vec::with_capacity(rows.len());
    let mut fresh: Vec<&LocatedRow> = Vec::new();
    for &row_id in &rows {
        let outcome = |entry_id, reason| PromoteOutcome {
            row_id,
            entry_id,
            created: false,
            reason,
        };
        let Some(row) = located.get(&row_id) else {
            out.push(outcome(None, Some(no_row(row_id))));
            continue;
        };
        match held.get(&row_id) {
            Some(Holder::Analysts(entry)) => out.push(outcome(Some(*entry), None)),
            Some(Holder::Agents(entry)) => out.push(outcome(
                Some(*entry),
                Some("agent entry; review it".to_string()),
            )),
            None => match inactive(row) {
                Some(reason) => out.push(outcome(None, Some(reason))),
                None => {
                    fresh.push(row);
                    out.push(outcome(None, None));
                }
            },
        }
    }

    let facts = read_facts(&tx, &fresh)?;
    let ids = next_ids(&tx, fresh.len())?;
    let mut made: HashMap<i64, i64> = HashMap::new();
    let mut entries = Vec::with_capacity(fresh.len());
    let mut attaches = Vec::with_capacity(fresh.len());
    for (row, id) in fresh.iter().zip(ids) {
        let facts = facts.get(&row.row_id).ok_or_else(|| {
            EngineError::Internal(format!(
                "row {} was located but could not be read from {}",
                row.row_id, row.table
            ))
        })?;
        let mut stored = Stored::new(id, title_for(row, facts), actor, &at);
        match &facts.time {
            Some(time) => {
                stored.time = Some(time.clone());
                stored.time_source = Some("row".to_string());
            }
            None => {
                if let Some((column, text)) = &facts.written {
                    stored.description = written_description(column, text);
                }
            }
        }
        attaches.push(Attach::new(id, row, facts));
        entries.push(stored);
        made.insert(row.row_id, id);
    }

    write_rows(
        &tx,
        INSERT_SQL,
        INSERT_ROW,
        &entries
            .iter()
            .map(Stored::insert_values)
            .collect::<Vec<_>>(),
    )?;
    record_all(
        &tx,
        actor,
        &at,
        entries.iter().map(Stored::created).collect(),
    )?;
    write_attaches(&tx, &attaches, actor, &at)?;
    let promoted: Vec<i64> = attaches.iter().map(|a| a.row_id).collect();
    let cleared = clear_where_allowed(&tx, &promoted, actor)?;
    record_all(&tx, actor, &at, cleared)?;
    tx.commit()?;

    for outcome in &mut out {
        if let Some(id) = made.get(&outcome.row_id) {
            outcome.entry_id = Some(*id);
            outcome.created = true;
        }
    }
    Ok(out)
}

/// A: attach each of `rows` to entry `entry_id` as evidence, linking the
/// entry to each row's host record (S7). A row already evidence on the
/// entry is done and unchanged; an unknown or inactive row is not done. A
/// row's Suspicious mark is left as it is. `Refused` for an entry the
/// actor may not change (an agent: only its own unreviewed ones);
/// `NotFound` for an unknown entry; the whole-call checks are
/// `promote_rows`'s.
pub fn attach_rows(
    case: &mut Case,
    entry_id: i64,
    rows: &[i64],
    actor: &Actor,
) -> Result<Vec<RowOutcome>> {
    let rows = check_batch(rows, actor)?;
    require_records(case)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    changeable(&tx, entry_id, actor)?;
    let located = locate_rows(&tx, &rows)?;
    let mut present: HashSet<i64> = HashSet::new();
    for chunk in rows.chunks(IDS_PER_STATEMENT) {
        let mut stmt = tx.prepare(&format!(
            "SELECT row_id FROM ox.evidence WHERE entry_id = ? AND row_id IN ({})",
            id_list(chunk)
        ))?;
        for row in stmt.query_map(params![entry_id], |r| r.get::<_, i64>(0))? {
            present.insert(row?);
        }
    }

    let mut out = Vec::with_capacity(rows.len());
    let mut fresh: Vec<&LocatedRow> = Vec::new();
    for &row_id in &rows {
        let Some(row) = located.get(&row_id) else {
            out.push(RowOutcome::refused(row_id, no_row(row_id)));
            continue;
        };
        if present.contains(&row_id) {
            out.push(RowOutcome::unchanged(
                row_id,
                format!("row {row_id} is already evidence on E-{entry_id}"),
            ));
        } else if let Some(reason) = inactive(row) {
            out.push(RowOutcome::refused(row_id, reason));
        } else {
            fresh.push(row);
            out.push(RowOutcome::changed(row_id));
        }
    }
    let facts = read_facts(&tx, &fresh)?;
    let attaches = fresh
        .iter()
        .map(|row| {
            facts
                .get(&row.row_id)
                .map(|f| Attach::new(entry_id, row, f))
                .ok_or_else(|| {
                    EngineError::Internal(format!(
                        "row {} was located but could not be read from {}",
                        row.row_id, row.table
                    ))
                })
        })
        .collect::<Result<Vec<_>>>()?;
    write_attaches(&tx, &attaches, actor, &at)?;
    tx.commit()?;
    Ok(out)
}

/// Detach row `row_id` from entry `entry_id`. Its snapshot goes with it;
/// the entry's host links stay (they are the entry's). With no other live
/// entry holding it, the row is no longer Malicious: what is left is its
/// Suspicious mark, if it still has one. `NotFound` for an unknown entry or
/// a row that is not its evidence; `Refused` as `attach_rows`.
pub fn detach_row(case: &mut Case, entry_id: i64, row_id: i64, actor: &Actor) -> Result<()> {
    require_records(case)?;
    let at = now_iso();
    let tx = case.conn_mut().transaction()?;
    changeable(&tx, entry_id, actor)?;
    let before: (String, Option<String>, String, String, String) = tx
        .prepare(
            "SELECT relation, host, row_hash, attached_by, attached_at FROM ox.evidence \
             WHERE entry_id = ? AND row_id = ? LIMIT 1",
        )?
        .query_map(params![entry_id, row_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .next()
        .transpose()?
        .ok_or_else(|| {
            EngineError::NotFound(format!("row {row_id} is not evidence on E-{entry_id}"))
        })?;
    tx.execute(
        "DELETE FROM ox.evidence WHERE entry_id = ? AND row_id = ?",
        params![entry_id, row_id],
    )?;
    let (relation, host, hash, attached_by, attached_at) = before;
    record_all(
        &tx,
        actor,
        &at,
        vec![Change {
            target_kind: "evidence",
            target_id: evidence_id(entry_id, row_id),
            action: "evidence_detach",
            before: Some(serde_json::json!({
                "entry_id": entry_id, "row_id": row_id, "relation": relation,
                "host": host, "row_hash": hash,
                "attached_by": attached_by, "attached_at": attached_at,
            })),
            after: None,
        }],
    )?;
    tx.commit()?;
    Ok(())
}

/// The rows attached to entry `entry_id` (deleted or not), in the order
/// they were attached, each re-checked against the case: unchanged or
/// changed while its row is in the view, from a superseded run when a
/// newer run had no identical row, or source removed. A case without the
/// record tables has none. `NotFound` for an unknown entry.
pub fn evidence(case: &Case, entry_id: i64) -> Result<Vec<Evidence>> {
    if !has_records(case) {
        return Ok(Vec::new());
    }
    let conn = case.conn();
    read_stored(conn, entry_id)?;
    type Attached = (i64, String, Option<String>, String, String, String, String);
    let stored: Vec<Attached> = conn
        .prepare(
            "SELECT row_id, relation, host, row_hash, snapshot_json, attached_by, attached_at \
             FROM ox.evidence WHERE entry_id = ? ORDER BY attached_at, row_id",
        )?
        .query_map(params![entry_id], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?
        .collect::<std::result::Result<_, _>>()?;
    let ids: Vec<i64> = stored.iter().map(|s| s.0).collect();
    let located = locate_rows(conn, &ids)?;
    let facts = read_facts(conn, &located.values().collect::<Vec<_>>())?;

    stored
        .into_iter()
        .map(
            |(row_id, relation, host, hash, snapshot, attached_by, attached_at)| {
                let standing = located.get(&row_id).map(|r| r.standing);
                let now = facts.get(&row_id);
                let standing = match (standing, now) {
                    (Some(Standing::Active), Some(now)) => {
                        if now.hash == hash
                            && same_cells(&snapshot, &now.snapshot, &now.provenance)?
                        {
                            EvidenceStanding::Unchanged
                        } else {
                            EvidenceStanding::Changed
                        }
                    }
                    // In the view but unreadable: not the row it was.
                    (Some(Standing::Active), None) => EvidenceStanding::Changed,
                    (Some(Standing::Superseded), _) => EvidenceStanding::Superseded,
                    (Some(Standing::Removed) | None, _) => EvidenceStanding::SourceRemoved,
                };
                let summary = match now {
                    Some(now) if !now.summary.is_empty() => now.summary.clone(),
                    _ => snapshot_summary(&relation, &snapshot)?,
                };
                Ok(Evidence {
                    row_id,
                    relation,
                    host,
                    standing,
                    summary,
                    attached_by,
                    attached_at,
                })
            },
        )
        .collect()
}

/// A stored snapshot, parsed. The engine writes every snapshot itself.
fn parse_snapshot(text: &str) -> Result<serde_json::Map<String, serde_json::Value>> {
    match serde_json::from_str(text) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        Ok(_) => Err(EngineError::Internal(
            "an evidence snapshot is not a JSON object".to_string(),
        )),
        Err(e) => Err(EngineError::Internal(format!(
            "an evidence snapshot does not parse: {e}"
        ))),
    }
}

/// Whether two snapshots hold the same cells, `ignored` columns left out.
fn same_cells(stored: &str, now: &str, ignored: &[String]) -> Result<bool> {
    let mut stored = parse_snapshot(stored)?;
    let mut now = parse_snapshot(now)?;
    for column in ignored {
        stored.remove(column);
        now.remove(column);
    }
    Ok(stored == now)
}

/// A summary made from a snapshot, for a row whose table is gone: the
/// relation's summary columns' non-empty texts, or without them the first
/// four non-empty values, joined by ` · ` and cut to 300 characters.
fn snapshot_summary(relation: &str, snapshot: &str) -> Result<String> {
    let cells = parse_snapshot(snapshot)?;
    let text = |v: &serde_json::Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    let listed: Vec<String> = summary_for(relation)
        .unwrap_or_default()
        .iter()
        .filter_map(|name| cells.get(*name).and_then(text))
        .collect();
    let values = if listed.is_empty() {
        cells
            .values()
            .filter_map(text)
            .take(SNAPSHOT_SUMMARY_VALUES)
            .collect()
    } else {
        listed
    };
    Ok(cut(&values.join(" · "), SNAPSHOT_SUMMARY_CAP))
}
