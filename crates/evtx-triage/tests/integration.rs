//! EvtxTriage integration tests.
//!
//! - maps-bundle: evidence-free; asserts the embedded corpus loaded.
//! - aggregate header / split: gated on `test captures/`; use a reliably-populated
//!   classic Windows log (System/Security/Application.evtx), since many channel
//!   logs are empty (0 records → no output file).

use std::path::{Path, PathBuf};

use assert_cmd::Command;

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

/// A reliably-populated `.evtx` under captures (the classic always-on logs), or
/// None if captures absent. Falls back to None rather than an empty log.
fn populated_evtx() -> Option<PathBuf> {
    let root = captures_root();
    if !root.exists() {
        return None;
    }
    let all = walk(&root);
    for name in ["System.evtx", "Security.evtx", "Application.evtx"] {
        if let Some(p) = all.iter().find(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("evtx"))
                .unwrap_or(false)
                && p.file_name().and_then(|n| n.to_str()) == Some(name)
        }) {
            return Some(p.clone());
        }
    }
    None
}

const HEADER: &str = "RecordNumber,EventRecordId,TimeCreated,EventId,Level,Provider,Channel,ProcessId,ThreadId,Computer,ChunkNumber,UserId,MapDescription,UserName,RemoteHost,PayloadData1,PayloadData2,PayloadData3,PayloadData4,PayloadData5,PayloadData6,ExecutableInfo,HiddenRecord,SourceFile,Keywords,ExtraDataOffset,Payload";

/// Read the first file whose name ends with `suffix` (the aggregate output is
/// `<timestamp>_EvtxTriage_Output.csv`, so we match on the suffix).
fn produced_suffix(root: &Path, suffix: &str) -> Option<String> {
    walk(root)
        .into_iter()
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(suffix))
        })
        .map(|p| std::fs::read_to_string(p).unwrap())
}

#[test]
fn maps_corpus_is_embedded_and_loads() {
    let n = evtx_triage::maps_embed::embedded_count();
    assert!(
        n > 0,
        "embedded evtx-maps corpus is empty (resources/evtx-maps not bundled?)"
    );
    let _maps = evtx_triage::maps_embed::load_bundled(); // must not panic
}

#[test]
fn aggregate_csv_header_when_evidence_present() {
    let Some(evtx) = populated_evtx() else { return };
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("EvtxTriage")
        .unwrap()
        .arg("-f")
        .arg(&evtx)
        .arg("--csv")
        .arg(tmp.path())
        .assert()
        .success();
    let csv = produced_suffix(tmp.path(), "_EvtxTriage_Output.csv")
        .expect("no <stamp>_EvtxTriage_Output.csv produced from a populated log");
    assert!(
        csv.starts_with(HEADER),
        "header mismatch:\n{}",
        csv.lines().next().unwrap_or("")
    );
    assert!(
        csv.lines().count() > 1,
        "expected at least one data row from a populated log"
    );
}

#[test]
fn split_writes_per_source_file_in_addition_to_the_aggregate_when_evidence_present() {
    let Some(evtx) = populated_evtx() else { return };
    // Isolate a single populated log into its own dir so --split is fast + deterministic.
    let src = tempfile::tempdir().unwrap();
    let stem = evtx.file_stem().and_then(|s| s.to_str()).unwrap();
    let local = src.path().join(format!("{stem}.evtx"));
    std::fs::copy(&evtx, &local).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("EvtxTriage")
        .unwrap()
        .arg("-d")
        .arg(src.path())
        .arg("--csv")
        .arg(tmp.path())
        .arg("--split")
        .assert()
        .success();

    let csvs: Vec<String> = walk(tmp.path())
        .into_iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .filter(|n| n.ends_with(".csv"))
        .collect();
    // Flat layout (default) folds the identity into the per-source name; the
    // tool is SystemWide so the identity is always `system`.
    assert!(
        csvs.iter().any(|n| n == &format!("{stem}_system.csv")),
        "expected per-source {stem}_system.csv; got {csvs:?}"
    );
    // --split is additive: the combined aggregate output is still produced too.
    assert!(
        csvs.iter().any(|n| n.ends_with("_EvtxTriage_Output.csv")),
        "expected the aggregate EvtxTriage_Output.csv alongside the per-source file; got {csvs:?}"
    );
}
