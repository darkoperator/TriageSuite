//! RETriage integration CLI tests. Mirrors jle-triage/tests/cli.rs structure.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::Path;

fn cmd() -> Command {
    Command::cargo_bin("RETriage").unwrap()
}

fn captures_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

/// `--help` prints usage and exits 0.
#[test]
fn help_exits_zero() {
    cmd()
        .arg("--help")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("RETriage"));
}

/// Running with a nonexistent file path exits non-zero.
#[test]
fn nonexistent_file_exits_nonzero() {
    cmd()
        .arg("-f")
        .arg("/tmp/does-not-exist-retriage-testfile.DAT")
        .arg("--csv")
        .arg("/tmp")
        .assert()
        .failure();
}

/// A directory with no hive files exits with code 2 (no matching files found)
/// or code 0 with 0 records — either indicates clean handling of empty dirs.
#[test]
fn empty_dir_exits_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    // An empty dir has no hive files, so the runner should exit with code 2
    // (no matching artifacts) or code 0 (no records). Either is acceptable.
    cmd()
        .arg("-d")
        .arg(dir.path())
        .arg("--csv")
        .arg(out.path())
        .assert()
        .code(predicate::in_iter([0i32, 2]));
}

/// Smoke test: run against the real captures if available.
#[test]
fn captures_produce_batch_rows() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    // Find a SOFTWARE hive in the captures.
    let mut hive_path = None;
    let mut stack = vec![captures_root()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "SOFTWARE") {
                hive_path = Some(p);
                break;
            }
        }
        if hive_path.is_some() {
            break;
        }
    }
    let Some(hive) = hive_path else {
        eprintln!("SKIP: no SOFTWARE hive found in captures");
        return;
    };
    let out = tempfile::tempdir().unwrap();
    cmd()
        .arg("-f")
        .arg(&hive)
        .arg("--csv")
        .arg(out.path())
        .arg("--nl") // skip log pairing for speed in CI
        .assert()
        .code(predicate::in_iter([0i32, 1]))
        .stderr(predicate::str::contains("Records emitted:"));
}
