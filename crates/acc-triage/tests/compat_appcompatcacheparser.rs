//! AppCompatTriage vs AppCompatCacheParser oracle gate — full row match, all 3
//! local collections. Same SYSTEM hive + exact logic replication → byte-for-byte
//! (timestamps normalized to whole-second; SourceFile by basename). Skips when a
//! fixture is absent (skip_if_missing); missing SYSTEM hive panics unless
//! TRIAGE_ALLOW_COMPAT_SKIP=1 is set.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use triage_testkit::skip_if_missing;

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}
fn fixture(coll: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/appcompatcacheparser")
        .join(coll)
        .join("AppCompatCache.csv")
}

fn system_hive(tag: &str) -> Option<PathBuf> {
    let coll = std::fs::read_dir(captures_root())
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains(tag))
                    .unwrap_or(false)
        })?;
    let p = coll.join("uploads/auto/C%3A/Windows/System32/config/SYSTEM");
    p.exists().then_some(p)
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
    let rows = rdr
        .records()
        .filter_map(|r| r.ok())
        .map(|r| r.iter().map(|s| s.to_string()).collect())
        .collect();
    (headers, rows)
}

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

fn basename(v: &str) -> String {
    v.rsplit(['/', '\\']).next().unwrap_or(v).to_string()
}

/// Normalize a row into (ControlSet,CacheEntryPosition) key + comparable cells.
fn key_and_cells(headers: &[String], row: &[String]) -> (String, Vec<String>) {
    let idx = |name: &str| headers.iter().position(|h| h == name).unwrap();
    let cs = row[idx("ControlSet")].clone();
    let pos = row[idx("CacheEntryPosition")].clone();
    let cells: Vec<String> = headers
        .iter()
        .zip(row)
        .map(|(h, c)| match h.as_str() {
            "LastModifiedTimeUTC" => norm_ts(c),
            "SourceFile" => basename(c),
            _ => c.clone(),
        })
        .collect();
    (format!("{cs}|{pos}"), cells)
}

fn our_csv(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.contains("AppCompatCache_Output") && n.ends_with(".csv"))
                .unwrap_or(false)
        })
}

fn run_gate(tag: &str, coll_fixture: &str) {
    let fx = fixture(coll_fixture);
    if skip_if_missing(&fx, &format!("AppCompatCacheParser {coll_fixture} fixture")) {
        return;
    }
    let Some(hive) = system_hive(tag) else {
        if std::env::var("TRIAGE_ALLOW_COMPAT_SKIP").as_deref() == Ok("1") {
            eprintln!("SKIP: {tag} SYSTEM hive absent");
            return;
        }
        panic!("{tag} SYSTEM hive not found — set TRIAGE_ALLOW_COMPAT_SKIP=1 to skip");
    };
    let out = tempfile::tempdir().unwrap();
    assert_cmd::Command::cargo_bin("AppCompatTriage")
        .unwrap()
        .arg("-f")
        .arg(&hive)
        .arg("--csv")
        .arg(out.path())
        .assert()
        .success();
    let ours = our_csv(out.path()).unwrap_or_else(|| panic!("{tag}: no output CSV"));

    let (oh, orows) = read_csv(&fx);
    let (uh, urows) = read_csv(&ours);
    assert_eq!(
        uh, oh,
        "{tag}: header mismatch\n oracle={oh:?}\n ours={uh:?}"
    );
    assert_eq!(
        urows.len(),
        orows.len(),
        "{tag}: row count ours={} oracle={}",
        urows.len(),
        orows.len()
    );

    let omap: HashMap<String, Vec<String>> = orows.iter().map(|r| key_and_cells(&oh, r)).collect();
    assert_eq!(
        omap.len(),
        orows.len(),
        "{tag}: oracle has duplicate (ControlSet,CacheEntryPosition) keys"
    );
    for r in &urows {
        let (k, cells) = key_and_cells(&uh, r);
        let oref = omap
            .get(&k)
            .unwrap_or_else(|| panic!("{tag}: our row {k} has no oracle match"));
        assert_eq!(
            &cells, oref,
            "{tag}: row {k} differs\n oracle={oref:?}\n ours  ={cells:?}"
        );
    }
    eprintln!(
        "{tag}: AppCompatCacheParser compat PASS ({} rows)",
        urows.len()
    );
}

#[test]
fn appcompat_compat_stdc1() {
    run_gate("STDC1", "STDC1");
}
#[test]
fn appcompat_compat_desktop() {
    run_gate("DESKTOP", "DESKTOP");
}
#[test]
fn appcompat_compat_stcl1() {
    run_gate("STCL1", "STCL1");
}
