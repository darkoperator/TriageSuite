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
//! grid's CSV export is. The record is read a page of `MAX_LIMIT` at a time
//! inside one read transaction, and each row is written as its page is
//! read (the evidence for a page's entries only), so the CSV and the report
//! hold one page at a time; the workbook's sheets are held by the workbook
//! writer until it is saved. The cancel flag is read before each page,
//! each row and each evidence row, and before the rename.

use super::{
    accounts, entries, has_records, hosts, iocs, AccountRecord, Entry, EntryQuery, HostRecord,
    IocQuery, IocRecord,
};
use crate::annot::id_list;
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

/// The pauses a test puts between an export's steps; each a no-op for a
/// real export.
struct Hooks<'a> {
    /// Once the first page of entries is read, before its evidence.
    after_entries: &'a mut dyn FnMut(),
    /// As each evidence row is read.
    on_evidence: &'a mut dyn FnMut(),
    /// Once the partial file is written and closed, before its last checks
    /// and the rename.
    before_rename: &'a mut dyn FnMut(),
}

/// Write the case's live timeline entries to `path` as CSV, in the
/// Timeline's order (by time, the undated last, then by id): a header of
/// `TIMELINE`'s names, then one record per entry, each field as the case
/// holds it (`id` as `E-12`, a list joined by `; `, a flag `true` or
/// `false`, a missing value empty). `rows` counts the entries.
///
/// `Invalid` for a `path` that is a folder or names no file, anything
/// already at `<path>.partial` (left as it is), and when `cancel` is set
/// (read before each page, each entry and each evidence row, and before
/// the rename; the message is `cancelled`). `NotFound` for a folder that
/// does not exist. `Refused` for the case file, its log, or a path inside
/// a loaded source. On any failure the partial file is removed and what
/// was at `path` is left as it was.
pub fn export_timeline_csv(case: &Case, path: &Path, cancel: &AtomicBool) -> Result<ExportReport> {
    timeline_csv(
        case,
        path,
        cancel,
        Hooks {
            after_entries: &mut || {},
            on_evidence: &mut || {},
            before_rename: &mut || {},
        },
    )
}

/// `export_timeline_csv`, calling `pause` between two of its reads (once
/// the first page of entries is read, before its evidence), for a test to
/// commit a change from another connection there. Built only with
/// `test-support`.
#[cfg(feature = "test-support")]
pub fn export_timeline_csv_paused(
    case: &Case,
    path: &Path,
    cancel: &AtomicBool,
    pause: &mut dyn FnMut(),
) -> Result<ExportReport> {
    timeline_csv(
        case,
        path,
        cancel,
        Hooks {
            after_entries: pause,
            on_evidence: &mut || {},
            before_rename: &mut || {},
        },
    )
}

/// `export_timeline_csv`, calling `hook` as it reads each evidence row,
/// for a test to set the cancel flag there. Built only with
/// `test-support`.
#[cfg(feature = "test-support")]
pub fn export_timeline_csv_on_evidence(
    case: &Case,
    path: &Path,
    cancel: &AtomicBool,
    hook: &mut dyn FnMut(),
) -> Result<ExportReport> {
    timeline_csv(
        case,
        path,
        cancel,
        Hooks {
            after_entries: &mut || {},
            on_evidence: hook,
            before_rename: &mut || {},
        },
    )
}

/// `export_timeline_csv`, calling `hook` once its partial file is written
/// and closed, before its last checks and the rename, for a test to look at
/// what holds then. Built only with `test-support`.
#[cfg(feature = "test-support")]
pub fn export_timeline_csv_before_rename(
    case: &Case,
    path: &Path,
    cancel: &AtomicBool,
    hook: &mut dyn FnMut(),
) -> Result<ExportReport> {
    timeline_csv(
        case,
        path,
        cancel,
        Hooks {
            after_entries: &mut || {},
            on_evidence: &mut || {},
            before_rename: hook,
        },
    )
}

fn timeline_csv(
    case: &Case,
    path: &Path,
    cancel: &AtomicBool,
    hooks: Hooks,
) -> Result<ExportReport> {
    let Hooks {
        after_entries,
        on_evidence,
        before_rename,
    } = hooks;
    write_whole(case, path, cancel, before_rename, |case, file| {
        let mut sink = CsvSink(csv::Writer::from_writer(file));
        let rows = stream_timeline(case, cancel, after_entries, on_evidence, &mut sink)?;
        sink.0.flush()?;
        Ok(rows)
    })
}

/// Write the case record to `path` as an XLSX workbook of four sheets,
/// `Timeline`, `Hosts`, `Accounts` and `IOCs`, each a header row and then
/// one row per live entry or record, every cell a string (never a
/// formula). `rows` counts the data rows of every sheet. Fails as
/// `export_timeline_csv` does, and `Invalid` for a sheet past a
/// workbook's 1,048,575 data rows.
pub fn export_workbook(case: &Case, path: &Path, cancel: &AtomicBool) -> Result<ExportReport> {
    workbook(
        case,
        path,
        cancel,
        Hooks {
            after_entries: &mut || {},
            on_evidence: &mut || {},
            before_rename: &mut || {},
        },
    )
}

/// `export_workbook`, calling `pause` where `export_timeline_csv_paused`
/// does. Built only with `test-support`.
#[cfg(feature = "test-support")]
pub fn export_workbook_paused(
    case: &Case,
    path: &Path,
    cancel: &AtomicBool,
    pause: &mut dyn FnMut(),
) -> Result<ExportReport> {
    workbook(
        case,
        path,
        cancel,
        Hooks {
            after_entries: pause,
            on_evidence: &mut || {},
            before_rename: &mut || {},
        },
    )
}

/// `export_workbook`, calling `hook` where
/// `export_timeline_csv_on_evidence` does. Built only with `test-support`.
#[cfg(feature = "test-support")]
pub fn export_workbook_on_evidence(
    case: &Case,
    path: &Path,
    cancel: &AtomicBool,
    hook: &mut dyn FnMut(),
) -> Result<ExportReport> {
    workbook(
        case,
        path,
        cancel,
        Hooks {
            after_entries: &mut || {},
            on_evidence: hook,
            before_rename: &mut || {},
        },
    )
}

/// `export_workbook`, calling `hook` where
/// `export_timeline_csv_before_rename` does. Built only with `test-support`.
#[cfg(feature = "test-support")]
pub fn export_workbook_before_rename(
    case: &Case,
    path: &Path,
    cancel: &AtomicBool,
    hook: &mut dyn FnMut(),
) -> Result<ExportReport> {
    workbook(
        case,
        path,
        cancel,
        Hooks {
            after_entries: &mut || {},
            on_evidence: &mut || {},
            before_rename: hook,
        },
    )
}

fn workbook(case: &Case, path: &Path, cancel: &AtomicBool, hooks: Hooks) -> Result<ExportReport> {
    let Hooks {
        after_entries,
        on_evidence,
        before_rename,
    } = hooks;
    write_whole(case, path, cancel, before_rename, |case, file| {
        let mut sink = BookSink {
            book: rust_xlsxwriter::Workbook::new(),
            sheet: None,
        };
        let rows = stream_tables(case, cancel, after_entries, on_evidence, &mut sink)?;
        check_cancel(cancel)?;
        sink.book.save_to_writer(&mut *file).map_err(xlsx_error)?;
        Ok(rows)
    })
}

/// Write the case record to `path` as a Markdown report: the title `Case
/// record`, the case's name and the export time (UTC), then a section per
/// sheet in the order Timeline, Hosts, Accounts, IOCs, each one table
/// (the Timeline's undated entries last) or a line saying there is none.
/// Every value from the case is escaped (`md`). `rows` counts the data
/// rows of every table. Fails as `export_timeline_csv` does.
pub fn export_report(case: &Case, path: &Path, cancel: &AtomicBool) -> Result<ExportReport> {
    report(
        case,
        path,
        cancel,
        Hooks {
            after_entries: &mut || {},
            on_evidence: &mut || {},
            before_rename: &mut || {},
        },
    )
}

/// `export_report`, calling `pause` where `export_timeline_csv_paused`
/// does. Built only with `test-support`.
#[cfg(feature = "test-support")]
pub fn export_report_paused(
    case: &Case,
    path: &Path,
    cancel: &AtomicBool,
    pause: &mut dyn FnMut(),
) -> Result<ExportReport> {
    report(
        case,
        path,
        cancel,
        Hooks {
            after_entries: pause,
            on_evidence: &mut || {},
            before_rename: &mut || {},
        },
    )
}

/// `export_report`, calling `hook` where
/// `export_timeline_csv_on_evidence` does. Built only with `test-support`.
#[cfg(feature = "test-support")]
pub fn export_report_on_evidence(
    case: &Case,
    path: &Path,
    cancel: &AtomicBool,
    hook: &mut dyn FnMut(),
) -> Result<ExportReport> {
    report(
        case,
        path,
        cancel,
        Hooks {
            after_entries: &mut || {},
            on_evidence: hook,
            before_rename: &mut || {},
        },
    )
}

/// `export_report`, calling `hook` where
/// `export_timeline_csv_before_rename` does. Built only with `test-support`.
#[cfg(feature = "test-support")]
pub fn export_report_before_rename(
    case: &Case,
    path: &Path,
    cancel: &AtomicBool,
    hook: &mut dyn FnMut(),
) -> Result<ExportReport> {
    report(
        case,
        path,
        cancel,
        Hooks {
            after_entries: &mut || {},
            on_evidence: &mut || {},
            before_rename: hook,
        },
    )
}

fn report(case: &Case, path: &Path, cancel: &AtomicBool, hooks: Hooks) -> Result<ExportReport> {
    let Hooks {
        after_entries,
        on_evidence,
        before_rename,
    } = hooks;
    write_whole(case, path, cancel, before_rename, |case, file| {
        let name = case.info()?.name;
        let mut out = std::io::BufWriter::new(file);
        out.write_all(b"# Case record\n\n")?;
        out.write_all(format!("Case: {}\n\n", md(&name)).as_bytes())?;
        out.write_all(format!("Exported: {} (UTC)\n", now_iso()).as_bytes())?;
        let mut sink = ReportSink {
            out,
            table: None,
            rows: 0,
        };
        let rows = stream_tables(case, cancel, after_entries, on_evidence, &mut sink)?;
        sink.out.flush()?;
        Ok(rows)
    })
}

/// Where an export's rows go as they are read: a table starts, its rows
/// follow in order, and it ends. Nothing is held back to be written later
/// but what the format itself holds (the workbook's sheets).
trait Sink {
    fn start(&mut self, name: &'static str, header: &'static [&'static str]) -> Result<()>;
    fn row(&mut self, row: &[String]) -> Result<()>;
    fn end(&mut self) -> Result<()>;
}

/// The Timeline as CSV: the header, then each row as it is read.
struct CsvSink<W: Write>(csv::Writer<W>);

impl<W: Write> Sink for CsvSink<W> {
    fn start(&mut self, _name: &'static str, header: &'static [&'static str]) -> Result<()> {
        self.0.write_record(header).map_err(csv_error)
    }
    fn row(&mut self, row: &[String]) -> Result<()> {
        self.0.write_record(row).map_err(csv_error)
    }
    fn end(&mut self) -> Result<()> {
        Ok(())
    }
}

/// The workbook: a sheet per table, each cell written as it is read.
struct BookSink {
    book: rust_xlsxwriter::Workbook,
    /// The sheet being written: its index, its name and its next row.
    sheet: Option<(usize, &'static str, u32)>,
}

impl Sink for BookSink {
    fn start(&mut self, name: &'static str, header: &'static [&'static str]) -> Result<()> {
        let sheet = self.book.add_worksheet();
        sheet.set_name(name).map_err(xlsx_error)?;
        for (col, cell) in header.iter().enumerate() {
            sheet
                .write_string(0, col_num(col)?, *cell)
                .map_err(xlsx_error)?;
        }
        let index = self.book.worksheets().len() - 1;
        self.sheet = Some((index, name, 1));
        Ok(())
    }
    fn row(&mut self, row: &[String]) -> Result<()> {
        let Some((index, name, row_num)) = self.sheet else {
            return Err(EngineError::Internal(
                "a workbook row came before its sheet".to_string(),
            ));
        };
        if row_num >= 1_048_576 {
            return Err(too_many_rows(name));
        }
        let sheet = self.book.worksheet_from_index(index).map_err(xlsx_error)?;
        for (col, cell) in row.iter().enumerate() {
            sheet
                .write_string(row_num, col_num(col)?, clip_cell(cell))
                .map_err(|e| match e {
                    rust_xlsxwriter::XlsxError::RowColumnLimitError => too_many_rows(name),
                    other => xlsx_error(other),
                })?;
        }
        self.sheet = Some((index, name, row_num + 1));
        Ok(())
    }
    fn end(&mut self) -> Result<()> {
        self.sheet = None;
        Ok(())
    }
}

/// The report: a section per table, its header written with its first
/// row, or a line saying there is none.
struct ReportSink<W: Write> {
    out: W,
    /// The table being written: its name and its header.
    table: Option<(&'static str, &'static [&'static str])>,
    /// The rows of it written so far.
    rows: u64,
}

impl<W: Write> Sink for ReportSink<W> {
    fn start(&mut self, name: &'static str, header: &'static [&'static str]) -> Result<()> {
        self.out.write_all(format!("\n## {name}\n\n").as_bytes())?;
        self.table = Some((name, header));
        self.rows = 0;
        Ok(())
    }
    fn row(&mut self, row: &[String]) -> Result<()> {
        let Some((name, header)) = self.table else {
            return Err(EngineError::Internal(
                "a report row came before its table".to_string(),
            ));
        };
        let mut text = String::new();
        if self.rows == 0 {
            if name == "Timeline" {
                text.push_str(
                    "In time order; entries that need a time are listed last, by id.\n\n",
                );
            }
            markdown_row(&mut text, header.iter().map(|h| md(h)));
            markdown_row(&mut text, header.iter().map(|_| "---".to_string()));
        }
        markdown_row(&mut text, row.iter().map(|c| md(c)));
        self.out.write_all(text.as_bytes())?;
        self.rows += 1;
        Ok(())
    }
    fn end(&mut self) -> Result<()> {
        if let Some((name, _)) = self.table.take() {
            if self.rows == 0 {
                self.out
                    .write_all(format!("No {}.\n", none_of(name)).as_bytes())?;
            }
        }
        Ok(())
    }
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
/// Every `://` is defanged to `[:]//`, so that no URL autolinks (and no
/// link reference can be defined: every `[` from the case is escaped).
/// Each bidi or invisible formatting character (`is_invisible`) is shown
/// as the text `\u{XXXX}`, so that none reorders or hides what follows.
/// Total: any text gives an answer. Applying it twice escapes the
/// backslashes the first added, so it is applied once, to the case's text.
pub(crate) fn md(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' | '`' | '|' | '<' | '>' | '[' | ']' | '*' | '_' => {
                out.push('\\');
                out.push(c);
            }
            ':' if text[at..].starts_with("://") => out.push_str("[:]"),
            '\r' => {
                if chars.peek().map(|&(_, next)| next) == Some('\n') {
                    chars.next();
                }
                out.push(' ');
            }
            c if c.is_control() => out.push(' '),
            c if is_invisible(c) => out.push_str(&format!("\\u{{{:04X}}}", u32::from(c))),
            c => out.push(c),
        }
    }
    out
}

/// The characters that change how the text around them reads without
/// showing themselves: the zero-width space, joiners and direction marks
/// (U+200B–U+200F), the bidi embeddings and overrides (U+202A–U+202E), the
/// bidi isolates (U+2066–U+2069), and the byte order mark (U+FEFF).
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
    )
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
/// created new beside it, inside one read transaction (`snapshot`), so it
/// can write each row as it reads it and every read sees the case as one
/// snapshot. The file is then synced, the target checked again outside the
/// transaction (a source loaded meanwhile may now hold it) and the cancel
/// flag read, then renamed over `path`. `before_rename` is called once the
/// partial file is closed (a test's pause; a no-op otherwise). On any
/// failure the partial file, this export's own, is removed and what was at
/// `path` is left as it was.
fn write_whole(
    case: &Case,
    path: &Path,
    cancel: &AtomicBool,
    before_rename: &mut dyn FnMut(),
    write: impl FnOnce(&Case, &mut File) -> Result<u64>,
) -> Result<ExportReport> {
    check_export_target(case, path)?;
    check_cancel(cancel)?;
    let (partial, mut file) = create_export_partial(path)?;
    let written = snapshot(case, |case| write(case, &mut file)).and_then(|rows| {
        file.sync_all()?;
        drop(file);
        before_rename();
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

/// `read(case)` inside one read transaction on the case's connection, so
/// that every read it makes (pages, evidence, records, the case's name)
/// sees the case as one snapshot, whatever another connection commits
/// meanwhile. `read` only reads, so the transaction is rolled back once it
/// returns, keeping nothing because there is nothing to keep. A connection
/// already in a transaction is refused by DuckDB's `BEGIN`, an error this
/// returns.
fn snapshot<T>(case: &Case, read: impl FnOnce(&Case) -> Result<T>) -> Result<T> {
    let conn = case.conn();
    conn.execute_batch("BEGIN TRANSACTION")?;
    let open = ReadTransaction(conn);
    let out = read(case);
    open.end()?;
    out
}

/// An open read transaction, rolled back when dropped: when the reads
/// unwind as much as when they end.
struct ReadTransaction<'a>(&'a duckdb::Connection);

impl ReadTransaction<'_> {
    /// Roll it back now, saying whether that failed.
    fn end(self) -> Result<()> {
        let conn = self.0;
        std::mem::forget(self);
        conn.execute_batch("ROLLBACK")?;
        Ok(())
    }
}

impl Drop for ReadTransaction<'_> {
    fn drop(&mut self) {
        // Reached only when the reads unwind: nothing was written, and the
        // panic under way is the failure worth reporting.
        let _ = self.0.execute_batch("ROLLBACK");
    }
}

/// Every page of a listing, `MAX_LIMIT` at a time from the start, each item
/// handed to `each` as soon as its page is read; the cancel flag is read
/// before each page and each item. How many items there were.
fn each_page<T>(
    cancel: &AtomicBool,
    mut page: impl FnMut(u32, u64) -> Result<Listing<T>>,
    mut each: impl FnMut(&[T]) -> Result<()>,
) -> Result<u64> {
    let mut offset: u64 = 0;
    loop {
        check_cancel(cancel)?;
        let listing = page(MAX_LIMIT, offset)?;
        each(&listing.items)?;
        offset += count(&listing.items);
        if !listing.truncated {
            return Ok(offset);
        }
    }
}

/// The four tables into `sink`, in the order the workbook and the report
/// write them; how many rows they had.
fn stream_tables(
    case: &Case,
    cancel: &AtomicBool,
    after_entries: &mut dyn FnMut(),
    on_evidence: &mut dyn FnMut(),
    sink: &mut dyn Sink,
) -> Result<u64> {
    let mut rows = stream_timeline(case, cancel, after_entries, on_evidence, sink)?;
    rows += stream_records(
        cancel,
        sink,
        "Hosts",
        HOSTS,
        |limit, offset| hosts(case, limit, offset),
        host_row,
    )?;
    rows += stream_records(
        cancel,
        sink,
        "Accounts",
        ACCOUNTS,
        |limit, offset| accounts(case, limit, offset),
        account_row,
    )?;
    rows += stream_records(
        cancel,
        sink,
        "IOCs",
        IOCS,
        |limit, offset| iocs(case, &IocQuery::default(), limit, offset),
        ioc_row,
    )?;
    Ok(rows)
}

/// One record table into `sink`, a page at a time.
fn stream_records<T>(
    cancel: &AtomicBool,
    sink: &mut dyn Sink,
    name: &'static str,
    header: &'static [&'static str],
    page: impl FnMut(u32, u64) -> Result<Listing<T>>,
    row: fn(&T) -> Vec<String>,
) -> Result<u64> {
    sink.start(name, header)?;
    let rows = each_page(cancel, page, |items| {
        for item in items {
            check_cancel(cancel)?;
            sink.row(&row(item))?;
        }
        Ok(())
    })?;
    sink.end()?;
    Ok(rows)
}

/// The Timeline table into `sink`: the live entries in the Timeline's
/// order, a page at a time, each with the rows attached to it as evidence,
/// in the order they were attached (read for that page's entries only).
/// `after_entries` is called once the first page is read, before its
/// evidence.
fn stream_timeline(
    case: &Case,
    cancel: &AtomicBool,
    after_entries: &mut dyn FnMut(),
    on_evidence: &mut dyn FnMut(),
    sink: &mut dyn Sink,
) -> Result<u64> {
    sink.start("Timeline", TIMELINE)?;
    let mut first = true;
    let rows = each_page(
        cancel,
        |limit, offset| entries(case, &EntryQuery::default(), limit, offset),
        |listed: &[Entry]| {
            if std::mem::take(&mut first) {
                after_entries();
            }
            let ids: Vec<i64> = listed.iter().map(|e| e.id).collect();
            let evidence = evidence_refs(case, &ids, cancel, on_evidence)?;
            for e in listed {
                check_cancel(cancel)?;
                sink.row(&entry_row(
                    e,
                    evidence.get(&e.id).map(Vec::as_slice).unwrap_or(&[]),
                ))?;
            }
            Ok(())
        },
    )?;
    sink.end()?;
    Ok(rows)
}

/// For each of `entries` (one page, at most `MAX_LIMIT`), its evidence rows
/// as `<relation>#<row id>`, in the order they were attached. The cancel
/// flag is read before each row; `on_evidence` is called as each is read.
/// None in a case without the record tables.
fn evidence_refs(
    case: &Case,
    entries: &[i64],
    cancel: &AtomicBool,
    on_evidence: &mut dyn FnMut(),
) -> Result<HashMap<i64, Vec<String>>> {
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    if entries.is_empty() || !has_records(case) {
        return Ok(out);
    }
    // Entry ids are integers the engine read, so the list is safe as text.
    let mut stmt = case.conn().prepare(&format!(
        "SELECT entry_id, relation, row_id FROM ox.evidence WHERE entry_id IN ({}) \
         ORDER BY entry_id, attached_at, row_id",
        id_list(entries)
    ))?;
    let mut rows = stmt.query([])?;
    loop {
        check_cancel(cancel)?;
        let Some(row) = rows.next()? else {
            return Ok(out);
        };
        on_evidence();
        let entry: i64 = row.get(0)?;
        let relation: String = row.get(1)?;
        let row_id: i64 = row.get(2)?;
        out.entry(entry)
            .or_default()
            .push(format!("{relation}#{row_id}"));
    }
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
        assert_eq!(
            md("http://x ftp://y a:/b ::"),
            "http[:]//x ftp[:]//y a:/b ::"
        );
        assert_eq!(md("a\u{202E}b\u{FEFF}"), "a\\u{202E}b\\u{FEFF}");
        // Just outside each range: left as it is.
        assert_eq!(md("\u{200A}\u{2010}\u{2065}"), "\u{200A}\u{2010}\u{2065}");
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
