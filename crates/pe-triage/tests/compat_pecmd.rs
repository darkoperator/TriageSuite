//! Spec section 10.3 compatibility tests: PETriage vs PECmd reference fixtures.
//!
//! The fixtures in tests/fixtures/pecmd/ were produced by real PECmd run
//! against pre-decompressed copies of the client captures (see
//! scripts/gen-pecmd-fixtures.sh). These tests run PETriage on the original
//! captures and require the outputs to match modulo the named accepted
//! deltas below — any other divergence fails the milestone gate.

use assert_cmd::Command;
use std::path::{Path, PathBuf};
use triage_testkit::{compare_csv, compare_csv_unkeyed, compare_ndjson, AcceptedDelta};

/// Walk `dir` recursively and return the first entry whose file name ends with
/// `suffix`. Output basenames now carry a `<yyyyMMddHHmmss>_` timestamp prefix,
/// so the exact name is not known ahead of time.
fn find_ending(dir: &Path, suffix: &str) -> PathBuf {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&d) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(suffix))
                {
                    return p;
                }
            }
        }
    }
    panic!(
        "no file ending with {suffix:?} found under {}",
        dir.display()
    );
}

fn fixture(capture: &str, name: &str) -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/pecmd")
        .join(capture)
        .join(name);
    p.exists().then_some(p)
}

fn capture_dir(needle: &str) -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir() && p.file_name().unwrap().to_string_lossy().contains(needle))
}

/// Named accepted incompatibilities (spec 10.3: no silent exclusions).
fn accepted() -> Vec<AcceptedDelta> {
    vec![
        AcceptedDelta {
            field: "SourceCreated",
            reason: "filesystem metadata of the decompressed staging copy used \
                     for fixture generation differs from the original capture file",
            compare: |_r, _o| true,
            row_guard: None,
        },
        AcceptedDelta {
            field: "SourceModified",
            reason: "same as SourceCreated",
            compare: |_r, _o| true,
            row_guard: None,
        },
        AcceptedDelta {
            field: "SourceAccessed",
            reason: "atime changes on every read of the evidence copy",
            compare: |_r, _o| true,
            row_guard: None,
        },
        AcceptedDelta {
            field: "ParsingError",
            reason: "C# bool literal (False / JSON false) vs our CSV-faithful \
                     string False; compared case-insensitively on string form",
            compare: |r, o| r.eq_ignore_ascii_case(o),
            row_guard: None,
        },
    ]
}

fn run_petriage(capture: &Path, out: &Path) {
    Command::cargo_bin("PETriage")
        .unwrap()
        .arg("-d")
        .arg(capture)
        .arg("--csv")
        .arg(out)
        .arg("--json")
        .arg(out)
        .arg("-q")
        .assert()
        .success();
}

fn compat_for(capture_needle: &str, fixture_dir: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
    if triage_testkit::skip_if_missing(&root, "test captures") {
        return;
    }
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/pecmd");
    if triage_testkit::skip_if_missing(&fixture_root, "pecmd fixtures") {
        return;
    }
    let (cap, ref_csv, ref_tl, ref_json) = (
        capture_dir(capture_needle).expect("capture dir"),
        fixture(fixture_dir, "pecmd.csv").expect("fixture csv"),
        fixture(fixture_dir, "pecmd_Timeline.csv").expect("fixture tl"),
        fixture(fixture_dir, "pecmd.json").expect("fixture json"),
    );
    let tmp = tempfile::tempdir().unwrap();
    run_petriage(&cap, tmp.path());
    // FLAT layout: outputs land directly under the run root with the identity
    // encoded in the filename; the recursive finder locates them by suffix.
    let ours = tmp.path().to_path_buf();

    let d = compare_csv(
        &ref_csv,
        &find_ending(&ours, "PETriage_Output.csv"),
        "SourceFilename",
        &accepted(),
    );
    assert!(
        d.is_match(),
        "main CSV ({fixture_dir}): {}/{} rows, mismatches: {:#?}",
        d.reference_rows,
        d.our_rows,
        d.mismatches
    );

    let d = compare_csv_unkeyed(
        &ref_tl,
        &find_ending(&ours, "PETriage_Output_Timeline.csv"),
        &[],
        &accepted(),
    );
    assert!(
        d.is_match(),
        "timeline CSV ({fixture_dir}): {}/{} rows, mismatches: {:#?}",
        d.reference_rows,
        d.our_rows,
        d.mismatches
    );

    let d = compare_ndjson(
        &ref_json,
        &find_ending(&ours, "PETriage_Output.json"),
        "SourceFilename",
        &accepted(),
    );
    assert!(
        d.is_match(),
        "JSON ({fixture_dir}): {}/{} rows, mismatches: {:#?}",
        d.reference_rows,
        d.our_rows,
        d.mismatches
    );
}

#[test]
fn compat_desktop_capture() {
    compat_for("DESKTOP", "DESKTOP");
}

#[test]
fn compat_stcl1_capture() {
    compat_for("STCL1", "STCL1");
}
