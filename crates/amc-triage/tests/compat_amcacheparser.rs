//! AmcacheTriage vs AmcacheParser (-i) oracle gate — STDC1 capture.
//!
//! DECODE FIDELITY: every row AmcacheTriage emits must byte-match a row in the
//! AmcacheParser oracle (all columns; timestamps normalized to whole-second,
//! since AmcacheParser's default `--dt` is `yyyy-MM-dd HH:mm:ss` while the suite
//! renders 7-digit ISO-8601 `…Z`). We never emit a row the oracle lacks.
//!
//! COVERAGE (documented known limitation, NOT an Amcache-logic bug): notatin (our
//! hive engine) recovers fewer *deleted* Amcache subkeys than AmcacheParser's
//! Eric-Zimmerman Registry library — both enable RecoverDeleted, but Registry.dll
//! recovers more from hive slack. So our output is a strict SUBSET of the oracle
//! for high-churn datasets (file entries, programs, shortcuts, driver binaries),
//! and an EXACT match for datasets with no recoverable-deleted entries
//! (DeviceContainers, DevicePnps). This gate therefore checks subset-fidelity plus
//! a per-dataset regression floor (so dropping rows fails), not a full row match.
//!
//! Gated on the STDC1 Amcache.hve in `test captures/` (gitignored evidence) and
//! the committed fixtures; skips cleanly (skip_if_missing) when either is absent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use triage_testkit::skip_if_missing;

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/amcacheparser/STDC1")
}

/// Locate the STDC1 collection's Amcache.hve (path casing varies: AppCompat/appcompat).
fn stdc1_amcache() -> Option<PathBuf> {
    let coll = std::fs::read_dir(captures_root())
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains("STDC1"))
                    .unwrap_or(false)
        })?;
    for c in [
        "uploads/auto/C%3A/Windows/AppCompat/Programs/Amcache.hve",
        "uploads/auto/C%3A/Windows/appcompat/Programs/Amcache.hve",
    ] {
        let p = coll.join(c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn read_csv(path: &Path) -> (Vec<String>, Vec<Vec<String>>) {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_path(path)
        .unwrap();
    let headers: Vec<String> = rdr
        .headers()
        .unwrap()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let rows: Vec<Vec<String>> = rdr
        .records()
        .filter_map(|r| r.ok())
        .map(|r| r.iter().map(|s| s.to_string()).collect())
        .collect();
    (headers, rows)
}

/// Columns rendered as timestamps by AmcacheTriage — normalized to whole-second
/// for comparison against AmcacheParser's `yyyy-MM-dd HH:mm:ss` output.
const TS_COLS: &[&str] = &[
    "FileKeyLastWriteTimestamp",
    "LinkDate",
    "KeyLastWriteTimestamp",
    "InstallDate",
    "InstallDateMsi",
    "DriverTimeStamp",
    "DriverLastWriteTime",
    "Date",
];

/// Normalize an ISO-8601 `YYYY-MM-DDThh:mm:ss.fffffffZ` (or already-space form)
/// to `YYYY-MM-DD hh:mm:ss`; pass through non-timestamp strings unchanged.
fn norm_ts(v: &str) -> String {
    let b = v.as_bytes();
    if b.len() >= 19
        && b[4] == b'-'
        && b[7] == b'-'
        && (b[10] == b' ' || b[10] == b'T')
        && b[13] == b':'
        && b[16] == b':'
    {
        format!("{} {}", &v[..10], &v[11..19])
    } else {
        v.to_string()
    }
}

fn norm_row(headers: &[String], row: &[String]) -> Vec<String> {
    headers
        .iter()
        .zip(row)
        .map(|(h, c)| {
            if TS_COLS.contains(&h.as_str()) {
                norm_ts(c)
            } else {
                c.clone()
            }
        })
        .collect()
}

/// Find our output CSV whose filename contains `needle` (recursive; flat layout puts
/// it at the root, but search subdirs too for robustness).
fn our_csv(dir: &Path, needle: &str) -> Option<PathBuf> {
    fn walk(d: &Path, needle: &str, out: &mut Option<PathBuf>) {
        if out.is_some() {
            return;
        }
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, needle, out);
            } else if p.extension().map(|x| x == "csv").unwrap_or(false)
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains(needle))
                    .unwrap_or(false)
            {
                *out = Some(p);
                return;
            }
        }
    }
    let mut out = None;
    walk(dir, needle, &mut out);
    out
}

/// (substring in OUR output filename, fixture basename, per-dataset regression floor).
/// Floors are the current verified counts — a drop below any of them is a regression.
const DATASETS: &[(&str, &str, usize)] = &[
    (
        "UnassociatedFileEntries",
        "Amcache_UnassociatedFileEntries.csv",
        418,
    ),
    (
        "AssociatedFileEntries",
        "Amcache_AssociatedFileEntries.csv",
        91,
    ),
    ("ProgramEntries", "Amcache_ProgramEntries.csv", 80),
    ("ShortCuts", "Amcache_ShortCuts.csv", 53),
    ("DriveBinaries", "Amcache_DriveBinaries.csv", 381),
    ("DeviceContainers", "Amcache_DeviceContainers.csv", 6),
    ("DevicePnps", "Amcache_DevicePnps.csv", 67),
    ("DriverPackages", "Amcache_DriverPackages.csv", 0),
];

#[test]
fn amcacheparser_compat_stdc1_subset_fidelity() {
    if skip_if_missing(&fixtures(), "AmcacheParser STDC1 fixtures") {
        return;
    }
    let Some(hive) = stdc1_amcache() else {
        // No STDC1 capture on this machine: honor the suite skip contract.
        if std::env::var("TRIAGE_ALLOW_COMPAT_SKIP").as_deref() == Ok("1") {
            eprintln!("SKIP: STDC1 Amcache.hve absent, TRIAGE_ALLOW_COMPAT_SKIP=1");
            return;
        }
        panic!(
            "STDC1 Amcache.hve not found under test captures — capture-gated test cannot run. \
             Set TRIAGE_ALLOW_COMPAT_SKIP=1 to allow skipping."
        );
    };

    let out = tempfile::tempdir().unwrap();
    assert_cmd::Command::cargo_bin("AmcacheTriage")
        .unwrap()
        .arg("-f")
        .arg(&hive)
        .arg("--csv")
        .arg(out.path())
        .assert()
        .success();

    let mut summary = String::new();
    for (needle, fixname, floor) in DATASETS {
        let (ohead, orows) = read_csv(&fixtures().join(fixname));
        let ours_path = our_csv(out.path(), needle);

        // Empty-oracle dataset (e.g. DriverPackages): AmcacheParser writes a
        // header-only CSV; our OutputRouter writes no file. Both = 0 data rows.
        if orows.is_empty() {
            let n = ours_path.as_ref().map(|p| read_csv(p).1.len()).unwrap_or(0);
            assert_eq!(n, 0, "{needle}: oracle has 0 rows but we emitted {n}");
            summary.push_str(&format!("  {needle:<26} 0/0 (empty)\n"));
            continue;
        }

        let ours_path =
            ours_path.unwrap_or_else(|| panic!("{needle}: AmcacheTriage produced no output CSV"));
        let (uhead, urows) = read_csv(&ours_path);

        assert_eq!(
            uhead, ohead,
            "{needle}: header mismatch\n oracle={ohead:?}\n ours  ={uhead:?}"
        );

        // Oracle multiset of normalized rows.
        let mut oracle_ms: HashMap<Vec<String>, usize> = HashMap::new();
        for r in &orows {
            *oracle_ms.entry(norm_row(&ohead, r)).or_insert(0) += 1;
        }

        // Subset fidelity: every emitted row must consume one matching oracle row.
        // A value mismatch or an invented row surfaces here as "not in oracle".
        for r in &urows {
            let nr = norm_row(&uhead, r);
            match oracle_ms.get_mut(&nr) {
                Some(c) if *c > 0 => *c -= 1,
                _ => panic!(
                    "{needle}: emitted a row that byte-matches NO oracle row (value bug or spurious row):\n{nr:?}"
                ),
            }
        }

        // Regression floor: must still emit at least the recorded count.
        assert!(
            urows.len() >= *floor,
            "{needle}: emitted {} rows < recorded floor {floor} (row regression)",
            urows.len()
        );

        summary.push_str(&format!(
            "  {needle:<26} {}/{} rows{}\n",
            urows.len(),
            orows.len(),
            if urows.len() == orows.len() {
                " (FULL match)"
            } else {
                " (subset-exact; notatin recovery-depth gap)"
            }
        ));
    }
    eprintln!("AmcacheParser compat (STDC1) — subset-fidelity PASS:\n{summary}");
}
