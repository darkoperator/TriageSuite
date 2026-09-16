//! Proves `write_sessions` is actually wired into a real run, not just
//! exercised directly by its own unit tests -- the same "zero production
//! call sites" trap an earlier task in this plan shipped (see
//! `velo_sysinfo_e2e.rs` for the SysInfo equivalent).
//!
//! Built entirely from a synthetic Velociraptor collection, same pattern as
//! `velo_layout.rs`: a minimal but genuinely valid Prefetch file so PETriage
//! emits a real `FileSystem/<stamp>_PETriage_results.csv`, which is exactly
//! the first pattern in the bundled `Execution_Analysis` session. No
//! `test captures/` evidence tree is needed, so this test always runs.
//!
//! The collection is the gate-passing variant
//! (`synthetic::write_gate_passing_collection`), so this run exercises the
//! pre-flight gate for real rather than turning it off: `--only pe`
//! discovers `*.pf` and never the placeholder files that satisfy the gate.

use assert_cmd::Command;
use tempfile::TempDir;
use triage_testkit::synthetic::write_gate_passing_collection;

const PINNED_STAMP: &str = "2026-03-13T192553Z";

/// A minimal, genuinely valid Version-17 (Windows XP/2003) SCCA prefetch
/// file: an 84-byte header, a 68-byte file-information block, and an
/// 18-byte filename-strings section holding the executable's own name.
/// Zero volumes, one non-zero run time -- enough for
/// `triage_prefetch::format::parse` to succeed and PETriage to emit a row.
/// Identical construction to `velo_layout.rs`'s `build_prefetch`.
fn build_prefetch() -> Vec<u8> {
    const EXE_NAME: &str = "TEST.EXE";
    const FI: usize = 84; // file-information block starts at byte 84
    const FILENAME_STRINGS_OFFSET: usize = FI + 68; // right after the v17 file-info block

    let filename_strings: Vec<u8> = format!("{EXE_NAME}\0")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let total = FILENAME_STRINGS_OFFSET + filename_strings.len();
    let mut buf = vec![0u8; total];

    buf[0..4].copy_from_slice(&17u32.to_le_bytes()); // version
    buf[4..8].copy_from_slice(b"SCCA"); // signature
    buf[12..16].copy_from_slice(&(total as u32).to_le_bytes()); // header file size

    let name_utf16: Vec<u8> = EXE_NAME.encode_utf16().flat_map(u16::to_le_bytes).collect();
    buf[16..16 + name_utf16.len()].copy_from_slice(&name_utf16); // executable name (60-byte field)
    buf[76..80].copy_from_slice(&0x1234_5678u32.to_le_bytes()); // hash

    buf[FI + 16..FI + 20].copy_from_slice(&(FILENAME_STRINGS_OFFSET as u32).to_le_bytes());
    buf[FI + 20..FI + 24].copy_from_slice(&(filename_strings.len() as u32).to_le_bytes());
    // volumes_info_offset/volume_count/volumes_info_size (fi+24/28/32) stay 0:
    // no volumes, so the parser's volume loop never runs.
    buf[FI + 36..FI + 44].copy_from_slice(&133_333_333_330_000_000u64.to_le_bytes()); // one run time
    buf[FI + 60..FI + 64].copy_from_slice(&1u32.to_le_bytes()); // run count

    buf[FILENAME_STRINGS_OFFSET..total].copy_from_slice(&filename_strings);
    buf
}

fn write_fixture_collection(coll: &std::path::Path) {
    write_gate_passing_collection(coll, "WS01");
    let pf_dir = coll.join("uploads/auto/C%3A/Windows/Prefetch");
    std::fs::create_dir_all(&pf_dir).unwrap();
    std::fs::write(pf_dir.join("TEST.EXE-12345678.pf"), build_prefetch()).unwrap();
}

/// A real run producing a real `FileSystem/<stamp>_PETriage_results.csv`
/// must also produce `Sessions/Execution_Analysis.tle_sess` naming that
/// exact file by its absolute, on-disk path -- proving `write_sessions` is
/// reached from `main.rs`, not just from its own unit tests.
#[test]
fn a_real_run_writes_a_session_file_with_real_content() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection(&coll);

    let out = td.path().join("out");
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args([
            "run",
            coll.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
            "--overwrite",
            "--layout",
            "velo",
            "--only",
            "pe",
        ])
        .assert()
        .success();

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let pe_csv = collection.join(format!("FileSystem/{PINNED_STAMP}_PETriage_results.csv"));
    assert!(pe_csv.is_file(), "expected PETriage output at {pe_csv:?}");

    let session_path = collection.join("Sessions/Execution_Analysis.tle_sess");
    let body = std::fs::read_to_string(&session_path)
        .unwrap_or_else(|e| panic!("expected {session_path:?} to exist: {e}"));
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    let files = parsed["SessionFiles"]
        .as_object()
        .expect("SessionFiles must be an object");
    assert_eq!(
        files.len(),
        1,
        "expected only the PETriage CSV to match, since --only pe ran no other tool: {files:?}"
    );

    let expected_absolute = pe_csv.canonicalize().unwrap();
    let recorded = files
        .keys()
        .next()
        .expect("exactly one entry was just asserted above");
    assert_eq!(
        std::path::Path::new(recorded),
        expected_absolute,
        "session must name the real PETriage CSV by its absolute path"
    );
    assert!(
        std::path::Path::new(recorded).is_file(),
        "{recorded} must exist on disk"
    );
    assert!(
        std::path::Path::new(recorded).is_absolute(),
        "{recorded} must be an absolute path"
    );

    // No LETriage/AppCompat/Amcache/LolTriage output exists in this run
    // (only `pe` was selected), so every other bundled session must have
    // been skipped rather than written empty.
    let sessions_dir = collection.join("Sessions");
    let written: Vec<String> = std::fs::read_dir(&sessions_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        written,
        vec!["Execution_Analysis.tle_sess".to_string()],
        "only the session with a real match should exist: {written:?}"
    );
}
