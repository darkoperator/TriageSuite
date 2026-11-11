//! A query too big for memory fails instead of spilling to disk. DuckDB's
//! default temporary directory is `.tmp` under the current directory, so
//! this test changes the process's current directory, and it lives in its
//! own test binary so no other test runs beside it.

mod common;

use common::loaded;
use oxl_engine::query_sql;
use std::path::Path;

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
    let before = std::env::current_dir().unwrap();
    std::env::set_current_dir(scratch.path()).unwrap();

    let result = query_sql(
        &path,
        "SELECT count(*) FROM \
         (SELECT range % 200000000 AS g, count(*) FROM range(400000000) GROUP BY ALL)",
        10,
        0,
    );

    std::env::set_current_dir(before).unwrap();
    let e = result.unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");
    assert!(e.to_string().contains("more memory"), "{e}");
    assert_eq!(spills(scratch.path()), Vec::<std::path::PathBuf>::new());
    assert_eq!(spills(d.path()), Vec::<std::path::PathBuf>::new());
}
