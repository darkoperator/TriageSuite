//! The case record's commands: marks, tags and notes on rows; timeline
//! entries and the rows attached to them as evidence; host, account and IOC
//! records and their links to entries; the reads over them (a row's
//! candidates, an IOC's sightings, an account's logons, the history); and
//! the three record exports.
//!
//! Every rule is the engine's (who may change what, what a field may hold,
//! where an entry's time comes from, which host a row links). A change is a
//! write (`AppState::write`, under the analyst's name), after which the
//! window hears `case-changed` naming what it touched:
//!
//! - `marks`: a row's mark, or an entry change that moves one (S2 derives
//!   Malicious from evidence, so attaching, detaching, reviewing and
//!   deleting move marks too);
//! - `tags`, `notes`: a row's tags and notes;
//! - `entries`: the Timeline, which also shows each entry's hosts, so a
//!   record's change or link names it as well;
//! - `records`: the Hosts, Accounts and IOCs sheets, whose entry counts and
//!   first and last times follow their links; M and A link the row's host
//!   (S7), so they name it.
//!
//! A read runs on the window's slot (`At`). `ioc_sightings` searches every
//! relation, so it also carries a cancel flag the engine looks at between
//! relations. The exports are reads and jobs, as `grid_export` is.

use super::grid::{job_label, EXPORT_JOB};
use super::At;
use crate::error::CmdError;
use crate::state::{AppState, Point};
use oxl_engine::{
    AccountRecord, AccountSpec, Candidate, Case, Entry, EntryEdit, EntryQuery, EntryStatus,
    Evidence, ExportReport, HistoryRow, HostRecord, HostSpec, IocQuery, IocRecord, IocSpec,
    Listing, LogonSummary, Note, PromoteOutcome, RecordKind, RecordStatus, RowOutcome, Sightings,
    TagInfo,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// What each kind of write touches, as `case-changed` names it.
const MARKS: &[&str] = &["marks"];
const TAGS: &[&str] = &["tags"];
const NOTES: &[&str] = &["notes"];
/// An entry's own fields: the Timeline only.
const ENTRY: &[&str] = &["entries"];
/// A change to which entries hold evidence, or whether one is reviewed:
/// the rows' derived marks move with it.
const REVIEW: &[&str] = &["entries", "marks"];
/// Evidence attached (which links the row's host, S7) or an entry deleted
/// (which drops its links): the marks and the records' counts move.
const EVIDENCE: &[&str] = &["entries", "marks", "records"];
/// A record's fields or its existence: the sheets, and the Timeline, which
/// shows each entry's host names.
const RECORD: &[&str] = &["records", "entries"];
/// A record's review state: the sheets only.
const RECORD_STATUS: &[&str] = &["records"];
/// A link between an entry and a record.
const LINK: &[&str] = &["entries", "records"];

/// A text field an edit sets or clears; a field the edit leaves as it is
/// is `null` instead. In JSON, `{ "set": "…" }` or `"clear"`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum TextChange {
    Set(String),
    Clear,
}

impl TextChange {
    fn edit(&self) -> Option<String> {
        match self {
            TextChange::Set(text) => Some(text.clone()),
            TextChange::Clear => None,
        }
    }
}

/// What `entry_new` and `entry_edit` set: each field `null` to leave it as
/// it is (a new entry's default: no description, time, tactic or
/// technique), and a new entry must give its title. A time is UTC text,
/// typed by hand (`time_source` becomes `typed`); a time the evidence gave
/// is kept for as long as an edit leaves `time` `null`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct EntryChanges {
    pub title: Option<String>,
    pub description: Option<String>,
    pub time: Option<TextChange>,
    /// One of the fourteen ATT&CK Enterprise tactics, spelled exactly.
    pub tactic: Option<TextChange>,
    /// The whole list (`[]` clears it), each `T1234` or `T1234.001`.
    pub technique_ids: Option<Vec<String>>,
}

impl EntryChanges {
    /// The engine's edit: `Some(None)` clears, `None` leaves.
    fn edit(&self) -> EntryEdit {
        EntryEdit {
            title: self.title.clone(),
            description: self.description.clone(),
            time: self.time.as_ref().map(TextChange::edit),
            tactic: self.tactic.as_ref().map(TextChange::edit),
            technique_ids: self.technique_ids.clone(),
        }
    }
}

/// A host, account or IOC record, tagged with its kind: `{ "kind":
/// "host", …HostRecord }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaseRecord {
    Host(HostRecord),
    Account(AccountRecord),
    Ioc(IocRecord),
}

/// What `record_upsert` stores, tagged with its kind: a new record with
/// `id` `null`, else every field of record `id`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordSpec {
    Host(HostSpec),
    Account(AccountSpec),
    Ioc(IocSpec),
}

/// Which records `records_list` lists: the live hosts or accounts, each
/// narrowed by `NameQuery`, or the IOCs `IocQuery` narrows to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordQuery {
    Host(NameQuery),
    Account(NameQuery),
    Ioc(IocQuery),
}

/// How the Hosts or Accounts list narrows: by text in a host's name, or in
/// an account's `DOMAIN\name` (so its name, its domain, or both as shown),
/// case ignored. Absent or `null`, every record. `order` is the sheet's
/// (by `name`, `entries`, `first_seen` or `last_seen`, then id); absent or
/// `null`, the closest matches first, then by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, ts_rs::TS)]
#[serde(deny_unknown_fields)]
pub struct NameQuery {
    #[serde(default)]
    #[ts(optional = nullable)]
    pub text: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub order: Option<oxl_engine::RecordOrder>,
}

fn listing<T>(list: Listing<T>, wrap: fn(T) -> CaseRecord) -> Listing<CaseRecord> {
    Listing {
        items: list.items.into_iter().map(wrap).collect(),
        truncated: list.truncated,
    }
}

/// Record `id` of `kind` as the engine reads it, a tombstone included.
fn record_of(case: &Case, kind: RecordKind, id: i64) -> oxl_engine::Result<CaseRecord> {
    Ok(match kind {
        RecordKind::Host => CaseRecord::Host(oxl_engine::host_record(case, id)?),
        RecordKind::Account => CaseRecord::Account(oxl_engine::account_record(case, id)?),
        RecordKind::Ioc => CaseRecord::Ioc(oxl_engine::ioc_record(case, id)?),
    })
}

// ---- marks, tags and notes on rows ----

/// S on `rows`: each marked Suspicious, or why not (`mark_suspicious`).
pub fn mark_set(state: &AppState, rows: &[i64]) -> Result<Vec<RowOutcome>, CmdError> {
    let actor = state.actor();
    state.write(MARKS, |c| oxl_engine::mark_suspicious(c, rows, &actor))
}

/// U on `rows`: each Suspicious mark cleared, or why not
/// (`clear_suspicious`).
pub fn mark_clear(state: &AppState, rows: &[i64]) -> Result<Vec<RowOutcome>, CmdError> {
    let actor = state.actor();
    state.write(MARKS, |c| oxl_engine::clear_suspicious(c, rows, &actor))
}

/// Tag each of `rows` with `tag` (`add_tag`).
pub fn tag_add(state: &AppState, rows: &[i64], tag: &str) -> Result<Vec<RowOutcome>, CmdError> {
    let actor = state.actor();
    state.write(TAGS, |c| oxl_engine::add_tag(c, rows, tag, &actor))
}

/// Take `tag` off each of `rows` (`remove_tag`).
pub fn tag_remove(state: &AppState, rows: &[i64], tag: &str) -> Result<Vec<RowOutcome>, CmdError> {
    let actor = state.actor();
    state.write(TAGS, |c| oxl_engine::remove_tag(c, rows, tag, &actor))
}

/// Up to `limit` of the tags some row carries, from `offset` (`tags`).
pub fn tags_list(
    state: &AppState,
    at: At<'_>,
    limit: u32,
    offset: u64,
) -> Result<Listing<TagInfo>, CmdError> {
    at.read(state, |c| oxl_engine::tags(c, limit, offset))
}

/// Write `text` as a note on row `row_id` (`add_note`).
pub fn note_add(state: &AppState, row_id: i64, text: &str) -> Result<Note, CmdError> {
    let actor = state.actor();
    state.write(NOTES, |c| oxl_engine::add_note(c, row_id, text, &actor))
}

/// Delete note `id` (`delete_note`).
pub fn note_delete(state: &AppState, id: i64) -> Result<(), CmdError> {
    let actor = state.actor();
    state.write(NOTES, |c| oxl_engine::delete_note(c, id, &actor))
}

/// Up to `limit` of row `row_id`'s notes from `offset`, oldest first
/// (`notes`).
pub fn notes_list(
    state: &AppState,
    at: At<'_>,
    row_id: i64,
    limit: u32,
    offset: u64,
) -> Result<Listing<Note>, CmdError> {
    at.read(state, |c| oxl_engine::notes(c, row_id, limit, offset))
}

// ---- timeline entries and their evidence ----

/// Up to `limit` live entries from `offset` that `query` selects, in the
/// Timeline's order, the undated last (`entries`).
pub fn entries_list(
    state: &AppState,
    at: At<'_>,
    query: &EntryQuery,
    limit: u32,
    offset: u64,
) -> Result<Listing<Entry>, CmdError> {
    at.read(state, |c| oxl_engine::entries(c, query, limit, offset))
}

/// Live entry `id` (`entry`).
pub fn entry_get(state: &AppState, at: At<'_>, id: i64) -> Result<Entry, CmdError> {
    at.read(state, |c| oxl_engine::entry(c, id))
}

/// A new entry from `edit`, which must give a title (`new_entry`).
pub fn entry_new(state: &AppState, edit: &EntryChanges) -> Result<Entry, CmdError> {
    let actor = state.actor();
    let edit = edit.edit();
    state.write(ENTRY, |c| oxl_engine::new_entry(c, &edit, &actor))
}

/// Change entry `id` as `edit` says (`edit_entry`).
pub fn entry_edit(state: &AppState, id: i64, edit: &EntryChanges) -> Result<Entry, CmdError> {
    let actor = state.actor();
    let edit = edit.edit();
    state.write(ENTRY, |c| oxl_engine::edit_entry(c, id, &edit, &actor))
}

/// Set entry `id`'s status (`set_entry_status`): reviewing an agent's entry
/// turns its rows from Agent: malicious to Malicious.
pub fn entry_status(state: &AppState, id: i64, status: EntryStatus) -> Result<Entry, CmdError> {
    let actor = state.actor();
    state.write(REVIEW, |c| {
        oxl_engine::set_entry_status(c, id, status, &actor)
    })
}

/// Delete entry `id`, leaving a tombstone (`delete_entry`).
pub fn entry_delete(state: &AppState, id: i64) -> Result<(), CmdError> {
    let actor = state.actor();
    state.write(EVIDENCE, |c| oxl_engine::delete_entry(c, id, &actor))
}

/// M on `rows`: an entry for each, timed by the row when it can be
/// (`promote_rows`).
pub fn rows_promote(state: &AppState, rows: &[i64]) -> Result<Vec<PromoteOutcome>, CmdError> {
    let actor = state.actor();
    state.write(EVIDENCE, |c| oxl_engine::promote_rows(c, rows, &actor))
}

/// A: attach `rows` to entry `entry_id` as evidence (`attach_rows`).
pub fn rows_attach(
    state: &AppState,
    entry_id: i64,
    rows: &[i64],
) -> Result<Vec<RowOutcome>, CmdError> {
    let actor = state.actor();
    state.write(EVIDENCE, |c| {
        oxl_engine::attach_rows(c, entry_id, rows, &actor)
    })
}

/// Take row `row_id` off entry `entry_id`'s evidence (`detach_row`). The
/// entry stays linked to the row's host.
pub fn evidence_detach(state: &AppState, entry_id: i64, row_id: i64) -> Result<(), CmdError> {
    let actor = state.actor();
    state.write(REVIEW, |c| {
        oxl_engine::detach_row(c, entry_id, row_id, &actor)
    })
}

/// Every row attached to live entry `entry_id`, each re-checked against
/// the case (`evidence`); `not_found` for an entry the case does not have,
/// as `entry_get` answers.
pub fn evidence_list(
    state: &AppState,
    at: At<'_>,
    entry_id: i64,
) -> Result<Vec<Evidence>, CmdError> {
    at.read(state, |c| {
        oxl_engine::entry(c, entry_id)?;
        oxl_engine::evidence(c, entry_id)
    })
}

// ---- host, account and IOC records ----

/// Up to `limit` live records of the kind `query` names, from `offset`
/// (`hosts`, `accounts`, `iocs`).
pub fn records_list(
    state: &AppState,
    at: At<'_>,
    query: &RecordQuery,
    limit: u32,
    offset: u64,
) -> Result<Listing<CaseRecord>, CmdError> {
    at.read(state, |c| {
        Ok(match query {
            RecordQuery::Host(q) => listing(
                oxl_engine::hosts_ordered(c, q.text.as_deref(), q.order, limit, offset)?,
                CaseRecord::Host,
            ),
            RecordQuery::Account(q) => listing(
                oxl_engine::accounts_ordered(c, q.text.as_deref(), q.order, limit, offset)?,
                CaseRecord::Account,
            ),
            RecordQuery::Ioc(q) => listing(oxl_engine::iocs(c, q, limit, offset)?, CaseRecord::Ioc),
        })
    })
}

/// Record `id` of `kind`, a deleted one too (`deleted` says so).
pub fn record_get(
    state: &AppState,
    at: At<'_>,
    kind: RecordKind,
    id: i64,
) -> Result<CaseRecord, CmdError> {
    at.read(state, |c| record_of(c, kind, id))
}

/// Store `spec`: a new record, or every field of record `spec.id`
/// (`upsert_host`, `upsert_account`, `upsert_ioc`).
pub fn record_upsert(state: &AppState, spec: &RecordSpec) -> Result<CaseRecord, CmdError> {
    let actor = state.actor();
    state.write(RECORD, |c| {
        Ok(match spec {
            RecordSpec::Host(s) => CaseRecord::Host(oxl_engine::upsert_host(c, s, &actor)?),
            RecordSpec::Account(s) => {
                CaseRecord::Account(oxl_engine::upsert_account(c, s, &actor)?)
            }
            RecordSpec::Ioc(s) => CaseRecord::Ioc(oxl_engine::upsert_ioc(c, s, &actor)?),
        })
    })
}

/// Delete record `id` of `kind`, leaving a tombstone (`delete_record`).
pub fn record_delete(state: &AppState, kind: RecordKind, id: i64) -> Result<(), CmdError> {
    let actor = state.actor();
    state.write(RECORD, |c| oxl_engine::delete_record(c, kind, id, &actor))
}

/// Merge record `from` into record `into`, both of `kind`
/// (`merge_records`).
pub fn record_merge(
    state: &AppState,
    kind: RecordKind,
    from: i64,
    into: i64,
) -> Result<(), CmdError> {
    let actor = state.actor();
    state.write(RECORD, |c| {
        oxl_engine::merge_records(c, kind, from, into, &actor)
    })
}

/// Set record `id`'s status (`set_record_status`).
pub fn record_status(
    state: &AppState,
    kind: RecordKind,
    id: i64,
    status: RecordStatus,
) -> Result<(), CmdError> {
    let actor = state.actor();
    state.write(RECORD_STATUS, |c| {
        oxl_engine::set_record_status(c, kind, id, status, &actor)
    })
}

/// Link entry `entry_id` to record `record_id` of `kind` (`link`).
pub fn record_link(
    state: &AppState,
    entry_id: i64,
    kind: RecordKind,
    record_id: i64,
) -> Result<(), CmdError> {
    let actor = state.actor();
    state.write(LINK, |c| {
        oxl_engine::link(c, entry_id, kind, record_id, &actor)
    })
}

/// Remove the link between entry `entry_id` and record `record_id` of
/// `kind` (`unlink`).
pub fn record_unlink(
    state: &AppState,
    entry_id: i64,
    kind: RecordKind,
    record_id: i64,
) -> Result<(), CmdError> {
    let actor = state.actor();
    state.write(LINK, |c| {
        oxl_engine::unlink(c, entry_id, kind, record_id, &actor)
    })
}

/// Up to `limit` live entries from `offset` linked to record `id` of
/// `kind`, in the Timeline's order (`record_entries`).
pub fn record_entries(
    state: &AppState,
    at: At<'_>,
    kind: RecordKind,
    id: i64,
    limit: u32,
    offset: u64,
) -> Result<Listing<Entry>, CmdError> {
    at.read(state, |c| {
        oxl_engine::record_entries(c, kind, id, limit, offset)
    })
}

// ---- reads over a row, an IOC, an account, the history ----

/// The accounts and IOCs row `row_id` names, for the analyst to link
/// (`candidates`).
pub fn candidates(state: &AppState, at: At<'_>, row_id: i64) -> Result<Vec<Candidate>, CmdError> {
    at.read(state, |c| oxl_engine::candidates(c, row_id))
}

/// Where IOC `id`'s value occurs in the loaded data (`ioc_sightings`):
/// computed, never stored (S5). The search goes relation by relation, so
/// the read carries a cancel flag the engine looks at between them, set
/// when the query is superseded, cancelled or its case closes; it answers
/// `superseded` or `interrupted` then, as any read does. The window runs a
/// sweep one IOC at a time on one slot (`sightings`).
pub fn ioc_sightings(state: &AppState, at: At<'_>, id: i64) -> Result<Sightings, CmdError> {
    let cancel = Arc::new(AtomicBool::new(false));
    state.read_with_flag(at.slot, at.query_id, cancel.clone(), |c| {
        engine::sightings(c, id, &cancel, &mut |_| {
            state.hook(Point::SightingsRelation)
        })
    })
}

/// The engine calls the app's commands make. Under test they are the
/// engine's `test-support` variants, which call a hook where a test holds
/// the work mid-way (`Point::SightingsRelation`, `Point::RecordBeforeRename`);
/// otherwise the plain calls, the hook unused.
#[cfg(test)]
mod engine {
    pub use oxl_engine::{
        export_report_before_rename as report, export_timeline_csv_before_rename as timeline,
        export_workbook_before_rename as workbook, ioc_sightings_paused as sightings,
    };
}

#[cfg(not(test))]
mod engine {
    use oxl_engine::{Case, ExportReport, Result, Sightings};
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    pub fn sightings(
        case: &Case,
        id: i64,
        cancel: &AtomicBool,
        _pause: &mut dyn FnMut(&str),
    ) -> Result<Sightings> {
        oxl_engine::ioc_sightings(case, id, cancel)
    }

    pub fn timeline(
        case: &Case,
        path: &Path,
        cancel: &AtomicBool,
        _hook: &mut dyn FnMut(),
    ) -> Result<ExportReport> {
        oxl_engine::export_timeline_csv(case, path, cancel)
    }

    pub fn workbook(
        case: &Case,
        path: &Path,
        cancel: &AtomicBool,
        _hook: &mut dyn FnMut(),
    ) -> Result<ExportReport> {
        oxl_engine::export_workbook(case, path, cancel)
    }

    pub fn report(
        case: &Case,
        path: &Path,
        cancel: &AtomicBool,
        _hook: &mut dyn FnMut(),
    ) -> Result<ExportReport> {
        oxl_engine::export_report(case, path, cancel)
    }
}

/// Account `id`'s logons, as ruling S6 reads the event log
/// (`logon_summary`).
pub fn logon_summary(state: &AppState, at: At<'_>, id: i64) -> Result<LogonSummary, CmdError> {
    at.read(state, |c| oxl_engine::logon_summary(c, id))
}

/// Up to `limit` history rows from `offset`, newest first: of one target
/// kind, or of one target of it (`history`).
pub fn history_list(
    state: &AppState,
    at: At<'_>,
    target_kind: Option<&str>,
    target_id: Option<&str>,
    limit: u32,
    offset: u64,
) -> Result<Listing<HistoryRow>, CmdError> {
    at.read(state, |c| {
        oxl_engine::history(c, target_kind, target_id, limit, offset)
    })
}

// ---- exports ----

/// An engine export of the case record, given the hook it calls once its
/// partial file is written, before its rename (`engine`).
type RecordExport =
    fn(&Case, &Path, &AtomicBool, &mut dyn FnMut()) -> oxl_engine::Result<ExportReport>;

/// The timeline as CSV, byte for byte as the case holds it
/// (`export_timeline_csv`).
pub fn export_timeline(
    state: &AppState,
    at: At<'_>,
    path: &Path,
) -> Result<ExportReport, CmdError> {
    export(state, at, path, engine::timeline)
}

/// The record as an XLSX workbook, every cell a string (`export_workbook`).
pub fn export_workbook(
    state: &AppState,
    at: At<'_>,
    path: &Path,
) -> Result<ExportReport, CmdError> {
    export(state, at, path, engine::workbook)
}

/// The record as a Markdown report, every value escaped (`export_report`).
pub fn export_report(state: &AppState, at: At<'_>, path: &Path) -> Result<ExportReport, CmdError> {
    export(state, at, path, engine::report)
}

/// Run `write` to `path` as a job (kind `export`, the file's name as its
/// label), as `grid_export` runs: on a pooled reader under `at`, with a
/// cancel flag registered with the query, answering `interrupted` (or
/// `superseded`) when stopped before the file is in place.
///
/// The record is the analyst's notes on a case, read whole and quickly, so
/// the export holds the write gate for all of it (waiting for a running
/// write first, as long as the query is not stopped): no load can make the
/// target part of a source between the engine's checks and its rename, and
/// the engine's own check just before the rename is the gate's re-check.
/// The engine reads the record in one read transaction on the reader, which
/// has none open. A write that arrives meanwhile answers `busy`.
fn export(
    state: &AppState,
    at: At<'_>,
    path: &Path,
    write: RecordExport,
) -> Result<ExportReport, CmdError> {
    state.current()?;
    state.announced(EXPORT_JOB, &job_label(path), || {
        let cancel = Arc::new(AtomicBool::new(false));
        state.read_to_completion(at.slot, at.query_id, cancel.clone(), |c| {
            state.hook(Point::BeforeGate);
            let gate = state.wait_for_gate(&cancel)?;
            let report = write(c, path, &cancel, &mut || {
                state.hook(Point::RecordBeforeRename)
            });
            drop(gate);
            report
        })
    })
}

/// The Tauri wrappers. The app's state is `app`, as in the grid's.
// Each wrapper's parameters are the command's IPC arguments, named as the
// window sends them; Tauri takes them one by one, not as a struct.
#[allow(clippy::too_many_arguments)]
pub mod commands {
    use super::{CaseRecord, EntryChanges, RecordQuery, RecordSpec};
    use crate::cmd::At;
    use crate::error::CmdError;
    use crate::state::AppState;
    use oxl_engine::{
        Candidate, Entry, EntryQuery, EntryStatus, Evidence, ExportReport, HistoryRow, Listing,
        LogonSummary, Note, PromoteOutcome, RecordKind, RecordStatus, RowOutcome, Sightings,
        TagInfo,
    };
    use std::path::PathBuf;
    use tauri::State;

    #[tauri::command(async)]
    pub fn mark_set(app: State<'_, AppState>, rows: Vec<i64>) -> Result<Vec<RowOutcome>, CmdError> {
        super::mark_set(&app, &rows)
    }

    #[tauri::command(async)]
    pub fn mark_clear(
        app: State<'_, AppState>,
        rows: Vec<i64>,
    ) -> Result<Vec<RowOutcome>, CmdError> {
        super::mark_clear(&app, &rows)
    }

    #[tauri::command(async)]
    pub fn tag_add(
        app: State<'_, AppState>,
        rows: Vec<i64>,
        tag: String,
    ) -> Result<Vec<RowOutcome>, CmdError> {
        super::tag_add(&app, &rows, &tag)
    }

    #[tauri::command(async)]
    pub fn tag_remove(
        app: State<'_, AppState>,
        rows: Vec<i64>,
        tag: String,
    ) -> Result<Vec<RowOutcome>, CmdError> {
        super::tag_remove(&app, &rows, &tag)
    }

    #[tauri::command(async)]
    pub fn tags_list(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        limit: u32,
        offset: u64,
    ) -> Result<Listing<TagInfo>, CmdError> {
        super::tags_list(&app, At::new(&query_id, &slot), limit, offset)
    }

    #[tauri::command(async)]
    pub fn note_add(app: State<'_, AppState>, row_id: i64, text: String) -> Result<Note, CmdError> {
        super::note_add(&app, row_id, &text)
    }

    #[tauri::command(async)]
    pub fn note_delete(app: State<'_, AppState>, id: i64) -> Result<(), CmdError> {
        super::note_delete(&app, id)
    }

    #[tauri::command(async)]
    pub fn notes_list(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        row_id: i64,
        limit: u32,
        offset: u64,
    ) -> Result<Listing<Note>, CmdError> {
        super::notes_list(&app, At::new(&query_id, &slot), row_id, limit, offset)
    }

    #[tauri::command(async)]
    pub fn entries_list(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        query: EntryQuery,
        limit: u32,
        offset: u64,
    ) -> Result<Listing<Entry>, CmdError> {
        super::entries_list(&app, At::new(&query_id, &slot), &query, limit, offset)
    }

    #[tauri::command(async)]
    pub fn entry_get(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        id: i64,
    ) -> Result<Entry, CmdError> {
        super::entry_get(&app, At::new(&query_id, &slot), id)
    }

    #[tauri::command(async)]
    pub fn entry_new(app: State<'_, AppState>, edit: EntryChanges) -> Result<Entry, CmdError> {
        super::entry_new(&app, &edit)
    }

    #[tauri::command(async)]
    pub fn entry_edit(
        app: State<'_, AppState>,
        id: i64,
        edit: EntryChanges,
    ) -> Result<Entry, CmdError> {
        super::entry_edit(&app, id, &edit)
    }

    #[tauri::command(async)]
    pub fn entry_status(
        app: State<'_, AppState>,
        id: i64,
        status: EntryStatus,
    ) -> Result<Entry, CmdError> {
        super::entry_status(&app, id, status)
    }

    #[tauri::command(async)]
    pub fn entry_delete(app: State<'_, AppState>, id: i64) -> Result<(), CmdError> {
        super::entry_delete(&app, id)
    }

    #[tauri::command(async)]
    pub fn rows_promote(
        app: State<'_, AppState>,
        rows: Vec<i64>,
    ) -> Result<Vec<PromoteOutcome>, CmdError> {
        super::rows_promote(&app, &rows)
    }

    #[tauri::command(async)]
    pub fn rows_attach(
        app: State<'_, AppState>,
        entry_id: i64,
        rows: Vec<i64>,
    ) -> Result<Vec<RowOutcome>, CmdError> {
        super::rows_attach(&app, entry_id, &rows)
    }

    #[tauri::command(async)]
    pub fn evidence_detach(
        app: State<'_, AppState>,
        entry_id: i64,
        row_id: i64,
    ) -> Result<(), CmdError> {
        super::evidence_detach(&app, entry_id, row_id)
    }

    #[tauri::command(async)]
    pub fn evidence_list(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        entry_id: i64,
    ) -> Result<Vec<Evidence>, CmdError> {
        super::evidence_list(&app, At::new(&query_id, &slot), entry_id)
    }

    #[tauri::command(async)]
    pub fn records_list(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        query: RecordQuery,
        limit: u32,
        offset: u64,
    ) -> Result<Listing<CaseRecord>, CmdError> {
        super::records_list(&app, At::new(&query_id, &slot), &query, limit, offset)
    }

    #[tauri::command(async)]
    pub fn record_get(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        kind: RecordKind,
        id: i64,
    ) -> Result<CaseRecord, CmdError> {
        super::record_get(&app, At::new(&query_id, &slot), kind, id)
    }

    #[tauri::command(async)]
    pub fn record_upsert(
        app: State<'_, AppState>,
        spec: RecordSpec,
    ) -> Result<CaseRecord, CmdError> {
        super::record_upsert(&app, &spec)
    }

    #[tauri::command(async)]
    pub fn record_delete(
        app: State<'_, AppState>,
        kind: RecordKind,
        id: i64,
    ) -> Result<(), CmdError> {
        super::record_delete(&app, kind, id)
    }

    #[tauri::command(async)]
    pub fn record_merge(
        app: State<'_, AppState>,
        kind: RecordKind,
        from_id: i64,
        into_id: i64,
    ) -> Result<(), CmdError> {
        super::record_merge(&app, kind, from_id, into_id)
    }

    #[tauri::command(async)]
    pub fn record_status(
        app: State<'_, AppState>,
        kind: RecordKind,
        id: i64,
        status: RecordStatus,
    ) -> Result<(), CmdError> {
        super::record_status(&app, kind, id, status)
    }

    #[tauri::command(async)]
    pub fn record_link(
        app: State<'_, AppState>,
        entry_id: i64,
        kind: RecordKind,
        record_id: i64,
    ) -> Result<(), CmdError> {
        super::record_link(&app, entry_id, kind, record_id)
    }

    #[tauri::command(async)]
    pub fn record_unlink(
        app: State<'_, AppState>,
        entry_id: i64,
        kind: RecordKind,
        record_id: i64,
    ) -> Result<(), CmdError> {
        super::record_unlink(&app, entry_id, kind, record_id)
    }

    #[tauri::command(async)]
    pub fn record_entries(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        kind: RecordKind,
        id: i64,
        limit: u32,
        offset: u64,
    ) -> Result<Listing<Entry>, CmdError> {
        let at = At::new(&query_id, &slot);
        super::record_entries(&app, at, kind, id, limit, offset)
    }

    #[tauri::command(async)]
    pub fn candidates(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        row_id: i64,
    ) -> Result<Vec<Candidate>, CmdError> {
        super::candidates(&app, At::new(&query_id, &slot), row_id)
    }

    #[tauri::command(async)]
    pub fn ioc_sightings(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        id: i64,
    ) -> Result<Sightings, CmdError> {
        super::ioc_sightings(&app, At::new(&query_id, &slot), id)
    }

    #[tauri::command(async)]
    pub fn logon_summary(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        id: i64,
    ) -> Result<LogonSummary, CmdError> {
        super::logon_summary(&app, At::new(&query_id, &slot), id)
    }

    #[tauri::command(async)]
    pub fn history_list(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        target_kind: Option<String>,
        target_id: Option<String>,
        limit: u32,
        offset: u64,
    ) -> Result<Listing<HistoryRow>, CmdError> {
        super::history_list(
            &app,
            At::new(&query_id, &slot),
            target_kind.as_deref(),
            target_id.as_deref(),
            limit,
            offset,
        )
    }

    #[tauri::command(async)]
    pub fn export_timeline(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        path: PathBuf,
    ) -> Result<ExportReport, CmdError> {
        super::export_timeline(&app, At::new(&query_id, &slot), &path)
    }

    #[tauri::command(async)]
    pub fn export_workbook(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        path: PathBuf,
    ) -> Result<ExportReport, CmdError> {
        super::export_workbook(&app, At::new(&query_id, &slot), &path)
    }

    #[tauri::command(async)]
    pub fn export_report(
        app: State<'_, AppState>,
        query_id: String,
        slot: String,
        path: PathBuf,
    ) -> Result<ExportReport, CmdError> {
        super::export_report(&app, At::new(&query_id, &slot), &path)
    }
}

/// What the export and sightings commands do while a test holds them at a
/// `Point`: a record export holds the write gate until its file is in place,
/// and a sightings read stopped while it runs answers its stop code.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::{case, load, query_cancel};
    use crate::events::NoSink;
    use crate::settings::SettingsStore;
    use oxl_engine::IocType;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    const LIMIT: Duration = Duration::from_secs(60);

    /// A case with the two-host run loaded, and its directory.
    fn setup() -> (tempfile::TempDir, Arc<AppState>) {
        let d = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new(SettingsStore::in_memory(), Arc::new(NoSink)));
        case::case_new(&state, &d.path().join("case.duckdb"), "WS01 triage").unwrap();
        load::load_run(&state, &oxl_fixtures::two_hosts(d.path())).unwrap();
        (d, state)
    }

    /// Hold the next pass through `point` until the returned sender sends;
    /// the receiver hears when it is reached.
    fn hold(state: &AppState, point: Point) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (reached_tx, reached) = mpsc::channel();
        let (go, go_rx) = mpsc::channel::<()>();
        state.on(point, move || {
            reached_tx.send(()).unwrap();
            go_rx.recv().unwrap();
        });
        (reached, go)
    }

    /// Run `work` on a thread of its own; its answer arrives on the receiver.
    fn spawn<T: Send + 'static>(
        state: &Arc<AppState>,
        work: impl FnOnce(&AppState) -> T + Send + 'static,
    ) -> mpsc::Receiver<T> {
        let (tx, rx) = mpsc::channel();
        let state = state.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work(&state));
        });
        rx
    }

    /// Register `n` passes through `point` that only count themselves, after
    /// whatever is registered there already; the count.
    fn count(state: &AppState, point: Point, n: usize) -> Arc<AtomicUsize> {
        let seen = Arc::new(AtomicUsize::new(0));
        for _ in 0..n {
            let seen = seen.clone();
            state.on(point, move || {
                seen.fetch_add(1, Ordering::SeqCst);
            });
        }
        seen
    }

    /// The relations the case holds, each of which a sweep searches.
    fn relation_count(state: &AppState) -> usize {
        let summary = case::case_summary(state, At::new("sum", "summary")).unwrap();
        assert!(
            summary.relations.len() >= 2,
            "a sweep needs relations to stop between"
        );
        summary.relations.len()
    }

    /// Held once its partial file is written and before its rename, each
    /// export still holds the write gate: a write answers `busy`, the partial
    /// file is there and the target is not. Released, the file lands, the
    /// partial is gone, and writes run again.
    #[test]
    fn a_record_export_holds_the_write_gate_until_its_file_lands() {
        type Export = fn(&AppState, At<'_>, &Path) -> Result<ExportReport, CmdError>;
        let exports: [(&str, Export); 3] = [
            ("timeline.csv", export_timeline),
            ("record.xlsx", export_workbook),
            ("record.md", export_report),
        ];
        for (name, export) in exports {
            let (d, state) = setup();
            entry_new(
                &state,
                &EntryChanges {
                    title: Some("Wiped logs".into()),
                    ..EntryChanges::default()
                },
            )
            .unwrap();
            let (reached, go) = hold(&state, Point::RecordBeforeRename);
            let out = d.path().join(name);
            let partial = d.path().join(format!("{name}.partial"));
            let done = {
                let out = out.clone();
                spawn(&state, move |s| export(s, At::new("x1", "export"), &out))
            };
            reached
                .recv_timeout(LIMIT)
                .expect("the export wrote its partial file");
            assert!(partial.is_file(), "{name}: no partial file at the pause");
            assert!(!out.exists(), "{name} landed before its rename");
            assert_eq!(mark_set(&state, &[1]).unwrap_err().code, "busy", "{name}");
            let note = note_add(&state, 1, "meanwhile");
            assert_eq!(note.unwrap_err().code, "busy", "{name}");
            go.send(()).unwrap();
            let report = done.recv_timeout(LIMIT).expect("the export answered");
            assert_eq!(report.unwrap().path, out, "{name}");
            assert!(out.is_file() && !partial.exists(), "{name}");
            mark_set(&state, &[1]).expect("a write runs once the export is done");
        }
    }

    /// A sweep held after its first relation, then superseded by a newer one
    /// on its slot, stops there: no other relation is searched, and it
    /// answers `superseded`, while the newer one answers the sightings.
    #[test]
    fn a_newer_sightings_query_supersedes_one_in_flight() {
        let (_d, state) = setup();
        let ioc = ioc_id(&state);
        let relations = relation_count(&state);
        let (reached, go) = hold(&state, Point::SightingsRelation);
        let first = spawn(&state, move |s| {
            ioc_sightings(s, At::new("s1", "sightings"), ioc)
        });
        reached
            .recv_timeout(LIMIT)
            .expect("the first sweep searched a relation");
        let second = ioc_sightings(&state, At::new("s2", "sightings"), ioc).unwrap();
        assert!(second.hits >= 2, "{second:?}");
        let after = count(&state, Point::SightingsRelation, relations);
        go.send(()).unwrap();
        let first = first.recv_timeout(LIMIT).expect("the first sweep answered");
        assert_eq!(first.unwrap_err().code, "superseded");
        assert_eq!(
            after.load(Ordering::SeqCst),
            0,
            "the superseded sweep searched on, past 1 of {relations} relations"
        );
    }

    /// A sweep held after its first relation, then cancelled by its id,
    /// stops there and answers `interrupted`.
    #[test]
    fn a_sightings_query_cancelled_in_flight_is_interrupted() {
        let (_d, state) = setup();
        let ioc = ioc_id(&state);
        let relations = relation_count(&state);
        let (reached, go) = hold(&state, Point::SightingsRelation);
        let first = spawn(&state, move |s| {
            ioc_sightings(s, At::new("s1", "sightings"), ioc)
        });
        reached
            .recv_timeout(LIMIT)
            .expect("the sweep searched a relation");
        let after = count(&state, Point::SightingsRelation, relations);
        query_cancel(&state, &["s1".to_string()]).unwrap();
        go.send(()).unwrap();
        let first = first.recv_timeout(LIMIT).expect("the sweep answered");
        assert_eq!(first.unwrap_err().code, "interrupted");
        assert_eq!(
            after.load(Ordering::SeqCst),
            0,
            "the cancelled sweep searched on, past 1 of {relations} relations"
        );
    }

    /// An IOC for the address both hosts' events name.
    fn ioc_id(state: &AppState) -> i64 {
        let spec = RecordSpec::Ioc(IocSpec {
            id: None,
            ioc_type: IocType::Ip,
            value: "203.0.113.7".into(),
            tool: None,
            attack_ids: Vec::new(),
            note: String::new(),
        });
        match record_upsert(state, &spec).unwrap() {
            CaseRecord::Ioc(i) => i.id,
            other => panic!("an IOC spec made {other:?}"),
        }
    }
}
