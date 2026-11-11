//! The case file: one DuckDB database holding the case tables (schema `ox`)
//! and every loaded source (schemas `src_<n>`).
//!
//! The file carries its own format number in `ox.meta`. A file written by a
//! newer build is refused rather than read, because reading a format this
//! build does not know is how annotations get silently misread.

use crate::actor::Actor;
use crate::error::{EngineError, Result};
use crate::time::now_iso;
use duckdb::{params, Connection};
use std::path::{Path, PathBuf};

/// The case file format this build reads and writes.
pub const CASE_FORMAT: u32 = 1;

/// The DDL for a new case, format 1.
const SCHEMA: &str = include_str!("schema.sql");

/// The suffix a case file must carry, so that a case is recognisable as a
/// DuckDB database and `create` cannot be pointed at an unrelated file.
const SUFFIX: &str = ".duckdb";

/// An open case file. Dropping it closes the file and releases its lock.
pub struct Case {
    path: PathBuf,
    conn: Connection,
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

impl Case {
    /// Create a new case file at `path`, which must end in `.duckdb` and
    /// must not exist yet.
    pub fn create(path: &Path, name: &str, actor: &Actor) -> Result<Case> {
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
        if path.try_exists()? {
            return Err(EngineError::Invalid(format!(
                "{} already exists; a new case never overwrites a file",
                path.display()
            )));
        }
        let mut conn = connect(path)?;
        match initialise(&mut conn, name, actor) {
            Ok(()) => Ok(Case {
                path: path.to_path_buf(),
                conn,
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

    /// Open an existing case file.
    pub fn open(path: &Path) -> Result<Case> {
        if !path.try_exists()? {
            return Err(EngineError::NotFound(format!(
                "no case file at {}",
                path.display()
            )));
        }
        let conn = connect(path)?;
        let format = read_format(&conn, path)?;
        if format > CASE_FORMAT {
            return Err(EngineError::NeedsUpgrade {
                found: format,
                expected: CASE_FORMAT,
            });
        }
        if format < CASE_FORMAT {
            return Err(EngineError::Invalid(format!(
                "{} declares case format {format}, which never existed",
                path.display()
            )));
        }
        Ok(Case {
            path: path.to_path_buf(),
            conn,
        })
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
                    duckdb::Error::QueryReturnedNoRows => {
                        EngineError::Invalid(format!("the case has no {key} in ox.meta"))
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

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The first column of the first row of `sql`, for integration tests.
    #[doc(hidden)]
    pub fn scalar_i64(&self, sql: &str) -> Result<i64> {
        Ok(self.conn().query_row(sql, [], |r| r.get(0))?)
    }
}

fn initialise(conn: &mut Connection, name: &str, actor: &Actor) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute_batch(SCHEMA)?;
    let format = CASE_FORMAT.to_string();
    let created_by = actor.label();
    let created_at = now_iso();
    for (key, value) in [
        ("name", name),
        ("format", format.as_str()),
        ("created_by", created_by.as_str()),
        ("created_at", created_at.as_str()),
    ] {
        tx.execute(
            "INSERT INTO ox.meta (key, value) VALUES (?, ?)",
            params![key, value],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Open the database, turning DuckDB's "someone else holds this file" into
/// `CaseLocked`.
fn connect(path: &Path) -> Result<Connection> {
    Connection::open(path).map_err(|e| {
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
fn is_lock_conflict(e: &duckdb::Error) -> bool {
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

fn read_format(conn: &Connection, path: &Path) -> Result<u32> {
    let not_a_case = || {
        EngineError::Invalid(format!(
            "{} is a DuckDB database but not an OxideLens case (no format in ox.meta)",
            path.display()
        ))
    };
    let has_meta: i64 = conn.query_row(
        "SELECT count(*) FROM duckdb_tables() \
         WHERE schema_name = 'ox' AND table_name = 'meta'",
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
fn wal_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".wal");
    PathBuf::from(name)
}
