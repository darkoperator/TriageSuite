//! Exports of the case record (job 13): the timeline as CSV, a workbook of
//! the Timeline, Hosts, Accounts and IOCs sheets, and a Markdown report.
//!
//! The four tables are read once, in one shape, and each export writes that
//! shape its own way:
//!
//! - **CSV** (the Timeline only): every cell byte-for-byte as the case holds
//!   it, as the grid's CSV export writes its cells; a value a spreadsheet
//!   would read as a formula is only quoted, because the file is data.
//! - **Workbook**: every cell, the header too, written with `write_string`,
//!   so a value that came from evidence is never a formula, a number or a
//!   link (ruling S11). A cell past the 32,767 characters a sheet cell
//!   holds is cut, saying so; nothing the analyst types reaches that
//!   length, only a list of many hosts or evidence rows.
//! - **Report**: a title, the case's name and the export time, then one
//!   table per sheet, every value from the case escaped so that it renders
//!   as the text it is (`md`).
//!
//! The Timeline is in its own order, by time with the undated (ruling S8,
//! "needs a time") after every dated entry. Evidence is named by relation
//! and row id (`evtxtriage_events#41`), never by its snapshot: the export
//! is the analyst's record, and the rows are in the case to re-read.
//!
//! Each export goes through `check_export_target` (never the case file, its
//! log, or inside a loaded source) and is written whole or not at all: into
//! `<path>.partial`, created new, renamed over `path` once complete, as the
//! grid's CSV export is. The record is read before anything is written, a
//! page of `MAX_LIMIT` at a time with the cancel flag read between pages;
//! it is the analyst's notes on a case, not its rows, so it is held whole.

use super::{
    accounts, entries, has_records, hosts, iocs, AccountRecord, Entry, EntryQuery, HostRecord,
    IocQuery, IocRecord,
};
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::listing::Listing;
use crate::query::MAX_LIMIT;
use crate::time::now_iso;
use crate::view::{check_export_target, create_export_partial, ExportReport};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// The most characters a workbook cell holds (Excel's limit, which
/// `rust_xlsxwriter` enforces).
const MAX_CELL: usize = 32_767;

/// The Timeline's columns, in the order every export writes them.
const TIMELINE: &[&str] = &[
    "id",
    "time",
    "time_source",
    "needs_time",
    "title",
    "description",
    "tactic",
    "technique_ids",
    "status",
    "hosts",
    "evidence_rows",
    "evidence",
    "created_by",
    "created_at",
    "modified_by",
    "modified_at",
    "reviewed_by",
    "agent_added",
];

const HOSTS: &[&str] = &[
    "id",
    "name",
    "role",
    "note",
    "status",
    "entries",
    "first_seen",
    "last_seen",
    "agent_added",
    "created_by",
    "created_at",
    "modified_by",
    "modified_at",
];

const ACCOUNTS: &[&str] = &[
    "id",
    "name",
    "domain",
    "sid",
    "description",
    "privilege",
    "note",
    "status",
    "entries",
    "first_seen",
    "last_seen",
    "hosts_seen",
    "agent_added",
    "created_by",
    "created_at",
    "modified_by",
    "modified_at",
];

const IOCS: &[&str] = &[
    "id",
    "type",
    "value",
    "tool",
    "attack_ids",
    "note",
    "status",
    "entries",
    "first_seen",
    "last_seen",
    "hosts_seen",
    "agent_added",
    "created_by",
    "created_at",
    "modified_by",
    "modified_at",
];

/// One sheet of the record: its name, its header, and its rows as text.
struct Table {
    name: &'static str,
    header: &'static [&'static str],
    rows: Vec<Vec<String>>,
}

/// Write the case's live timeline entries to `path` as CSV, in the
/// Timeline's order (by time, the undated last, then by id): a header of
/// `TIMELINE`'s names, then one record per entry, each field as the case
/// holds it (`id` as `E-12`, a list joined by `; `, a flag `true` or
/// `false`, a missing value empty). `rows` counts the entries.
///
/// `Invalid` for a `path` that is a folder or names no file, anything
/// already at `<path>.partial` (left as it is), and when `cancel` is set
/// (read between pages and before the rename; the message is `cancelled`).
/// `NotFound` for a folder that does not exist. `Refused` for the case
/// file, its log, or a path inside a loaded source. On any failure the
/// partial file is removed and what was at `path` is left as it was.
pub fn export_timeline_csv(case: &Case, path: &Path, cancel: &AtomicBool) -> Result<ExportReport> {
    check_export_target(case, path)?;
    let timeline = timeline(case, cancel)?;
    write_whole(case, path, cancel, |file| {
        let mut out = csv::Writer::from_writer(file);
        out.write_record(timeline.header).map_err(csv_error)?;
        for row in &timeline.rows {
            out.write_record(row).map_err(csv_error)?;
        }
        out.flush()?;
        Ok(count(&timeline.rows))
    })
}

/// Write the case record to `path` as an XLSX workbook of four sheets,
/// `Timeline`, `Hosts`, `Accounts` and `IOCs`, each a header row and then
/// one row per live entry or record, every cell a string (never a
/// formula). `rows` counts the data rows of every sheet. Fails as
/// `export_timeline_csv` does, and `Invalid` for a sheet past a
/// workbook's 1,048,575 data rows.
pub fn export_workbook(case: &Case, path: &Path, cancel: &AtomicBool) -> Result<ExportReport> {
    check_export_target(case, path)?;
    let tables = record_tables(case, cancel)?;
    let mut book = rust_xlsxwriter::Workbook::new();
    for table in &tables {
        let sheet = book.add_worksheet();
        sheet.set_name(table.name).map_err(xlsx_error)?;
        for (col, name) in table.header.iter().enumerate() {
            sheet
                .write_string(0, col_num(col)?, *name)
                .map_err(xlsx_error)?;
        }
        for (at, row) in table.rows.iter().enumerate() {
            let row_num = u32::try_from(at + 1)
                .ok()
                .filter(|&r| r < 1_048_576)
                .ok_or_else(|| too_many_rows(table.name))?;
            for (col, cell) in row.iter().enumerate() {
                sheet
                    .write_string(row_num, col_num(col)?, clip_cell(cell))
                    .map_err(|e| match e {
                        rust_xlsxwriter::XlsxError::RowColumnLimitError => {
                            too_many_rows(table.name)
                        }
                        other => xlsx_error(other),
                    })?;
            }
        }
    }
    write_whole(case, path, cancel, |file| {
        book.save_to_writer(&mut *file).map_err(xlsx_error)?;
        Ok(tables.iter().map(|t| count(&t.rows)).sum())
    })
}

/// Write the case record to `path` as a Markdown report: the title `Case
/// record`, the case's name and the export time (UTC), then a section per
/// sheet in the order Timeline, Hosts, Accounts, IOCs, each one table
/// (the Timeline's undated entries last) or a line saying there is none.
/// Every value from the case is escaped (`md`). `rows` counts the data
/// rows of every table. Fails as `export_timeline_csv` does.
pub fn export_report(case: &Case, path: &Path, cancel: &AtomicBool) -> Result<ExportReport> {
    check_export_target(case, path)?;
    let name = case.info()?.name;
    let tables = record_tables(case, cancel)?;
    let mut text = String::new();
    text.push_str("# Case record\n\n");
    text.push_str(&format!("Case: {}\n\n", md(&name)));
    text.push_str(&format!("Exported: {} (UTC)\n", now_iso()));
    for table in &tables {
        text.push_str(&format!("\n## {}\n\n", table.name));
        if table.rows.is_empty() {
            text.push_str(&format!("No {}.\n", none_of(table.name)));
            continue;
        }
        if table.name == "Timeline" {
            text.push_str("In time order; entries that need a time are listed last, by id.\n\n");
        }
        markdown_row(&mut text, table.header.iter().map(|h| md(h)));
        markdown_row(&mut text, table.header.iter().map(|_| "---".to_string()));
        for row in &table.rows {
            markdown_row(&mut text, row.iter().map(|c| md(c)));
        }
    }
    write_whole(case, path, cancel, |file| {
        file.write_all(text.as_bytes())?;
        Ok(tables.iter().map(|t| count(&t.rows)).sum())
    })
}

/// What a section with no rows says it has none of.
fn none_of(table: &str) -> &'static str {
    match table {
        "Timeline" => "timeline entries",
        "Hosts" => "host records",
        "Accounts" => "account records",
        _ => "IOC records",
    }
}

/// One Markdown table row of `cells`, each already escaped (`md`) or the
/// engine's own text (the rule under the header).
fn markdown_row(text: &mut String, cells: impl Iterator<Item = String>) {
    text.push('|');
    for cell in cells {
        text.push(' ');
        text.push_str(&cell);
        text.push_str(" |");
    }
    text.push('\n');
}

/// `text` as Markdown that renders as that text (ruling S11): each of `\`,
/// `` ` ``, `|`, `<`, `>`, `[`, `]`, `*` and `_` escaped with a backslash,
/// so that no value opens a code span, an emphasis, a link, a raw HTML tag
/// or a new table cell; every line break (`\r\n`, `\r`, `\n`) and any other
/// control character a space, so that a value stays on its table row.
/// Total: any text gives an answer. Applying it twice escapes the
/// backslashes the first added, so it is applied once, to the case's text.
pub(crate) fn md(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' | '`' | '|' | '<' | '>' | '[' | ']' | '*' | '_' => {
                out.push('\\');
                out.push(c);
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(' ');
            }
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// `cell`, or for one past `MAX_CELL` characters its start and a note of
/// how long it is in the case, within `MAX_CELL`.
fn clip_cell(cell: &str) -> Cow<'_, str> {
    let n = cell.chars().count();
    if n <= MAX_CELL {
        return Cow::Borrowed(cell);
    }
    let note = format!(" … (cut: {n} characters in the case)");
    let keep = MAX_CELL - note.chars().count();
    let cut = cell.char_indices().nth(keep).map_or(cell.len(), |(i, _)| i);
    Cow::Owned(format!("{}{note}", &cell[..cut]))
}

fn col_num(col: usize) -> Result<u16> {
    // Every table has fewer than twenty columns.
    u16::try_from(col).map_err(|_| EngineError::Internal(format!("a sheet has no column {col}")))
}

fn too_many_rows(sheet: &str) -> EngineError {
    EngineError::Invalid(format!(
        "the {sheet} sheet has more rows than a workbook holds (1,048,575); export the \
         timeline as CSV instead"
    ))
}

fn xlsx_error(e: rust_xlsxwriter::XlsxError) -> EngineError {
    match e {
        rust_xlsxwriter::XlsxError::IoError(e) => EngineError::Io(e),
        other => EngineError::Internal(format!("the workbook could not be written: {other}")),
    }
}

fn csv_error(e: csv::Error) -> EngineError {
    EngineError::Io(e.into())
}

fn count<T>(rows: &[T]) -> u64 {
    u64::try_from(rows.len()).unwrap_or(u64::MAX)
}

fn cancelled() -> EngineError {
    EngineError::Invalid("cancelled".to_string())
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    Ok(())
}

/// Write `path` whole or not at all: `write` fills `<path>.partial`,
/// created new beside it, which is synced, the target checked again (a
/// source loaded meanwhile may now hold it) and the cancel flag read, then
/// renamed over `path`. On any failure the partial file, this export's
/// own, is removed and what was at `path` is left as it was.
fn write_whole(
    case: &Case,
    path: &Path,
    cancel: &AtomicBool,
    write: impl FnOnce(&mut File) -> Result<u64>,
) -> Result<ExportReport> {
    check_cancel(cancel)?;
    let (partial, mut file) = create_export_partial(path)?;
    let written = write(&mut file).and_then(|rows| {
        file.sync_all()?;
        drop(file);
        check_cancel(cancel)?;
        check_export_target(case, path)?;
        std::fs::rename(&partial, path)?;
        Ok(rows)
    });
    match written {
        Ok(rows) => Ok(ExportReport {
            rows,
            path: path.to_path_buf(),
        }),
        Err(e) => {
            // The error that ended the export is the one worth reporting.
            let _ = std::fs::remove_file(&partial);
            Err(e)
        }
    }
}

/// Every page of a listing, `MAX_LIMIT` at a time from the start, the
/// cancel flag read before each.
fn all<T>(
    cancel: &AtomicBool,
    mut page: impl FnMut(u32, u64) -> Result<Listing<T>>,
) -> Result<Vec<T>> {
    let mut out = Vec::new();
    loop {
        check_cancel(cancel)?;
        let offset = u64::try_from(out.len()).unwrap_or(u64::MAX);
        let listing = page(MAX_LIMIT, offset)?;
        out.extend(listing.items);
        if !listing.truncated {
            return Ok(out);
        }
    }
}

/// The four tables, in the order the workbook and the report write them.
fn record_tables(case: &Case, cancel: &AtomicBool) -> Result<[Table; 4]> {
    let timeline = timeline(case, cancel)?;
    let hosts: Vec<HostRecord> = all(cancel, |limit, offset| hosts(case, limit, offset))?;
    let accounts: Vec<AccountRecord> = all(cancel, |limit, offset| accounts(case, limit, offset))?;
    let iocs: Vec<IocRecord> = all(cancel, |limit, offset| {
        iocs(case, &IocQuery::default(), limit, offset)
    })?;
    Ok([
        timeline,
        Table {
            name: "Hosts",
            header: HOSTS,
            rows: hosts.iter().map(host_row).collect(),
        },
        Table {
            name: "Accounts",
            header: ACCOUNTS,
            rows: accounts.iter().map(account_row).collect(),
        },
        Table {
            name: "IOCs",
            header: IOCS,
            rows: iocs.iter().map(ioc_row).collect(),
        },
    ])
}

/// The Timeline table: the live entries in the Timeline's order, each with
/// the rows attached to it as evidence, in the order they were attached.
fn timeline(case: &Case, cancel: &AtomicBool) -> Result<Table> {
    let listed: Vec<Entry> = all(cancel, |limit, offset| {
        entries(case, &EntryQuery::default(), limit, offset)
    })?;
    let evidence = evidence_refs(case)?;
    let rows = listed
        .iter()
        .map(|e| entry_row(e, evidence.get(&e.id).map(Vec::as_slice).unwrap_or(&[])))
        .collect();
    Ok(Table {
        name: "Timeline",
        header: TIMELINE,
        rows,
    })
}

/// Per entry, its evidence rows as `<relation>#<row id>`, in the order they
/// were attached. None in a case without the record tables.
fn evidence_refs(case: &Case) -> Result<HashMap<i64, Vec<String>>> {
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    if !has_records(case) {
        return Ok(out);
    }
    let mut stmt = case.conn().prepare(
        "SELECT entry_id, relation, row_id FROM ox.evidence \
         ORDER BY entry_id, attached_at, row_id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let entry: i64 = row.get(0)?;
        let relation: String = row.get(1)?;
        let row_id: i64 = row.get(2)?;
        out.entry(entry)
            .or_default()
            .push(format!("{relation}#{row_id}"));
    }
    Ok(out)
}

fn opt(value: &Option<String>) -> String {
    value.clone().unwrap_or_default()
}

fn flag(value: bool) -> String {
    value.to_string()
}

fn list(items: &[String]) -> String {
    items.join("; ")
}

fn entry_row(e: &Entry, evidence: &[String]) -> Vec<String> {
    vec![
        format!("E-{}", e.id),
        opt(&e.time),
        opt(&e.time_source),
        flag(e.needs_time),
        e.title.clone(),
        e.description.clone(),
        opt(&e.tactic),
        list(&e.technique_ids),
        e.status.as_str().to_string(),
        list(&e.hosts),
        e.evidence_rows.to_string(),
        list(evidence),
        e.created_by.clone(),
        e.created_at.clone(),
        e.modified_by.clone(),
        e.modified_at.clone(),
        opt(&e.reviewed_by),
        flag(e.agent_added),
    ]
}

fn host_row(h: &HostRecord) -> Vec<String> {
    vec![
        format!("H-{}", h.id),
        h.name.clone(),
        h.role.clone(),
        h.note.clone(),
        h.status.as_str().to_string(),
        h.entries.to_string(),
        opt(&h.first_seen),
        opt(&h.last_seen),
        flag(h.agent_added),
        h.created_by.clone(),
        h.created_at.clone(),
        h.modified_by.clone(),
        h.modified_at.clone(),
    ]
}

fn account_row(a: &AccountRecord) -> Vec<String> {
    vec![
        format!("A-{}", a.id),
        a.name.clone(),
        opt(&a.domain),
        opt(&a.sid),
        a.description.clone(),
        a.privilege.as_str().to_string(),
        a.note.clone(),
        a.status.as_str().to_string(),
        a.entries.to_string(),
        opt(&a.first_seen),
        opt(&a.last_seen),
        list(&a.hosts_seen),
        flag(a.agent_added),
        a.created_by.clone(),
        a.created_at.clone(),
        a.modified_by.clone(),
        a.modified_at.clone(),
    ]
}

fn ioc_row(i: &IocRecord) -> Vec<String> {
    vec![
        format!("I-{}", i.id),
        i.ioc_type.as_str().to_string(),
        i.value.clone(),
        opt(&i.tool),
        list(&i.attack_ids),
        i.note.clone(),
        i.status.as_str().to_string(),
        i.entries.to_string(),
        opt(&i.first_seen),
        opt(&i.last_seen),
        list(&i.hosts_seen),
        flag(i.agent_added),
        i.created_by.clone(),
        i.created_at.clone(),
        i.modified_by.clone(),
        i.modified_at.clone(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md_escapes_every_markup_character() {
        assert_eq!(
            md("\\`|<>[]*_"),
            "\\\\\\`\\|\\<\\>\\[\\]\\*\\_",
            "each of S11's characters, and only one backslash each"
        );
        assert_eq!(md("a\r\nb\rc\nd\u{0}e\tf"), "a b c d e f");
        assert_eq!(md("plain text, 1.5 # - +"), "plain text, 1.5 # - +");
        assert_eq!(md(""), "");
    }

    #[test]
    fn a_cell_past_the_limit_is_cut_within_it() {
        let long = "é".repeat(MAX_CELL + 10);
        let cut = clip_cell(&long);
        assert_eq!(cut.chars().count(), MAX_CELL);
        assert!(cut.ends_with(&format!("(cut: {} characters in the case)", MAX_CELL + 10)));
        let fits = "x".repeat(MAX_CELL);
        assert!(matches!(clip_cell(&fits), Cow::Borrowed(_)));
    }
}
