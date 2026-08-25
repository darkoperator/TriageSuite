//! Spec section 10.3 compatibility tests: RBTriage vs RBCmd reference fixtures.

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use triage_testkit::{compare_csv_composite, composite_sid_basename, AcceptedDelta};

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rbcmd")
}

fn capture_dir(needle: &str) -> Option<PathBuf> {
    std::fs::read_dir(captures_root())
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir() && p.file_name().unwrap().to_string_lossy().contains(needle))
}

fn accepted() -> Vec<AcceptedDelta> {
    vec![]
}

#[test]
fn compat_stcl1_recycle_bin() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    if triage_testkit::skip_if_missing(&fixture_root(), "rbcmd fixtures") {
        return;
    }
    let cap = capture_dir("STCL1").expect("STCL1 capture");
    let ref_csv = fixture_root().join("STCL1/rbcmd.csv");
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("RBTriage")
        .unwrap()
        .arg("-d")
        .arg(&cap)
        .arg("--csv")
        .arg(tmp.path())
        .arg("-q")
        .assert()
        .success();

    // FLAT layout: output lands directly under the run root with the identity
    // encoded in the filename; the recursive finder locates it by suffix.
    let ours = find_one(tmp.path(), "RBTriage_Output.csv").expect("RBTriage CSV produced");

    let d = compare_csv_composite(
        &ref_csv,
        &ours,
        &["SourceName"],
        composite_sid_basename,
        &accepted(),
    );
    assert!(
        d.is_match(),
        "RBCmd compat: {}/{} rows, mismatches: {:#?}",
        d.reference_rows,
        d.our_rows,
        d.mismatches
    );
}

fn find_one(root: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .is_some_and(|f| f.to_string_lossy().ends_with(name))
            {
                return Some(p);
            }
        }
    }
    None
}
