//! LETriage integration matrix + functional CLI tests.
//!
//! Modeled on rb-triage/tests/cli.rs. All three captures contain LNK files,
//! so unlike PETriage/RBTriage every capture must yield output here.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::{Path, PathBuf};

fn cmd() -> Command {
    Command::cargo_bin("LETriage").unwrap()
}

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

/// All capture directories under `test captures/`.
fn capture_dirs() -> Vec<PathBuf> {
    let mut caps: Vec<PathBuf> = std::fs::read_dir(captures_root())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        // Only real Velociraptor capture dirs (`Collection-*`); ignore any other
        // content placed under `test captures/` (e.g. user output folders).
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("Collection"))
                .unwrap_or(false)
        })
        .collect();
    caps.sort();
    caps
}

/// Count data rows (lines minus header) of a CSV, or 0 when absent.
fn csv_data_rows(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|s| s.lines().count().saturating_sub(1))
        .unwrap_or(0)
}

/// All `LETriage_Output.csv` files anywhere under `root`.
fn all_output_csvs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|n| n.ends_with("LETriage_Output.csv"))
            {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn three_capture_integration_matrix() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let caps = capture_dirs();
    assert_eq!(
        caps.len(),
        3,
        "expected exactly three captures, got {}",
        caps.len()
    );

    let tmp = tempfile::tempdir().unwrap();
    for (i, c) in caps.iter().enumerate() {
        let out = tmp.path().join(format!("c{i}"));
        cmd()
            .arg("-d")
            .arg(c)
            .arg("--csv")
            .arg(&out)
            .arg("--json")
            .arg(&out)
            .arg("-q")
            .assert()
            .code(0)
            .stdout(predicate::str::is_empty());

        let csvs = all_output_csvs(&out);
        assert!(
            !csvs.is_empty(),
            "capture {} produced no LETriage_Output.csv",
            c.display()
        );
        let total: usize = csvs.iter().map(|p| csv_data_rows(p)).sum();
        assert!(
            total > 0,
            "capture {} produced 0 LNK rows (all three have LNKs)",
            c.display()
        );
    }
}

#[test]
fn explicit_file_mode_single_lnk() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let lnk = capture_dirs()
        .iter()
        .find_map(|c| first_lnk(c))
        .expect("a real .lnk in the captures");
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .arg("-f")
        .arg(&lnk)
        .arg("--csv")
        .arg(tmp.path())
        .arg("--json")
        .arg(tmp.path())
        .assert()
        .code(0)
        .stderr(predicate::str::contains("Records emitted: 1"));
}

#[test]
fn removable_only_reduces_output() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let cap = capture_dirs().into_iter().next().expect("a capture");
    let tmp = tempfile::tempdir().unwrap();

    // Full run row count.
    let full = tmp.path().join("full");
    cmd()
        .arg("-d")
        .arg(&cap)
        .arg("--csv")
        .arg(&full)
        .arg("-q")
        .assert()
        .code(0);
    let full_rows: usize = all_output_csvs(&full)
        .iter()
        .map(|p| csv_data_rows(p))
        .sum();

    // Removable-only run: must not crash, and yields <= the full count (these
    // are fixed-drive LNKs, so typically 0).
    let rem = tmp.path().join("rem");
    cmd()
        .arg("-d")
        .arg(&cap)
        .arg("-r")
        .arg("--csv")
        .arg(&rem)
        .arg("-q")
        .assert()
        .code(0);
    let rem_rows: usize = all_output_csvs(&rem).iter().map(|p| csv_data_rows(p)).sum();
    assert!(
        rem_rows <= full_rows,
        "--removable-only produced {rem_rows} rows, more than the full {full_rows}"
    );
}

#[test]
fn codepage_932_parses_without_error() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let lnk = capture_dirs()
        .iter()
        .find_map(|c| first_lnk(c))
        .expect("a real .lnk in the captures");
    let tmp = tempfile::tempdir().unwrap();
    // Shift-JIS (cp932) decode path must not panic on a Latin LNK.
    cmd()
        .arg("--cp")
        .arg("932")
        .arg("-f")
        .arg(&lnk)
        .arg("--csv")
        .arg(tmp.path())
        .assert()
        .code(0);
}

#[test]
fn output_routing_user_vs_system() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    // A capture containing a LNK under Users/<u>/ must route into
    // LETriage/users/<u>/; system-scoped LNKs route into LETriage/system/.
    let cap = capture_dirs()
        .into_iter()
        .find(|c| lnk_under_users(c))
        .expect("a capture with a Users/<u>/ LNK");
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .arg("-d")
        .arg(&cap)
        .arg("--csv")
        .arg(tmp.path())
        .arg("-q")
        .assert()
        .code(0);

    // FLAT layout: identity is encoded as the filename prefix, not a directory.
    // A LNK under Users/<u>/ yields `<u>_<stamp>_LETriage_Output.csv`; the
    // system remainder (Start Menu / ProgramData) yields `system_...`.
    let csvs = all_output_csvs(tmp.path());
    let prefixes: Vec<String> = csvs
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .map(|n| n.split('_').next().unwrap_or("").to_string())
        .collect();
    assert!(
        prefixes
            .iter()
            .any(|p| p != "system" && p != "unknown" && !p.is_empty()),
        "expected at least one user-identity-prefixed LETriage_Output.csv, got {prefixes:?}"
    );
}

fn first_lnk(root: &Path) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
                return Some(p);
            }
        }
    }
    None
}

/// True when the capture contains a `.lnk` somewhere beneath a `Users` segment.
fn lnk_under_users(root: &Path) -> bool {
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk"))
                && p.components().any(|c| {
                    c.as_os_str()
                        .to_string_lossy()
                        .eq_ignore_ascii_case("Users")
                })
            {
                return true;
            }
        }
    }
    false
}
