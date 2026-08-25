//! JLETriage integration matrix + functional CLI tests.
//!
//! Modeled on le-triage/tests/cli.rs (loud `triage_testkit::skip_if_missing`).
//! All three captures contain BOTH AutomaticDestinations and CustomDestinations
//! jump lists, so every capture must yield both datasets.

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::Write;
use std::path::{Path, PathBuf};

fn cmd() -> Command {
    Command::cargo_bin("JLETriage").unwrap()
}

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

/// All capture directories under `test captures/`.
fn capture_dirs() -> Vec<PathBuf> {
    let mut caps: Vec<PathBuf> = std::fs::read_dir(captures_root())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        // Only real Velociraptor capture dirs (`Collection-*`); ignore any other
        // content placed under `test captures/` (e.g. user output folders).
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("Collection"))
                .unwrap_or(false)
        })
        .collect();
    caps.sort();
    caps
}

/// Count data rows (lines minus header) of a CSV, or 0 when absent.
fn csv_data_rows(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|s| s.lines().count().saturating_sub(1))
        .unwrap_or(0)
}

/// Every file whose name ends with `name` anywhere under `root` (a timestamp
/// prefix is prepended to output basenames, so match by suffix not equality).
fn find_all(root: &Path, name: &str) -> Vec<PathBuf> {
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
            } else if p
                .file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|f| f.ends_with(name))
            {
                out.push(p);
            }
        }
    }
    out
}

/// Aggregate data-row count of every output CSV named `name` anywhere under the
/// run root (flat layout: per-identity outputs are filename-prefixed).
fn total_rows(out_root: &Path, name: &str) -> usize {
    // FLAT layout: per-identity outputs land directly under the run root.
    find_all(out_root, name)
        .iter()
        .map(|p| csv_data_rows(p))
        .sum()
}

/// First file under `root` whose name ends with `ext` (case-sensitive suffix).
fn first_with_ext(root: &Path, ext: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(ext))
            {
                return Some(p);
            }
        }
    }
    None
}

/// First jump-list file with the given extension that, run in explicit-file
/// mode, actually emits at least one record (some real jump lists are empty
/// stubs with a header but zero entries).
fn first_nonempty(ext: &str) -> Option<PathBuf> {
    for cap in capture_dirs() {
        let mut stack = vec![cap];
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
                    .is_some_and(|n| n.ends_with(ext))
                {
                    let tmp = tempfile::tempdir().unwrap();
                    let ok = cmd()
                        .arg("-f")
                        .arg(&p)
                        .arg("--csv")
                        .arg(tmp.path())
                        .assert();
                    let out = ok.get_output();
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    if stderr
                        .lines()
                        .find_map(|l| l.strip_prefix("Records emitted: "))
                        .and_then(|n| n.trim().parse::<u64>().ok())
                        .is_some_and(|n| n > 0)
                    {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}

#[test]
fn three_capture_integration_matrix() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let caps = capture_dirs();
    assert_eq!(
        caps.len(),
        3,
        "expected exactly three captures, got {}",
        caps.len()
    );

    let tmp = tempfile::tempdir().unwrap();
    for (i, c) in caps.iter().enumerate() {
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

        // Every capture yields BOTH datasets with > 0 rows.
        let auto = total_rows(&out, "JLETriage_AutomaticDestinations_Output.csv");
        let custom = total_rows(&out, "JLETriage_CustomDestinations_Output.csv");
        assert!(
            auto > 0,
            "capture {} produced 0 AutomaticDestinations rows",
            c.display()
        );
        assert!(
            custom > 0,
            "capture {} produced 0 CustomDestinations rows",
            c.display()
        );
    }
}

#[test]
fn explicit_file_mode_single_automatic() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let auto = first_nonempty(".automaticDestinations-ms")
        .expect("a non-empty .automaticDestinations-ms in the captures");
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .arg("-f")
        .arg(&auto)
        .arg("--csv")
        .arg(tmp.path())
        .arg("--json")
        .arg(tmp.path())
        .assert()
        .code(0)
        .stderr(
            predicate::str::contains("Records emitted:")
                .and(predicate::str::contains("Records emitted: 0").not()),
        );
}

#[test]
fn explicit_file_mode_single_custom() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let custom = first_with_ext(&captures_root(), ".customDestinations-ms")
        .expect("a .customDestinations-ms in the captures");
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .arg("-f")
        .arg(&custom)
        .arg("--csv")
        .arg(tmp.path())
        .arg("--json")
        .arg(tmp.path())
        .assert()
        .code(0);
}

#[test]
fn codepage_932_parses() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let auto = first_with_ext(&captures_root(), ".automaticDestinations-ms")
        .expect("a .automaticDestinations-ms in the captures");
    let tmp = tempfile::tempdir().unwrap();
    // Shift-JIS (cp932) decode path must not panic on a Latin jump list.
    cmd()
        .arg("--cp")
        .arg("932")
        .arg("-f")
        .arg(&auto)
        .arg("--csv")
        .arg(tmp.path())
        .assert()
        .code(0);
}

#[test]
fn with_dir_does_not_reduce_auto_rows() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let cap = capture_dirs().into_iter().next().expect("a capture");
    let tmp = tempfile::tempdir().unwrap();

    let base = tmp.path().join("base");
    cmd()
        .arg("-d")
        .arg(&cap)
        .arg("--csv")
        .arg(&base)
        .arg("-q")
        .assert()
        .code(0);
    let base_rows = total_rows(&base, "JLETriage_AutomaticDestinations_Output.csv");

    // --withDir surfaces numbered streams not referenced by the DestList, so it
    // can only ADD orphan rows — never drop DestList-driven ones.
    let wd = tmp.path().join("wd");
    cmd()
        .arg("-d")
        .arg(&cap)
        .arg("--withDir")
        .arg("--csv")
        .arg(&wd)
        .arg("-q")
        .assert()
        .code(0);
    let wd_rows = total_rows(&wd, "JLETriage_AutomaticDestinations_Output.csv");

    assert!(
        wd_rows >= base_rows,
        "--withDir produced {wd_rows} auto rows, fewer than the base {base_rows}"
    );
}

#[test]
fn appids_override_reflected() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    // A user AppIDs override file ("appid"|"description" per line). We cannot
    // assert a specific description surfaces unless a capture happens to carry
    // that appid, so the contract here is simply that --appIds is accepted and
    // the run still succeeds.
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(f, "deadbeefdeadbeef|My Test App").unwrap();
    let cap = capture_dirs().into_iter().next().expect("a capture");
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .arg("-d")
        .arg(&cap)
        .arg("--appIds")
        .arg(f.path())
        .arg("--csv")
        .arg(tmp.path())
        .arg("-q")
        .assert()
        .code(0);
}

#[test]
fn output_routing_user() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    // A jump list under Users/<u>/ must route into JLETriage/users/<u>/.
    let cap = capture_dirs()
        .into_iter()
        .find(|c| jumplist_under_users(c))
        .expect("a capture with a Users/<u>/ jump list");
    let tmp = tempfile::tempdir().unwrap();
    cmd()
        .arg("-d")
        .arg(&cap)
        .arg("--csv")
        .arg(tmp.path())
        .arg("-q")
        .assert()
        .code(0);

    // FLAT layout: a jump list under Users/<u>/ yields an output file whose
    // name carries the `<u>_` identity prefix (no users/<u>/ directory).
    let outputs = find_all(tmp.path(), "Destinations_Output.csv");
    let prefixes: Vec<String> = outputs
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .map(|n| n.split('_').next().unwrap_or("").to_string())
        .collect();
    assert!(
        prefixes
            .iter()
            .any(|p| p != "system" && p != "unknown" && !p.is_empty()),
        "expected at least one user-identity-prefixed JLETriage output, got {prefixes:?}"
    );
}

/// True when the capture contains a jump-list file beneath a `Users` segment.
fn jumplist_under_users(root: &Path) -> bool {
    let mut stack = vec![root.to_path_buf()];
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
                .is_some_and(|n| n.ends_with("Destinations-ms"))
                && p.components().any(|c| {
                    c.as_os_str()
                        .to_string_lossy()
                        .eq_ignore_ascii_case("Users")
                })
            {
                return true;
            }
        }
    }
    false
}
