//! Proves `write_sysinfo` is actually wired into a real run, not just
//! exercised directly by its own unit tests -- the same "zero production
//! call sites" trap an earlier task in this plan shipped.
//!
//! Runs the real `TriageSuite` binary over real SYSTEM/SOFTWARE hives from
//! the evidence tree (`RETriage` parses them for real; nothing here is a
//! synthetic fixture) and asserts the resulting `CaseInfo/<stamp>_SysInfo.txt`
//! carries real values read back out of RETriage's own CSV.
//!
//! Gated on `test captures/` via `triage_testkit::skip_if_missing`, same as
//! every other capture-backed test in this workspace: `TRIAGE_ALLOW_COMPAT_SKIP=1`
//! lets it skip when the (gitignored) evidence tree is absent, and its
//! absence otherwise panics rather than silently passing.
//!
//! Runs with `--no-validate`: the fixture is deliberately two hives and
//! nothing else -- no event logs, no SAM/SECURITY -- which the pre-flight
//! gate rejects, and completing it would mean parsing a whole collection
//! instead of two files.

use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

const PINNED_STAMP: &str = "2026-03-13T192553Z";

fn captures_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

/// Find the first `SYSTEM`/`SOFTWARE` pair under a Velociraptor
/// `.../config/` directory in the captures tree, skipping `RegBack` copies so
/// the pairing is unambiguous.
fn find_system_software_pair() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let mut stack = vec![captures_root()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut system = None;
        let mut software = None;
        let mut subdirs = Vec::new();
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "RegBack") {
                    continue;
                }
                subdirs.push(path);
            } else if path.file_name().is_some_and(|n| n == "SYSTEM") {
                system = Some(path);
            } else if path.file_name().is_some_and(|n| n == "SOFTWARE") {
                software = Some(path);
            }
        }
        if let (Some(s), Some(w)) = (system, software) {
            return Some((s, w));
        }
        stack.extend(subdirs);
    }
    None
}

/// A real run over real hives produces `CaseInfo/<stamp>_SysInfo.txt` with
/// real content -- not the function existing with no caller, and not a
/// hand-written fixture standing in for RETriage's actual output shape.
#[test]
fn a_real_run_writes_a_sysinfo_report_with_real_content() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let Some((system_hive, software_hive)) = find_system_software_pair() else {
        eprintln!("SKIP: no SYSTEM/SOFTWARE pair found in captures");
        return;
    };

    // A raw directory holding just the two hives, not the full collection:
    // `TriageSuite run` treats an unrecognized directory as a single raw
    // capture (`capture::enumerate_multi`'s `raw_fallback`), so RETriage
    // discovers exactly these two files instead of every hive in a full
    // Velociraptor collection -- the difference between a ~20s test and a
    // multi-minute one on these multi-hundred-MB hives.
    let raw = TempDir::new().unwrap();
    std::fs::copy(&system_hive, raw.path().join("SYSTEM")).unwrap();
    std::fs::copy(&software_hive, raw.path().join("SOFTWARE")).unwrap();

    let out = TempDir::new().unwrap();
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args([
            "run",
            "--no-validate",
            raw.path().to_str().unwrap(),
            "--out",
            out.path().to_str().unwrap(),
            "--only",
            "re",
            "--overwrite",
        ])
        .assert()
        .success();

    let host_name = raw
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let collection = out
        .path()
        .join(format!("Processed-{host_name}-{PINNED_STAMP}"));
    let registry_csv = collection.join(format!(
        "Registry/{PINNED_STAMP}_RETriage_results_Batch.csv"
    ));
    assert!(
        registry_csv.is_file(),
        "expected RETriage's batch CSV at {registry_csv:?}"
    );

    let sysinfo_path = collection.join(format!("CaseInfo/{PINNED_STAMP}_SysInfo.txt"));
    let body = std::fs::read_to_string(&sysinfo_path)
        .unwrap_or_else(|e| panic!("expected {sysinfo_path:?} to exist: {e}"));

    assert!(
        body.contains("This is not the SAM local-account list"),
        "the SAM gap must be stated in the report's own header: {body}"
    );
    assert!(
        body.contains("Operating system:") && !body.contains("Operating system: (not found)"),
        "expected a real OS name read back from RETriage's CurrentVersion row: {body}"
    );
    assert!(
        body.contains("Computer name:") && !body.contains("Computer name: (not found)"),
        "expected a real computer name: {body}"
    );
    assert!(
        body.contains("Time zone:") && !body.contains("Time zone: (not found)"),
        "expected a real time zone read back from the TimeZoneInfo plugin's output: {body}"
    );
    assert!(
        body.contains("User profiles (from ProfileList):") && body.contains("S-1-5-"),
        "expected at least one real SID decoded out of ProfileList's Multiple/KeyName/\
         ProfileImagePath row shape: {body}"
    );
}
