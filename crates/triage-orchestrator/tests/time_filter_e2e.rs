//! Proves `--start`/`--end` is passthrough only: it reaches EvtxTriage (and,
//! via the manifest, is recorded as `not_applicable` for every tool that has
//! no time filter), and the honesty statement about that scope actually
//! appears in the run's own output -- not just that the manifest field
//! exists.
//!
//! Runs with `--no-validate`. The collection here is the minimal synthetic
//! one and the run deliberately selects every tool, so the placeholder hives
//! and EVTX that would satisfy the pre-flight gate
//! (`synthetic::write_gate_passing_collection`) would instead be handed to
//! the parsers and fail, changing the very exit status this asserts.

use assert_cmd::Command;
use std::fs;
use tempfile::TempDir;
use triage_testkit::synthetic::write_collection;

fn run_with(td: &TempDir, extra: &[&str]) -> (assert_cmd::assert::Assert, std::path::PathBuf) {
    let coll = td.path().join("Collection-HOSTX-2026");
    write_collection(&coll, "HOSTX");
    let out = td.path().join("out");
    let mut args: Vec<&str> = vec![
        "run",
        "--no-validate",
        coll.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--overwrite",
    ];
    args.extend_from_slice(extra);
    let assert = Command::cargo_bin("TriageSuite")
        .unwrap()
        .args(&args)
        .assert();
    (assert, out)
}

#[test]
fn the_range_reaches_evtx_and_is_marked_not_applicable_elsewhere() {
    let td = TempDir::new().unwrap();
    let (assert, out) = run_with(
        &td,
        &[
            "--start",
            "2026-03-01T00:00:00Z",
            "--end",
            "2026-03-10T00:00:00Z",
        ],
    );
    assert.success();

    let text = fs::read_to_string(out.join("run_manifest.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
    let tools = manifest["hosts"][0]["tools"].as_array().unwrap();
    let evtx = tools.iter().find(|t| t["key"] == "evtx").unwrap();
    let mft = tools.iter().find(|t| t["key"] == "mft").unwrap();
    assert_eq!(evtx["time_filter"], "applied");
    assert_eq!(mft["time_filter"], "not_applicable");
}

#[test]
fn without_a_range_no_tool_claims_a_filter() {
    let td = TempDir::new().unwrap();
    let (assert, out) = run_with(&td, &[]);
    assert.success();

    let text = fs::read_to_string(out.join("run_manifest.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
    for tool in manifest["hosts"][0]["tools"].as_array().unwrap() {
        assert_eq!(tool["time_filter"], serde_json::Value::Null, "{tool}");
    }
}

#[test]
fn an_end_before_the_start_is_a_usage_error() {
    let td = TempDir::new().unwrap();
    let (assert, _out) = run_with(
        &td,
        &[
            "--start",
            "2026-03-10T00:00:00Z",
            "--end",
            "2026-03-01T00:00:00Z",
        ],
    );
    assert.failure().code(2);
}

/// The manifest field alone would let an implementer skip the plain-language
/// statement this task exists for -- so this proves the sentence actually
/// appears in stdout and in the per-tool process log, not just that
/// `time_filter` is set correctly.
/// Unlike the other tests in this file, this one gives EvtxTriage a
/// discoverable (if garbage) `.evtx` candidate: `run_tool_on_host` only
/// opens a tool's process log once it has at least one candidate at all --
/// a genuinely zero-match tool gets no output tree (see the
/// `no_candidates_at_all` comment in `execute.rs`), so there would be no log
/// to check the statement was written into. The garbage file fails
/// validation, which makes this run's aggregate exit non-zero -- irrelevant
/// here, since the point is what got written, not the exit code.
#[test]
fn the_honesty_statement_appears_in_stdout_and_the_process_log() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-HOSTX-2026");
    write_collection(&coll, "HOSTX");
    std::fs::create_dir_all(coll.join("uploads")).unwrap();
    std::fs::write(
        coll.join("uploads/placeholder.evtx"),
        b"not a real evtx file",
    )
    .unwrap();
    let out = td.path().join("out");
    let assert = Command::cargo_bin("TriageSuite")
        .unwrap()
        .args([
            "run",
            "--no-validate",
            coll.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
            "--overwrite",
            "--start",
            "2026-03-01T00:00:00Z",
            "--end",
            "2026-03-10T00:00:00Z",
        ])
        .assert();
    let output = assert.get_output().stdout.clone();
    let stdout = String::from_utf8_lossy(&output);
    assert!(
        stdout.contains("Time range 2026-03-01T00:00:00Z .. 2026-03-10T00:00:00Z")
            && stdout.contains("event logs only")
            && stdout.contains("EvtxTriage")
            && stdout.contains("Hayabusa")
            && stdout.contains("Takajo"),
        "run summary must state the range's scope plainly: {stdout}"
    );

    // Find EvtxTriage's process log under the Velo-shaped output tree and
    // confirm the same statement is written there too.
    let log = walk_for_evtx_log(&out)
        .unwrap_or_else(|| panic!("no EvtxTriage process log found under {}", out.display()));
    let body = fs::read_to_string(&log).unwrap();
    assert!(
        body.contains("Time range 2026-03-01T00:00:00Z .. 2026-03-10T00:00:00Z"),
        "process log {} must carry the honesty statement: {body}",
        log.display()
    );
}

fn walk_for_evtx_log(root: &std::path::Path) -> Option<std::path::PathBuf> {
    for entry in walkdir(root) {
        let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if entry.is_file() && name.contains("EvtxTriage") && name.ends_with(".log") {
            return Some(entry);
        }
    }
    None
}

/// Minimal recursive walk -- this crate does not depend on `walkdir` and one
/// test file does not need a new dependency for it.
fn walkdir(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}
