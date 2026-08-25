//! Spec section 10.3 compatibility gate: SQLETriage vs SQLECmd reference fixtures.
//!
//! Each fixture (tests/fixtures/sqlecmd/) was produced by the official SQLECmd
//! net9 binary over one SQLite database, using `--hunt --dedupe` flags (matching
//! our SQLETriage defaults). The SQLETriage binary parses the same database into
//! a temp --csv dir (via assert_cmd, exercising the full runner); each produced
//! CSV is compared row-for-row against its fixture.
//!
//! Compat facts addressed:
//! 1. SQLECmd appends a trailing `SourceFile` column to every output CSV. Our
//!    SQLETriage now mirrors this (added in lib.rs).
//! 2. Timestamps are raw SQL `datetime()` text (`2023-04-20 14:19:56`) on both
//!    sides. The testkit's `compare_csv_unkeyed` auto-normalizes the REFERENCE
//!    side only; we pre-normalize OUR CSV cells through
//!    `triage_testkit::normalize_reference_timestamp` so both sides match.
//! 3. dbid = sha1(absolute_path_string)[:8] (lowercase hex), where the path
//!    is rooted at the repo's `test captures` symlink (NOT symlink-resolved).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use triage_testkit::{compare_csv_unkeyed, normalize_reference_timestamp, skip_if_missing};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn captures_root() -> PathBuf {
    repo_root().join("test captures")
}

fn fixture_dir() -> PathBuf {
    repo_root().join("tests/fixtures/sqlecmd")
}

/// Walk a directory recursively, collecting all file paths.
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

/// Compute dbid: sha1(absolute_path_string)[:8] (lowercase hex, first 4 bytes).
fn dbid(path: &Path) -> String {
    use sha1::{Digest, Sha1};
    let s = path.to_string_lossy();
    let d = Sha1::digest(s.as_bytes());
    d.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// Run the real SQLETriage binary over `db` into a leaked temp dir; return root.
fn run_sqletriage(db: &Path) -> PathBuf {
    use assert_cmd::Command;
    let tmp = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let out = tmp.path().to_path_buf();
    Command::cargo_bin("SQLETriage")
        .unwrap()
        .arg("-f")
        .arg(db)
        .arg("--csv")
        .arg(&out)
        .assert()
        .success();
    out
}

/// Find a produced CSV by its basename anywhere under root.
///
/// In the default Flat layout the side-car filename carries the identity before
/// the extension (e.g. `ChromiumBrowser_HistoryVisits.csv` ->
/// `ChromiumBrowser_HistoryVisits_alice.csv`), so we accept either the exact
/// bare name (Nested mode) or `<stem>_<identity>.<ext>` (Flat mode).
fn produced(root: &Path, basename: &str) -> Option<PathBuf> {
    let (bare_stem, bare_ext) = match basename.rsplit_once('.') {
        Some(parts) => parts,
        None => {
            return walk(root)
                .into_iter()
                .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(basename))
        }
    };
    walk(root).into_iter().find(|p| {
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        if name == basename {
            return true;
        }
        match name.rsplit_once('.') {
            Some((stem, ext)) => {
                ext == bare_ext
                    && stem.starts_with(bare_stem)
                    && stem.len() > bare_stem.len()
                    && stem.as_bytes()[bare_stem.len()] == b'_'
            }
            None => false,
        }
    })
}

/// Re-write every cell of our CSV through `normalize_reference_timestamp`,
/// matching the testkit's reference normalization so both sides compare equal.
/// Returns the path of a temp file containing the normalized CSV.
fn normalize_our_csv(path: &Path) -> PathBuf {
    let mut rdr = csv::Reader::from_path(path)
        .unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display()));
    let headers: Vec<String> = rdr.headers().unwrap().iter().map(str::to_string).collect();

    let tmp = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let out_path = tmp.path().join("sqle_normalized.csv");
    let mut wtr = csv::Writer::from_path(&out_path).unwrap();
    wtr.write_record(&headers).unwrap();

    for rec in rdr.records() {
        let rec = rec.unwrap();
        let row: Vec<String> = rec.iter().map(normalize_reference_timestamp).collect();
        wtr.write_record(&row).unwrap();
    }
    wtr.flush().unwrap();
    out_path
}

#[test]
fn all_sqlecmd_fixtures_match() {
    // If test captures are absent (gitignored on this machine), skip.
    if skip_if_missing(&captures_root(), "test captures") {
        return;
    }

    // Build dbid → DB path map by walking captures_root() and keeping SQLite
    // files. Path strings must NOT be canonicalized — the gen script used paths
    // rooted at the `test captures` symlink.
    const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
    let mut dbid_map: HashMap<String, PathBuf> = HashMap::new();
    for path in walk(&captures_root()) {
        if let Ok(mut f) = std::fs::File::open(&path) {
            let mut buf = [0u8; 16];
            if f.read_exact(&mut buf).is_ok() && &buf == SQLITE_MAGIC {
                let id = dbid(&path);
                // Multiple DBs can hash to the same id in theory; keep first.
                dbid_map.entry(id).or_insert(path);
            }
        }
    }

    // Cache: db path → output root (each DB is only processed once).
    let mut run_cache: HashMap<PathBuf, PathBuf> = HashMap::new();

    let mut failures: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut passed = 0usize;
    let mut skipped_no_db = 0usize;
    let mut skipped_no_csv = 0usize;

    // Collect and sort fixture files for deterministic ordering.
    let mut fixtures: Vec<PathBuf> = walk(&fixture_dir())
        .into_iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("csv"))
        .collect();
    fixtures.sort();

    for fixture_path in &fixtures {
        let stem = fixture_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        // Fixture stem: <COLLECTION>__<user>__<dbid>__<basename>
        // where <basename> may contain underscores but NOT double-underscores.
        let parts: Vec<&str> = stem.splitn(4, "__").collect();
        if parts.len() != 4 {
            // Unexpected name; skip.
            eprintln!("SKIP (unexpected fixture name): {stem}");
            continue;
        }
        let fid = parts[2];
        let basename_stem = parts[3]; // e.g. "ChromiumBrowser_HistoryVisits"

        total += 1;

        // Look up the DB.
        let Some(db_path) = dbid_map.get(fid) else {
            eprintln!("SKIP (no DB for dbid {fid}): {stem}");
            skipped_no_db += 1;
            continue;
        };

        // Run SQLETriage (or use cache).
        let out_root = run_cache
            .entry(db_path.clone())
            .or_insert_with(|| run_sqletriage(db_path))
            .clone();

        // Find the produced CSV.
        let csv_name = format!("{basename_stem}.csv");
        let Some(our_csv) = produced(&out_root, &csv_name) else {
            eprintln!("SKIP (SQLETriage produced no {csv_name} for {fid}): {stem}");
            skipped_no_csv += 1;
            continue;
        };

        // Normalize our timestamps so they compare equal to the testkit's
        // auto-normalized reference side.
        let norm_csv = normalize_our_csv(&our_csv);

        // Compare (unkeyed sorted multiset; SourceFile compared by basename).
        let diff = compare_csv_unkeyed(fixture_path, &norm_csv, &["SourceFile"], &[]);
        if diff.is_match() {
            passed += 1;
        } else {
            let msg = format!(
                "FAIL {stem}:\n  ref_rows={} our_rows={}\n  {}",
                diff.reference_rows,
                diff.our_rows,
                diff.mismatches.join("\n  ")
            );
            eprintln!("{msg}");
            failures.push(msg);
        }
    }

    eprintln!(
        "\nSQLECmd compat: {passed}/{total} fixtures passed ({skipped_no_db} skipped no-db, {skipped_no_csv} skipped no-csv)"
    );

    assert!(
        failures.is_empty(),
        "{} fixture(s) failed:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
