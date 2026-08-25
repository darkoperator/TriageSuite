//! Spec section 10.3 compatibility gate: JLETriage vs JLECmd reference fixtures.
//!
//! The fixtures in tests/fixtures/jlecmd/{DESKTOP,STCL1,STDC1}/{auto,custom}.csv
//! were produced by real JLECmd. JLECmd writes ONE combined CSV/JSON per dataset
//! per capture (all jump lists together); JLETriage splits its output by
//! identity (users/<u>/, system/). To compare, we AGGREGATE every per-identity
//! JLETriage_{Automatic,Custom}Destinations_Output.{csv,json} back into one
//! combined file, then compare keyed per dataset.
//!
//! Keys:
//!  - Automatic: (SourceFile capture-suffix + EntryNumber). EntryNumber is
//!    unique within one jump list, and the capture-relative SourceFile suffix
//!    distinguishes same-named jump lists under different user directories — so
//!    the pair is unique per row. Compared with `compare_csv_composite`.
//!  - Custom: (SourceFile capture-suffix + EntryName). Several LNKs share one
//!    category name within a file, so this is a GROUP key; `compare_csv_grouped`
//!    disambiguates duplicates by intra-file row order (no silent row drops).

use assert_cmd::Command;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use triage_testkit::{compare_csv_composite, compare_csv_grouped, AcceptedDelta};

/// Capture-relative suffix of a SourceFile, anchored at the `uploads/`
/// collection segment (everything before it is host prefix the harness already
/// excuses). Falls back to the last five segments when no anchor is present.
fn source_suffix(v: &str) -> String {
    let segs: Vec<&str> = v.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if let Some(pos) = segs.iter().position(|s| s.eq_ignore_ascii_case("uploads")) {
        return segs[pos..].join("/");
    }
    let start = segs.len().saturating_sub(5);
    segs[start..].join("/")
}

/// Automatic key: SourceFile suffix + EntryNumber (unique per row).
fn auto_key(row: &BTreeMap<String, String>, _path_cols: &[&str]) -> String {
    let sf = row.get("SourceFile").cloned().unwrap_or_default();
    let en = row.get("EntryNumber").cloned().unwrap_or_default();
    format!("{}|{en}", source_suffix(&sf))
}

/// Custom GROUP key: SourceFile suffix + EntryName (may repeat; disambiguated
/// by the grouped comparer's per-group occurrence index in row order).
fn custom_key(row: &BTreeMap<String, String>, _path_cols: &[&str]) -> String {
    let sf = row.get("SourceFile").cloned().unwrap_or_default();
    let en = row.get("EntryName").cloned().unwrap_or_default();
    format!("{}|{en}", source_suffix(&sf))
}

fn fixture(capture: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/jlecmd")
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

/// Path-valued columns compared by basename (host prefix excused).
const PATH_COLS: &[&str] = &[
    "SourceFile",
    "RelativePath",
    "WorkingDirectory",
    "LocalPath",
    "CommonPath",
];

/// Named accepted incompatibilities (spec 10.3: no silent exclusions).
///
/// Source* timestamps: capture-copy fs metadata, unrelated to JLECmd's run-time
/// metadata on the original system — row_guard None (every row).
///
/// The remaining five deltas all stem from ONE root cause: any embedded LNK
/// whose Arguments exceed 260 characters. JLECmd's LNK parser truncates such
/// Arguments at exactly 260 chars AND then fails to read that LNK's trailing
/// ExtraData blocks. In the STCL1 capture this hits two distinct custom files
/// (`ccba5a5986c77e43` "Recently closed" and `6824f4a902c78fbd`), each with a
/// Bing-search argument over 260 chars — the row_guard keys on exactly that
/// 260-char trigger, so the deltas apply to those rows and no others.
/// Specifically JLECmd truncates that LNK's Arguments at exactly 260 characters
/// AND then fails to read the trailing ExtraData blocks — so the reference row
/// shows a 260-char Arguments prefix and EMPTY ExtraBlocksPresent / MachineID /
/// MachineMACAddress / TrackerCreatedOn. JLETriage parses the LNK fully (the
/// reference Arguments is a strict 260-char prefix of ours). We do NOT corrupt
/// our correct output to mimic JLECmd's defect; instead each delta is scoped so
/// it can fire ONLY on this row:
///  - Arguments: accepted only when the reference is a 260-char prefix of ours
///    (self-bounding; a real Arguments divergence on any other row is NOT a
///    260-char prefix and still fails).
///  - The four block-derived fields: accepted only when the reference is empty
///    AND ours is non-empty AND the row is the truncated-arg row (row_guard:
///    reference Arguments length == 260). The guard is the tightening — without
///    it, "reference empty, ours non-empty" would mask a regression on any
///    custom row whose block extraction broke.
fn accepted() -> Vec<AcceptedDelta> {
    fn is_truncated_arg_row(row: &triage_testkit::RowMap) -> bool {
        row.get("Arguments")
            .map(|a| a.len() == 260)
            .unwrap_or(false)
    }
    vec![
        AcceptedDelta {
            field: "SourceCreated",
            reason: "fs metadata of the capture copy differs from JLECmd's run-time \
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
        AcceptedDelta {
            field: "Arguments",
            reason: "JLECmd truncates this one oversized (>260 char) Bing-search \
                     argument at 260 chars; ours is the full value — STCL1 \
                     ccba5a5986c77e43 'Recently closed' only (2 capture copies)",
            // Self-bounding: accept only when the reference is exactly the
            // 260-char prefix of our full value.
            compare: |reference, ours| reference.len() == 260 && ours.starts_with(reference),
            row_guard: None,
        },
        AcceptedDelta {
            field: "ExtraBlocksPresent",
            reason: "JLECmd lost this LNK's ExtraData blocks after truncating its \
                     oversized arguments; ours parsed them — same row as Arguments",
            compare: |reference, ours| reference.is_empty() && !ours.is_empty(),
            row_guard: Some(is_truncated_arg_row),
        },
        AcceptedDelta {
            field: "MachineID",
            reason: "carried in the TrackerDataBaseBlock JLECmd failed to read on \
                     the truncated-arguments LNK — same row as Arguments",
            compare: |reference, ours| reference.is_empty() && !ours.is_empty(),
            row_guard: Some(is_truncated_arg_row),
        },
        AcceptedDelta {
            field: "MachineMACAddress",
            reason: "carried in the TrackerDataBaseBlock JLECmd failed to read on \
                     the truncated-arguments LNK — same row as Arguments",
            compare: |reference, ours| reference.is_empty() && !ours.is_empty(),
            row_guard: Some(is_truncated_arg_row),
        },
        AcceptedDelta {
            field: "TrackerCreatedOn",
            reason: "carried in the TrackerDataBaseBlock JLECmd failed to read on \
                     the truncated-arguments LNK — same row as Arguments",
            compare: |reference, ours| reference.is_empty() && !ours.is_empty(),
            row_guard: Some(is_truncated_arg_row),
        },
    ]
}

fn run_jletriage(capture: &Path, out: &Path) {
    Command::cargo_bin("JLETriage")
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
/// + all data rows in file order), return it.
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
/// combined NDJSON file in file order, return it.
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

/// Every file whose name ends with `name` under `root`, sorted for a stable
/// aggregation order (a timestamp prefix is prepended to output basenames, so
/// match by suffix not equality).
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
                .is_some_and(|f| f.ends_with(name))
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Guard both required trees; returns true when the test should skip.
fn gated() -> bool {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
    if triage_testkit::skip_if_missing(&root, "test captures") {
        return true;
    }
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/jlecmd");
    triage_testkit::skip_if_missing(&fixture_root, "jlecmd fixtures")
}

/// Compare the AUTOMATIC dataset (CSV + NDJSON) for one capture.
fn compat_auto(capture_needle: &str, fixture_dir: &str) {
    if gated() {
        return;
    }
    let cap = capture_dir(capture_needle).expect("capture dir");
    let ref_csv = fixture(fixture_dir, "auto.csv");

    let tmp = tempfile::tempdir().unwrap();
    run_jletriage(&cap, tmp.path());

    let combined_csv = aggregate_csv(
        tmp.path(),
        "JLETriage_AutomaticDestinations_Output.csv",
        &tmp.path().join("auto.csv"),
    );
    let d = compare_csv_composite(&ref_csv, &combined_csv, PATH_COLS, auto_key, &accepted());
    assert!(
        d.is_match(),
        "auto CSV ({fixture_dir}): reference {} / ours {} rows, mismatches: {:#?}",
        d.reference_rows,
        d.our_rows,
        d.mismatches
    );

    // NDJSON self-consistency (suite convention): our NDJSON property-set must
    // equal our CSV columns and every value must match our CSV — so the JSON
    // sink emits exactly what the (fixture-validated) CSV does, minus the
    // null-omitted empties. No JLECmd JSON fixture exists for jump lists.
    let combined_json = aggregate_ndjson(
        tmp.path(),
        "JLETriage_AutomaticDestinations_Output.json",
        &tmp.path().join("auto.json"),
    );
    assert_ndjson_matches_csv(
        &combined_csv,
        &combined_json,
        &format!("auto {fixture_dir}"),
    );
}

/// Compare the CUSTOM dataset (CSV + NDJSON) for one capture.
fn compat_custom(capture_needle: &str, fixture_dir: &str) {
    if gated() {
        return;
    }
    let cap = capture_dir(capture_needle).expect("capture dir");
    let ref_csv = fixture(fixture_dir, "custom.csv");

    let tmp = tempfile::tempdir().unwrap();
    run_jletriage(&cap, tmp.path());

    let combined_csv = aggregate_csv(
        tmp.path(),
        "JLETriage_CustomDestinations_Output.csv",
        &tmp.path().join("custom.csv"),
    );
    let d = compare_csv_grouped(&ref_csv, &combined_csv, PATH_COLS, custom_key, &accepted());
    assert!(
        d.is_match(),
        "custom CSV ({fixture_dir}): reference {} / ours {} rows, mismatches: {:#?}",
        d.reference_rows,
        d.our_rows,
        d.mismatches
    );

    let combined_json = aggregate_ndjson(
        tmp.path(),
        "JLETriage_CustomDestinations_Output.json",
        &tmp.path().join("custom.json"),
    );
    assert_ndjson_matches_csv(
        &combined_csv,
        &combined_json,
        &format!("custom {fixture_dir}"),
    );
}

/// Verify our NDJSON is consistent with our CSV: every CSV row has exactly one
/// NDJSON record (compared positionally, since both are emitted in the same
/// order), the record's property-set equals the CSV's non-empty columns (our
/// JSON sink omits empty/null values), and every property value equals the CSV
/// cell. This is the suite's NDJSON convention when no reference JSON exists.
fn assert_ndjson_matches_csv(csv_path: &Path, ndjson_path: &Path, label: &str) {
    let mut rdr = csv::Reader::from_path(csv_path).unwrap();
    let headers: Vec<String> = rdr.headers().unwrap().iter().map(str::to_string).collect();
    let csv_rows: Vec<Vec<String>> = rdr
        .records()
        .map(|r| r.unwrap().iter().map(str::to_string).collect())
        .collect();

    let json_text = std::fs::read_to_string(ndjson_path).unwrap();
    let json_rows: Vec<serde_json::Map<String, serde_json::Value>> = json_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(
            |l| match serde_json::from_str::<serde_json::Value>(l).unwrap() {
                serde_json::Value::Object(o) => o,
                other => panic!("[{label}] non-object NDJSON line: {other}"),
            },
        )
        .collect();

    assert_eq!(
        csv_rows.len(),
        json_rows.len(),
        "[{label}] CSV has {} rows but NDJSON has {} records",
        csv_rows.len(),
        json_rows.len()
    );

    let stringify = |v: &serde_json::Value| -> String {
        match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    };

    for (i, (row, rec)) in csv_rows.iter().zip(json_rows.iter()).enumerate() {
        // 1. Every property present in the record carries the same value as its
        //    CSV cell. (Always-present String fields legitimately serialize as
        //    "" when empty; Option fields are null-omitted. Either way, when the
        //    property IS present its value must equal the CSV cell.)
        for (h, cell) in headers.iter().zip(row.iter()) {
            if let Some(v) = rec.get(h) {
                assert_eq!(
                    &stringify(v),
                    cell,
                    "[{label}] row {i}: property {h} value differs (json {:?} vs csv {cell:?})",
                    stringify(v)
                );
            } else {
                // A property may be absent ONLY when the CSV cell is empty
                // (null-omission); a non-empty cell missing from JSON is a bug.
                assert!(
                    cell.is_empty(),
                    "[{label}] row {i}: CSV {h}={cell:?} but NDJSON omits the property"
                );
            }
        }
        // 2. The record carries no property outside the CSV column set.
        for prop in rec.keys() {
            assert!(
                headers.iter().any(|h| h == prop),
                "[{label}] row {i}: NDJSON has extra property {prop} not in CSV columns"
            );
        }
    }
}

#[test]
fn compat_desktop_auto() {
    compat_auto("DESKTOP", "DESKTOP");
}

#[test]
fn compat_desktop_custom() {
    compat_custom("DESKTOP", "DESKTOP");
}

#[test]
fn compat_stcl1_auto() {
    compat_auto("STCL1", "STCL1");
}

#[test]
fn compat_stcl1_custom() {
    compat_custom("STCL1", "STCL1");
}

#[test]
fn compat_stdc1_auto() {
    compat_auto("STDC1", "STDC1");
}

#[test]
fn compat_stdc1_custom() {
    compat_custom("STDC1", "STDC1");
}
