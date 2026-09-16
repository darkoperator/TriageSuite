//! Proves the per-collection pipeline's ordering claim in `main.rs`: by the
//! time `write_output_hashes` walks a collection, every other
//! output-producing step -- the copied VeloResults tree, Timeline Explorer
//! sessions, and the SysInfo report -- has already run, so all three are
//! covered by `CaseInfo/<stamp>_OutputHashes.txt`.
//!
//! This is the single assertion that closes out the `PROVISIONAL` call site
//! that shipped with the hashing task: the three tasks that added those
//! output-producing steps each kept the comment there accurate, but nothing
//! actually exercised a real run producing all three at once until now.
//!
//! Gated on `test captures/` via `triage_testkit::skip_if_missing`, same as
//! `velo_sysinfo_e2e.rs`: real SYSTEM/SOFTWARE hives are needed for RETriage
//! to actually emit a `SysInfo.txt` (an absent `Registry/` batch CSV means
//! `write_sysinfo` returns `Ok(None)` and writes nothing at all).
//!
//! Runs with `--no-validate`: the fixture is hand-assembled from exactly the
//! artifacts the three output steps need, so it has no event logs and no
//! SAM/SECURITY hives and the pre-flight gate rejects it.

use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

const PINNED_STAMP: &str = "2026-03-13T192553Z";

fn captures_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures")
}

/// Find the first `SYSTEM`/`SOFTWARE` pair under a Velociraptor
/// `.../config/` directory in the captures tree, skipping `RegBack` copies so
/// the pairing is unambiguous. Identical to `velo_sysinfo_e2e.rs`'s helper.
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

/// A minimal, genuinely valid Version-17 (Windows XP/2003) SCCA prefetch
/// file. Identical construction to `velo_sessions_e2e.rs`'s `build_prefetch`,
/// used here only to make PETriage emit a real CSV so `write_sessions`
/// produces a real `Sessions/Execution_Analysis.tle_sess` alongside the
/// SysInfo report.
fn build_prefetch() -> Vec<u8> {
    const EXE_NAME: &str = "TEST.EXE";
    const FI: usize = 84;
    const FILENAME_STRINGS_OFFSET: usize = FI + 68;

    let filename_strings: Vec<u8> = format!("{EXE_NAME}\0")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let total = FILENAME_STRINGS_OFFSET + filename_strings.len();
    let mut buf = vec![0u8; total];

    buf[0..4].copy_from_slice(&17u32.to_le_bytes());
    buf[4..8].copy_from_slice(b"SCCA");
    buf[12..16].copy_from_slice(&(total as u32).to_le_bytes());

    let name_utf16: Vec<u8> = EXE_NAME.encode_utf16().flat_map(u16::to_le_bytes).collect();
    buf[16..16 + name_utf16.len()].copy_from_slice(&name_utf16);
    buf[76..80].copy_from_slice(&0x1234_5678u32.to_le_bytes());

    buf[FI + 16..FI + 20].copy_from_slice(&(FILENAME_STRINGS_OFFSET as u32).to_le_bytes());
    buf[FI + 20..FI + 24].copy_from_slice(&(filename_strings.len() as u32).to_le_bytes());
    buf[FI + 36..FI + 44].copy_from_slice(&133_333_333_330_000_000u64.to_le_bytes());
    buf[FI + 60..FI + 64].copy_from_slice(&1u32.to_le_bytes());

    buf[FILENAME_STRINGS_OFFSET..total].copy_from_slice(&filename_strings);
    buf
}

/// A real run producing SysInfo, Sessions, and VeloResults output must have
/// all three listed in `CaseInfo/<stamp>_OutputHashes.txt` -- proving
/// `write_output_hashes` runs strictly after every other output-producing
/// step for the collection, not just after some of them.
#[test]
fn output_hashes_covers_veloresults_sessions_and_sysinfo() {
    if triage_testkit::skip_if_missing(&captures_root(), "test captures") {
        return;
    }
    let Some((system_hive, software_hive)) = find_system_software_pair() else {
        eprintln!("SKIP: no SYSTEM/SOFTWARE pair found in captures");
        return;
    };

    // A raw directory: the two hives (for RETriage/SysInfo), a Prefetch file
    // (for PETriage/Sessions), and a `results/` tree (for the VeloResults
    // passthrough) -- `TriageSuite run` treats an unrecognized directory as a
    // single raw capture, so its own path becomes both `collection_dir` and
    // `artifact_root` for this host, and `results/` sits right where
    // `copy_velo_results` expects it: alongside the raw evidence, not nested
    // under it.
    let raw = TempDir::new().unwrap();
    std::fs::copy(&system_hive, raw.path().join("SYSTEM")).unwrap();
    std::fs::copy(&software_hive, raw.path().join("SOFTWARE")).unwrap();
    let pf_dir = raw.path().join("Prefetch");
    std::fs::create_dir_all(&pf_dir).unwrap();
    std::fs::write(pf_dir.join("TEST.EXE-12345678.pf"), build_prefetch()).unwrap();
    std::fs::create_dir_all(raw.path().join("results")).unwrap();
    std::fs::write(raw.path().join("results/report.json"), "{}").unwrap();

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
            "--csv",
            "--overwrite",
            "--only",
            "re,pe",
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

    let velo_results_file = collection.join("VeloResults/report.json");
    assert!(
        velo_results_file.is_file(),
        "expected the copied VeloResults tree at {velo_results_file:?}"
    );
    let session_file = collection.join("Sessions/Execution_Analysis.tle_sess");
    assert!(
        session_file.is_file(),
        "expected a Timeline Explorer session at {session_file:?}"
    );
    let sysinfo_file = collection.join(format!("CaseInfo/{PINNED_STAMP}_SysInfo.txt"));
    assert!(
        sysinfo_file.is_file(),
        "expected a SysInfo report at {sysinfo_file:?}"
    );

    let hashes_path = collection.join(format!("CaseInfo/{PINNED_STAMP}_OutputHashes.txt"));
    let hashes = std::fs::read_to_string(&hashes_path)
        .unwrap_or_else(|e| panic!("expected {hashes_path:?} to exist: {e}"));

    assert!(
        hashes.contains("VeloResults/report.json"),
        "OutputHashes.txt must list the copied VeloResults file, proving it \
         was written before the hash walk: {hashes}"
    );
    assert!(
        hashes.contains("Sessions/Execution_Analysis.tle_sess"),
        "OutputHashes.txt must list the Timeline Explorer session file: {hashes}"
    );
    assert!(
        hashes.contains(&format!("CaseInfo/{PINNED_STAMP}_SysInfo.txt")),
        "OutputHashes.txt must list the SysInfo report: {hashes}"
    );
}
