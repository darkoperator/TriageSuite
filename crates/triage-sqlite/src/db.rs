//! Open a SQLite database for read-only forensic access without mutating
//! evidence. Two paths:
//!
//! * **Companions present** (`-wal` and/or `-shm` exist): copy the database and
//!   its companions into a temp directory and open the COPY read-write, so
//!   SQLite checkpoints the WAL into the main file (newest records live in the
//!   WAL). The original evidence files are never opened for writing.
//! * **No companions**: open the original with `immutable=1` — read-only, no
//!   locks, never creates a journal/WAL.
//!
//! The temp working set (when used) is removed when the `Database` is dropped.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

/// An open SQLite database. Holds the live `rusqlite::Connection` and, when a
/// temp working copy was made, the `TempDir` that owns it (dropped here).
pub struct Database {
    pub(crate) conn: Connection,
    // Kept alive so the temp working set is cleaned up on drop. `None` for the
    // immutable-open path.
    _work: Option<tempfile::TempDir>,
}

impl Database {
    /// Open `path` honoring the evidence-integrity rules above.
    pub fn open(path: &Path) -> Result<Database, rusqlite::Error> {
        let companions = companion_paths(path);
        let has_companions = companions.iter().any(|c| c.exists());

        if has_companions {
            Database::open_with_wal_merge(path, &companions)
        } else {
            Database::open_immutable(path)
        }
    }

    fn open_immutable(path: &Path) -> Result<Database, rusqlite::Error> {
        // immutable=1 promises the file will not change; SQLite skips locking
        // and never writes a journal/WAL. Read-only flags belt-and-suspenders.
        let uri = format!("file:{}?immutable=1&mode=ro", uri_encode(path));
        let conn = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?;
        Ok(Database { conn, _work: None })
    }

    fn open_with_wal_merge(
        path: &Path,
        companions: &[PathBuf],
    ) -> Result<Database, rusqlite::Error> {
        let work = tempfile::tempdir().map_err(|e| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                Some(format!("temp dir: {e}")),
            )
        })?;
        let file_name = path.file_name().unwrap_or_default();
        let work_db = work.path().join(file_name);
        // Copy main db + every existing companion, preserving file names so
        // SQLite finds `<db>-wal`/`<db>-shm` next to the copied main file.
        copy_file(path, &work_db)?;
        for c in companions {
            if c.exists() {
                let dest = work.path().join(c.file_name().unwrap_or_default());
                copy_file(c, &dest)?;
            }
        }
        // Open the COPY read-write so the WAL is checkpointed into the main db.
        let conn = Connection::open(&work_db)?;
        // Force a full checkpoint so all WAL records are merged before queries.
        let _ = conn.pragma_update(None, "wal_checkpoint", "TRUNCATE");
        Ok(Database {
            conn,
            _work: Some(work),
        })
    }
}

/// The `-wal` and `-shm` companion paths for a database path.
fn companion_paths(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(2);
    for suffix in ["-wal", "-shm"] {
        let mut s = path.as_os_str().to_owned();
        s.push(suffix);
        out.push(PathBuf::from(s));
    }
    out
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), rusqlite::Error> {
    std::fs::copy(src, dst).map_err(|e| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
            Some(format!("copy {} -> {}: {e}", src.display(), dst.display())),
        )
    })?;
    Ok(())
}

/// Encode a filesystem path for a SQLite `file:` URI (minimal: spaces and a
/// few reserved characters). Capture paths often contain percent-escaped
/// Windows segments (e.g. `C%3A`). We must re-encode the literal `%` as
/// `%25` so SQLite's URI parser sees the on-disk name rather than
/// URL-decoding it; similarly `&` must become `%26` to avoid being parsed as
/// a URI parameter separator.
fn uri_encode(path: &Path) -> String {
    let s = path.to_string_lossy();
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '&' => out.push_str("%26"),
            ' ' => out.push_str("%20"),
            '?' => out.push_str("%3f"),
            '#' => out.push_str("%23"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_db(path: &Path) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(
            "CREATE TABLE t(id INTEGER, name TEXT);
             INSERT INTO t VALUES (1, 'alpha'), (2, 'beta');",
        )
        .unwrap();
    }

    #[test]
    fn immutable_open_reads_a_plain_database() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("plain.db");
        make_db(&db_path);

        let db = Database::open(&db_path).unwrap();
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn wal_companion_is_merged_via_temp_copy() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("waldb.db");
        // Create a db in WAL mode and write a row WITHOUT checkpointing, so the
        // new row lives only in the -wal file.
        {
            let c = Connection::open(&db_path).unwrap();
            c.pragma_update(None, "journal_mode", "WAL").unwrap();
            c.execute_batch("CREATE TABLE t(id INTEGER);").unwrap();
            c.execute_batch("INSERT INTO t VALUES (1);").unwrap();
            // Leak the connection so SQLite does not auto-checkpoint on close.
            std::mem::forget(c);
        }
        assert!(db_path.with_file_name("waldb.db-wal").exists());

        let db = Database::open(&db_path).unwrap();
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "row in the WAL must be visible after merge");

        // Evidence integrity: the original -wal still exists (we copied, not moved).
        assert!(db_path.with_file_name("waldb.db-wal").exists());
    }

    #[test]
    fn immutable_open_handles_percent_escaped_capture_paths() {
        // Real capture paths contain percent-escaped Windows segments like "C%3A".
        // The immutable file: URI must encode '%' so SQLite opens the literal
        // on-disk name rather than URL-decoding it.
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("C%3A").join("Users");
        std::fs::create_dir_all(&sub).unwrap();
        let db_path = sub.join("ActivitiesCache.db");
        make_db(&db_path);

        let db = Database::open(&db_path).unwrap();
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }
}
