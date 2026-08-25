use assert_cmd::Command;

fn cmd() -> Command {
    Command::cargo_bin("AnyDeskTriage").unwrap()
}

fn find_output(root: &std::path::Path, suffix: &str) -> Option<std::path::PathBuf> {
    for e in std::fs::read_dir(root).ok()?.flatten() {
        let p = e.path();
        if p.file_name()
            .is_some_and(|n| n.to_string_lossy().ends_with(suffix))
        {
            return Some(p);
        }
    }
    None
}

// Example lines as published across independent DFIR write-ups (see project
// memory for sources); not yet cross-checked against a self-generated sample.
const AD_TRACE_SAMPLE: &str = "info 2022-03-18 01:56:24.672 front 2428 7036 main - Process started at 2022-03-18. PID 2428. OS is Windows 10 (64 bit)\n\
info 2021-02-04 23:25:10.500 lsvc 9988 6992 3 anynet.relay_conn - External address: 116.255.x.x:47220.\n";

#[test]
fn discovers_ad_trace_under_a_capture_directory() {
    let tmp = tempfile::tempdir().unwrap();

    // Mimic a Velociraptor capture: the trace file nested a few levels deep,
    // alongside unrelated files that must not be picked up.
    let capture = tmp
        .path()
        .join("uploads/auto/C%3A/Users/alice/AppData/Roaming/AnyDesk");
    std::fs::create_dir_all(&capture).unwrap();
    std::fs::write(capture.join("ad.trace"), AD_TRACE_SAMPLE).unwrap();
    std::fs::write(capture.join("unrelated.txt"), "not a trace file\n").unwrap();

    let out = tmp.path().join("out");
    cmd()
        .arg("-d")
        .arg(tmp.path())
        .arg("--csv")
        .arg(&out)
        .arg("-q")
        .assert()
        .code(0);

    let output = find_output(&out, "AnyDeskTriage_Output.csv").expect("AnyDeskTriage output");
    let content = std::fs::read_to_string(output).unwrap();
    assert!(content.contains("Process started"), "{content}");
    assert!(content.contains("anynet.relay_conn"), "{content}");
    assert!(content.contains("External address"), "{content}");
    assert!(!content.contains("not a trace file"), "{content}");
}

#[test]
fn discovers_connection_trace_alongside_ad_trace() {
    let tmp = tempfile::tempdir().unwrap();
    let capture = tmp.path().join("uploads/auto/C%3A/ProgramData/AnyDesk");
    std::fs::create_dir_all(&capture).unwrap();
    std::fs::write(capture.join("ad_svc.trace"), AD_TRACE_SAMPLE).unwrap();
    std::fs::write(
        capture.join("connection_trace.txt"),
        "Incoming 2022-03-18, 02:50 User 732092099 732092099\n",
    )
    .unwrap();

    let out = tmp.path().join("out");
    cmd()
        .arg("-d")
        .arg(tmp.path())
        .arg("--csv")
        .arg(&out)
        .arg("-q")
        .assert()
        .code(0);

    let output = find_output(&out, "AnyDeskTriage_Output.csv").expect("AnyDeskTriage output");
    let content = std::fs::read_to_string(output).unwrap();
    assert!(content.contains("ConnectionEvent"), "{content}");
    assert!(content.contains("Incoming"), "{content}");
    assert!(content.contains("732092099"), "{content}");
    assert!(content.contains("TraceLine"), "{content}");
}

#[test]
fn explicit_file_argument_is_accepted_without_directory_search() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("ad_svc.trace");
    std::fs::write(&input, AD_TRACE_SAMPLE).unwrap();

    let out = tmp.path().join("out");
    cmd()
        .arg("-f")
        .arg(&input)
        .arg("--csv")
        .arg(&out)
        .arg("-q")
        .assert()
        .code(0);

    let output = find_output(&out, "AnyDeskTriage_Output.csv").expect("AnyDeskTriage output");
    let content = std::fs::read_to_string(output).unwrap();
    assert!(content.contains("Process started"), "{content}");
}

#[test]
fn unrelated_files_under_the_capture_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("notes.txt"), "some notes\n").unwrap();

    let out = tmp.path().join("out");
    cmd()
        .arg("-d")
        .arg(tmp.path())
        .arg("--csv")
        .arg(&out)
        .arg("-q")
        .assert()
        .code(0);

    // No ad.trace-shaped file discovered -> no output directory/dataset file.
    assert!(find_output(&out, "AnyDeskTriage_Output.csv").is_none());
}
