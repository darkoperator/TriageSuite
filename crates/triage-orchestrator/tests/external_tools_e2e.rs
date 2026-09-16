//! End-to-end test for `--config`/`--profile`: proves the orchestrator loads a TOML config,
//! resolves the named profile, and runs the (stubbed) hayabusa/takajo binaries per host,
//! recording the results in the manifest. Uses shell-script stubs instead of the real
//! binaries so this runs everywhere without needing hayabusa/takajo installed.
//!
//! The five runs that reach the orchestrator proper pass `--no-validate`:
//! the collection is the minimal synthetic one and those runs select every
//! tool, so the placeholder hives and EVTX that would satisfy the pre-flight
//! gate (`synthetic::write_gate_passing_collection`) would instead be handed
//! to the parsers and fail the run. `only_still_rejects_an_external_tool_key`
//! needs no pin: `--only hayabusa` is rejected by `select_tools` before the
//! input is looked at at all.

#![cfg(unix)]

use assert_cmd::Command;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;
use triage_testkit::synthetic::{write_collection, write_executable, write_stub};

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

/// No `--layout` flag is passed, so this proves the Velo default: Hayabusa's
/// output must land under `Processed-HOSTX-<stamp>/EventLogs`, the forensic
/// category assigned to it (defect 2), and the manifest's reported
/// `output_paths` for the `hayabusa-csv` entry must name that exact path --
/// not the old hardcoded `<out>/<HOST>/Hayabusa` tree, which defect 2 wrote
/// to regardless of `--layout`.
#[test]
fn config_and_profile_drive_stubbed_hayabusa_and_takajo() {
    let (td, coll, config_path) = stubbed_collection(
        "\n[profiles.quick.hayabusa]\nmin_level = \"high\"\n\n[profiles.quick.takajo]\nenabled = false\n",
    );
    let out = td.path().join("out");
    const STAMP: &str = "2026-03-13T192553Z";

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", STAMP)
        .args([
            "run",
            "--no-validate",
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

    let timeline = out
        .join(format!("Processed-HOSTX-{STAMP}"))
        .join("EventLogs/timeline.csv");
    assert!(
        timeline.is_file(),
        "expected Hayabusa's CSV timeline under the EventLogs category: {timeline:?}"
    );
    assert!(
        !out.join("HOSTX/Hayabusa").exists(),
        "the native per-host tree must not appear when Velo is the default"
    );
    let reported_path = timeline
        .to_str()
        .expect("temp path is valid UTF-8")
        .replace('\\', "\\\\");
    assert!(
        manifest_text.contains(&reported_path),
        "manifest output_paths must report the real EventLogs path: {manifest_text}"
    );
}

/// Same scenario under `--layout native`: Hayabusa and Takajo keep today's
/// `<out>/<HOST>/Hayabusa|Takajo` paths, unaffected by the Velo category
/// placement defect 2 fixed.
#[test]
fn native_layout_keeps_the_old_hayabusa_and_takajo_paths() {
    let (td, coll, config_path) = stubbed_collection(
        "\n[profiles.quick.hayabusa]\nmin_level = \"high\"\n\n[profiles.quick.takajo]\nenabled = false\n",
    );
    let out = td.path().join("out");

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args([
            "run",
            "--no-validate",
            coll.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
            "--overwrite",
            "--layout",
            "native",
            "--config",
            config_path.to_str().unwrap(),
            "--profile",
            "quick",
        ])
        .assert()
        .success();

    assert!(out.join("HOSTX/Hayabusa/timeline.csv").is_file());
    assert!(!out.join("HOSTX/EventLogs").exists());
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
            "--no-validate",
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

/// A real run must write an external tool's actual stdout/stderr into
/// `process_logs/<report_name>.log` under `--layout velo` (the default) --
/// not a summary reconstructed after the fact, the genuine bytes the process
/// wrote. Hayabusa's stub is replaced here with one that emits distinct,
/// recognizable text on both streams, so this test can tell "the real
/// output landed" apart from "some placeholder text happens to match".
#[test]
fn external_tool_process_log_carries_real_stdout_and_stderr() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-HOSTX-2026");
    write_collection(&coll, "HOSTX");

    let stub_dir = td.path().join("bin");
    fs::create_dir_all(&stub_dir).unwrap();
    let hayabusa_stub = stub_dir.join("hayabusa");
    write_executable(
        &hayabusa_stub,
        "#!/bin/sh\necho 'hello from hayabusa stdout'\necho 'hello from hayabusa stderr' >&2\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--output\" ]; then\n    echo stub > \"$a\"\n  fi\n  prev=\"$a\"\ndone\nexit 0\n",
    );
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

    let out = td.path().join("out");
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", "2026-03-13T192553Z")
        .args([
            "run",
            "--no-validate",
            coll.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
            "--overwrite",
            "--config",
            config_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    let log_path = out
        .join("Processed-HOSTX-2026-03-13T192553Z")
        .join("process_logs/hayabusa-csv.log");
    let body = std::fs::read_to_string(&log_path)
        .unwrap_or_else(|e| panic!("expected {log_path:?} to exist: {e}"));
    assert!(
        body.contains("hello from hayabusa stdout"),
        "the log must carry the process's real stdout, not a summary: got {body}"
    );
    assert!(
        body.contains("hello from hayabusa stderr"),
        "the log must carry the process's real stderr: got {body}"
    );
    assert!(
        body.contains("--- command ---") && body.contains(hayabusa_stub.to_str().unwrap()),
        "the log must record the command line that was actually run: got {body}"
    );
    assert!(
        body.contains("--directory"),
        "the recorded command line must include its arguments, not just the binary: got {body}"
    );
}

/// A relative capture path must not break external-tool invocation.
///
/// `invoke::invoke` sets every external tool's cwd to the tool's own install
/// directory (Takajo refuses to run otherwise), so any path an invocation's
/// `plan()` built while it was still relative resolves against the wrong
/// directory once the process actually spawns. Hayabusa's `--directory` was
/// exactly this bug: a relative capture path silently produced "no .evtx
/// files found" instead of an error, which reads as a clean host. This stub
/// fails loudly instead of silently succeeding on a relative path, so a
/// regression here fails the test rather than reproducing the silent no-op.
#[test]
fn a_relative_capture_path_still_produces_external_tool_output() {
    let td = TempDir::new().unwrap();
    let coll_name = "Collection-HOSTX-2026";
    write_collection(&td.path().join(coll_name), "HOSTX");

    let stub_dir = td.path().join("bin");
    fs::create_dir_all(&stub_dir).unwrap();
    // Fails if `--directory`'s value is not absolute, rather than silently
    // "succeeding" with no matching files the way the real bug did.
    let hayabusa_stub = stub_dir.join("hayabusa");
    write_executable(
        &hayabusa_stub,
        "#!/bin/sh\nprev=\"\"\nfor a in \"$@\"; do\n  if [ \"$prev\" = \"--directory\" ]; then\n    case \"$a\" in\n      /*) ;;\n      *) echo \"relative --directory: $a\" >&2; exit 1 ;;\n    esac\n  fi\n  if [ \"$prev\" = \"--output\" ]; then\n    echo stub > \"$a\"\n  fi\n  prev=\"$a\"\ndone\nexit 0\n",
    );
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

    // Both the capture and `--out` are relative to the child process's cwd
    // (`td.path()`, set below), which is a different directory from
    // Hayabusa's stub install dir (`bin/`) — reproducing the exact mismatch
    // the real bug depended on.
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .current_dir(td.path())
        .args([
            "run",
            "--no-validate",
            coll_name,
            "--out",
            "out",
            "--csv",
            "--overwrite",
            "--config",
            config_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    let manifest_text = fs::read_to_string(td.path().join("out/run_manifest.json"))
        .expect("manifest must be written");
    assert!(
        manifest_text.contains("\"tool\": \"hayabusa-csv\""),
        "manifest missing hayabusa-csv entry: {manifest_text}"
    );
    assert!(
        !manifest_text.contains("relative --directory"),
        "hayabusa must have received an absolute --directory: {manifest_text}"
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
