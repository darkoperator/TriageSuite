//! Individual (per-source-log) CSV export tests.
//!
//! Gated on `test captures/`, same as `integration.rs`: self-skips (rather
//! than panicking) when the evidence tree isn't present, since these tests
//! need a real, reliably-populated `.evtx` (System/Security/Application) to
//! prove real content is written.
//!
//! `individual_filename` itself is evidence-free (pure string sanitizer) and
//! always runs.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use evtx_triage::individual_filename;

fn captures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// A reliably-populated `.evtx` under captures (the classic always-on logs), or
/// None if captures absent. Falls back to None rather than an empty log.
fn populated_evtx() -> Option<PathBuf> {
    let root = captures_root();
    if !root.exists() {
        return None;
    }
    let all = walk(&root);
    for name in ["System.evtx", "Security.evtx", "Application.evtx"] {
        if let Some(p) = all.iter().find(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("evtx"))
                .unwrap_or(false)
                && p.file_name().and_then(|n| n.to_str()) == Some(name)
        }) {
            return Some(p.clone());
        }
    }
    None
}

fn filenames(root: &Path) -> Vec<String> {
    walk(root)
        .into_iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .collect()
}

/// Standalone EvtxTriage (Flat layout, System identity) writes the aggregate
/// output as `<stamp>_EvtxTriage_Output.csv` — `default_basename` is
/// `EvtxTriage_Output` (lib.rs's DATASETS), and only the orchestrator's
/// Velo layout mode maps that to a `..._results...` name via `velo_basename`.
/// This test runs the binary directly, so `_Output` is the name that context
/// actually produces.
#[test]
fn individual_exports_write_one_file_per_source_log_with_content() {
    let Some(evtx) = populated_evtx() else {
        return;
    };
    let stem = evtx.file_stem().and_then(|s| s.to_str()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("EvtxTriage")
        .unwrap()
        .arg("-f")
        .arg(&evtx)
        .arg("--csv")
        .arg(tmp.path())
        .assert()
        .success();

    let names = filenames(tmp.path());
    // Flat layout folds the identity into every file it writes, including
    // dynamic side-cars (the tool is SystemWide, so identity is "system").
    let expected_individual = format!(
        "{}_system.csv",
        individual_filename(stem).trim_end_matches(".csv")
    );
    assert!(
        names.iter().any(|n| n == &expected_individual),
        "expected Individual/{expected_individual}; got {names:?}"
    );
    let individual_path = walk(tmp.path())
        .into_iter()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(expected_individual.as_str()))
        .expect("individual file located by name above must exist");
    assert!(
        individual_path.starts_with(tmp.path().join("Individual")),
        "expected the individual export under Individual/, got {individual_path:?}"
    );
    let content = std::fs::read_to_string(&individual_path).unwrap();
    assert!(
        content.lines().count() > 1,
        "expected header + at least one data row in the individual export"
    );

    // The consolidated file is still written, unaffected.
    assert!(
        names.iter().any(|n| n.ends_with("_EvtxTriage_Output.csv")),
        "expected the consolidated aggregate CSV alongside the individual exports; got {names:?}"
    );
}

#[test]
fn no_individual_suppresses_them() {
    let Some(evtx) = populated_evtx() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    Command::cargo_bin("EvtxTriage")
        .unwrap()
        .arg("-f")
        .arg(&evtx)
        .arg("--csv")
        .arg(tmp.path())
        .arg("--no-individual")
        .assert()
        .success();

    assert!(
        !tmp.path().join("Individual").exists(),
        "expected no Individual/ directory when --no-individual is passed"
    );
    let names = filenames(tmp.path());
    assert!(
        names.iter().any(|n| n.ends_with("_EvtxTriage_Output.csv")),
        "expected the consolidated aggregate CSV even with --no-individual; got {names:?}"
    );
}

/// A log name reaches this from the source filename (event Channel), so it
/// must not be able to steer where output is written.
///
/// Traced by hand against the sanitizer (map every non-alphanumeric,
/// non-`-` character to `_`, then trim leading/trailing `_`):
/// `../../etc/passwd` maps character-by-character to `______etc_passwd`
/// (six separators become six leading underscores, then `etc`, one more
/// separator, then `passwd`); trimming the leading run of underscores (there
/// is no trailing one) leaves `etc_passwd`.
#[test]
fn a_hostile_log_name_cannot_escape_the_individual_directory() {
    assert_eq!(individual_filename("../../etc/passwd"), "etc_passwd.csv");
    assert_eq!(individual_filename("Security"), "Security.csv");
    assert_eq!(individual_filename(""), "unknown.csv");
}
