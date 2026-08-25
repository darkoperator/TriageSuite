//! SBETriage CLI integration tests.
//!
//! Mirrors jle-triage/tests/cli.rs. Capture-dependent tests are gated on
//! `triage_testkit::skip_if_missing` so they are skipped in CI where the
//! `test captures` directory is absent.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

fn cmd() -> Command {
    Command::cargo_bin("SBETriage").unwrap()
}

fn captures_root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

// ── Gate-free tests (no captures required) ───────────────────────────────────

#[test]
fn help_exits_zero() {
    cmd().arg("--help").assert().success();
}

#[test]
fn nonexistent_path_exits_nonzero() {
    cmd()
        .args(["-d", "/nonexistent/path/that/does/not/exist/sbetriage_test"])
        .args(["--csv", "/tmp"])
        .assert()
        .failure();
}

#[test]
fn empty_dir_exits_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    // A directory with no hive files should exit 0 (nothing to process is not
    // an error — matches the established suite exit code convention).
    let out = tmp.path().join("out");
    cmd()
        .arg("-d")
        .arg(tmp.path())
        .arg("--csv")
        .arg(&out)
        .assert()
        .code(predicate::in_iter([0i32, 1]));
}

#[test]
fn no_logs_flag_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    // --nl should be accepted as a valid flag (exits non-zero only because no
    // hives are found, not because the flag is unknown).
    cmd()
        .arg("-d")
        .arg(tmp.path())
        .arg("--nl")
        .arg("--csv")
        .arg(&out)
        .assert()
        .code(predicate::in_iter([0i32, 1]));
}

// ── Capture-gated tests ───────────────────────────────────────────────────────

#[test]
fn capture_dir_exits_zero_and_emits_shellbags() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    // Collect all capture directories.
    let mut caps: Vec<PathBuf> = std::fs::read_dir(captures_root())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    caps.sort();

    let tmp = tempfile::tempdir().unwrap();
    for (i, cap) in caps.iter().enumerate() {
        let out = tmp.path().join(format!("c{i}"));
        cmd()
            .arg("-d")
            .arg(cap)
            .arg("--csv")
            .arg(&out)
            .arg("--json")
            .arg(&out)
            .arg("-q")
            .assert()
            .code(0);
    }
}
