//! End-to-end test for `--config`/`--profile`: proves the orchestrator loads a TOML config,
//! resolves the named profile, and runs the (stubbed) hayabusa/takajo binaries per host,
//! recording the results in the manifest. Uses shell-script stubs instead of the real
//! binaries so this runs everywhere without needing hayabusa/takajo installed.

#![cfg(unix)]

use assert_cmd::Command;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;
use triage_testkit::synthetic::{write_collection, write_stub};

/// A synthetic collection, the two stubs, and a config pointing at them with
/// `extra` appended after the two tool tables. Returns (tempdir, collection,
/// config path).
fn stubbed_collection(extra: &str) -> (TempDir, PathBuf, PathBuf) {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-HOSTX-2026");
    write_collection(&coll, "HOSTX");

    let stub_dir = td.path().join("bin");
    fs::create_dir_all(&stub_dir).unwrap();
    let hayabusa_stub = write_stub(&stub_dir, "hayabusa", "--output", false);
    let takajo_stub = write_stub(&stub_dir, "takajo", "-o", true);

    let config_path = td.path().join("triage.toml");
    fs::write(
        &config_path,
        format!(
            "[hayabusa]\nbin = \"{hb}\"\n\n[takajo]\nbin = \"{tj}\"\n{extra}",
            hb = hayabusa_stub.to_str().unwrap(),
            tj = takajo_stub.to_str().unwrap(),
        ),
    )
    .unwrap();

    (td, coll, config_path)
}

#[test]
fn config_and_profile_drive_stubbed_hayabusa_and_takajo() {
    let (td, coll, config_path) = stubbed_collection(
        "\n[profiles.quick.hayabusa]\nmin_level = \"high\"\n\n[profiles.quick.takajo]\nenabled = false\n",
    );
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
            "--config",
            config_path.to_str().unwrap(),
            "--profile",
            "quick",
        ])
        .assert()
        .success();

    let manifest_text =
        fs::read_to_string(out.join("run_manifest.json")).expect("manifest must be written");
    // hayabusa ran (csv only checked here; json/logon_summary default true too, so all
    // three should appear in the manifest alongside this one)
    assert!(
        manifest_text.contains("\"tool\": \"hayabusa-csv\""),
        "manifest missing hayabusa-csv entry: {manifest_text}"
    );
    // the "quick" profile disables takajo
    assert!(
        !manifest_text.contains("\"tool\": \"takajo-automagic\""),
        "quick profile should have disabled takajo: {manifest_text}"
    );
    assert!(out.join("HOSTX/Hayabusa/timeline.csv").is_file());
}

/// `--skip` accepts external-tool keys and in-process parser keys in one list.
/// The external keys are stripped before the in-process registry validates the
/// list — otherwise it would reject them as unknown — and then force-disable
/// their tools.
#[test]
fn skip_disables_an_external_tool_without_tripping_registry_validation() {
    let (td, coll, config_path) = stubbed_collection("");
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
            "--config",
            config_path.to_str().unwrap(),
            "--skip",
            "hayabusa,takajo,re",
        ])
        .assert()
        .success();

    let manifest_text =
        fs::read_to_string(out.join("run_manifest.json")).expect("manifest must be written");
    assert!(
        !manifest_text.contains("\"tool\": \"hayabusa-csv\""),
        "--skip hayabusa should have disabled it: {manifest_text}"
    );
    assert!(
        !manifest_text.contains("\"tool\": \"takajo-automagic\""),
        "--skip takajo should have disabled it: {manifest_text}"
    );
    assert!(
        !out.join("HOSTX/Hayabusa").exists(),
        "a skipped tool must not create its output directory"
    );
}

/// The filtering is deliberately one-way. `--only` selects which in-process
/// parsers run, and an external binary is not one of them, so naming one there
/// stays an error rather than silently becoming a no-op.
#[test]
fn only_still_rejects_an_external_tool_key() {
    let (td, coll, _config_path) = stubbed_collection("");
    let out = td.path().join("out");

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args([
            "run",
            coll.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--only",
            "hayabusa",
        ])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("unknown tool key: hayabusa"));
}
