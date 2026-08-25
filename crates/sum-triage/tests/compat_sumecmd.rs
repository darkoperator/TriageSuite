//! Compatibility gate: SumETriage vs SumECmd reference fixtures.
//!
//! The clean SUM set lives at `<repo>/../test captures/Sum/` (all .mdb are
//! CleanShutdown rev-300, matching the inputs the oracle was generated from).
//! We run the SumETriage binary over that directory and compare each emitted
//! dataset CSV against its fixture as an unordered multiset.
//!
//! Timestamp normalization: our WinTimestamp renders `…T…0000000Z`; SumECmd
//! renders whole-second `yyyy-MM-dd HH:mm:ss`. We strip our fractional suffix,
//! and `compare_csv_unkeyed` normalizes the reference side.

use std::path::{Path, PathBuf};

use triage_testkit::{compare_csv_unkeyed, skip_if_missing, AcceptedDelta};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sumecmd")
}

fn clean_sum_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures/Sum")
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

fn run_sumetriage(sum_dir: &Path) -> PathBuf {
    use assert_cmd::Command;
    let tmp = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let out = tmp.path().to_path_buf();
    Command::cargo_bin("SumETriage")
        .unwrap()
        .arg("-d")
        .arg(sum_dir)
        .arg("--csv")
        .arg(&out)
        .assert()
        .success();
    out
}

fn produced(root: &Path, basename: &str) -> Option<PathBuf> {
    // Output filenames now carry a 14-digit `<yyyyMMddHHmmss>_` timestamp prefix,
    // so match any directory entry whose filename ends with the expected basename.
    walk(root).into_iter().find(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(basename))
    })
}

/// Strip our 7-digit fractional seconds (`.NNNNNNNZ` → `Z`) from all cells.
fn normalize_our_csv(our_csv: &Path) -> PathBuf {
    let mut rdr = csv::Reader::from_path(our_csv).unwrap();
    let headers: Vec<String> = rdr.headers().unwrap().iter().map(str::to_string).collect();
    let tmp = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let out_path = tmp.path().join("sum_normalized.csv");
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

/// (fixture name, produced CSV basename, accepted deltas)
const DATASETS: &[(&str, &str, &[AcceptedDelta])] = &[
    (
        "SystemIdentInfo.csv",
        "SumETriage_SystemIdentInfo_Output.csv",
        &[],
    ),
    ("RoleInfos.csv", "SumETriage_RoleInfos_Output.csv", &[]),
    (
        "ChainedDbInfo.csv",
        "SumETriage_ChainedDbInfo_Output.csv",
        &[],
    ),
    ("Clients.csv", "SumETriage_Clients_Output.csv", &[]),
    (
        "ClientsDetailed.csv",
        "SumETriage_ClientsDetailed_Output.csv",
        &[],
    ),
    ("DnsInfo.csv", "SumETriage_DnsInfo_Output.csv", &[]),
    (
        "RoleAccesses.csv",
        "SumETriage_RoleAccesses_Output.csv",
        &[],
    ),
];

#[test]
fn compat_sumecmd_clean_set() {
    let sum_dir = clean_sum_dir();
    if skip_if_missing(&sum_dir, "test captures/Sum") {
        return;
    }
    let root = run_sumetriage(&sum_dir);

    let mut failures: Vec<String> = Vec::new();
    for (fixture_name, csv_basename, deltas) in DATASETS {
        let fixture = fixture_dir().join(fixture_name);
        if skip_if_missing(&fixture, fixture_name) {
            continue;
        }
        let Some(our_csv) = produced(&root, csv_basename) else {
            failures.push(format!("{fixture_name}: no {csv_basename} produced"));
            continue;
        };
        let norm = normalize_our_csv(&our_csv);
        let diff = compare_csv_unkeyed(&fixture, &norm, &[], deltas);
        if !diff.is_match() {
            failures.push(format!(
                "{fixture_name}: ref_rows={} our_rows={} mismatches={:?}",
                diff.reference_rows, diff.our_rows, diff.mismatches
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "SumECmd compat failures:\n  {}",
        failures.join("\n  ")
    );
}

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

/// Find the in-collection SystemIdentity.mdb for STDC1 (its sibling Current.mdb
/// is the live, dirty database).
fn find_in_collection_anchor() -> Option<PathBuf> {
    walk(&captures_root()).into_iter().find(|p| {
        let s = p.to_string_lossy();
        p.file_name().and_then(|n| n.to_str()) == Some("SystemIdentity.mdb")
            && s.contains("/Collection-STDC1")
    })
}

#[test]
fn dirty_current_mdb_parses_with_warning() {
    let Some(anchor) = find_in_collection_anchor() else {
        if skip_if_missing(&captures_root(), "test captures") {
            return;
        }
        panic!("captures present but no in-collection STDC1 SystemIdentity.mdb found");
    };

    use assert_cmd::Command;
    let tmp = tempfile::tempdir().unwrap();
    let assert = Command::cargo_bin("SumETriage")
        .unwrap()
        .arg("-f")
        .arg(&anchor)
        .arg("--csv")
        .arg(tmp.path())
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("not cleanly shut down"),
        "expected a dirty-database WARNING on stderr; got: {stderr}"
    );

    let produced = walk(tmp.path())
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with("_Output.csv"))
        })
        .count();
    assert!(
        produced > 0,
        "dirty SUM set should still yield dataset CSV output"
    );
}
