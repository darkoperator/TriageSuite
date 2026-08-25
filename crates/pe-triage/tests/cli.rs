use assert_cmd::Command;
use predicates::prelude::*;
use std::path::{Path, PathBuf};

fn cmd() -> Command {
    Command::cargo_bin("PETriage").unwrap()
}

/// FLAT layout: output lands directly under the run root with the identity
/// encoded in the filename (`<identity>_<yyyyMMddHHmmss>_..._Output.csv`).
/// Walk `dir` recursively and return the first file whose name ends with
/// `suffix`. (Recursive so it also works under `--nested-output`.)
fn find_ending(dir: &Path, suffix: &str) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(suffix))
            {
                return Some(p);
            }
        }
    }
    None
}

fn captures_root() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
    p.exists().then_some(p)
}

/// Spec 10.1: capture containing "DC1" (case-insensitive) is the server.
fn capture_dirs() -> Option<(PathBuf, Vec<PathBuf>)> {
    let root = captures_root()?;
    let mut server = None;
    let mut clients = vec![];
    for e in std::fs::read_dir(&root).ok()?.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        // Only real Velociraptor capture dirs (`Collection-*`); ignore any other
        // content placed under `test captures/` (e.g. user output folders).
        if !p
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("Collection"))
            .unwrap_or(false)
        {
            continue;
        }
        if p.file_name()
            .unwrap()
            .to_string_lossy()
            .to_uppercase()
            .contains("DC1")
        {
            server = Some(p);
        } else {
            clients.push(p);
        }
    }
    Some((server?, clients))
}

#[test]
fn three_capture_integration_matrix() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
    if triage_testkit::skip_if_missing(&root, "test captures") {
        return;
    }
    let (server, clients) = capture_dirs().expect("captures present but unreadable");
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .arg("-d")
        .arg(&server)
        .arg("--csv")
        .arg(tmp.path().join("srv"))
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("no supported artifacts found"));
    assert_eq!(clients.len(), 2, "expected exactly two client captures");
    for (i, c) in clients.iter().enumerate() {
        let out = tmp.path().join(format!("c{i}"));
        cmd()
            .arg("-d")
            .arg(c)
            .arg("--csv")
            .arg(&out)
            .arg("--json")
            .arg(&out)
            .arg("-q")
            .assert()
            .code(0)
            .stdout(predicate::str::is_empty());
        let csv = find_ending(&out, "PETriage_Output.csv");
        let tl = find_ending(&out, "PETriage_Output_Timeline.csv");
        let json = find_ending(&out, "PETriage_Output.json");
        assert!(csv.is_some() && tl.is_some() && json.is_some());
        assert!(find_ending(&out, "PETriage_Output_Timeline.json").is_none());
        let csv = csv.unwrap();
        let lines = std::fs::read_to_string(&csv).unwrap().lines().count();
        assert!(lines > 200, "expected 200+ rows, got {lines}");
    }
}

#[test]
fn explicit_file_mode_single_pf() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
    if triage_testkit::skip_if_missing(&root, "test captures") {
        return;
    }
    let (_, clients) = capture_dirs().expect("captures present but unreadable");
    let pf = first_pf(&clients[0]).expect("client capture has pf files");
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .arg("-f")
        .arg(&pf)
        .arg("--csv")
        .arg(tmp.path())
        .assert()
        .code(0)
        .stderr(predicate::str::contains("Records emitted:"));
}

#[test]
fn dedupe_skips_identical_content() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
    if triage_testkit::skip_if_missing(&root, "test captures") {
        return;
    }
    let (_, clients) = capture_dirs().expect("captures present but unreadable");
    let pf = first_pf(&clients[0]).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let cap = tmp.path().join("cap");
    std::fs::create_dir_all(&cap).unwrap();
    std::fs::copy(&pf, cap.join("A.pf")).unwrap();
    std::fs::copy(&pf, cap.join("B.pf")).unwrap();
    cmd()
        .arg("-d")
        .arg(&cap)
        .arg("--csv")
        .arg(tmp.path().join("out"))
        .arg("--dedupe")
        .arg("true")
        .assert()
        .code(0)
        .stderr(predicate::str::contains("Deduplicated: 1"))
        .stderr(predicate::str::contains("Parsed: 1"));
}

#[test]
fn csvf_override_names_both_datasets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
    if triage_testkit::skip_if_missing(&root, "test captures") {
        return;
    }
    let (_, clients) = capture_dirs().expect("captures present but unreadable");
    let pf = first_pf(&clients[0]).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .arg("-f")
        .arg(&pf)
        .arg("--csv")
        .arg(tmp.path())
        .arg("--csvf")
        .arg("run1.csv")
        .assert()
        .code(0);
    // FLAT layout inserts the identity before the extension of custom
    // (unstamped) names: `run1.csv` -> `run1_system.csv`, and the suffixed
    // timeline dataset `run1_Timeline.csv` -> `run1_Timeline_system.csv`.
    assert!(find_ending(tmp.path(), "run1_system.csv").is_some());
    assert!(find_ending(tmp.path(), "run1_Timeline_system.csv").is_some());
}

#[test]
fn nested_output_flag_restores_legacy_layout() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
    if triage_testkit::skip_if_missing(&root, "test captures") {
        return;
    }
    let (_, clients) = capture_dirs().expect("captures present but unreadable");
    let pf = first_pf(&clients[0]).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .arg("-f")
        .arg(&pf)
        .arg("--csv")
        .arg(tmp.path())
        .arg("--nested-output")
        .assert()
        .code(0);
    // --nested-output restores the legacy `<root>/<Tool>/system/...` tree; a
    // system-scoped prefetch file must land under PETriage/system/.
    let sys = tmp.path().join("PETriage/system");
    assert!(
        sys.is_dir(),
        "expected legacy nested dir {} to exist",
        sys.display()
    );
    assert!(
        find_ending(&sys, "PETriage_Output.csv").is_some(),
        "expected a PETriage_Output.csv under the legacy nested PETriage/system/ dir"
    );
}

fn first_pf(root: &Path) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pf")) {
                return Some(p);
            }
        }
    }
    None
}
