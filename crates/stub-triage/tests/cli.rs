use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

fn cmd() -> Command {
    Command::cargo_bin("StubTriage").unwrap()
}

/// Locate a flat-layout output file under `root` (recursively): match a file
/// whose name carries the `<identity>_` prefix and ends with `suffix`.
/// Panics with a clear message if no such entry exists.
fn find_output(root: &Path, identity: &str, suffix: &str) -> PathBuf {
    let prefix = format!("{identity}_");
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.starts_with(&prefix) && name.ends_with(suffix) {
                    return p;
                }
            }
        }
    }
    panic!(
        "no flat output file '{prefix}*{suffix}' found under {}",
        root.display()
    );
}

fn write(p: &Path, content: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

#[test]
fn no_args_is_usage_error_exit_2() {
    cmd().assert().code(2);
}

#[test]
fn missing_input_directory_exit_3() {
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .args(["-d", "/nonexistent/capture", "--csv"])
        .arg(tmp.path().join("out"))
        .assert()
        .code(3);
}

#[test]
fn missing_output_flag_exit_2() {
    let tmp = tempfile::tempdir().unwrap();
    cmd().arg("-d").arg(tmp.path()).assert().code(2);
}

#[test]
fn empty_capture_is_success_with_warning_and_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    cmd()
        .arg("-d")
        .arg(tmp.path())
        .arg("--csv")
        .arg(&out)
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("no supported artifacts found"))
        .stderr(predicate::str::contains("Discovered: 0"));
}

#[test]
fn capture_mode_attributes_users_and_writes_both_formats() {
    let tmp = tempfile::tempdir().unwrap();
    let cap = tmp.path().join("cap");
    write(
        &cap.join("C/Users/alice/Recent/a.stub"),
        "STUB\nk1=v1\nk2=v2\n",
    );
    write(&cap.join("C/Windows/system.stub"), "STUB\nk3=v3\n");
    let out = tmp.path().join("out");

    cmd()
        .arg("-d")
        .arg(&cap)
        .arg("--csv")
        .arg(&out)
        .arg("--json")
        .arg(&out)
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("Records emitted: 3"));

    let alice_csv = find_output(&out, "alice", "StubTriage_Output.csv");
    let unknown_json = find_output(&out, "unknown", "StubTriage_Output.json");
    assert_eq!(
        fs::read_to_string(&alice_csv)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
        "Name,Value,SourceFile"
    );
    let json_line = fs::read_to_string(&unknown_json).unwrap();
    assert!(json_line.starts_with("{\"Name\":\"k3\""), "got {json_line}");
}

#[test]
fn explicit_file_mode_works() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("one.stub");
    write(&f, "STUB\nk=v\n");
    let out = tmp.path().join("out");
    cmd()
        .arg("-f")
        .arg(&f)
        .arg("--json")
        .arg(&out)
        .assert()
        .code(0)
        .stderr(predicate::str::contains("Records emitted: 1"));
}

#[test]
fn corrupt_plus_valid_is_partial_exit_5() {
    let tmp = tempfile::tempdir().unwrap();
    let cap = tmp.path().join("cap");
    write(&cap.join("good.stub"), "STUB\nk=v\n");
    write(&cap.join("bad.stub"), "STUBxx"); // passes magic check, fails parse
    let out = tmp.path().join("out");
    cmd()
        .arg("-d")
        .arg(&cap)
        .arg("--csv")
        .arg(&out)
        .assert()
        .code(5)
        .stderr(predicate::str::contains("Failed: 1"))
        .stderr(predicate::str::contains("Parsed: 1"));
}

#[test]
fn corrupt_matching_content_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let cap = tmp.path().join("cap");
    write(&cap.join("not_a_stub.stub"), "GARBAGE");
    let out = tmp.path().join("out");
    cmd()
        .arg("-d")
        .arg(&cap)
        .arg("--csv")
        .arg(&out)
        .assert()
        .code(6)
        .stderr(predicate::str::contains("Corrupt: 1"));
}

#[test]
fn output_collision_without_overwrite_fails_with_overwrite_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("one.stub");
    write(&f, "STUB\nk=v\n");
    let out = tmp.path().join("out");

    cmd()
        .arg("-f")
        .arg(&f)
        .arg("--csv")
        .arg(&out)
        .assert()
        .code(0);
    cmd()
        .arg("-f")
        .arg(&f)
        .arg("--csv")
        .arg(&out)
        .assert()
        .code(4);
    cmd()
        .arg("-f")
        .arg(&f)
        .arg("--csv")
        .arg(&out)
        .arg("--overwrite")
        .assert()
        .code(0);
}

#[test]
fn version_output_includes_author() {
    cmd()
        .arg("--version")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("Carlos (DarkOperator) Perez"));
}

#[test]
fn help_output_includes_author_banner() {
    cmd()
        .arg("--help")
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "Author: Carlos (DarkOperator) Perez",
        ));
}

#[test]
fn per_file_messages_shown_by_default_suppressed_by_quiet() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("one.stub");
    write(&f, "STUB\nk=v\n");

    let out1 = tmp.path().join("out1");
    cmd()
        .arg("-f")
        .arg(&f)
        .arg("--csv")
        .arg(&out1)
        .assert()
        .code(0)
        .stderr(predicate::str::contains("Processing: one.stub"));

    let out2 = tmp.path().join("out2");
    cmd()
        .arg("-f")
        .arg(&f)
        .arg("--csv")
        .arg(&out2)
        .arg("-q")
        .assert()
        .code(0)
        .stderr(predicate::str::contains("Processing: one.stub").not())
        .stderr(predicate::str::contains("--- Summary ---")); // summary survives -q
}

#[test]
fn pretty_json_is_equivalent_data() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("one.stub");
    write(&f, "STUB\nk=v\n");
    let out = tmp.path().join("out");
    cmd()
        .arg("-f")
        .arg(&f)
        .arg("--json")
        .arg(&out)
        .arg("--pretty")
        .assert()
        .code(0);
    let text = fs::read_to_string(find_output(&out, "unknown", "StubTriage_Output.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["Name"], "k");
}
