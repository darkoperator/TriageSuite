use assert_cmd::Command;
use serde_json::Value;
use std::fs;

#[test]
fn corrupt_only_run_exits_six_and_audits_failure() {
    let temp = tempfile::tempdir().unwrap();
    let capture = temp.path().join("capture");
    fs::create_dir_all(&capture).unwrap();
    fs::write(capture.join("broken.pf"), b"not a prefetch file").unwrap();
    let out = temp.path().join("out");

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args([
            "run",
            capture.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--only",
            "pe",
        ])
        .assert()
        .code(6);

    let manifest: Value =
        serde_json::from_slice(&fs::read(out.join("run_manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["final_exit_status"], 6);
    assert_eq!(manifest["hosts"][0]["tools"][0]["corrupt"], 1);
    assert_eq!(manifest["hosts"][0]["tools"][0]["parsed"], 0);
}

#[test]
fn manifest_preserves_host_and_uses_safe_output_id() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("Collection-hostile");
    fs::create_dir_all(collection.join("uploads")).unwrap();
    fs::write(collection.join("uploads.json"), "{}").unwrap();
    fs::write(
        collection.join("client_info.json"),
        r#"{"Hostname":"../CON","Platform":"Windows"}"#,
    )
    .unwrap();
    let out = temp.path().join("out");

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args([
            "run",
            collection.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--only",
            "pe",
        ])
        .assert()
        .success();

    let manifest: Value =
        serde_json::from_slice(&fs::read(out.join("run_manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["hosts"][0]["host"], "../CON");
    let output_id = manifest["hosts"][0]["output_id"].as_str().unwrap();
    assert!(!output_id.contains(['/', '\\']));
    let run_id = manifest["run_id"].as_str().unwrap();
    assert!(out.join(format!("run_manifest_{run_id}.json")).is_file());
}
