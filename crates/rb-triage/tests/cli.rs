use assert_cmd::Command;
use predicates::prelude::*;
use std::path::{Path, PathBuf};

fn cmd() -> Command {
    Command::cargo_bin("RBTriage").unwrap()
}

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

/// Spec 10.1: capture containing "DC1" is the server; others are clients.
fn capture_dirs() -> (PathBuf, Vec<PathBuf>) {
    let mut server = None;
    let mut clients = vec![];
    for e in std::fs::read_dir(captures_root()).unwrap().flatten() {
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
    clients.sort();
    (server.expect("DC1 server capture"), clients)
}

#[test]
fn three_capture_integration_matrix() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let (server, clients) = capture_dirs();
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

    assert_eq!(clients.len(), 2);
    let mut total_with_output = 0;
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
        if let Some(csv) = find_one(&out, "RBTriage_Output.csv") {
            let rows = std::fs::read_to_string(&csv).unwrap().lines().count();
            assert!(rows >= 2, "expected header+rows, got {rows}");
            total_with_output += 1;
        }
    }
    assert_eq!(
        total_with_output, 1,
        "exactly one client should yield recycle-bin output"
    );
}

#[test]
fn explicit_file_mode_single_dollar_i() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let (_, clients) = capture_dirs();
    let i = clients
        .iter()
        .find_map(|c| first_dollar_i(c))
        .expect("an $I file");
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .arg("-f")
        .arg(&i)
        .arg("--csv")
        .arg(tmp.path())
        .arg("--json")
        .arg(tmp.path())
        .assert()
        .code(0)
        .stderr(predicate::str::contains("Records emitted: 1"));
}

#[test]
fn output_lands_under_sid_identity() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let (_, clients) = capture_dirs();
    let stcl1 = clients
        .iter()
        .find(|c| first_dollar_i(c).is_some())
        .expect("STCL1");
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .arg("-d")
        .arg(stcl1)
        .arg("--csv")
        .arg(tmp.path())
        .arg("-q")
        .assert()
        .code(0);
    // FLAT layout: the SID identity is encoded as the output filename prefix
    // (`S-1-5-21-..._<stamp>_RBTriage_Output.csv`), not a users/<SID>/ directory.
    let names = output_names(tmp.path());
    assert!(
        names.iter().any(|n| n.starts_with("S-1-5-21-")),
        "expected an output file prefixed with the SID identity, got {names:?}"
    );
}

/// Names of every `RBTriage_Output.csv` anywhere under `root`.
fn output_names(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Some(n) = p.file_name().and_then(|f| f.to_str()) {
                if n.ends_with("RBTriage_Output.csv") {
                    out.push(n.to_string());
                }
            }
        }
    }
    out
}

/// REVIEW TEST: -f explicit-file mode must attribute capture-relative.
/// The capture lives under /Users/<analyst>/... on the host, so the absolute
/// $I path contains a host "Users/<analyst>" segment BEFORE the in-capture
/// "$Recycle.Bin/<SID>". Output must land under users/S-1-5-21-.../, never
/// under users/<analyst>/.
#[test]
fn explicit_file_mode_attributes_to_sid_not_host() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let (_, clients) = capture_dirs();
    let i = clients
        .iter()
        .find_map(|c| first_dollar_i(c))
        .expect("an $I file");
    // Sanity: the host path really does contain a "Users/<name>" segment that
    // would mis-attribute if attribution were not capture-relative.
    assert!(
        i.components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case("Users")),
        "test precondition: capture must be stored under a host Users/ dir; got {}",
        i.display()
    );
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .arg("-f")
        .arg(&i)
        .arg("--csv")
        .arg(tmp.path())
        .assert()
        .code(0)
        .stderr(predicate::str::contains("Records emitted: 1"));

    // FLAT layout: identity is the output filename prefix. The file must be
    // prefixed with the in-capture SID, never with the host account name.
    let names = output_names(tmp.path());
    assert!(
        names.iter().any(|n| n.starts_with("S-1-5-21-")),
        "expected output prefixed with S-1-5-21-..., got: {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.to_ascii_lowercase().starts_with("carlosperez_")),
        "MIS-ATTRIBUTION: output prefixed with the host account, got: {names:?}"
    );
}

fn find_one(root: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .is_some_and(|f| f.to_string_lossy().ends_with(name))
            {
                return Some(p);
            }
        }
    }
    None
}

fn first_dollar_i(root: &Path) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .is_some_and(|f| f.to_string_lossy().starts_with("$I"))
            {
                return Some(p);
            }
        }
    }
    None
}
