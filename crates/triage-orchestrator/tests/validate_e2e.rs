//! The pre-flight capture-validation gate, end to end: `TriageSuite
//! validate <input>` and the same gate on the `run` path.
//!
//! The tests that must prove a *real* capture passes are gated on
//! `test captures/`: the real Collection-DESKTOP-OA8SHHC directory has real
//! EVTX and registry hives under Velociraptor's URL-encoded
//! `uploads/auto/C%3A/...` layout, which is the shape a synthetic fixture
//! cannot fake without becoming a second implementation of the parsers this
//! task validates ahead of. `TRIAGE_ALLOW_COMPAT_SKIP=1` lets a checkout
//! without that evidence tree skip cleanly instead of failing on missing
//! fixtures. The tests about *which inputs the gate is applied to* need no
//! evidence tree: the gate reads names, so a synthetic collection carrying
//! placeholder artifacts is a faithful fixture for that question.

use assert_cmd::Command;
use std::path::{Path, PathBuf};

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

#[test]
fn validate_exits_zero_on_a_real_capture() {
    let root = captures_root();
    let capture = root.join("Collection-DESKTOP-OA8SHHC-2026-03-12T22_54_56Z");
    if triage_testkit::skip_if_missing(&capture, "Collection-DESKTOP-OA8SHHC fixture") {
        return;
    }

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args(["validate", capture.to_str().unwrap()])
        .assert()
        .code(0);
}

#[test]
fn validate_exits_three_on_a_capture_missing_event_logs_and_hives() {
    let tmp = tempfile::tempdir().unwrap();
    let capture = tmp.path().join("broken");
    // Neither event logs nor registry hives: every error condition fires.
    std::fs::create_dir_all(&capture).unwrap();
    std::fs::write(capture.join("readme.txt"), b"not a capture").unwrap();

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args(["validate", capture.to_str().unwrap()])
        .assert()
        .code(3);
}

/// Nothing above exercises `run` rejecting anything: a refactor that broke
/// the gate on the `run` path, or dropped the call to `validate_capture`
/// from `run` entirely, would leave every other `run`-path test in this
/// workspace green, because those either select tools the gate's artifacts
/// are invisible to or pass `--no-validate` outright. This proves the gate
/// fires there: the deficient capture is skipped rather than processed, the
/// skip and its reason land in the manifest, and the process exits 3 -- the
/// same `RunExit::InputMissing` code `validate` uses, confirmed empirically
/// rather than assumed (`main.rs`'s `run()` returns it whenever the gate
/// left no collection to process).
#[test]
fn run_without_no_validate_skips_a_deficient_capture_and_records_why() {
    let td = tempfile::tempdir().unwrap();
    let capture = td.path().join("Collection-DEFICIENT");
    triage_testkit::synthetic::write_collection(&capture, "DEFICIENT");
    let out = td.path().join("out");

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args([
            "run",
            capture.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
            "--overwrite",
        ])
        .assert()
        .code(3);

    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("run_manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["final_exit_status"], 3);
    assert_eq!(
        manifest["hosts"].as_array().unwrap().len(),
        0,
        "a skipped input must not be processed: {manifest}"
    );
    let archives = manifest["archives"].as_array().unwrap();
    assert_eq!(archives.len(), 1, "got {manifest}");
    assert_eq!(archives[0]["status"], "skipped");
    let reason = archives[0]["error"].as_str().unwrap();
    assert!(
        reason.contains("event log") && reason.contains("SYSTEM"),
        "manifest must record why: {reason}"
    );
    // The skipped input is a directory, not an archive: its size and hash
    // are absent rather than a directory's inode size and a null hash
    // sitting under fields an analyst reads as the archive's own.
    assert!(archives[0]["size_bytes"].is_null(), "got {manifest}");
    assert!(archives[0]["sha256"].is_null(), "got {manifest}");
}

/// Companion to the skip case above: a gate that rejected everything would
/// also make the skip test above look correct. A capture that genuinely
/// satisfies every criterion (real EVTX, real SYSTEM/SOFTWARE/SAM/SECURITY
/// hives) must still run to completion with the gate active and no
/// `--no-validate`. Real files out of `test captures/` are what make this
/// different from the placeholder-artifact fixtures below: here the files
/// that satisfy the gate are also handed to a parser (`--only re`) and
/// parsed for real, so the gate and the tools agree about the same
/// capture -- same reasoning as `velo_sysinfo_e2e.rs`, and `--only re`
/// keeps it to four files rather than a whole collection.
#[test]
fn run_without_no_validate_processes_a_capture_that_passes() {
    let root = captures_root();
    let source = root.join("Collection-DESKTOP-OA8SHHC-2026-03-12T22_54_56Z");
    if triage_testkit::skip_if_missing(&source, "Collection-DESKTOP-OA8SHHC fixture") {
        return;
    }
    let config_dir = source.join("uploads/auto/C%3A/Windows/System32/config");
    let evtx_dir = source.join("uploads/auto/C%3A/Windows/System32/winevt/Logs");
    let evtx_file = std::fs::read_dir(&evtx_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("evtx"))
        })
        .expect("fixture must contain at least one .evtx file");

    let td = tempfile::tempdir().unwrap();
    let capture = td.path().join("Collection-PASSES");
    let dest_config = capture.join("config");
    let dest_logs = capture.join("winevt/Logs");
    std::fs::create_dir_all(&dest_config).unwrap();
    std::fs::create_dir_all(&dest_logs).unwrap();
    for hive in ["SYSTEM", "SOFTWARE", "SAM", "SECURITY"] {
        std::fs::copy(config_dir.join(hive), dest_config.join(hive)).unwrap();
    }
    std::fs::copy(&evtx_file, dest_logs.join(evtx_file.file_name().unwrap())).unwrap();

    let out = td.path().join("out");
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args([
            "run",
            capture.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
            "--overwrite",
            "--only",
            "re",
        ])
        .assert()
        .success();

    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("run_manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["final_exit_status"], 0);
    assert_eq!(
        manifest["hosts"].as_array().unwrap().len(),
        1,
        "a capture that passes validation must still be processed: {manifest}"
    );
}

/// A folder of collector ZIPs is a documented input shape
/// (`TriageSuite run ./engagement-zips`), and the gate must not reject it.
/// Checking the folder itself sees ZIP filenames, not the artifacts inside
/// them, so every criterion fails at once and no host is ever processed;
/// the gate therefore has to run over each enumerated collection instead.
///
/// The collections here carry placeholder hive/EVTX bytes (see
/// `synthetic::GATE_ARTIFACT_FILES`): the gate reads names, never file
/// contents, and `--only pe` keeps every parser away from them.
#[test]
fn run_without_no_validate_processes_every_host_in_a_folder_of_zips() {
    let td = tempfile::tempdir().unwrap();
    let zips = td.path().join("engagement-zips");
    std::fs::create_dir_all(&zips).unwrap();
    triage_testkit::synthetic::write_gate_passing_collection_zip(
        &zips.join("Collection-HOSTA.zip"),
        "",
        "HOSTA",
    );
    triage_testkit::synthetic::write_gate_passing_collection_zip(
        &zips.join("Collection-HOSTB.zip"),
        "",
        "HOSTB",
    );
    let out = td.path().join("out");

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args([
            "run",
            zips.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
            "--overwrite",
            "--only",
            "pe",
        ])
        .assert()
        .success();

    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("run_manifest.json")).unwrap()).unwrap();
    let mut hosts: Vec<&str> = manifest["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["host"].as_str().unwrap())
        .collect();
    hosts.sort_unstable();
    assert_eq!(hosts, vec!["HOSTA", "HOSTB"], "got {manifest}");
}

/// Per-input skipping, which is the other half of the fix: one deficient
/// archive in a folder must not take the rest of the engagement down with
/// it. The deficient collection is the minimal synthetic one -- no event
/// logs and no hives -- so the gate rejects it on its own merits, and the
/// manifest has to say which input was dropped and why.
#[test]
fn run_without_no_validate_skips_only_the_deficient_archive_in_a_folder() {
    let td = tempfile::tempdir().unwrap();
    let zips = td.path().join("engagement-zips");
    std::fs::create_dir_all(&zips).unwrap();
    triage_testkit::synthetic::write_gate_passing_collection_zip(
        &zips.join("Collection-GOOD.zip"),
        "",
        "GOOD",
    );
    triage_testkit::synthetic::write_collection_zip(&zips.join("Collection-BAD.zip"), "", "BAD");
    let out = td.path().join("out");

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args([
            "run",
            zips.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
            "--overwrite",
            "--only",
            "pe",
        ])
        .assert()
        .success();

    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("run_manifest.json")).unwrap()).unwrap();
    let hosts = manifest["hosts"].as_array().unwrap();
    assert_eq!(hosts.len(), 1, "got {manifest}");
    assert_eq!(hosts[0]["host"], "GOOD");

    let skipped: Vec<&serde_json::Value> = manifest["archives"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["status"] == "skipped")
        .collect();
    assert_eq!(skipped.len(), 1, "got {manifest}");
    let reason = skipped[0]["error"].as_str().unwrap();
    assert!(
        reason.contains("event log") && reason.contains("SYSTEM"),
        "manifest must record why: {reason}"
    );

    // `archives[]` is chain-of-custody output, so the skipped entry must
    // identify the deficient *archive* -- with its real size and SHA256 --
    // and not the directory it was unpacked into, whose inode size would
    // read as the archive's own. One entry per input, too: the archive does
    // not appear a second time as a plain extraction.
    let path = skipped[0]["archive_path"].as_str().unwrap();
    assert!(
        path.ends_with("Collection-BAD.zip"),
        "the skip must name the deficient archive: {manifest}"
    );
    let on_disk = std::fs::metadata(path).unwrap().len();
    assert_eq!(
        skipped[0]["size_bytes"].as_u64(),
        Some(on_disk),
        "got {manifest}"
    );
    assert_eq!(
        skipped[0]["sha256"].as_str().map(str::len),
        Some(64),
        "got {manifest}"
    );
    let named_twice = manifest["archives"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["archive_path"] == path)
        .count();
    assert_eq!(named_twice, 1, "one entry per input: {manifest}");
}

/// `validate` accepts the same input shapes `run` does, its own help text
/// says so, and a folder of collector ZIPs is one of them: checking the
/// folder as if it were a single capture classifies a lone archive as
/// double-zipped and fails a folder of good ones on every criterion.
#[test]
fn validate_expands_a_folder_of_zips_into_its_individual_archives() {
    let td = tempfile::tempdir().unwrap();
    let zips = td.path().join("engagement-zips");
    std::fs::create_dir_all(&zips).unwrap();
    triage_testkit::synthetic::write_gate_passing_collection_zip(
        &zips.join("Collection-ONLY.zip"),
        "",
        "ONLY",
    );

    // A single archive in the folder: the case the container check reported
    // as "double-zipped".
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args(["validate", zips.to_str().unwrap()])
        .assert()
        .code(0);

    // A second, deficient archive alongside it makes the whole check fail,
    // and the failure has to name the archive rather than the folder.
    triage_testkit::synthetic::write_collection_zip(&zips.join("Collection-BAD.zip"), "", "BAD");
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args(["validate", zips.to_str().unwrap()])
        .assert()
        .code(3)
        .stderr(predicates::str::contains("Collection-BAD.zip"));
}
