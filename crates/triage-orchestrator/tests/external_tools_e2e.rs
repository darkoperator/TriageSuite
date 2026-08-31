//! End-to-end test for `--config`/`--profile`: proves the orchestrator loads a TOML config,
//! resolves the named profile, and runs the (stubbed) hayabusa/takajo binaries per host,
//! recording the results in the manifest. Uses shell-script stubs instead of the real
//! binaries so this runs everywhere without needing hayabusa/takajo installed.

#![cfg(unix)]

use assert_cmd::Command;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

fn write_stub(
    dir: &std::path::Path,
    name: &str,
    output_flag: &str,
    as_dir: bool,
) -> std::path::PathBuf {
    let path = dir.join(name);
    let body = if as_dir {
        format!(
            "#!/bin/sh\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"{output_flag}\" ]; then\n    mkdir -p \"$a\"\n    echo stub > \"$a/report.txt\"\n  fi\n  prev=\"$a\"\ndone\nexit 0\n"
        )
    } else {
        format!(
            "#!/bin/sh\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"{output_flag}\" ]; then\n    echo stub > \"$a\"\n  fi\n  prev=\"$a\"\ndone\nexit 0\n"
        )
    };
    fs::write(&path, body).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

#[test]
fn config_and_profile_drive_stubbed_hayabusa_and_takajo() {
    let td = TempDir::new().unwrap();

    // Synthetic Velociraptor collection (same shape as tests/e2e.rs).
    let coll = td.path().join("Collection-HOSTX-2026");
    fs::create_dir_all(coll.join("uploads/auto/C%3A/Windows/System32/winevt/Logs")).unwrap();
    fs::write(coll.join("uploads.json"), "{}").unwrap();
    fs::write(
        coll.join("client_info.json"),
        r#"{"Hostname":"HOSTX","Platform":"Microsoft Windows 11 Enterprise","PlatformVersion":"23H2"}"#,
    )
    .unwrap();

    let stub_dir = td.path().join("bin");
    fs::create_dir_all(&stub_dir).unwrap();
    let hayabusa_stub = write_stub(&stub_dir, "hayabusa", "--output", false);
    let takajo_stub = write_stub(&stub_dir, "takajo", "-o", true);

    let config_path = td.path().join("triage.toml");
    fs::write(
        &config_path,
        format!(
            r#"
[hayabusa]
bin = "{hb}"

[takajo]
bin = "{tj}"

[profiles.quick.hayabusa]
min_level = "high"

[profiles.quick.takajo]
enabled = false
"#,
            hb = hayabusa_stub.to_str().unwrap(),
            tj = takajo_stub.to_str().unwrap(),
        ),
    )
    .unwrap();

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

/// Build the same synthetic collection the test above uses, plus the two stubs
/// and a config pointing at them. Returns (tempdir, collection, config path).
fn stubbed_collection() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-HOSTX-2026");
    fs::create_dir_all(coll.join("uploads/auto/C%3A/Windows/System32/winevt/Logs")).unwrap();
    fs::write(coll.join("uploads.json"), "{}").unwrap();
    fs::write(
        coll.join("client_info.json"),
        r#"{"Hostname":"HOSTX","Platform":"Microsoft Windows 11 Enterprise","PlatformVersion":"23H2"}"#,
    )
    .unwrap();

    let stub_dir = td.path().join("bin");
    fs::create_dir_all(&stub_dir).unwrap();
    let hayabusa_stub = write_stub(&stub_dir, "hayabusa", "--output", false);
    let takajo_stub = write_stub(&stub_dir, "takajo", "-o", true);

    let config_path = td.path().join("triage.toml");
    fs::write(
        &config_path,
        format!(
            "[hayabusa]\nbin = \"{hb}\"\n\n[takajo]\nbin = \"{tj}\"\n",
            hb = hayabusa_stub.to_str().unwrap(),
            tj = takajo_stub.to_str().unwrap(),
        ),
    )
    .unwrap();

    (td, coll, config_path)
}

/// `--skip` accepts external-tool keys and in-process parser keys in one list.
/// The external keys are stripped before the in-process registry validates the
/// list — otherwise it would reject them as unknown — and then force-disable
/// their tools.
#[test]
fn skip_disables_an_external_tool_without_tripping_registry_validation() {
    let (td, coll, config_path) = stubbed_collection();
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
    let (td, coll, _config_path) = stubbed_collection();
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
