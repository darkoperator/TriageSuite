//! Helpers shared by the engine's integration tests.

// Each test file compiles this module on its own and uses only some of it.
#![allow(dead_code)]

use oxl_engine::{load_run, Actor, Case};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};

pub fn analyst() -> Actor {
    Actor::Analyst("tester".into())
}

pub fn agent() -> Actor {
    Actor::Agent("claude".into())
}

/// A new case with the two-host run loaded. Keep the directory alive for as
/// long as the case: it holds both the case file and the run.
pub fn loaded() -> (tempfile::TempDir, Case) {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(d.path());
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &root, &analyst()).unwrap();
    (d, case)
}

/// Copy a run tree, so a test can alter the copy and never the run the
/// fixture built.
pub fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A directory under `base`, created, whose own path is already past the 260
/// characters the classic Windows file API accepts, so that every file
/// written below it is too.
pub fn long_dir(base: &Path) -> std::path::PathBuf {
    let mut dir = base.to_path_buf();
    while dir.as_os_str().len() <= 260 {
        dir.push("d".repeat(40));
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The first column of the first row of `sql`, as an integer.
pub fn n(case: &Case, sql: &str) -> i64 {
    case.scalar_i64(sql)
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
}

/// Another process holding the case file at `path` open read-write, which
/// is the only portable way to hold DuckDB's file lock against this one.
pub struct Holder(Child);

impl Holder {
    /// Start `hold_case` on `path` and wait until it holds the file.
    pub fn start(path: &Path) -> Holder {
        let mut holder = Command::new(env!("CARGO"));
        holder
            .args([
                "run",
                "-q",
                "-p",
                "oxl-fixtures",
                "--example",
                "hold_case",
                "--",
            ])
            .arg(path);
        // Cargo gives this test process its per-package variables, and a
        // nested cargo would pass them on to build scripts. ring's tracks
        // CARGO_MANIFEST_DIR, so inheriting it rebuilds ring and, through it,
        // DuckDB on every run -- and then again for the next ordinary build.
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy();
            let per_package = [
                "CARGO_MANIFEST_",
                "CARGO_PKG_",
                "CARGO_CRATE_",
                "CARGO_BIN_",
            ]
            .iter()
            .any(|prefix| key.starts_with(prefix))
                || key == "CARGO_PRIMARY_PACKAGE"
                || key == "CARGO_TARGET_TMPDIR";
            if per_package {
                holder.env_remove(key.as_ref());
            }
        }
        let mut child = holder
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(
            line.trim(),
            "held",
            "hold_case did not report holding the case"
        );
        Holder(child)
    }

    /// Let the holder close the file and exit, and check it exited cleanly.
    pub fn release(mut self) {
        // Closing stdin is what lets the holder exit.
        drop(self.0.stdin.take());
        let status = self.0.wait().unwrap();
        assert!(status.success(), "hold_case exited with {status}");
    }
}
