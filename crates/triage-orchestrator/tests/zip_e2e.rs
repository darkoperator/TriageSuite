//! End-to-end coverage for ZIP capture input: a `.zip`, a folder of them, a
//! mixed folder, and the skip/exit-code contract. Drives the real binary, so
//! it also pins the console output an analyst relies on.
//!
//! Runs with `--no-validate`. These collections are the minimal synthetic
//! ones and the runs select every tool, so the placeholder hives and EVTX
//! that would satisfy the pre-flight gate
//! (`synthetic::write_gate_passing_collection`) would instead be parsed and
//! fail. The gate's own behaviour over a folder of ZIPs is covered in
//! `validate_e2e.rs`, which restricts the run to one tool for that reason.

use assert_cmd::Command;
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;
use triage_testkit::synthetic::{write_collection, COLLECTION_MARKER_FILE};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

/// Zip a collection with its contents at the archive root, which is how the
/// offline collector writes them.
fn zip_collection(zip_path: &Path, host: &str) {
    triage_testkit::synthetic::write_collection_zip(zip_path, "", host);
}

fn run(args: &[&str]) -> std::process::Output {
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args(args)
        .output()
        .unwrap()
}

fn manifest(out: &Path) -> serde_json::Value {
    let text = fs::read_to_string(out.join("run_manifest.json")).expect("manifest must exist");
    serde_json::from_str(&text).unwrap()
}

#[test]
fn a_single_zip_is_extracted_and_parsed() {
    let td = TempDir::new().unwrap();
    let z = td.path().join("Collection-H1.zip");
    zip_collection(&z, "H1");
    let out = td.path().join("out");

    let o = run(&[
        "run",
        "--no-validate",
        z.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--overwrite",
    ]);
    assert!(o.status.success(), "run failed: {o:?}");

    let m = manifest(&out);
    assert_eq!(m["hosts"].as_array().unwrap().len(), 1);
    assert_eq!(m["hosts"][0]["host"], "H1");
    assert_eq!(m["hosts"][0]["source_archive"], "Collection-H1.zip");
    assert_eq!(m["archives"][0]["status"], "extracted");

    // Extraction is kept, and the URL-encoded path survived byte-for-byte.
    // Discovery matches on filename only, so this must not be decoded.
    let extracted = out.join("_extracted/Collection-H1");
    assert!(extracted.is_dir(), "_extracted should be kept");
    assert!(extracted.join(COLLECTION_MARKER_FILE).is_file());
}

/// A real run over a `.zip` capture must produce both the manifest's
/// per-archive `sha256`/`sha256_skipped`/`size_bytes` fields and the actual
/// `CaseInfo/<stamp>_SHA256_HashLog.txt` file naming that archive --
/// `write_source_hash_log` having its own passing unit tests previously
/// wasn't enough to guarantee any real run called it.
#[test]
fn source_archive_is_hashed_in_the_manifest_and_the_hash_log() {
    const STAMP: &str = "2026-03-13T192553Z";
    let td = TempDir::new().unwrap();
    let z = td.path().join("Collection-H1.zip");
    zip_collection(&z, "H1");
    let out = td.path().join("out");

    let o = Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", STAMP)
        .args([
            "run",
            "--no-validate",
            z.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
            "--overwrite",
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "run failed: {o:?}");

    let m = manifest(&out);
    let archive = &m["archives"][0];
    assert!(
        archive["sha256"].is_string(),
        "expected a real digest: {archive}"
    );
    assert_eq!(archive["sha256_skipped"], false);
    assert!(
        archive["size_bytes"].is_u64(),
        "expected a real size: {archive}"
    );

    let hash_log = out
        .join(format!("Processed-H1-{STAMP}"))
        .join("CaseInfo")
        .join(format!("{STAMP}_SHA256_HashLog.txt"));
    let body = std::fs::read_to_string(&hash_log)
        .unwrap_or_else(|e| panic!("expected {hash_log:?} to exist: {e}"));
    assert!(body.contains("Collection-H1.zip"), "got {body}");
}

#[test]
fn a_folder_of_zips_runs_every_host() {
    let td = TempDir::new().unwrap();
    let zips = td.path().join("zips");
    fs::create_dir_all(&zips).unwrap();
    zip_collection(&zips.join("A.zip"), "HOSTA");
    zip_collection(&zips.join("B.zip"), "HOSTB");
    let out = td.path().join("out");

    let o = run(&[
        "run",
        "--no-validate",
        zips.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--overwrite",
    ]);
    assert!(o.status.success());

    let m = manifest(&out);
    let mut hosts: Vec<&str> = m["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["host"].as_str().unwrap())
        .collect();
    hosts.sort();
    assert_eq!(hosts, vec!["HOSTA", "HOSTB"]);
}

#[test]
fn a_folder_mixing_zips_and_unzipped_collections_runs_both() {
    let td = TempDir::new().unwrap();
    let dir = td.path().join("mixed");
    fs::create_dir_all(&dir).unwrap();
    zip_collection(&dir.join("Z.zip"), "ZIPPED");
    write_collection(&dir.join("Collection-PLAIN"), "PLAIN");
    let out = td.path().join("out");

    let o = run(&[
        "run",
        "--no-validate",
        dir.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--overwrite",
    ]);
    assert!(o.status.success());

    let m = manifest(&out);
    let mut hosts: Vec<&str> = m["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["host"].as_str().unwrap())
        .collect();
    hosts.sort();
    assert_eq!(hosts, vec!["PLAIN", "ZIPPED"]);
}

#[test]
fn an_unusable_archive_is_skipped_without_failing_the_run() {
    let td = TempDir::new().unwrap();
    let zips = td.path().join("zips");
    fs::create_dir_all(&zips).unwrap();
    zip_collection(&zips.join("good.zip"), "GOOD");
    fs::write(zips.join("garbage.zip"), b"definitely not a zip").unwrap();
    let out = td.path().join("out");

    let o = run(&[
        "run",
        "--no-validate",
        zips.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--overwrite",
    ]);
    // A bad archive alongside a good one must not fail the run.
    assert!(o.status.success(), "skips must not fail the run");
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(
        stderr.contains("garbage.zip"),
        "skip must be visible: {stderr}"
    );
    assert!(stderr.contains("skipped"));

    let m = manifest(&out);
    assert_eq!(m["hosts"].as_array().unwrap().len(), 1);
    let statuses: Vec<&str> = m["archives"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["status"].as_str().unwrap())
        .collect();
    assert!(statuses.contains(&"skipped"));
}

#[test]
fn nothing_usable_exits_three_instead_of_reporting_empty_success() {
    let td = TempDir::new().unwrap();
    let zips = td.path().join("zips");
    fs::create_dir_all(&zips).unwrap();
    fs::write(zips.join("a.zip"), b"not a zip").unwrap();
    fs::write(zips.join("b.zip"), b"also not a zip").unwrap();
    let out = td.path().join("out");

    let o = run(&[
        "run",
        "--no-validate",
        zips.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
    ]);
    assert_eq!(
        o.status.code(),
        Some(3),
        "an empty run must not look like success"
    );
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("no usable capture"), "{stderr}");
}

#[test]
fn a_second_run_reuses_the_existing_extraction() {
    let td = TempDir::new().unwrap();
    let z = td.path().join("C.zip");
    zip_collection(&z, "H1");
    let out = td.path().join("out");
    let args = [
        "run",
        "--no-validate",
        z.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
    ];

    let first = run(&args);
    assert!(first.status.success());
    assert!(String::from_utf8_lossy(&first.stderr).contains("extracted"));

    let second = run(&args);
    assert!(second.status.success());
    assert!(
        String::from_utf8_lossy(&second.stderr).contains("reusing existing extraction"),
        "second run should reuse: {}",
        String::from_utf8_lossy(&second.stderr)
    );
}

#[test]
fn a_plain_directory_behaves_exactly_as_before() {
    let td = TempDir::new().unwrap();
    let dir = td.path().join("plain");
    write_collection(&dir.join("Collection-P"), "P");
    let out = td.path().join("out");

    let o = run(&[
        "run",
        "--no-validate",
        dir.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--overwrite",
    ]);
    assert!(o.status.success());

    let m = manifest(&out);
    assert_eq!(m["hosts"][0]["host"], "P");
    // No archives were involved, so the array is omitted entirely and no
    // extraction directory is created.
    assert!(m.get("archives").is_none(), "archives[] should be omitted");
    assert!(m["hosts"][0].get("source_archive").is_none());
    assert!(!out.join("_extracted").exists());
}

#[test]
fn a_zip_entry_escaping_the_destination_is_rejected() {
    let td = TempDir::new().unwrap();
    let z = td.path().join("evil.zip");
    {
        let f = fs::File::create(&z).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        w.start_file("uploads.json", opts).unwrap();
        w.write_all(b"{}").unwrap();
        w.start_file("client_info.json", opts).unwrap();
        w.write_all(br#"{"Hostname":"H1"}"#).unwrap();
        w.start_file("../../escaped.txt", opts).unwrap();
        w.write_all(b"pwned").unwrap();
        w.finish().unwrap();
    }
    let out = td.path().join("out");
    let o = run(&[
        "run",
        "--no-validate",
        z.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--overwrite",
    ]);
    assert!(o.status.success());
    assert!(
        !td.path().join("escaped.txt").exists(),
        "zip-slip entry escaped the destination"
    );
}

/// An archive whose only entry is another `.zip`: the collection was zipped
/// twice. `write_collection_zip` is the inner archive, so the fixture is a
/// real collection that is simply packaged one layer too deep.
fn write_double_zipped(path: &Path, host: &str) {
    let td = TempDir::new().unwrap();
    let inner = td.path().join(format!("Collection-{host}.zip"));
    triage_testkit::synthetic::write_collection_zip(&inner, "", host);
    let f = fs::File::create(path).unwrap();
    let mut w = zip::ZipWriter::new(f);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    w.start_file(format!("Collection-{host}.zip"), opts)
        .unwrap();
    w.write_all(&fs::read(&inner).unwrap()).unwrap();
    w.finish().unwrap();
}

/// A rejection that never produced a host is still a run, and a run leaves a
/// chain-of-custody record: exit 3 alone tells an analyst nothing about
/// *which* input was refused or why.
#[test]
fn a_double_zipped_archive_still_writes_a_rejection_manifest() {
    let td = TempDir::new().unwrap();
    let z = td.path().join("double.zip");
    write_double_zipped(&z, "H1");
    let out = td.path().join("out");

    let o = run(&[
        "run",
        "--no-validate",
        z.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
    ]);
    assert_eq!(o.status.code(), Some(3), "a rejected input must exit 3");

    let m = manifest(&out);
    assert_eq!(m["final_exit_status"], 3);
    assert!(
        m["hosts"].as_array().unwrap().is_empty(),
        "nothing ran, so there is no host to report"
    );
    let archives = m["archives"].as_array().unwrap();
    assert_eq!(archives.len(), 1, "one input, one entry: {archives:?}");
    let entry = &archives[0];
    assert_eq!(entry["archive"], "double.zip");
    assert_eq!(entry["status"], "skipped");
    assert!(
        entry["error"]
            .as_str()
            .unwrap()
            .contains("double-zipped collection"),
        "the run path must name the packaging mistake: {entry:?}"
    );
    // The archive is still real evidence, so its size and digest are still
    // recorded even though nothing was processed out of it.
    assert!(entry["size_bytes"].as_u64().unwrap() > 0);
    assert!(entry["sha256"].as_str().is_some());
}

/// A folder whose archives are all unusable: every archive gets its own
/// entry with its own reason, and the folder the user actually pointed `run`
/// at gets one carrying the run-level reason -- which no per-archive entry
/// states.
#[test]
fn a_folder_of_only_malformed_archives_still_writes_a_rejection_manifest() {
    let td = TempDir::new().unwrap();
    let zips = td.path().join("zips");
    fs::create_dir_all(&zips).unwrap();
    fs::write(zips.join("a.zip"), b"not a zip").unwrap();
    fs::write(zips.join("b.zip"), b"also not a zip").unwrap();
    let out = td.path().join("out");

    let o = run(&[
        "run",
        "--no-validate",
        zips.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
    ]);
    assert_eq!(o.status.code(), Some(3));

    let m = manifest(&out);
    assert_eq!(m["final_exit_status"], 3);
    assert!(m["hosts"].as_array().unwrap().is_empty());
    let archives = m["archives"].as_array().unwrap();
    assert!(
        archives.iter().all(|a| a["status"] == "skipped"),
        "{archives:?}"
    );
    for name in ["a.zip", "b.zip"] {
        let entry = archives
            .iter()
            .find(|a| a["archive"] == name)
            .unwrap_or_else(|| panic!("{name} missing from {archives:?}"));
        assert!(
            entry["error"]
                .as_str()
                .unwrap()
                .contains("not a valid zip archive"),
            "{entry:?}"
        );
    }
    let folder = archives
        .iter()
        .find(|a| a["archive"] == "zips")
        .unwrap_or_else(|| panic!("the input folder itself is missing from {archives:?}"));
    assert!(
        folder["error"]
            .as_str()
            .unwrap()
            .contains("no usable capture"),
        "{folder:?}"
    );
    // A directory has no evidentiary size and no digest; `null` says so
    // rather than passing an inode size off as a file size.
    assert!(folder["size_bytes"].is_null(), "{folder:?}");
}

/// The sharp end of the finding: a rejected run over a reused `--out` must
/// replace the previous run's manifest, not leave it standing. A stale
/// success record describes a run that did not happen.
#[test]
fn a_rejected_run_does_not_leave_the_previous_runs_successful_manifest() {
    let td = TempDir::new().unwrap();
    let out = td.path().join("out");

    let good = td.path().join("Collection-H1.zip");
    zip_collection(&good, "H1");
    let o = run(&[
        "run",
        "--no-validate",
        good.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
    ]);
    assert!(o.status.success(), "setup run failed: {o:?}");
    let first = manifest(&out);
    assert_eq!(first["final_exit_status"], 0);
    assert_eq!(first["hosts"][0]["host"], "H1");

    let bad = td.path().join("double.zip");
    write_double_zipped(&bad, "H2");
    let o = run(&[
        "run",
        "--no-validate",
        bad.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
    ]);
    assert_eq!(o.status.code(), Some(3));

    let second = manifest(&out);
    assert_eq!(
        second["final_exit_status"], 3,
        "run_manifest.json still describes the previous successful run"
    );
    assert!(
        second["hosts"].as_array().unwrap().is_empty(),
        "the previous run's host survived into the rejected run's manifest"
    );
    assert_ne!(
        second["run_id"], first["run_id"],
        "run_manifest.json was not replaced at all"
    );
    // The previous run's own immutable copy is untouched: replacing the
    // rolling manifest must not rewrite history.
    let immutable = out.join(format!(
        "run_manifest_{}.json",
        first["run_id"].as_str().unwrap()
    ));
    let kept: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(immutable).unwrap()).unwrap();
    assert_eq!(kept["final_exit_status"], 0);
    assert_eq!(kept["hosts"][0]["host"], "H1");
}

/// An archive that extracts cleanly and *then* enumerates to nothing: the
/// input is recorded by folding the run-level reason into the entry the
/// extraction already earned, which keeps the archive's real `size_bytes`,
/// `sha256` and `extracted_to` rather than emitting a second, emptier record
/// of the same file.
///
/// Constructed by making the two collection markers *directory* entries.
/// `archive::holds_collection` matches marker names in the central
/// directory, while `capture::is_collection` requires `is_file()` — so the
/// archive probes as usable, extracts, and yields no collection. (A second
/// route exists on a case-sensitive filesystem: `holds_collection` compares
/// with `eq_ignore_ascii_case` and `is_collection` does not, so
/// `Uploads.json` diverges the same way. Not tested, because it depends on
/// the host filesystem.)
#[test]
fn an_archive_that_extracts_but_yields_no_collection_folds_into_its_own_entry() {
    let td = TempDir::new().unwrap();
    let z = td.path().join("dirmarkers.zip");
    {
        let f = fs::File::create(&z).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for marker in ["uploads.json", "client_info.json"] {
            w.add_directory(marker, opts).unwrap();
        }
        w.start_file("uploads/auto/note.txt", opts).unwrap();
        w.write_all(b"x").unwrap();
        w.finish().unwrap();
    }
    let out = td.path().join("out");

    let o = run(&[
        "run",
        "--no-validate",
        z.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
    ]);
    assert_eq!(o.status.code(), Some(3));

    let m = manifest(&out);
    assert_eq!(m["final_exit_status"], 3);
    assert!(m["hosts"].as_array().unwrap().is_empty());
    let archives = m["archives"].as_array().unwrap();
    assert_eq!(archives.len(), 1, "one input, one entry: {archives:?}");
    let entry = &archives[0];
    assert_eq!(entry["archive"], "dirmarkers.zip");
    assert_eq!(entry["status"], "skipped");
    assert!(
        entry["error"]
            .as_str()
            .unwrap()
            .contains("no usable capture found"),
        "the extraction has no reason of its own; the run-level one must land here: {entry:?}"
    );
    // The extraction's own facts survive the fold.
    assert_eq!(entry["extracted_to"], "_extracted/dirmarkers");
    assert_eq!(entry["files_written"], 1);
    assert!(entry["size_bytes"].as_u64().unwrap() > 0);
    assert!(entry["sha256"].as_str().is_some());
}

/// An extraction that *failed* already carries the reason it failed. The
/// rejection record must not replace it: "no usable capture found" is the
/// run-level verdict, while "Not a directory (os error 20)" is the cause, and
/// a chain-of-custody record that keeps only the verdict cannot be used to
/// explain what happened to the evidence.
///
/// A regular file sitting where `<out>/_extracted` must go is a cheap way to
/// make every extraction under it fail for a real filesystem reason.
#[test]
fn a_failed_extraction_keeps_its_own_error_in_the_rejection_manifest() {
    let td = TempDir::new().unwrap();
    let z = td.path().join("Collection-H1.zip");
    zip_collection(&z, "H1");
    let out = td.path().join("out");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("_extracted"), b"blocker").unwrap();

    let o = run(&[
        "run",
        "--no-validate",
        z.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
    ]);
    assert_eq!(o.status.code(), Some(3), "a rejected input must exit 3");
    let stderr = String::from_utf8_lossy(&o.stderr).into_owned();

    let m = manifest(&out);
    let archives = m["archives"].as_array().unwrap();
    assert_eq!(archives.len(), 1, "one input, one entry: {archives:?}");
    let entry = &archives[0];
    assert_eq!(
        entry["status"], "failed",
        "the extraction failed; that is what happened to this input: {entry:?}"
    );
    let error = entry["error"].as_str().unwrap();
    assert!(
        !error.contains("no usable capture"),
        "the generic run-level verdict replaced the extraction's own reason: {entry:?}"
    );
    // Tied to the console line rather than to a platform's errno wording:
    // whatever the OS called the failure, the manifest must say the same.
    assert!(
        stderr.contains(error),
        "the manifest error must be the diagnostic the run reported: {error:?} not in {stderr:?}"
    );
}

/// A refused input that is not a regular file must still reach manifest
/// publication. `sha256_of` opened whatever path it was handed, and opening a
/// FIFO with no writer blocks forever (a character device such as `/dev/zero`
/// reads forever instead) -- so the run never wrote `run_manifest.json`, and
/// over a reused `--out` the previous run's *successful* manifest stayed on
/// disk as a success record for a run that never happened.
///
/// The deadline is deliberate: with the defect present this test fails on the
/// timeout instead of wedging the suite.
#[cfg(unix)]
#[test]
fn a_rejected_fifo_input_still_writes_a_rejection_manifest() {
    let td = TempDir::new().unwrap();
    let fifo = td.path().join("pipe.zip");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo must be available on a unix host");
    assert!(made.success(), "mkfifo failed");
    let out = td.path().join("out");

    let o = run_with_deadline(
        &[
            "run",
            "--no-validate",
            fifo.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
        ],
        std::time::Duration::from_secs(30),
    );
    assert_eq!(o.status.code(), Some(3), "a rejected input must exit 3");

    let m = manifest(&out);
    assert_eq!(m["final_exit_status"], 3);
    let archives = m["archives"].as_array().unwrap();
    assert_eq!(archives.len(), 1, "one input, one entry: {archives:?}");
    let entry = &archives[0];
    assert_eq!(entry["archive_path"], fifo.display().to_string());
    // Neither weighable nor hashable, and both absences are explicit -- the
    // same answer `size_bytes` already gives for a directory-origin skip.
    assert!(entry["size_bytes"].is_null(), "{entry:?}");
    assert!(entry["sha256"].is_null(), "{entry:?}");
    assert_eq!(
        entry["sha256_skipped"], false,
        "--skip-hashes was not given; the digest is absent for another reason"
    );
}

/// Run the binary, killing it if it outlives `limit`. A test that hangs is
/// worse than no test: the defect this guards against is precisely a run that
/// never returns, so the failure has to be bounded and reported.
#[cfg(unix)]
fn run_with_deadline(args: &[&str], limit: std::time::Duration) -> std::process::Output {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_TriageSuite"))
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the run did not finish within {limit:?}: it hung before publishing a manifest");
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    child.wait_with_output().unwrap()
}
