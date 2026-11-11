//! The SQL path's tests that need a particular current directory. A query
//! too big for memory must fail instead of spilling to disk (DuckDB's
//! default temporary directory is `.tmp` under the current directory). A
//! bare file name in FROM must not be scanned from the current directory.
//! These tests change the process's current directory, so they live in their
//! own test binary and take `CWD` one at a time.

mod common;

use common::loaded;
use oxl_engine::query_sql;
use std::path::Path;
use std::sync::Mutex;

/// Held while a test has changed the current directory.
static CWD: Mutex<()> = Mutex::new(());

/// Run `f` with the current directory set to `dir`, and set it back after.
fn in_dir<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let _turn = CWD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let before = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir).unwrap();
    let out = f();
    std::env::set_current_dir(before).unwrap();
    out
}

/// Every file and directory under `dir` whose name looks like a DuckDB spill.
fn spills(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name == ".tmp" || name.starts_with("duckdb_temp_storage") {
            found.push(path.clone());
        }
        if path.is_dir() {
            found.extend(spills(&path));
        }
    }
    found
}

#[test]
fn a_query_over_the_memory_limit_is_invalid_and_spills_nothing() {
    let (d, case) = loaded();
    let path = case.path().to_path_buf();
    drop(case);
    let scratch = tempfile::tempdir().unwrap();

    let result = in_dir(scratch.path(), || {
        query_sql(
            &path,
            "SELECT count(*) FROM \
             (SELECT range % 200000000 AS g, count(*) FROM range(400000000) GROUP BY ALL)",
            10,
            0,
        )
    });

    let e = result.unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");
    assert!(e.to_string().contains("more memory"), "{e}");
    assert_eq!(spills(scratch.path()), Vec::<std::path::PathBuf>::new());
    assert_eq!(spills(d.path()), Vec::<std::path::PathBuf>::new());
}

#[test]
fn a_bare_database_file_name_in_the_cwd_is_not_scanned() {
    // Unquoted, `one.duckdb` has no path character in any part, yet DuckDB
    // would answer it by scanning ./one.duckdb, here a copy of the case.
    let (_d, case) = loaded();
    let path = case.path().to_path_buf();
    drop(case);
    let scratch = tempfile::tempdir().unwrap();
    std::fs::copy(&path, scratch.path().join("one.duckdb")).unwrap();

    let result = in_dir(scratch.path(), || {
        query_sql(&path, "select * from one.duckdb", 10, 0)
    });

    let e = result.unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");
}
