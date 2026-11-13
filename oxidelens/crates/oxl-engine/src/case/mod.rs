//! The case file: one DuckDB database holding the case tables (schema `ox`)
//! and every loaded source (schemas `src_<n>`).
//!
//! The file carries its own format number in `ox.meta`. A file written by a
//! newer build is refused rather than read, because reading a format this
//! build does not know is how annotations get silently misread.
//!
//! Format 1 is step 1's. Opened to change it, a format-1 case is upgraded
//! in place to format 2, in one transaction (`upgrade_from_1`); opened
//! read-only, it is read as it is, and every reader of a format-2 table
//! answers that table's default (`Case::format` tells them apart).

mod settings;

pub use settings::{display_zone, set_display_zone};

use crate::actor::Actor;
use crate::error::{EngineError, Result};
use crate::time::now_iso;
use duckdb::{params, AccessMode, Config, Connection};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use triage_core::output::duckdb::sql::{quote_ident, quote_literal};

/// The case file format this build writes. It reads this one and every
/// earlier one (format 1).
pub const CASE_FORMAT: u32 = 2;

/// The oldest case file format this build reads.
const OLDEST_FORMAT: u32 = 1;

/// The DDL for a new case, format 2.
const SCHEMA: &str = include_str!("schema.sql");

/// The DDL that brings a format-1 case to format 2.
const UPGRADE: &str = include_str!("upgrade.sql");

/// The suffix a case file must carry, so that a case is recognisable as a
/// DuckDB database and `create` cannot be pointed at an unrelated file.
const SUFFIX: &str = ".duckdb";

/// An open case file: one connection to its database. Dropping it closes
/// the connection; the file and its lock are released when the last
/// connection to it (see `try_clone`) closes.
pub struct Case {
    path: PathBuf,
    conn: Connection,
    /// The file's format as it was opened: `CASE_FORMAT`, or an older one
    /// only for a case opened read-only.
    format: u32,
}

/// What `ox.meta` says about a case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseInfo {
    pub name: String,
    pub format: u32,
    pub created_by: String,
    pub created_at: String,
    pub path: PathBuf,
}

/// How much of the case file DuckDB uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaseSize {
    /// The file's size.
    pub bytes: u64,
    /// The part DuckDB freed (a removed source's tables, rewritten data)
    /// and keeps for reuse; the file never shrinks by itself.
    /// `Case::compact_to` writes a copy without it.
    pub free_bytes: u64,
}

/// What `Case::compact_to` wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactReport {
    pub path: PathBuf,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

/// Refuse `path` for a new case file: it must end in `.duckdb` and must not
/// exist yet.
fn check_new_path(path: &Path) -> Result<()> {
    let has_suffix = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.len() > SUFFIX.len() && n.ends_with(SUFFIX));
    if !has_suffix {
        return Err(EngineError::Invalid(format!(
            "a case file name must end in {SUFFIX}: {}",
            path.display()
        )));
    }
    if entry_exists(path)? {
        return Err(EngineError::Invalid(format!(
            "{} already exists; a new case never overwrites a file",
            path.display()
        )));
    }
    Ok(())
}

impl Case {
    /// Create a new case file at `path`, which must end in `.duckdb` and
    /// must not exist yet.
    pub fn create(path: &Path, name: &str, actor: &Actor) -> Result<Case> {
        check_new_path(path)?;
        Case::create_with(path, |conn| initialise(conn, name, actor), CASE_FORMAT)
    }

    /// A new case at `path` made by `ddl` and declared format 1, exactly as
    /// step 1 made one, and left open at format 1 to change: only the
    /// upgrade tests make one, to have a step 1 case to load, annotate and
    /// then upgrade. Every other way to open a format-1 case to change it
    /// upgrades it.
    #[doc(hidden)]
    pub fn create_at_format_1(path: &Path, ddl: &str, name: &str, actor: &Actor) -> Result<Case> {
        check_new_path(path)?;
        Case::create_with(
            path,
            |conn| {
                let tx = conn.transaction()?;
                tx.execute_batch(ddl)?;
                write_meta(&tx, name, OLDEST_FORMAT, actor)?;
                tx.commit()?;
                Ok(())
            },
            OLDEST_FORMAT,
        )
    }

    fn create_with(
        path: &Path,
        make: impl FnOnce(&mut Connection) -> Result<()>,
        format: u32,
    ) -> Result<Case> {
        let mut conn = connect(path, AccessMode::ReadWrite)?;
        match make(&mut conn) {
            Ok(()) => Ok(Case {
                path: path.to_path_buf(),
                conn,
                format,
            }),
            Err(e) => {
                // A half-written case is worse than none: it would open later
                // and look like a case with no history. The file did not
                // exist before this call, so removing it destroys nothing.
                drop(conn);
                let _ = std::fs::remove_file(path);
                let _ = std::fs::remove_file(wal_path(path));
                Err(e)
            }
        }
    }

    /// Open an existing case file to change it. The file is held to this
    /// process alone: `CaseLocked` when any other process has it open, and
    /// for any other process until this one lets go.
    ///
    /// A format-1 case is upgraded to format 2 first, in place and in one
    /// transaction: the format-2 tables, a new `case_id`, the sessions of
    /// each run still in the case (read again from where the run is, best
    /// effort), and a `case_upgrade` history row. When any of it fails,
    /// none of it is kept, the file is still format 1, and the error is
    /// returned.
    pub fn open(path: &Path) -> Result<Case> {
        Case::open_as(path, AccessMode::ReadWrite, true)
    }

    /// Open an existing case file for reading only, which is how any number
    /// of processes may have it open at once. `CaseLocked` only while a
    /// process has it open to change it. A call that changes the case fails
    /// on a case opened this way. A format-1 case is read as it is: what
    /// format 2 added reads as its default (`format` says which it is).
    pub fn open_read_only(path: &Path) -> Result<Case> {
        Case::open_as(path, AccessMode::ReadOnly, false)
    }

    fn open_as(path: &Path, mode: AccessMode, upgrade: bool) -> Result<Case> {
        if !path.try_exists()? {
            return Err(EngineError::NotFound(format!(
                "no case file at {}",
                path.display()
            )));
        }
        let mut conn = connect(path, mode)?;
        let mut format = check_format(&conn, path)?;
        if upgrade && format == OLDEST_FORMAT {
            upgrade_from_1(&mut conn)?;
            format = CASE_FORMAT;
        }
        Ok(Case {
            path: path.to_path_buf(),
            conn,
            format,
        })
    }

    /// A second connection to this case's database, in this process: it
    /// sees everything committed through any connection to it, and runs its
    /// own statements, on its own thread if need be, beside this one's.
    /// The clone of a case opened read-only is read-only. The file stays
    /// open, and its lock held, until the last connection to it closes.
    pub fn try_clone(&self) -> Result<Case> {
        Ok(Case {
            path: self.path.clone(),
            conn: self.conn.try_clone()?,
            format: self.format,
        })
    }

    /// A handle that interrupts the statement this connection is running,
    /// from any thread; it fails with DuckDB's interrupt error. DuckDB
    /// clears an interrupt when the next statement starts, so one that
    /// lands between statements is lost: a caller that must stop a call
    /// interrupts again until the call returns. A handle outliving its
    /// connection does nothing.
    pub fn interrupt_handle(&self) -> Arc<duckdb::InterruptHandle> {
        self.conn.interrupt_handle()
    }

    /// The case file's format: `CASE_FORMAT`, except for a format-1 case
    /// opened read-only, which is 1.
    pub fn format(&self) -> u32 {
        self.format
    }

    /// The case's id, 32 lowercase hex digits made when the case was
    /// created (or upgraded) and fixed from then on; empty for a format-1
    /// case opened read-only, which has none.
    pub fn case_id(&self) -> Result<String> {
        if self.format < CASE_FORMAT {
            return Ok(String::new());
        }
        let id: Option<String> = self
            .conn()
            .query_row("SELECT value FROM ox.meta WHERE key = 'case_id'", [], |r| {
                r.get(0)
            })
            .map(Some)
            .or_else(|e| match e {
                duckdb::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        match id {
            Some(id) if is_case_id(&id) => Ok(id),
            Some(id) => Err(EngineError::Internal(format!(
                "the case's case_id is not 32 lowercase hex digits: {id:?}"
            ))),
            None => Err(EngineError::Internal(
                "the case has no case_id in ox.meta".to_string(),
            )),
        }
    }

    /// The case's name, format and creation record.
    pub fn info(&self) -> Result<CaseInfo> {
        let get = |key: &str| -> Result<String> {
            self.conn()
                .query_row(
                    "SELECT value FROM ox.meta WHERE key = ?",
                    params![key],
                    |r| r.get(0),
                )
                .map_err(|e| match e {
                    // Opening checked the format; a case the engine made
                    // has every key.
                    duckdb::Error::QueryReturnedNoRows => {
                        EngineError::Internal(format!("the case has no {key} in ox.meta"))
                    }
                    other => other.into(),
                })
        };
        Ok(CaseInfo {
            name: get("name")?,
            format: parse_format(&get("format")?, &self.path)?,
            created_by: get("created_by")?,
            created_at: get("created_at")?,
            path: self.path.clone(),
        })
    }

    /// The file's size and the part of it DuckDB holds free.
    pub fn size(&self) -> Result<CaseSize> {
        let (block_size, free_blocks): (i64, i64) = self.conn().query_row(
            "SELECT block_size, free_blocks FROM pragma_database_size() \
             WHERE database_name = current_database()",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let free = u64::try_from(block_size.saturating_mul(free_blocks)).unwrap_or(0);
        let bytes = std::fs::metadata(&self.path)?.len();
        Ok(CaseSize {
            bytes,
            free_bytes: free.min(bytes),
        })
    }

    /// Write a copy of the case to `out`, a new `.duckdb` file, without the
    /// space DuckDB holds free in this one. Everything is copied as it is:
    /// sources, annotations, history and the id sequences, so the copy
    /// carries on where the case was and never hands out an id it gave
    /// before. This case's content is left as it is; as when any command
    /// that opened it to change it closes, a log it had pending may be
    /// folded into its file.
    ///
    /// Beside the case, nothing that exists when the call starts is written
    /// over or removed, and `out` is never replaced. The copy is made in a working directory
    /// this call creates beside `out` (creating a directory fails when it
    /// exists), checked, and then linked to `out`, which fails rather than
    /// replace a file that appeared there meanwhile. Cleanup removes the
    /// copy and its log by name, then the directory, and stops at anything
    /// else in it; when it cannot finish, the error names the directory.
    ///
    /// The directory of `out` is the analyst's own, and is trusted while
    /// the call runs: another process writing into the new working
    /// directory, or a log beside `out`, during those seconds is not
    /// defended against.
    pub fn compact_to(&self, out: &Path) -> Result<CompactReport> {
        check_new_path(out)?;
        // DuckDB is handed the path as text, so it must be the exact path.
        if out.to_str().is_none() {
            return Err(EngineError::Invalid(format!(
                "{} is not valid Unicode, so DuckDB cannot be given it exactly",
                out.display()
            )));
        }
        refuse_wal(out)?;
        // The copy is opened at `out` from then on, its log beside it.
        refuse_too_long_for_duckdb(&wal_path(out))?;
        let parent = match out.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        // Short, since DuckDB is given this path and not `out`.
        let tag = (nanos ^ u128::from(std::process::id())) & 0xffff_ffff;
        let staging = parent.join(format!(".oxc-{tag:08x}"));
        let copy = staging.join("c.duckdb");
        refuse_too_long_for_duckdb(&wal_path(&copy))?;
        std::fs::create_dir(&staging)?;
        let result = self.copy_into(&copy).and_then(|()| publish_new(&copy, out));
        if let Err(cleanup) = remove_staging(&staging, &copy) {
            let left = format!(
                "the working copy at {} could not be removed ({cleanup}); remove it",
                staging.display()
            );
            return Err(match result {
                Err(e) => EngineError::Io(std::io::Error::other(format!("{e}; {left}"))),
                Ok(()) => EngineError::Io(std::io::Error::other(format!(
                    "{} was written, but {left}",
                    out.display()
                ))),
            });
        }
        result?;
        // The published name, which this call just created, opens as a case.
        if let Err(e) = Case::open_read_only(out) {
            return Err(match std::fs::remove_file(out) {
                Ok(()) => e,
                Err(left) => EngineError::Io(std::io::Error::other(format!(
                    "{e}; and {} could not be removed ({left}); remove it",
                    out.display()
                ))),
            });
        }
        Ok(CompactReport {
            path: out.to_path_buf(),
            bytes_before: std::fs::metadata(&self.path)?.len(),
            bytes_after: std::fs::metadata(out)?.len(),
        })
    }

    /// Copy the whole case into `copy`, a path in a directory only this call
    /// uses, and check the copy opens as a case.
    fn copy_into(&self, copy: &Path) -> Result<()> {
        // `out` was checked to be Unicode, and the rest of the path is ours.
        let target = copy.to_str().ok_or_else(|| {
            EngineError::Invalid(format!("{} is not valid Unicode", copy.display()))
        })?;
        let conn = self.conn();
        let current: String = conn.query_row("SELECT current_database()", [], |r| r.get(0))?;
        // DuckDB compares names without regard to case.
        let alias = if current.eq_ignore_ascii_case("ox_compact") {
            "ox_compact_copy"
        } else {
            "ox_compact"
        };
        let copied = conn
            .execute_batch(&format!(
                "ATTACH {target} AS {alias}; \
                 COPY FROM DATABASE {from} TO {alias};",
                target = quote_literal(target),
                alias = quote_ident(alias),
                from = quote_ident(&current),
            ))
            .map_err(EngineError::from);
        // Detached whether or not the copy worked, so the file is closed
        // before it is checked or its directory removed.
        let detached = conn
            .execute_batch(&format!(
                "DETACH DATABASE IF EXISTS {};",
                quote_ident(alias)
            ))
            .map_err(EngineError::from);
        copied.and(detached)?;
        Case::open_read_only(copy)?;
        // Detaching checkpoints the copy; a log left over would be lost when
        // the file alone is published.
        refuse_wal(copy)
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Mutable access, which a transaction needs.
    pub(crate) fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The first column of the first row of `sql`, for integration tests.
    #[doc(hidden)]
    pub fn scalar_i64(&self, sql: &str) -> Result<i64> {
        Ok(self.conn().query_row(sql, [], |r| r.get(0))?)
    }

    /// `scalar_i64` with bound parameters, for integration tests that run a
    /// compiled fragment before the query API exists to run it for them.
    #[doc(hidden)]
    pub fn scalar_i64_with(&self, sql: &str, params: &[duckdb::types::Value]) -> Result<i64> {
        Ok(self
            .conn()
            .query_row(sql, duckdb::params_from_iter(params), |r| r.get(0))?)
    }

    /// Run `call` and return, with its result, the SQL of every statement
    /// the case's database ran meanwhile, for integration tests that pin how
    /// many statements a call costs. DuckDB's own query log does the
    /// counting, so a statement run on any connection to the case is seen.
    #[doc(hidden)]
    pub fn statements_during<T>(&self, call: impl FnOnce() -> T) -> Result<(T, Vec<String>)> {
        self.start_query_log()?;
        let out = call();
        Ok((out, self.stop_query_log()?))
    }

    /// `statements_during` for a call that changes the case.
    #[doc(hidden)]
    pub fn statements_during_mut<T>(
        &mut self,
        call: impl FnOnce(&mut Case) -> T,
    ) -> Result<(T, Vec<String>)> {
        self.start_query_log()?;
        let out = call(self);
        Ok((out, self.stop_query_log()?))
    }

    fn start_query_log(&self) -> Result<()> {
        self.conn()
            .execute_batch("CALL truncate_duckdb_logs(); CALL enable_logging('QueryLog');")?;
        Ok(())
    }

    /// Stop the query log and return what it holds, the stop itself left out.
    fn stop_query_log(&self) -> Result<Vec<String>> {
        const STOP: &str = "CALL disable_logging()";
        self.conn().execute_batch(STOP)?;
        Ok(self
            .conn()
            .prepare("SELECT message FROM duckdb_logs WHERE type = 'QueryLog'")?
            .query_map([], |r| r.get::<_, String>(0))?
            .filter(|sql| sql.as_deref().map_or(true, |sql| sql.trim() != STOP))
            .collect::<std::result::Result<_, _>>()?)
    }
}

fn initialise(conn: &mut Connection, name: &str, actor: &Actor) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(SCHEMA)?;
    write_meta(&tx, name, CASE_FORMAT, actor)?;
    tx.execute(
        "INSERT INTO ox.meta (key, value) VALUES ('case_id', ?)",
        params![new_case_id()?],
    )?;
    tx.commit()?;
    Ok(())
}

/// The facts every case's `ox.meta` has had since format 1.
fn write_meta(conn: &Connection, name: &str, format: u32, actor: &Actor) -> Result<()> {
    let format = format.to_string();
    let created_by = actor.label();
    let created_at = now_iso();
    for (key, value) in [
        ("name", name),
        ("format", format.as_str()),
        ("created_by", created_by.as_str()),
        ("created_at", created_at.as_str()),
    ] {
        conn.execute(
            "INSERT INTO ox.meta (key, value) VALUES (?, ?)",
            params![key, value],
        )?;
    }
    Ok(())
}

/// A new case id: 128 bits from the operating system's random source, as
/// 32 lowercase hex digits. Random rather than derived from the path or the
/// time, so that a copy of a case keeps its id and two cases never share one.
fn new_case_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| {
        EngineError::Io(std::io::Error::other(format!(
            "the operating system's random source failed: {e}"
        )))
    })?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn is_case_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Bring the format-1 case on `conn` to format 2, all in one transaction:
/// the format-2 tables (`upgrade.sql`), a new `case_id`, each run's
/// sessions read again from where the run is (best effort: a run that moved
/// or whose sessions cannot be read has none), the format, and a
/// `case_upgrade` history row. Any failure rolls all of it back.
///
/// Opening takes no actor, so the history row names the analyst the
/// operating system user stands for, as the CLI's default actor does.
fn upgrade_from_1(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(UPGRADE)?;
    let id = new_case_id()?;
    tx.execute(
        "INSERT INTO ox.meta (key, value) VALUES ('case_id', ?)",
        params![id],
    )?;
    crate::load::backfill_sessions(&tx)?;
    let changed = tx.execute(
        "UPDATE ox.meta SET value = ? WHERE key = 'format' AND value = ?",
        params![CASE_FORMAT.to_string(), OLDEST_FORMAT.to_string()],
    )?;
    if changed != 1 {
        return Err(EngineError::Internal(format!(
            "the case's format changed while it was upgraded ({changed} rows)"
        )));
    }
    let user = std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" }).unwrap_or_default();
    tx.execute(
        "INSERT INTO ox.history \
         (id, \"at\", \"by\", target_kind, target_id, action, before_json, after_json) \
         VALUES (nextval('ox.history_seq'), ?, ?, 'case', ?, 'case_upgrade', ?, ?)",
        params![
            now_iso(),
            Actor::from_os_user(&user).label(),
            id,
            serde_json::json!({ "format": OLDEST_FORMAT }).to_string(),
            serde_json::json!({ "format": CASE_FORMAT }).to_string()
        ],
    )?;
    tx.commit()?;
    Ok(())
}

/// Open the database, turning DuckDB's "someone else holds this file" into
/// `CaseLocked`. DuckDB shares a file between processes that open it
/// read-only and gives it whole to one that opens it read-write.
fn connect(path: &Path, mode: AccessMode) -> Result<Connection> {
    let config = Config::default().access_mode(mode)?;
    Connection::open_with_flags(path, config).map_err(|e| {
        if is_lock_conflict(&e) {
            EngineError::CaseLocked(path.to_path_buf())
        } else {
            e.into()
        }
    })
}

/// DuckDB reports a lock conflict only as message text, and the text is the
/// operating system's: POSIX `fcntl` locking says "Could not set lock ...
/// Conflicting lock is held", Windows' sharing violation says the file "is
/// being used by another process".
pub(crate) fn is_lock_conflict(e: &duckdb::Error) -> bool {
    let message = e.to_string();
    [
        "Could not set lock",
        "Conflicting lock",
        "being used by another process",
        "already open",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

/// The `ox.meta` format of the case at `path`, refused when this build
/// does not read it. `conn`'s default database must be the case.
pub(crate) fn check_format(conn: &Connection, path: &Path) -> Result<u32> {
    let format = read_format(conn, path)?;
    if format > CASE_FORMAT {
        return Err(EngineError::NeedsUpgrade {
            found: format,
            expected: CASE_FORMAT,
        });
    }
    if format < OLDEST_FORMAT {
        return Err(EngineError::Invalid(format!(
            "{} declares case format {format}, which never existed",
            path.display()
        )));
    }
    Ok(format)
}

fn read_format(conn: &Connection, path: &Path) -> Result<u32> {
    let not_a_case = || {
        EngineError::Invalid(format!(
            "{} is a DuckDB database but not an OxideLens case (no format in ox.meta)",
            path.display()
        ))
    };
    let has_meta: i64 = conn.query_row(
        "SELECT count(*) FROM duckdb_tables() \
         WHERE database_name = current_database() \
           AND schema_name = 'ox' AND table_name = 'meta'",
        [],
        |r| r.get(0),
    )?;
    if has_meta == 0 {
        return Err(not_a_case());
    }
    let text: String = conn
        .query_row("SELECT value FROM ox.meta WHERE key = 'format'", [], |r| {
            r.get(0)
        })
        .map_err(|e| match e {
            duckdb::Error::QueryReturnedNoRows => not_a_case(),
            other => other.into(),
        })?;
    parse_format(&text, path)
}

fn parse_format(text: &str, path: &Path) -> Result<u32> {
    text.parse().map_err(|_| {
        EngineError::Invalid(format!(
            "{} declares a case format that is not a number: {text:?}",
            path.display()
        ))
    })
}

/// DuckDB's write-ahead log sits beside the database as `<file>.wal`.
/// Whether anything is at `path`: a file, a directory, or a link, even one
/// pointing nowhere (which `Path::try_exists` reports as absent).
fn entry_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Refuse when `path` has a write-ahead log beside it: DuckDB would replay a
/// log it finds there into whatever file comes to be at `path`.
fn refuse_wal(path: &Path) -> Result<()> {
    let wal = wal_path(path);
    if entry_exists(&wal)? {
        return Err(EngineError::Invalid(format!(
            "{} already exists; a case is never written beside another's log",
            wal.display()
        )));
    }
    Ok(())
}

/// The longest path, in UTF-16 units, DuckDB 1.5 opens on Windows: it does
/// not take the verbatim `\\?\` form for a database file.
#[cfg(windows)]
const DUCKDB_PATH_MAX: usize = 259;

/// Refuse a path DuckDB could not open, before anything is written.
fn refuse_too_long_for_duckdb(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let absolute = std::path::absolute(path)?;
        if absolute.as_os_str().encode_wide().count() > DUCKDB_PATH_MAX {
            return Err(EngineError::Invalid(format!(
                "{} is too long for DuckDB to write a database at (at most {DUCKDB_PATH_MAX} \
                 characters for the working copy beside it); choose a shorter path",
                path.display()
            )));
        }
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}

/// Remove compaction's working directory: the copy and its log by name,
/// then the directory, which fails if anything else was put in it, rather
/// than delete what this call did not write.
fn remove_staging(staging: &Path, copy: &Path) -> std::io::Result<()> {
    for file in [copy.to_path_buf(), wal_path(copy)] {
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    std::fs::remove_dir(staging)
}

/// Give the finished file `from` the name `to`, which must not exist: a
/// hard link fails when `to` exists, where a rename would replace it on
/// some systems. `from` is left for its caller to remove.
fn publish_new(from: &Path, to: &Path) -> Result<()> {
    refuse_wal(to)?;
    std::fs::hard_link(from, to).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => EngineError::Invalid(format!(
            "{} already exists; a new case never overwrites a file",
            to.display()
        )),
        _ => EngineError::from(e),
    })
}

fn wal_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".wal");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reviewed: a file that appears at the name while the copy is made is
    /// neither replaced nor removed.
    #[test]
    fn publishing_never_replaces_a_file_that_appeared_meanwhile() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("copy.duckdb"), d.path().join("out.duckdb"));
        std::fs::write(&from, b"the copy").unwrap();
        std::fs::write(&to, b"someone else's").unwrap();
        assert_eq!(publish_new(&from, &to).unwrap_err().code(), "invalid");
        assert_eq!(std::fs::read(&to).unwrap(), b"someone else's");
        assert_eq!(std::fs::read(&from).unwrap(), b"the copy");
        std::fs::remove_file(&to).unwrap();
        publish_new(&from, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"the copy");
    }

    /// Reviewed: cleanup removes the copy and its log, and leaves anything
    /// else that was put in the working directory, saying it could not.
    #[test]
    fn cleanup_removes_only_what_compaction_wrote() {
        let d = tempfile::tempdir().unwrap();
        let staging = d.path().join(".oxc-test");
        std::fs::create_dir(&staging).unwrap();
        let copy = staging.join("c.duckdb");
        std::fs::write(&copy, b"copy").unwrap();
        std::fs::write(wal_path(&copy), b"log").unwrap();
        std::fs::write(staging.join("theirs"), b"not ours").unwrap();
        assert!(remove_staging(&staging, &copy).is_err());
        assert_eq!(std::fs::read(staging.join("theirs")).unwrap(), b"not ours");
        assert!(!copy.exists() && !wal_path(&copy).exists());
        std::fs::remove_file(staging.join("theirs")).unwrap();
        std::fs::write(&copy, b"copy").unwrap();
        remove_staging(&staging, &copy).unwrap();
        assert!(!staging.exists());
    }

    /// Reviewed: a link at the log's name counts as a log, even one that
    /// points nowhere. Skipped where links cannot be made (Windows without
    /// the right).
    #[test]
    fn a_dangling_link_counts_as_an_existing_log() {
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("out.duckdb");
        let missing = d.path().join("nowhere");
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&missing, wal_path(&out));
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&missing, wal_path(&out));
        if linked.is_err() {
            return;
        }
        assert!(!wal_path(&out).try_exists().unwrap());
        assert_eq!(refuse_wal(&out).unwrap_err().code(), "invalid");
    }

    /// Reviewed: the working copy's path is what DuckDB opens, so one too
    /// long for it is refused before anything is written.
    #[cfg(windows)]
    #[test]
    fn a_path_too_long_for_duckdb_is_refused_before_anything_is_written() {
        let d = tempfile::tempdir().unwrap();
        let mut parent = d.path().to_path_buf();
        while parent.as_os_str().len() < 250 {
            parent.push("deep-directory-name");
        }
        std::fs::create_dir_all(&parent).unwrap();
        let case = Case::create(
            &d.path().join("a.case.duckdb"),
            "IR-1",
            &Actor::Analyst("tester".into()),
        )
        .unwrap();
        let out = parent.join("x.duckdb");
        assert_eq!(case.compact_to(&out).unwrap_err().code(), "invalid");
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 0);
        // A short directory with a long name: the working copy would fit,
        // the published file would not.
        let long_name = format!("{}.duckdb", "n".repeat(255 - d.path().as_os_str().len()));
        let out = d.path().join(long_name);
        assert_eq!(case.compact_to(&out).unwrap_err().code(), "invalid");
        assert!(!out.exists());
    }

    /// Reviewed: DuckDB takes the path as text, and a lossy one could name
    /// another file, so a path that is not Unicode is refused before
    /// anything is written.
    #[test]
    fn a_path_that_is_not_unicode_is_refused_before_anything_is_written() {
        let d = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let mut name = {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[0xD800])
        };
        #[cfg(unix)]
        let mut name = {
            use std::os::unix::ffi::OsStrExt;
            std::ffi::OsStr::from_bytes(b"\xff").to_os_string()
        };
        name.push(".duckdb");
        let out = d.path().join(name);
        assert!(out.to_str().is_none());
        let case = Case::create(
            &d.path().join("a.case.duckdb"),
            "IR-1",
            &Actor::Analyst("tester".into()),
        )
        .unwrap();
        assert_eq!(case.compact_to(&out).unwrap_err().code(), "invalid");
        let written: Vec<String> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with("a.case.duckdb"))
            .collect();
        assert!(written.is_empty(), "{written:?}");
    }
}
