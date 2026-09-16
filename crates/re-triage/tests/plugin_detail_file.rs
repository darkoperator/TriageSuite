//! `PluginDetailFile` names the per-plugin detail CSV a batch row's plugin
//! wrote. This test runs the real `RETriage` binary over a real per-user
//! `NTUSER.DAT` and asserts every non-empty reference resolves from the batch
//! CSV's own directory.
//!
//! The standalone CLI writes the default Flat layout, where the router folds
//! the identity into a side-car's filename (`TypedURLs_NTUSER.DAT_cperez.csv`)
//! while the batch CSV and the side-cars share one directory. The reference
//! used to name the bare `TypedURLs_NTUSER.DAT.csv`, so for a per-user hive it
//! pointed at nothing at all.
//!
//! `velo_plugin_detail_file.rs` (triage-orchestrator) is the counterpart for
//! the Velo layout, where the side-cars sit in a directory of their own.
//!
//! Gated on `test captures/` via `triage_testkit::skip_if_missing`: it is a
//! real hive that makes a plugin fire and write a detail CSV at all.

use assert_cmd::Command;
use std::path::{Path, PathBuf};

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

/// The first `.../Users/<profile>/NTUSER.DAT` in the captures tree.
fn find_per_user_ntuser() -> Option<PathBuf> {
    let mut stack = vec![captures_root()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == "NTUSER.DAT") {
                return Some(path);
            }
        }
    }
    None
}

#[test]
fn every_plugin_detail_file_reference_resolves_from_the_batch_csv() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let Some(hive) = find_per_user_ntuser() else {
        eprintln!("SKIP: no per-user NTUSER.DAT in the captures tree");
        return;
    };

    let out = tempfile::tempdir().unwrap();
    Command::cargo_bin("RETriage")
        .unwrap()
        .arg("-f")
        .arg(&hive)
        .arg("--csv")
        .arg(out.path())
        .assert()
        .success();

    let batch_csv = std::fs::read_dir(out.path())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("_RETriage_Batch_Output") && n.ends_with(".csv"))
        })
        .unwrap_or_else(|| panic!("RETriage produced no batch CSV under {:?}", out.path()));
    let batch_dir = batch_csv.parent().unwrap().to_path_buf();

    let mut reader = csv::Reader::from_path(&batch_csv).unwrap();
    let headers = reader.headers().unwrap().clone();
    let column = headers
        .iter()
        .position(|h| h == "PluginDetailFile")
        .expect("the batch CSV must carry a PluginDetailFile column");

    let mut total_rows = 0usize;
    let mut referencing_rows = 0usize;
    let mut dangling: Vec<String> = Vec::new();
    for record in reader.records() {
        let record = record.unwrap();
        total_rows += 1;
        let Some(reference) = record.get(column).filter(|v| !v.is_empty()) else {
            continue;
        };
        referencing_rows += 1;
        if !batch_dir.join(reference).is_file() {
            dangling.push(reference.to_string());
        }
    }

    assert!(total_rows > 0, "the run produced no batch rows at all");
    assert!(
        referencing_rows > 0,
        "no batch row carried a PluginDetailFile, so nothing was actually checked"
    );
    dangling.sort();
    dangling.dedup();
    assert!(
        dangling.is_empty(),
        "{} of {referencing_rows} PluginDetailFile references do not resolve next to {}:\n{}",
        dangling.len(),
        batch_csv.display(),
        dangling.join("\n")
    );
}
