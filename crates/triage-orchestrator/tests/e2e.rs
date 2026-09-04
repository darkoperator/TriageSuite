//! End-to-end test for the `run` subcommand, wired against a synthetic
//! Velociraptor collection built entirely in a tempdir. No committed
//! artifact fixtures are required, so this test runs everywhere — it proves
//! the whole pipeline (capture detection -> tool selection -> shared file
//! index -> per-tool execution -> manifest assembly/write) is wired
//! end-to-end, even though every tool legitimately reports zero matches
//! over this deliberately empty capture.

use assert_cmd::Command;
use std::fs;
use tempfile::TempDir;
use triage_testkit::synthetic::write_collection;

#[test]
fn run_over_a_synthetic_collection_produces_manifest_and_output() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-HOSTX-2026");
    write_collection(&coll, "HOSTX");

    let out = td.path().join("out");
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args([
            "run",
            coll.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
            "--overwrite",
        ])
        .assert()
        .success();

    let manifest_path = out.join("run_manifest.json");
    assert!(manifest_path.is_file(), "run_manifest.json must be written");
    let text = fs::read_to_string(&manifest_path).unwrap();
    assert!(
        text.contains(r#""host": "HOSTX""#),
        "manifest must record the detected host: {text}"
    );
    assert!(
        text.contains(r#""capture_type": "velociraptor""#),
        "manifest must record the detected capture type: {text}"
    );
    // The synthetic capture has no parsable artifacts, so every tool
    // legitimately reports zero matches -- still a valid success path that
    // proves the whole pipeline wires together end-to-end.
    assert!(
        text.contains(r#""files_matched": 0"#),
        "expected zero matches over the empty capture: {text}"
    );
}
