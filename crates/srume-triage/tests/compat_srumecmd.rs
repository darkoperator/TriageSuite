//! Spec section 10.3 compatibility gate: SrumETriage vs SrumECmd reference fixtures.
//!
//! Each fixture (tests/fixtures/srumecmd/) was produced by the official SrumECmd
//! Windows binary over a real SRUDB.dat with its SOFTWARE hive. Our SrumETriage
//! binary parses the same databases; each produced CSV is compared row-for-row
//! against its fixture.
//!
//! Row key: `Id` (the AutoIncId — unique per row in every dataset).
//!
//! Timestamp normalization (Reconciliation A): our WinTimestamp renders
//! `2026-02-09T14:35:00.0000000Z`; SrumECmd renders `2026-02-09 14:35:00`.
//! The testkit normalizes reference timestamps from space-separator to `T...Z`
//! form. We pre-normalize OUR CSV by stripping the 7-digit fractional suffix
//! (`.0000000Z` → `Z`), making them match exactly.
//!
//! SidType (Reconciliation B): implemented in `sidtype.rs` — no delta needed.
//!
//! Duration (Reconciliation C): implemented in `compute_duration` — no delta needed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use triage_testkit::{compare_csv, skip_if_missing, AcceptedDelta};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/srumecmd")
}

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Locate the SRUDB.dat for a named collection prefix (e.g. "STCL1", "DESKTOP").
fn find_srudb(collection_prefix: &str) -> Option<PathBuf> {
    walk(&captures_root()).into_iter().find(|p| {
        let s = p.to_string_lossy();
        s.ends_with("SRUDB.dat") && s.contains(&format!("/Collection-{collection_prefix}"))
    })
}

/// Locate the SOFTWARE hive in the same collection tree.
fn find_software(collection_prefix: &str) -> Option<PathBuf> {
    walk(&captures_root()).into_iter().find(|p| {
        let s = p.to_string_lossy();
        // Must end with "config/SOFTWARE" (not "RegBack/SOFTWARE")
        s.contains(&format!("/Collection-{collection_prefix}")) && s.ends_with("config/SOFTWARE")
    })
}

/// Run the real SrumETriage binary over `srudb` (optionally with `--software`)
/// into a temp dir; return the temp dir path (leaked so files persist).
fn run_srumetriage(srudb: &Path, software: Option<&Path>) -> PathBuf {
    use assert_cmd::Command;
    let tmp = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let out = tmp.path().to_path_buf();
    let mut cmd = Command::cargo_bin("SrumETriage").unwrap();
    cmd.arg("-f").arg(srudb).arg("--csv").arg(&out);
    if let Some(sw) = software {
        cmd.arg("--software").arg(sw);
    }
    cmd.assert().success();
    out
}

fn produced(root: &Path, basename: &str) -> Option<PathBuf> {
    // Output filenames now carry a 14-digit `<yyyyMMddHHmmss>_` timestamp prefix,
    // so match any entry whose filename ENDS WITH the expected basename rather
    // than requiring exact equality.
    walk(root).into_iter().find(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(basename))
    })
}

/// Strip our 7-digit fractional seconds (`.NNNNNNNZ` → `Z`) from ALL columns
/// of our CSV (any cell ending with `.NNNNNNNZ` where N is a digit). Returns a
/// path to a normalized temp copy.
fn normalize_srum_csv(our_csv: &Path) -> PathBuf {
    let mut rdr = csv::Reader::from_path(our_csv).unwrap();
    let headers: Vec<String> = rdr.headers().unwrap().iter().map(str::to_string).collect();
    let tmp = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let out_path = tmp.path().join("srum_normalized.csv");
    let mut wtr = csv::Writer::from_path(&out_path).unwrap();
    wtr.write_record(&headers).unwrap();
    for rec in rdr.records() {
        let rec = rec.unwrap();
        let row: Vec<String> = rec.iter().map(strip_fractional).collect();
        wtr.write_record(&row).unwrap();
    }
    wtr.flush().unwrap();
    out_path
}

fn strip_fractional(s: &str) -> String {
    if let Some(dot) = s.rfind('.') {
        if s.ends_with('Z') && s[dot + 1..].len() > 1 {
            return format!("{}Z", &s[..dot]);
        }
    }
    s.to_string()
}

/// (dataset_id, CSV basename, fixture name suffix, accepted deltas)
const DATASETS: &[(&str, &str, &str, &[AcceptedDelta])] = &[
    (
        "NetworkUsages",
        "SrumETriage_NetworkUsages_Output.csv",
        "NetworkUsages",
        EXE_TIMESTAMP_DELTAS,
    ),
    (
        "NetworkConnections",
        "SrumETriage_NetworkConnections_Output.csv",
        "NetworkConnections",
        EXE_TIMESTAMP_DELTAS,
    ),
    (
        "AppResourceUseInfo",
        "SrumETriage_AppResourceUseInfo_Output.csv",
        "AppResourceUseInfo",
        EXE_TIMESTAMP_DELTAS,
    ),
    (
        "PushNotifications",
        "SrumETriage_PushNotifications_Output.csv",
        "PushNotifications",
        EXE_TIMESTAMP_DELTAS,
    ),
    (
        "EnergyUsage",
        "SrumETriage_EnergyUsage_Output.csv",
        "EnergyUsage",
        EXE_TIMESTAMP_DELTAS,
    ),
    (
        "AppTimelineProvider",
        "SrumETriage_AppTimelineProvider_Output.csv",
        "AppTimelineProvider",
        EXE_TIMESTAMP_DELTAS,
    ),
    (
        "vfuprov",
        "SrumETriage_vfuprov_Output.csv",
        "vfuprov",
        EXE_TIMESTAMP_DELTAS,
    ),
];

/// AcceptedDelta for ExeTimestamp format mismatch.
///
/// SrumECmd serializes `ExeTimestamp` (a `DateTimeOffset?`) using the .NET
/// default `DateTimeOffset.ToString()` which is culture-sensitive and emits
/// `"MM/dd/yyyy HH:mm:ss +00:00"` (US en-US culture).  Our tool uses ISO 8601
/// (`"yyyy-MM-ddTHH:mm:ssZ"`).  Both represent the same UTC instant;
/// the testkit's reference normalizer only handles `YYYY-MM-DD …` forms, so we
/// accept this per-field divergence with a parser that normalizes both sides.
///
/// Reason: SrumECmd quirk — ExeTimestamp uses the implicit DateTimeOffset
/// default format rather than the user-supplied `--dt` format string.
const EXE_TIMESTAMP_DELTAS: &[AcceptedDelta] = &[AcceptedDelta {
    field: "ExeTimestamp",
    reason: "SrumECmd formats ExeTimestamp via DateTimeOffset.ToString() default \
             (MM/dd/yyyy HH:mm:ss +00:00) rather than the --dt format string; \
             our tool emits ISO 8601. Both represent the same UTC instant.",
    compare: |reference, ours| {
        // reference: "MM/dd/yyyy HH:mm:ss +00:00" or possibly already normalized
        // ours: "yyyy-MM-ddTHH:mm:ssZ"  (may have been strip_fractional'd)
        let rn = normalize_exe_ts(reference);
        let on = normalize_exe_ts(ours);
        rn == on
    },
    row_guard: None,
}];

/// Normalize both SrumECmd's `MM/dd/yyyy HH:mm:ss +00:00` form AND our
/// `yyyy-MM-ddTHH:mm:ssZ` form to a canonical `yyyy-MM-ddTHH:mm:ssZ` string
/// for comparison, returning the input unchanged if it matches neither pattern.
fn normalize_exe_ts(s: &str) -> String {
    // Already in ISO form from our side (possibly already strip_fractional'd).
    if s.len() >= 20 && &s[4..5] == "-" && &s[7..8] == "-" && s.ends_with('Z') {
        return s.to_string();
    }
    // Try MM/dd/yyyy HH:mm:ss +00:00
    // Expected: 24 chars exactly for "01/23/2025 14:35:00 +00:00"
    if s.len() == 26 && s.as_bytes()[2] == b'/' && s.as_bytes()[5] == b'/' {
        let mm = &s[0..2];
        let dd = &s[3..5];
        let yyyy = &s[6..10];
        let hh = &s[11..13];
        let mi = &s[14..16];
        let ss = &s[17..19];
        return format!("{yyyy}-{mm}-{dd}T{hh}:{mi}:{ss}Z");
    }
    s.to_string()
}

/// Run one collection against all 7 datasets.
///
/// Cache the SrumETriage run once per (srudb, software) pair; compare all 7
/// datasets. Returns a map of dataset-name → failure message (empty = all pass).
fn run_collection(collection_prefix: &str) -> HashMap<String, String> {
    let mut failures = HashMap::new();

    let Some(srudb) = find_srudb(collection_prefix) else {
        // Captures absent — skip via triage_testkit convention.
        if skip_if_missing(&captures_root(), "test captures") {
            return failures;
        }
        panic!("captures present but no SRUDB.dat found for collection {collection_prefix}");
    };
    let software = find_software(collection_prefix);
    let root = run_srumetriage(&srudb, software.as_deref());

    for (ds_name, csv_basename, fixture_suffix, deltas) in DATASETS {
        let fixture_name = format!("{collection_prefix}__{fixture_suffix}.csv");
        let fixture = fixture_dir().join(&fixture_name);
        if skip_if_missing(&fixture, &fixture_name) {
            continue;
        }
        let Some(our_csv) = produced(&root, csv_basename) else {
            failures.insert(
                format!("{collection_prefix}/{ds_name}"),
                format!("no {csv_basename} produced"),
            );
            continue;
        };
        let norm = normalize_srum_csv(&our_csv);
        let diff = compare_csv(&fixture, &norm, "Id", deltas);
        if !diff.is_match() {
            failures.insert(
                format!("{collection_prefix}/{ds_name}"),
                format!(
                    "ref_rows={} our_rows={} mismatches={:?}",
                    diff.reference_rows, diff.our_rows, diff.mismatches
                ),
            );
        }
    }
    failures
}

#[test]
fn compat_stcl1() {
    let failures = run_collection("STCL1");
    assert!(
        failures.is_empty(),
        "STCL1 compat failures:\n{}",
        failures
            .iter()
            .map(|(k, v)| format!("  {k}: {v}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn compat_desktop() {
    let failures = run_collection("DESKTOP");
    assert!(
        failures.is_empty(),
        "DESKTOP compat failures:\n{}",
        failures
            .iter()
            .map(|(k, v)| format!("  {k}: {v}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn stdc1_dirty_rev300_parses_with_warning() {
    use triage_ese::header::format_revision;

    let Some(srudb) = find_srudb("STDC1") else {
        if skip_if_missing(&captures_root(), "test captures") {
            return;
        }
        panic!("captures present but no STDC1 SRUDB.dat found");
    };

    assert_eq!(
        format_revision(&srudb).expect("read STDC1 SRUDB header"),
        300,
        "STDC1 SRUDB format revision should be 300"
    );

    use assert_cmd::Command;
    let tmp = tempfile::tempdir().unwrap();
    let assert = Command::cargo_bin("SrumETriage")
        .unwrap()
        .arg("-f")
        .arg(&srudb)
        .arg("--csv")
        .arg(tmp.path())
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("not cleanly shut down"),
        "expected a dirty-database WARNING on stderr; got: {stderr}"
    );

    let produced = walk_count_csvs(tmp.path());
    assert!(
        produced > 0,
        "dirty rev-300 SRUDB should still yield dataset CSV output"
    );
}

/// Count `*_Output.csv` files anywhere under `root` (output is nested under
/// SrumETriage/<scope>/...).
fn walk_count_csvs(root: &std::path::Path) -> usize {
    let mut n = 0;
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
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("_Output.csv"))
            {
                n += 1;
            }
        }
    }
    n
}
