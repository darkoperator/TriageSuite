//! End-to-end coverage for ZIP capture input: a `.zip`, a folder of them, a
//! mixed folder, and the skip/exit-code contract. Drives the real binary, so
//! it also pins the console output an analyst relies on.

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
