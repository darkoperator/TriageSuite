//! Spec section 10.3 compatibility gate: LETriage vs LECmd reference fixtures.
//!
//! The fixtures in tests/fixtures/lecmd/{DESKTOP,STCL1,STDC1}/ were produced by
//! real LECmd. LECmd writes ONE combined CSV/JSON per capture (all LNKs
//! together); LETriage splits its output by identity (users/<u>/, system/). To
//! compare, we AGGREGATE every per-identity LETriage_Output.{csv,json} back into
//! one combined file, then compare keyed by the SourceFile basename.

use assert_cmd::Command;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use triage_testkit::{compare_csv_composite, compare_ndjson_composite, AcceptedDelta};

/// Key a row by the capture-relative suffix of its SourceFile. LECmd writes
/// many same-named LNKs (e.g. `Microsoft Edge.lnk`) under different directories
/// into one file, so a basename-only key collides. Both the fixture and our
/// output share the in-capture path beginning at the `uploads/` collection
/// segment; everything before it is host prefix the harness already excuses.
/// Falls back to the last four path segments when no anchor is present.
fn source_suffix_key(row: &BTreeMap<String, String>, _path_cols: &[&str]) -> String {
    let v = row.get("SourceFile").cloned().unwrap_or_default();
    let segs: Vec<&str> = v.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if let Some(pos) = segs.iter().position(|s| s.eq_ignore_ascii_case("uploads")) {
        return segs[pos..].join("/");
    }
    let start = segs.len().saturating_sub(4);
    segs[start..].join("/")
}

fn fixture(capture: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/lecmd")
        .join(capture)
        .join(name)
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
            reason: "fs metadata of the capture copy differs from LECmd's run-time \
                     metadata on the original system",
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
        // The Windows Search "Users property view" shell item (class 0x00 with
        // an embedded PropertyStore) is the only shell-item subtype not yet
        // rendered structurally: extracting its name and the MFT reference
        // buried in its property sheets requires a full PropertyStore parser.
        // It affects exactly 4 of STDC1's 117 rows (the CLSID_SearchFolder
        // search-query LNKs: powershe, power, Microsoft-Windows-PowerShell…,
        // Windows PowerShell). All other shell-item types — 0x1f/0x2e/0x2f/0x31/
        // 0x32 incl. @resource localized names, 0x01/0x61/0x71/0x74 — match
        // exactly across all three captures.
        //
        // Scoped narrowly so it cannot mask a real divergence:
        //  - TargetIDAbsolutePath is excused ONLY when the reference path begins
        //    with `CLSID_SearchFolder` (i.e. one of these 4 search items).
        //    row_guard: None — the compare closure is already self-bounding on
        //    the reference value, so no additional row-level guard is needed.
        AcceptedDelta {
            field: "TargetIDAbsolutePath",
            reason: "Windows Search property-view (0x00 PropertyStore) item; \
                     4 CLSID_SearchFolder rows in STDC1 only — see module note",
            compare: |reference, _ours| reference.starts_with("CLSID_SearchFolder"),
            row_guard: None,
        },
        //  - The MFT entry/sequence (buried in the same property view's beef0004)
        //    is excused ONLY when ours is empty AND the reference is a hex MFT id
        //    `0x…`, AND the row is itself a Windows Search property-view row
        //    (TargetIDAbsolutePath starts with CLSID_SearchFolder).  The row
        //    guard is the key tightening: without it the `ours.is_empty() &&
        //    reference.starts_with("0x")` predicate would also silently accept a
        //    regression on any of the 68 ordinary STDC1 LNKs whose MFT extraction
        //    broke and returned an empty string.
        AcceptedDelta {
            field: "TargetMFTEntryNumber",
            reason: "MFT reference inside a Windows Search property-view item; \
                     2 STDC1 rows only — see module note",
            compare: |reference, ours| ours.is_empty() && reference.starts_with("0x"),
            row_guard: Some(|row| {
                row.get("TargetIDAbsolutePath")
                    .is_some_and(|p| p.starts_with("CLSID_SearchFolder"))
            }),
        },
        AcceptedDelta {
            field: "TargetMFTSequenceNumber",
            reason: "MFT sequence inside a Windows Search property-view item; \
                     2 STDC1 rows only — see module note",
            compare: |reference, ours| ours.is_empty() && reference.starts_with("0x"),
            row_guard: Some(|row| {
                row.get("TargetIDAbsolutePath")
                    .is_some_and(|p| p.starts_with("CLSID_SearchFolder"))
            }),
        },
    ]
}

fn run_letriage(capture: &Path, out: &Path) {
    Command::cargo_bin("LETriage")
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

/// Walk the run root collecting every `filename` (CSV) — flat layout puts each
/// identity's output there — concatenate them into one combined CSV (header once
/// + all data rows), write it to `dest`, return it.
fn aggregate_csv(output_root: &Path, filename: &str, dest: &Path) -> PathBuf {
    // FLAT layout: per-identity outputs land directly under the run root; the
    // recursive finder matches each by its `*<filename>` suffix.
    let files = find_all(output_root, filename);
    assert!(
        !files.is_empty(),
        "no {filename} produced under {}",
        output_root.display()
    );
    let mut header: Option<String> = None;
    let mut body = String::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        let mut lines = text.lines();
        if let Some(h) = lines.next() {
            match &header {
                None => header = Some(h.to_string()),
                Some(existing) => assert_eq!(existing, h, "header drift across {filename} parts"),
            }
        }
        for line in lines {
            body.push_str(line);
            body.push('\n');
        }
    }
    let mut combined = header.expect("at least one header");
    combined.push('\n');
    combined.push_str(&body);
    std::fs::write(dest, combined).unwrap();
    dest.to_path_buf()
}

/// Walk the run root collecting every `filename` (NDJSON) — flat layout puts
/// each identity's output there — concatenate their non-empty lines into one
/// combined NDJSON file, write it to `dest`, return it.
fn aggregate_ndjson(output_root: &Path, filename: &str, dest: &Path) -> PathBuf {
    let files = find_all(output_root, filename);
    assert!(
        !files.is_empty(),
        "no {filename} produced under {}",
        output_root.display()
    );
    let mut body = String::new();
    for f in &files {
        for line in std::fs::read_to_string(f).unwrap().lines() {
            if !line.trim().is_empty() {
                body.push_str(line);
                body.push('\n');
            }
        }
    }
    std::fs::write(dest, body).unwrap();
    dest.to_path_buf()
}

fn find_all(root: &Path, name: &str) -> Vec<PathBuf> {
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
                .is_some_and(|n| n.ends_with(name))
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn compat_for(capture_needle: &str, fixture_dir: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
    if triage_testkit::skip_if_missing(&root, "test captures") {
        return;
    }
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/lecmd");
    if triage_testkit::skip_if_missing(&fixture_root, "lecmd fixtures") {
        return;
    }

    let cap = capture_dir(capture_needle).expect("capture dir");
    let ref_csv = fixture(fixture_dir, "lecmd.csv");
    let ref_json = fixture(fixture_dir, "lecmd.json");

    let tmp = tempfile::tempdir().unwrap();
    run_letriage(&cap, tmp.path());

    let combined_csv = aggregate_csv(
        tmp.path(),
        "LETriage_Output.csv",
        &tmp.path().join("combined.csv"),
    );
    let d = compare_csv_composite(
        &ref_csv,
        &combined_csv,
        &["SourceFile"],
        source_suffix_key,
        &accepted(),
    );
    assert!(
        d.is_match(),
        "CSV ({fixture_dir}): reference {} / ours {} rows, mismatches: {:#?}",
        d.reference_rows,
        d.our_rows,
        d.mismatches
    );

    let combined_json = aggregate_ndjson(
        tmp.path(),
        "LETriage_Output.json",
        &tmp.path().join("combined.json"),
    );
    let d = compare_ndjson_composite(
        &ref_json,
        &combined_json,
        "SourceFile",
        &["SourceFile"],
        source_suffix_key,
        &accepted(),
    );
    assert!(
        d.is_match(),
        "JSON ({fixture_dir}): reference {} / ours {} rows, mismatches: {:#?}",
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

#[test]
fn compat_stdc1_capture() {
    compat_for("STDC1", "STDC1");
}
