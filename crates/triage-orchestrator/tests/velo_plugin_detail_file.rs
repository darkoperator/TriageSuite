//! `PluginDetailFile` is a cross-reference an analyst follows from a batch
//! row to the per-plugin detail CSV that row's plugin wrote. This test runs
//! the real `TriageSuite` binary over two real per-user `NTUSER.DAT` hives in
//! the default Velo layout and asserts every non-empty reference names a file
//! that is actually there.
//!
//! Two independent ways the reference used to dangle, both of which this test
//! catches:
//!
//! 1. **The identity suffix.** The router folds the profile name into a
//!    per-user side-car's filename (`TypedURLs_NTUSER.DAT_alice.csv`); the
//!    reference named the bare `TypedURLs_NTUSER.DAT.csv`.
//! 2. **The directory level.** A discriminated dataset's per-user slices live
//!    under `PerUser/<Discriminator>/` while its dynamic side-cars live in
//!    `PerUser/` (`OutputLayout::for_velo_dataset`), and the merged batch CSV
//!    lives at the category root. A reference that fixed only the filename
//!    would still name a file in the wrong directory.
//!
//! The assertion is resolution against the **category root** — the directory
//! holding the merged batch CSV, which is the batch file an analyst opens and
//! the root of the `PerUser/` tree the side-cars live in. See
//! `OutputLayout::side_car_reference` for why one row text cannot resolve
//! relative to both the merged file and the per-user slice it was merged from.
//!
//! Gated on `test captures/` via `triage_testkit::skip_if_missing`, like every
//! other capture-backed test here: `TRIAGE_ALLOW_COMPAT_SKIP=1` lets it skip
//! when the (gitignored) evidence tree is absent, and its absence otherwise
//! panics rather than silently passing.

use assert_cmd::Command;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const PINNED_STAMP: &str = "2026-03-13T192553Z";

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

/// Every `.../Users/<profile>/NTUSER.DAT` in the captures tree, keyed by
/// profile name so the caller can pick two that are genuinely different
/// profiles. `Default`/`Public` and the service profiles are included; the
/// test only needs two hives that attribute to two distinct users.
fn per_user_ntuser_hives() -> BTreeMap<String, PathBuf> {
    let mut found = BTreeMap::new();
    let mut stack = vec![captures_root()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == "NTUSER.DAT") {
                let Some(profile) = path.parent().and_then(|p| p.file_name()) else {
                    continue;
                };
                found
                    .entry(profile.to_string_lossy().to_string())
                    .or_insert(path);
            }
        }
    }
    found
}

/// Read a CSV's rows, each one a map of column name to value.
fn read_csv(path: &Path) -> Vec<BTreeMap<String, String>> {
    let mut reader = csv::Reader::from_path(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let headers = reader.headers().unwrap().clone();
    reader
        .records()
        .map(|record| {
            let record = record.unwrap();
            headers
                .iter()
                .zip(record.iter())
                .map(|(h, v)| (h.to_string(), v.to_string()))
                .collect()
        })
        .collect()
}

/// Every non-empty `PluginDetailFile` in a real Velo run resolves to a file
/// that exists, and names the detail CSV for the row's own hive and user.
#[test]
fn every_plugin_detail_file_reference_resolves_against_the_category_root() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let hives = per_user_ntuser_hives();
    let mut hives = hives.values();
    let (Some(first), Some(second)) = (hives.next(), hives.next()) else {
        eprintln!("SKIP: fewer than two per-user NTUSER.DAT hives in the captures tree");
        return;
    };

    // A raw directory holding just the two hives under two profile names, not
    // a full collection: `TriageSuite run` treats an unrecognized directory as
    // a single raw capture (`capture::enumerate_multi`'s `raw_fallback`), so
    // RETriage discovers exactly these two files. The profile directory is
    // what the attributor reads the identity from, so the two copies land as
    // two distinct users regardless of which hives were picked above.
    let raw = TempDir::new().unwrap();
    for (profile, hive) in [("alice", first), ("bob", second)] {
        let dir = raw.path().join("Users").join(profile);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(hive, dir.join("NTUSER.DAT")).unwrap();
    }

    let out = TempDir::new().unwrap();
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args([
            "run",
            "--no-validate",
            raw.path().to_str().unwrap(),
            "--out",
            out.path().to_str().unwrap(),
            "--only",
            "re",
            "--overwrite",
            "--no-progress",
            "--skip-hashes",
        ])
        .assert()
        .success();

    let host = raw
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let category_root = out
        .path()
        .join(format!("Processed-{host}-{PINNED_STAMP}"))
        .join("Registry");
    let merged = category_root.join(format!("{PINNED_STAMP}_RETriage_results_Batch.csv"));
    assert!(
        merged.is_file(),
        "expected the merged batch CSV at {}",
        merged.display()
    );

    // The merged file plus every per-user slice that fed it: the same row text
    // is published to both, so both are checked against the same anchor.
    let mut batch_csvs = vec![merged.clone()];
    let per_user_batch = category_root.join("PerUser").join("Batch");
    for entry in std::fs::read_dir(&per_user_batch)
        .unwrap_or_else(|e| panic!("expected {}: {e}", per_user_batch.display()))
        .flatten()
    {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "csv") {
            batch_csvs.push(path);
        }
    }

    let mut total_rows = 0usize;
    let mut referencing_rows = 0usize;
    let mut dangling: Vec<String> = Vec::new();
    let mut disagreements: Vec<String> = Vec::new();

    for csv_path in &batch_csvs {
        for row in read_csv(csv_path) {
            total_rows += 1;
            let reference = row.get("PluginDetailFile").cloned().unwrap_or_default();
            if reference.is_empty() {
                continue;
            }
            referencing_rows += 1;

            let resolved = category_root.join(&reference);
            if !resolved.is_file() {
                dangling.push(format!("{} -> {reference}", csv_path.display()));
                continue;
            }

            // Hive agreement: the detail CSV a row names must be the one
            // written for that row's own hive and profile. Both hives here
            // are an `NTUSER.DAT`, so the profile label is the only thing
            // telling the two detail files apart — which is exactly the
            // confusion a reference that omits it produces.
            let name = resolved.file_name().unwrap().to_string_lossy().to_string();
            let hive = Path::new(row.get("HivePath").map(String::as_str).unwrap_or_default())
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let user = Path::new(row.get("HivePath").map(String::as_str).unwrap_or_default())
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !name.contains(&hive) || !name.contains(&user) {
                disagreements.push(format!(
                    "{} -> {reference} (hive {hive}, user {user})",
                    csv_path.display()
                ));
            }
        }
    }

    // A failure here is thousands of rows citing a handful of distinct
    // values; the distinct values are what identifies the defect.
    dangling.sort();
    dangling.dedup();
    disagreements.sort();
    disagreements.dedup();

    assert!(total_rows > 0, "the run produced no batch rows at all");
    assert!(
        referencing_rows > 0,
        "no batch row carried a PluginDetailFile, so nothing was actually checked"
    );
    assert!(
        dangling.is_empty(),
        "{} distinct PluginDetailFile references (of {referencing_rows} referencing rows) \
         do not resolve under {}:\n{}",
        dangling.len(),
        category_root.display(),
        dangling.join("\n")
    );
    assert!(
        disagreements.is_empty(),
        "{} distinct PluginDetailFile references name another hive's or another user's \
         detail CSV:\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );
}
