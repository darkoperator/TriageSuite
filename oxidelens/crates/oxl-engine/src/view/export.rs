//! CSV export of what the grid shows: the rows the state selects, in its
//! order, with its columns.
//!
//! Each cell is written as the case stores it: the source's own text (a
//! typed timestamp's `__text`, UTC as the suite wrote it). With a zone, a
//! `utc` timestamp that converted is written as the row detail shows it,
//! `<local> <offset>`, through the grid's own renderer. Nothing else is
//! changed: a value that a spreadsheet would read as a formula
//! (`=HYPERLINK(…)`) is written byte-for-byte, only quoted as CSV quotes any
//! field, because the file is data, not a workbook.
//!
//! Evidence and run output are never written: a path inside any loaded
//! source, or the case file itself, is refused before anything is created.
//! The rows are written to `<path>.partial` beside the target, a file this
//! export creates new (anything already at that name, a link above all, is
//! refused, never followed or truncated), and renamed over the target only
//! once complete, so a cancelled or failed export leaves what was at the
//! path as it was. A rename replaces a link at the target itself; it never
//! writes through one. The rows are streamed from DuckDB a chunk at a
//! time, never held in memory together.

use super::page::render_time;
use super::GridState;
use crate::case::{wal_path, Case};
use crate::catalog::{SqlType, TimeSemantics};
use crate::error::{EngineError, Result};
use crate::load::{canonical_path, sources};
use crate::query::{check_zone, order_sql, scope, Scope, ISO_FORMAT, ROW_ALIAS};
use duckdb::arrow::array::{Array, AsArray};
use duckdb::arrow::compute::cast;
use duckdb::arrow::datatypes::DataType;
use duckdb::params_from_iter;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use triage_core::output::duckdb::sql::quote_ident;

/// How many rows are written between two looks at the cancel flag.
const CANCEL_EVERY: u64 = 10_000;

/// What an export wrote.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ExportReport {
    /// Data rows, the header not counted.
    pub rows: u64,
    /// The file written, as the caller named it.
    pub path: PathBuf,
}

/// Write the rows `state` selects (its chips, its search in filter mode,
/// its hosts), in its order, to `path` as CSV: a header of the columns'
/// logical names (`state.columns`, or every catalogued column), then one
/// record per row, each cell its column's stored text, an empty field for an
/// empty cell. With `zone_times`, an IANA zone, a `utc` timestamp cell that
/// converted is written `<local> <offset>` in that zone instead.
///
/// `Invalid` for a bad state or an unknown zone, a `path` that is a folder
/// or names no file, anything already at `<path>.partial` (left as it is),
/// and when `cancel` is set (it is read before the first
/// row and every `CANCEL_EVERY` rows after; the message is `cancelled`).
/// `NotFound` for a folder that does not exist. `Refused` for a path inside
/// any source not removed from the case (a run's folder, or a loaded file
/// itself), or the case file. On any failure the partial file is removed and
/// whatever was at `path` is left as it was.
pub fn export_csv(
    case: &Case,
    state: &GridState,
    path: &Path,
    zone_times: Option<&str>,
    cancel: &AtomicBool,
) -> Result<ExportReport> {
    export_csv_guarded(case, state, path, zone_times, cancel, |_| Ok(()))
}

/// `export_csv`, with `guard` taken once every row is written and held
/// across a second `check_export_target` and the rename. A caller in which
/// the case's sources can change while the rows are written (the app, whose
/// loads run beside its reads) passes a guard that keeps them from
/// changing, so that a load which made the target part of a source after
/// the first check is seen, and the export is `Refused` with its partial
/// file removed, instead of landing inside the source. `guard`'s error ends
/// the export the same way.
pub fn export_csv_guarded<G>(
    case: &Case,
    state: &GridState,
    path: &Path,
    zone_times: Option<&str>,
    cancel: &AtomicBool,
    guard: impl FnOnce(&Case) -> Result<G>,
) -> Result<ExportReport> {
    if let Some(zone) = zone_times {
        check_zone(zone)?;
    }
    let q = state.query()?;
    let scope = scope(case, &q)?;
    let order = order_sql(&scope, &q)?;
    check_export_target(case, path)?;

    let (partial, file) = create_export_partial(path)?;
    let written = write_rows(case, &scope, &order, zone_times, file, cancel).and_then(|rows| {
        let held = guard(case)?;
        check_export_target(case, path)?;
        let renamed = std::fs::rename(&partial, path);
        drop(held);
        renamed.map(|()| rows).map_err(Into::into)
    });
    match written {
        Ok(rows) => Ok(ExportReport {
            rows,
            path: path.to_path_buf(),
        }),
        Err(e) => {
            // The partial file is this export's own; the error that ended
            // the export is the one worth reporting.
            let _ = std::fs::remove_file(&partial);
            Err(e)
        }
    }
}

/// `<path>.partial`: in the same folder as `path`, so the rename that
/// replaces it is within one volume.
fn partial_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".partial");
    PathBuf::from(name)
}

/// Create `<path>.partial` new, beside `path`, for a file that is renamed
/// over `path` once complete (as `create_partial`); the partial's path and
/// the open file. The app writes an exported view file through it.
pub fn create_export_partial(path: &Path) -> Result<(PathBuf, File)> {
    let partial = partial_path(path);
    let file = create_partial(&partial)?;
    Ok((partial, file))
}

/// Create the partial file, new. Anything already at that name, a file, a
/// folder, or a link (which an open that may create would follow, and
/// truncate what it points at), is `Invalid` and left as it is: only a file
/// this export created is ever written or removed. Creating new is atomic
/// and never follows a link in the last component (`O_CREAT | O_EXCL`,
/// `CREATE_NEW`).
fn create_partial(partial: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(partial)
        .map_err(|e| {
            // Windows answers a junction or a folder at the name with
            // "access denied", not "already exists"; either way something
            // is there, which is what the analyst needs to hear.
            if e.kind() == std::io::ErrorKind::AlreadyExists
                || std::fs::symlink_metadata(partial).is_ok()
            {
                EngineError::Invalid(format!(
                    "a file named {} already exists; remove it or choose another name",
                    partial.display()
                ))
            } else {
                e.into()
            }
        })
}

/// Stream the scope's rows into `file` as CSV; the number of data rows.
fn write_rows(
    case: &Case,
    scope: &Scope,
    order: &str,
    zone: Option<&str>,
    file: File,
    cancel: &AtomicBool,
) -> Result<u64> {
    // Per shown column its text, and for a UTC time rendered in a zone, its
    // typed value as R1 text after it: what says it converted, and what
    // stands in when the text is not in the UTC form (`render_time`).
    let mut select = Vec::with_capacity(scope.columns.len() + 1);
    let mut renders = Vec::with_capacity(scope.columns.len());
    for c in &scope.columns {
        select.push(format!(
            "CAST({ROW_ALIAS}.{} AS VARCHAR)",
            quote_ident(&c.text_column)
        ));
        let typed = c.typed_column.as_ref().filter(|_| {
            zone.is_some()
                && c.sql_type == Some(SqlType::Timestamp)
                && c.time_semantics == Some(TimeSemantics::Utc)
        });
        if let Some(typed) = typed {
            select.push(format!(
                "strftime({ROW_ALIAS}.{}, '{ISO_FORMAT}')",
                quote_ident(typed)
            ));
        }
        renders.push(typed.is_some());
    }
    // Never an empty select list, whatever the columns; never read either.
    let read = select.len();
    select.push(format!("{ROW_ALIAS}._ox_row"));
    let sql = format!(
        "SELECT {} FROM {} WHERE {} ORDER BY {order}",
        select.join(", "),
        scope.from_sql,
        scope.where_sql,
    );

    let mut out = csv::Writer::from_writer(file);
    out.write_record(scope.columns.iter().map(|c| c.name.as_bytes()))
        .map_err(csv_error)?;

    let mut stmt = case.conn().prepare(&sql)?;
    // Executed as a stream: each `step` fetches the next chunk, so the rows
    // are never all in memory. The iterator `stream_arrow` returns panics on
    // a failed fetch, so it is dropped and the chunks are read by `step`,
    // which returns the error.
    drop(stmt.stream_arrow(params_from_iter(&scope.params))?);
    let mut rows: u64 = 0;
    let mut texts = Vec::with_capacity(select.len());
    while let Some(chunk) = stmt.step()? {
        texts.clear();
        for column in chunk.columns().iter().take(read) {
            texts.push(cast(column, &DataType::Utf8).map_err(|e| {
                EngineError::Internal(format!("an exported column is not text: {e}"))
            })?);
        }
        for i in 0..chunk.len() {
            if rows.is_multiple_of(CANCEL_EVERY) && cancel.load(Ordering::Relaxed) {
                return Err(EngineError::Invalid("cancelled".to_string()));
            }
            let cell = |at: usize| -> Option<&str> {
                let text = texts[at].as_string::<i32>();
                (!text.is_null(i)).then(|| text.value(i))
            };
            let mut at = 0;
            for render in &renders {
                let text = cell(at);
                at += 1;
                let shown = match (zone, render) {
                    (Some(zone), true) => {
                        let typed = cell(at);
                        at += 1;
                        typed
                            .and_then(|typed| render_time(text, typed, zone))
                            .map(|r| format!("{} {}", r.local, r.offset))
                    }
                    _ => None,
                };
                match shown {
                    Some(shown) => out.write_field(shown),
                    None => out.write_field(text.unwrap_or_default()),
                }
                .map_err(csv_error)?;
            }
            out.write_record(None::<&[u8]>).map_err(csv_error)?;
            rows += 1;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(EngineError::Invalid("cancelled".to_string()));
    }
    let file = out
        .into_inner()
        .map_err(|e| EngineError::Io(std::io::Error::other(e.to_string())))?;
    file.sync_all()?;
    Ok(rows)
}

fn csv_error(e: csv::Error) -> EngineError {
    EngineError::Io(e.into())
}

/// Refuse a target an export may not write: `Invalid` for a folder or a
/// path that names no file, `NotFound` for a folder that is not there, and
/// `Refused` when the file or its partial file would be inside a source the
/// case has not removed, or would be the case file or its log. Paths are
/// compared canonical: the folder's, and the file's own when it exists, so a
/// link or a `..` cannot lead into a source. Every file the app writes for
/// the analyst (a CSV export, an exported view) is checked by it.
pub fn check_export_target(case: &Case, path: &Path) -> Result<()> {
    if std::fs::metadata(path).is_ok_and(|m| m.is_dir()) {
        return Err(EngineError::Invalid(format!(
            "{} is a folder; an export is written to a file",
            path.display()
        )));
    }
    let Some(name) = path.file_name() else {
        return Err(EngineError::Invalid(format!(
            "{} names no file to write",
            path.display()
        )));
    };
    let folder = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let folder = canonical_path(folder)?;
    let target = if std::fs::symlink_metadata(path).is_ok() {
        canonical_path(path)?
    } else {
        folder.join(name)
    };
    let partial = partial_path(&folder.join(name));

    for source in sources(case)?.into_iter().filter(|s| !s.removed) {
        let stored = PathBuf::from(&source.path);
        // A source that has moved or gone since it was loaded is compared
        // as it was recorded, which was canonical then.
        let root = canonical_path(&stored).unwrap_or(stored);
        if inside(&target, &root) || inside(&partial, &root) {
            return Err(EngineError::Refused(format!(
                "{} is inside {}, which the case loaded; evidence and run output are never \
                 written, so choose a folder outside it",
                path.display(),
                root.display()
            )));
        }
    }
    let case_file = canonical_path(case.path()).unwrap_or_else(|_| case.path().to_path_buf());
    for own in [wal_path(&case_file), case_file] {
        if same(&target, &own) || same(&partial, &own) {
            return Err(EngineError::Refused(format!(
                "{} is the case file; an export never writes over it",
                path.display()
            )));
        }
    }
    Ok(())
}

/// `path` is `root` or below it, compared component by component, and on
/// Windows without regard to case, as its file systems compare names.
fn inside(path: &Path, root: &Path) -> bool {
    let (path, root) = (folded(path), folded(root));
    path.len() >= root.len() && path[..root.len()] == root[..]
}

fn same(a: &Path, b: &Path) -> bool {
    folded(a) == folded(b)
}

fn folded(path: &Path) -> Vec<String> {
    path.components()
        .map(|c| {
            let text = c.as_os_str().to_string_lossy();
            if cfg!(windows) {
                text.to_lowercase()
            } else {
                text.into_owned()
            }
        })
        .collect()
}
