//! End-to-end test for `--layout velo` (the default) and `--layout native`,
//! wired against a synthetic Velociraptor collection built entirely in a
//! tempdir, same as `e2e.rs`. Unlike `e2e.rs`'s deliberately-empty capture,
//! this one carries two genuinely parsable artifacts -- a minimal but valid
//! Prefetch file (PETriage, `Scope::SystemWide`) and a minimal but valid LNK
//! file under a `Users/jdoe/` path (LETriage, `Scope::UserElseSystem`) -- so
//! the test can assert on real output file paths, not just "the run
//! succeeded". Both tools map to the `FileSystem` category
//! (`triage_orchestrator::velo::category_for_key`), so this also proves two
//! tools land in the same category root without colliding.
//!
//! `TRIAGE_RUN_STAMP` is pinned so the Velo run stamp
//! (`velo_run_stamp()`, `crates/triage-core/src/output/router.rs`) is
//! deterministic; that environment-override branch had no test coverage
//! anywhere in the workspace before this test.
//!
//! The collection is the gate-passing variant
//! (`synthetic::write_gate_passing_collection`), so these runs exercise the
//! pre-flight gate for real rather than turning it off: every run here
//! selects `le`, `pe` or `jle` only (`*.lnk`, `*.pf`, `*.automaticDestinations-ms`),
//! and none of those tools discovers the placeholder hives and EVTX that
//! satisfy the gate.

use assert_cmd::Command;
use tempfile::TempDir;
use triage_testkit::synthetic::write_gate_passing_collection;

const PINNED_STAMP: &str = "2026-03-13T192553Z";

/// A minimal, genuinely valid Version-17 (Windows XP/2003) SCCA prefetch
/// file: an 84-byte header, a 68-byte file-information block, and an
/// 18-byte filename-strings section holding the executable's own name.
/// Zero volumes, one non-zero run time -- enough for
/// `triage_prefetch::format::parse` to succeed and PETriage to emit a row.
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

/// A minimal, genuinely valid Shell Link: just the fixed 0x4C-byte header
/// (correct size field, class id, and otherwise all-zero fields -- no data
/// flags set, so there is no LinkTargetIDList, LinkInfo, or StringData to
/// parse). Enough for `triage_lnk::parse_with_codepage` to succeed and
/// LETriage to emit a row.
fn build_lnk() -> Vec<u8> {
    const CLASS_ID: [u8; 16] = [
        0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ];
    let mut buf = vec![0u8; 0x4C];
    buf[0..4].copy_from_slice(&0x0000_004Cu32.to_le_bytes());
    buf[4..20].copy_from_slice(&CLASS_ID);
    buf
}

/// A minimal, genuinely valid `customDestinations-ms`: a 16-byte category
/// header whose `HeaderType` (u32 at offset 12) is non-zero -- so no display
/// name follows -- one embedded Shell Link, and the 4-byte terminal footer
/// (`triage_jumplist::custom::parse`). JLETriage's `CustomDestinations`
/// dataset is what this exists for: it is the only *discriminated* per-user
/// dataset in the registry that a synthetic fixture can reach, and a
/// discriminated dataset is the one whose per-user slices land in their own
/// `PerUser/<Discriminator>/` directory.
fn build_custom_destinations() -> Vec<u8> {
    const FOOTER: [u8; 4] = [0xAB, 0xFB, 0xBF, 0xBA];
    let mut buf = vec![0u8; 16];
    buf[12..16].copy_from_slice(&1u32.to_le_bytes()); // HeaderType != 0: no name
    buf.extend_from_slice(&build_lnk());
    buf.extend_from_slice(&FOOTER);
    buf
}

/// Writes a synthetic Velociraptor collection for host `WS01` (the host name
/// `capture::host_from_collection` reads back out of `client_info.json`)
/// carrying one system-wide `.pf` and one per-user `.lnk` under
/// `Users/jdoe/`, matching the path shape `derive_user` requires to
/// attribute a file to `Identity::User("jdoe")`
/// (`crates/triage-core/src/attribution.rs`).
fn write_fixture_collection(coll: &std::path::Path) {
    write_gate_passing_collection(coll, "WS01");
    let uploads = coll.join("uploads/auto/C%3A");
    let pf_dir = uploads.join("Windows/Prefetch");
    std::fs::create_dir_all(&pf_dir).unwrap();
    std::fs::write(pf_dir.join("TEST.EXE-12345678.pf"), build_prefetch()).unwrap();

    let lnk_dir = uploads.join("Users/jdoe/AppData/Roaming/Microsoft/Windows/Recent");
    std::fs::create_dir_all(&lnk_dir).unwrap();
    std::fs::write(lnk_dir.join("test.lnk"), build_lnk()).unwrap();
}

/// Same fixture as `write_fixture_collection`, plus a second `.lnk` outside
/// any `Users/` path so `derive_user` attributes it to `Identity::System`
/// (`crates/triage-core/src/attribution.rs`) -- giving LETriage both a
/// system-scope and a per-user file in the same run, the exact shape that
/// exposed defect 1 (the merged `TriageUser` file never materializing for
/// `Scope::UserElseSystem` tools).
fn write_fixture_collection_with_system_scope_lnk(coll: &std::path::Path) {
    write_fixture_collection(coll);
    let uploads = coll.join("uploads/auto/C%3A");
    let sys_lnk_dir = uploads.join("ProgramData/Microsoft/Windows/Recent");
    std::fs::create_dir_all(&sys_lnk_dir).unwrap();
    // A trailing byte, not just an identical copy of the jdoe fixture:
    // `dedupe_by_content` (default on) would otherwise collapse the two
    // byte-identical files into a single parse and hide the very
    // system-plus-user shape this test needs. The parser reads strictly by
    // header offsets with no data flags set, so the extra byte is never
    // read and does not change the parsed row.
    let mut lnk = build_lnk();
    lnk.push(0);
    std::fs::write(sys_lnk_dir.join("system.lnk"), lnk).unwrap();
}

/// Same fixture as `write_fixture_collection_with_system_scope_lnk`, plus a
/// third `.lnk` under `Users/System/` (capital S, the reviewer's exact
/// reproduction) -- a real interactive profile literally named "System"
/// (`system`/`System` is not in `attribution::SPECIAL_PROFILES`, so this
/// genuinely derives `Identity::User("System")`, case preserved, not
/// `Identity::System`). Before the case-folding fix, this real account's
/// `PerUser/<stem>_System.csv` and the reclaim's `PerUser/<stem>_system.csv`
/// were the same file on a case-insensitive filesystem (macOS, Windows by
/// default): the reclaim's rename silently destroyed the real account's row
/// and the merge emitted the reclaimed row twice.
fn write_fixture_collection_with_real_system_user(coll: &std::path::Path) {
    write_fixture_collection_with_system_scope_lnk(coll);
    let uploads = coll.join("uploads/auto/C%3A");
    let real_system_dir = uploads.join("Users/System/AppData/Roaming/Microsoft/Windows/Recent");
    std::fs::create_dir_all(&real_system_dir).unwrap();
    // Two trailing bytes: distinct content from both the jdoe and the
    // system-scope fixtures, so `dedupe_by_content` does not collapse this
    // third file into either of them.
    let mut lnk = build_lnk();
    lnk.push(0);
    lnk.push(0);
    std::fs::write(real_system_dir.join("real.lnk"), lnk).unwrap();
}

/// The reviewer's exact reproduction (`--only le --overwrite`, previously
/// exit 0 with the real account silently destroyed and a duplicated
/// system-scope row): a real account named `System` (capital S) must
/// survive untouched, on whatever filesystem this test happens to run on
/// (case-sensitive or not -- the fix does not depend on which), and the
/// merged output must carry one row per source with no duplicate.
#[test]
fn a_real_account_named_system_survives_the_reviewers_reproduction() {
    for overwrite in [false, true] {
        let td = TempDir::new().unwrap();
        let coll = td.path().join("Collection-WS01-2026");
        write_fixture_collection_with_real_system_user(&coll);

        let out = td.path().join("out");
        let mut args = vec![
            "run",
            coll.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
            "--layout",
            "velo",
            "--only",
            "le",
        ];
        if overwrite {
            args.push("--overwrite");
        }

        Command::cargo_bin("TriageSuite")
            .unwrap()
            .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
            .args(&args)
            .assert()
            .success();

        let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
        let stem = format!("{PINNED_STAMP}_LETriage_results");
        let per_user = collection.join("FileSystem/PerUser");

        // The real account's own file, case preserved, is never touched by
        // the reclaim -- still a plain per-user slice, no TriageUser column.
        let real_slice = per_user.join(format!("{stem}_System.csv"));
        let real_body = std::fs::read_to_string(&real_slice)
            .unwrap_or_else(|e| panic!("overwrite={overwrite}: expected {real_slice:?}: {e}"));
        assert!(
            !real_body.contains(",TriageUser"),
            "overwrite={overwrite}: the real account's own file must stay a \
             plain per-user slice: got {real_body}"
        );

        let merged_path = collection.join(format!("FileSystem/{stem}.csv"));
        let merged = std::fs::read_to_string(&merged_path)
            .unwrap_or_else(|e| panic!("overwrite={overwrite}: expected {merged_path:?}: {e}"));
        let body: Vec<&str> = merged.lines().skip(1).collect();
        assert_eq!(
            body.len(),
            3,
            "overwrite={overwrite}: one row per source (System, jdoe, the \
             reclaimed system-scope slice), no duplicate: got {body:?}"
        );
        assert!(
            body.iter().any(|l| l.ends_with(",System")),
            "overwrite={overwrite}: the real account's row, case preserved: \
             got {body:?}"
        );
        assert!(
            body.iter().any(|l| l.ends_with(",jdoe")),
            "overwrite={overwrite}: got {body:?}"
        );
        assert_eq!(
            body.iter().filter(|l| l.ends_with(",system")).count(),
            1,
            "overwrite={overwrite}: exactly one system-scope row from the \
             reclaim, not duplicated: got {body:?}"
        );
    }
}

/// Finding 2 (corrected): re-running the identical `UserElseSystem` scenario
/// twice with `--overwrite` must succeed both times -- the reclaimed system
/// slice left behind under `PerUser/` by the first run's reclaim is a leftover
/// artifact of this same pipeline's own derived output, not source evidence,
/// so `--overwrite` legitimately replaces it on the second run.
#[test]
fn overwrite_merges_successfully_on_repeated_runs_of_the_same_scenario() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection_with_system_scope_lnk(&coll);

    let out = td.path().join("out");
    let args = [
        "run",
        coll.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--overwrite",
        "--layout",
        "velo",
        "--only",
        "le",
    ];

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let stem = format!("{PINNED_STAMP}_LETriage_results");
    let merged_path = collection.join(format!("FileSystem/{stem}.csv"));

    for run_number in 1..=2 {
        Command::cargo_bin("TriageSuite")
            .unwrap()
            .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
            .args(args)
            .assert()
            .success();

        let merged = std::fs::read_to_string(&merged_path).unwrap_or_else(|e| {
            panic!("run {run_number}: expected merged file at {merged_path:?}: {e}")
        });
        assert_eq!(
            merged.lines().count(),
            3,
            "run {run_number}: header plus two rows, not doubled by the re-run: \
             got {merged}"
        );
        assert!(
            merged.contains(",system\n") && merged.contains(",jdoe\n"),
            "run {run_number}: both slices must still be present: got {merged}"
        );
    }
}

/// Finding 2 (corrected): without `--overwrite`, re-running the identical
/// scenario a second time must still fail -- consistent with every other
/// output collision in this tool, the reclaimed system slice left under
/// `PerUser/` by the first run blocks a second write to it without
/// `--overwrite`.
#[test]
fn without_overwrite_a_repeated_run_still_fails_on_the_second_attempt() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection_with_system_scope_lnk(&coll);

    let out = td.path().join("out");
    let args = [
        "run",
        coll.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--layout",
        "velo",
        "--only",
        "le",
    ];

    // First run succeeds and leaves the reclaimed system slice under PerUser/ behind.
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(args)
        .assert()
        .success();

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let stem = format!("{PINNED_STAMP}_LETriage_results");
    let merged_path = collection.join(format!("FileSystem/{stem}.csv"));
    let first_merged = std::fs::read_to_string(&merged_path).unwrap();

    // Second run, same command, still no --overwrite: every other output
    // collision in this tool fails here, and this must be no exception.
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(args)
        .assert()
        .failure();

    // The first run's merged output is not required to be pristine here --
    // only that the run reports failure, matching every other collision.
    let _ = first_merged;
}

/// Regression test for defect 1: a `Scope::UserElseSystem` tool
/// (LETriage) with both a system-scope and a per-user artifact must, in a
/// default run (no `--overwrite`), produce a reclaimed system slice and
/// `PerUser/<stem>_jdoe.csv` under `PerUser/`, AND a category-root merged file carrying BOTH
/// rows with a trailing `TriageUser` column ("system" and "jdoe").
/// Before the fix, the router's system-scope write occupied the exact path
/// the merge post-pass needed for its output, so the merge failed and no
/// merged file -- the tool's central deliverable -- ever appeared.
#[test]
fn user_else_system_tool_produces_a_merged_file_with_both_slices() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection_with_system_scope_lnk(&coll);

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
            "--layout",
            "velo",
            "--only",
            "le",
            // Deliberately no --overwrite: this is the default-run path
            // defect 1 broke.
        ])
        .assert()
        .success();

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let stem = format!("{PINNED_STAMP}_LETriage_results");
    let per_user = collection.join("FileSystem/PerUser");

    // The reclaimed system slice's on-disk label is an internal
    // implementation detail (`RECLAIM_LABEL`, not `"system"` -- see its doc
    // comment in `merge.rs`), so this checks for "some file besides jdoe's"
    // rather than hardcoding the private label string.
    let per_user_files: Vec<String> = std::fs::read_dir(&per_user)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        per_user_files
            .iter()
            .any(|name| name.starts_with(&format!("{stem}_")) && !name.contains("jdoe")),
        "expected the reclaimed system slice to land in PerUser/, not block \
         the merge: got {per_user_files:?}"
    );
    assert!(
        per_user.join(format!("{stem}_jdoe.csv")).is_file(),
        "expected the jdoe per-user slice"
    );

    let merged_path = collection.join(format!("FileSystem/{stem}.csv"));
    let merged = std::fs::read_to_string(&merged_path)
        .unwrap_or_else(|e| panic!("expected merged file at {merged_path:?}: {e}"));
    let mut lines = merged.lines();
    let header = lines.next().unwrap();
    assert!(
        header.ends_with(",TriageUser"),
        "merged header must be the per-user header plus a trailing TriageUser: got {header}"
    );
    let body: Vec<&str> = lines.collect();
    assert_eq!(body.len(), 2, "one row per slice: got {body:?}");
    assert!(
        body.iter().any(|l| l.ends_with(",system")),
        "expected a system-attributed row: got {body:?}"
    );
    assert!(
        body.iter().any(|l| l.ends_with(",jdoe")),
        "expected a jdoe-attributed row: got {body:?}"
    );
}

/// Regression test for the review finding on top of defect 1: `FileSystem/`
/// is a *shared category* directory, holding PETriage's (`Scope::SystemWide`)
/// output alongside LETriage's (`Scope::UserElseSystem`). A real run with
/// `--only pe,le` and no `--overwrite` must not sweep PETriage's root file
/// into `PerUser/` or give it a bogus `TriageUser` column merely because its
/// `UserElseSystem` sibling created `PerUser/` in the same category -- the
/// reclaim in `merge_per_user` must be gated on "a per-user slice exists for
/// *this* stem", not on `PerUser/`'s mere existence.
#[test]
fn a_system_wide_sibling_is_unaffected_by_a_user_else_system_tools_per_user_tree() {
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
            "--layout",
            "velo",
            "--only",
            "pe,le",
            // Deliberately no --overwrite: this is the exact default-run
            // shape the review reproduced the regression with.
        ])
        .assert()
        .success();

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let file_system = collection.join("FileSystem");
    let pe_stem = format!("{PINNED_STAMP}_PETriage_results");
    let le_stem = format!("{PINNED_STAMP}_LETriage_results");

    // No `*_PETriage_results*` file of any kind under PerUser/: PETriage
    // never had a per-user slice, so nothing about it should ever have been
    // reclaimed there.
    if let Ok(entries) = std::fs::read_dir(file_system.join("PerUser")) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            assert!(
                !name.contains("PETriage"),
                "PETriage output must never appear under PerUser/: found {name}"
            );
        }
    }

    let pe_root = std::fs::read_to_string(file_system.join(format!("{pe_stem}.csv")))
        .expect("PETriage's root CSV must still exist, untouched");
    let pe_header = pe_root.lines().next().unwrap_or_default();
    assert!(
        !pe_header.ends_with(",TriageUser"),
        "SystemWide PETriage must stay Zimmerman-exact, no TriageUser column: got {pe_header}"
    );

    // LETriage, a genuine UserElseSystem tool with a real per-user slice,
    // still gets its merged file with the trailing TriageUser column.
    let le_merged = std::fs::read_to_string(file_system.join(format!("{le_stem}.csv")))
        .expect("LETriage's merged file must still be produced");
    let le_header = le_merged.lines().next().unwrap_or_default();
    assert!(
        le_header.ends_with(",TriageUser"),
        "LETriage must still gain the merged TriageUser column: got {le_header}"
    );
}

#[test]
fn velo_layout_writes_the_category_tree() {
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
            "pe,le",
        ])
        .assert()
        .success();

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    assert!(
        collection.join("FileSystem").is_dir(),
        "expected a FileSystem category directory under {collection:?}"
    );
    assert!(
        collection
            .join(format!("FileSystem/{PINNED_STAMP}_PETriage_results.csv"))
            .is_file(),
        "expected PETriage's system-scope output directly under FileSystem"
    );
    assert!(
        collection
            .join(format!(
                "FileSystem/PerUser/{PINNED_STAMP}_LETriage_results_jdoe.csv"
            ))
            .is_file(),
        "expected LETriage's jdoe-attributed output under FileSystem/PerUser"
    );
    // PETriage is Scope::SystemWide, so it must not produce a PerUser tree.
    assert!(!collection
        .join(format!(
            "FileSystem/PerUser/{PINNED_STAMP}_PETriage_results_jdoe.csv"
        ))
        .exists());
}

/// Proves `Velo` is the *default*, not merely a working option: no
/// `--layout` flag is passed at all. Asserts both directions -- the Velo
/// tree appears unasked, and the native `<host>/<Tool>/` tree does not --
/// so a refactor that silently flips the default back to `Native` fails
/// this test regardless of which way it drifts. Uses the same fixture as
/// `velo_layout_writes_the_category_tree` (real parsed PE/LE output, not a
/// zero-match capture), and asserts on a real output *file*, not just a
/// directory, so an early return before the layout branch runs (see
/// `execute.rs`'s `if files.is_empty()` guard) cannot make this vacuously
/// pass.
#[test]
fn velo_is_the_default_without_an_explicit_layout_flag() {
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
            "--only",
            "pe,le",
            // Deliberately no --layout: the point of this test is the default.
        ])
        .assert()
        .success();

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    assert!(
        collection
            .join(format!("FileSystem/{PINNED_STAMP}_PETriage_results.csv"))
            .is_file(),
        "expected the Velo category tree with a real PETriage output file \
         when --layout is omitted entirely"
    );
    assert!(
        collection
            .join(format!(
                "FileSystem/PerUser/{PINNED_STAMP}_LETriage_results_jdoe.csv"
            ))
            .is_file(),
        "expected the Velo per-user tree with a real LETriage output file \
         when --layout is omitted entirely"
    );
    assert!(
        !out.join("WS01/PETriage").exists(),
        "the native per-tool tree must not appear when Velo is the default"
    );
}

#[test]
fn native_layout_is_unchanged() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection(&coll);

    let out = td.path().join("out");
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args([
            "run",
            coll.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
            "--overwrite",
            "--layout",
            "native",
            "--only",
            "pe,le",
        ])
        .assert()
        .success();

    assert!(out.join("WS01/PETriage/system").is_dir());
    assert!(!out.join(format!("Processed-WS01-{PINNED_STAMP}")).exists());
}

/// A real run must actually produce `CaseInfo/<stamp>_SHA256_HashLog.txt`,
/// not just the unit-tested `write_source_hash_log` function in isolation --
/// that gap (the function existed but had no production call site) is
/// exactly what let the feature ship inert once before. This capture is a
/// raw directory (no source archive), so it also exercises the "no source
/// archive" branch end to end.
#[test]
fn velo_run_writes_the_source_hash_log() {
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
            "pe,le",
        ])
        .assert()
        .success();

    let hash_log = out
        .join(format!("Processed-WS01-{PINNED_STAMP}"))
        .join("CaseInfo")
        .join(format!("{PINNED_STAMP}_SHA256_HashLog.txt"));
    let body = std::fs::read_to_string(&hash_log)
        .unwrap_or_else(|e| panic!("expected {hash_log:?} to exist: {e}"));
    assert!(
        body.to_lowercase().contains("no source archive"),
        "raw-directory input has no archive: got {body}"
    );
}

/// A real run through the CLI binary must actually produce
/// `process_logs/<Tool>.log` for each in-process tool that ran, not just the
/// unit-tested `ProcessLog` type in isolation -- that exact gap (a function
/// with passing unit tests but no production call site) shipped inert once
/// before in this plan. Reuses the PE/LE fixture from
/// `velo_layout_writes_the_category_tree`, so both a system-wide tool
/// (PETriage) and a per-user tool (LETriage) get checked.
#[test]
fn velo_run_writes_process_logs_for_in_process_tools() {
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
            "pe,le",
        ])
        .assert()
        .success();

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));

    let pe_log = collection.join("process_logs/PETriage.log");
    let pe_body = std::fs::read_to_string(&pe_log)
        .unwrap_or_else(|e| panic!("expected {pe_log:?} to exist: {e}"));
    assert!(
        pe_body.contains("discovered 1 candidate files"),
        "got {pe_body}"
    );
    assert!(pe_body.contains("parsed: 1"), "got {pe_body}");
    assert!(pe_body.contains("failed: 0"), "got {pe_body}");
    assert!(pe_body.contains("records:"), "got {pe_body}");
    assert!(pe_body.contains("duration:"), "got {pe_body}");

    let le_log = collection.join("process_logs/LETriage.log");
    let le_body = std::fs::read_to_string(&le_log)
        .unwrap_or_else(|e| panic!("expected {le_log:?} to exist: {e}"));
    assert!(
        le_body.contains("discovered 1 candidate files"),
        "got {le_body}"
    );
    assert!(le_body.contains("parsed: 1"), "got {le_body}");
    assert!(le_body.contains("records:"), "got {le_body}");
}

/// Under `--layout native` there is no `Processed-<HOST>-<stamp>` directory
/// of the shape process logs need (`crate::velo::proclog`'s design note),
/// matching how the source hash log above is gated on the same
/// `Layout::Velo` condition -- so a real native-layout run must not produce
/// a `process_logs/` directory anywhere under its output root.
#[test]
fn native_layout_writes_no_process_logs() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection(&coll);

    let out = td.path().join("out");
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .args([
            "run",
            coll.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--csv",
            "--overwrite",
            "--layout",
            "native",
            "--only",
            "pe,le",
        ])
        .assert()
        .success();

    assert!(out.join("WS01/PETriage/system").is_dir());
    assert!(
        !walk_has_process_logs(&out),
        "no process_logs/ directory should exist anywhere under --layout native"
    );
}

/// Small recursive scan for a directory named `process_logs` anywhere under
/// `root`, used only by the native-layout negative test above.
fn walk_has_process_logs(root: &std::path::Path) -> bool {
    if !root.exists() {
        return false;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) == Some("process_logs") {
                    return true;
                }
                stack.push(path);
            }
        }
    }
    false
}

/// The per-user-only rerun: LETriage sees one `.lnk` under `Users/jdoe/` and
/// nothing outside a user profile, so the router writes a per-user slice and
/// *nothing* at the category root -- the merged file there is written by the
/// merge post-pass itself. Re-running with the same stamp and `--overwrite`
/// must rebuild that merged file from the per-user slices, never treat the
/// previous run's merged file as this run's system-scope output.
///
/// Before the fix, the reclaim asked only whether its *destination* under
/// `PerUser/` was router-owned; nothing established that the category-root
/// file it was about to move had been published by this run. So the previous
/// run's merged file was renamed into `PerUser/` and folded back in as a
/// system slice: for CSV that then failed on the `TriageUser` column the
/// previous merge had already appended -- after the rename had already
/// carried the only merged copy off the category root.
#[test]
fn a_previous_runs_merged_csv_is_not_reclaimed_on_a_rerun() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection(&coll);

    let out = td.path().join("out");
    let base = [
        "run",
        coll.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--layout",
        "velo",
        "--only",
        "le",
    ];

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(base)
        .assert()
        .success();

    let mut second = base.to_vec();
    second.push("--overwrite");
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(&second)
        .assert()
        .success();

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let stem = format!("{PINNED_STAMP}_LETriage_results");
    let per_user = collection.join("FileSystem/PerUser");

    // Exactly the slices the parser wrote: jdoe's, and nothing else. A
    // reclaimed previous merged file would show up here as a second file for
    // this stem.
    let mut per_user_files: Vec<String> = std::fs::read_dir(&per_user)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&format!("{stem}_")))
        .collect();
    per_user_files.sort();
    assert_eq!(
        per_user_files,
        vec![format!("{stem}_jdoe.csv")],
        "the previous run's merged file must not be moved into PerUser/"
    );

    let merged_path = collection.join(format!("FileSystem/{stem}.csv"));
    let merged = std::fs::read_to_string(&merged_path)
        .unwrap_or_else(|e| panic!("expected merged file at {merged_path:?}: {e}"));
    let mut lines = merged.lines();
    let header = lines.next().unwrap();
    assert!(
        header.ends_with(",TriageUser"),
        "merged header must be the per-user header plus one trailing \
         TriageUser column, not two: got {header}"
    );
    let body: Vec<&str> = lines.collect();
    assert_eq!(
        body.len(),
        1,
        "one row, from the one per-user slice -- not doubled by the rerun: \
         got {body:?}"
    );
    assert!(
        body[0].ends_with(",jdoe"),
        "the row belongs to jdoe, not to a fabricated system slice: got {body:?}"
    );
}

/// NDJSON counterpart of
/// [`a_previous_runs_merged_csv_is_not_reclaimed_on_a_rerun`]. This is the
/// case that corrupts silently rather than failing: NDJSON has no header to
/// disagree, so the previous run's merged rows were re-read, had their
/// `TriageUser` overwritten with `"system"`, and were appended alongside the
/// fresh per-user rows.
#[test]
fn a_previous_runs_merged_ndjson_is_not_duplicated_or_relabelled_on_a_rerun() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection(&coll);

    let out = td.path().join("out");
    let base = [
        "run",
        coll.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--json",
        "--layout",
        "velo",
        "--only",
        "le",
    ];

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(base)
        .assert()
        .success();

    let mut second = base.to_vec();
    second.push("--overwrite");
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(&second)
        .assert()
        .success();

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let stem = format!("{PINNED_STAMP}_LETriage_results");
    let per_user = collection.join("FileSystem/PerUser");

    let mut per_user_files: Vec<String> = std::fs::read_dir(&per_user)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&format!("{stem}_")))
        .collect();
    per_user_files.sort();
    assert_eq!(
        per_user_files,
        vec![format!("{stem}_jdoe.json")],
        "the previous run's merged file must not be moved into PerUser/"
    );

    let merged_path = collection.join(format!("FileSystem/{stem}.json"));
    let merged = std::fs::read_to_string(&merged_path)
        .unwrap_or_else(|e| panic!("expected merged file at {merged_path:?}: {e}"));
    let rows: Vec<serde_json::Value> = merged
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "one row, from the one per-user slice -- the rerun must not append \
         the previous merged file's rows again: got {merged}"
    );
    assert_eq!(
        rows[0].get("TriageUser").and_then(|v| v.as_str()),
        Some("jdoe"),
        "jdoe's row must stay attributed to jdoe, never relabelled system: \
         got {merged}"
    );
}

/// The same per-user-only rerun *without* `--overwrite`: the run fails on the
/// output collision, as every other collision in this tool does, but the
/// previous run's merged file -- the only copy of those rows at the category
/// root -- must still be sitting there, byte for byte, afterwards. Before the
/// fix the merge ran unconditionally after the router's failure and renamed
/// it into `PerUser/`.
#[test]
fn a_failed_rerun_without_overwrite_leaves_the_previous_merged_csv_in_place() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection(&coll);

    let out = td.path().join("out");
    let base = [
        "run",
        coll.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--layout",
        "velo",
        "--only",
        "le",
    ];

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(base)
        .assert()
        .success();

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let stem = format!("{PINNED_STAMP}_LETriage_results");
    let merged_path = collection.join(format!("FileSystem/{stem}.csv"));
    let first_merged = std::fs::read_to_string(&merged_path).unwrap();

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(base)
        .assert()
        .failure();

    let after = std::fs::read_to_string(&merged_path).unwrap_or_else(|e| {
        panic!("the previous run's merged file must survive a failed rerun: {merged_path:?}: {e}")
    });
    assert_eq!(
        after, first_merged,
        "a failed rerun must not rewrite or relocate the previous merged file"
    );
}

/// The reclaim now turns on an exact `PathBuf` comparison between the
/// category-root path the merge builds and the destinations the router
/// published (`router_wrote`), so a *false negative* there would no longer
/// merely skip a reclaim -- with `--overwrite` it would replace the router's
/// system-scope slice with a merged file built from the per-user slices
/// alone, dropping the system rows. Relative-versus-absolute is the way that
/// comparison could plausibly drift (both sides come from the same
/// `csv_root` value, which is exactly what this pins), so drive the real
/// binary with a *relative* `--out` from a different working directory and
/// assert the system rows still arrive.
#[test]
fn a_relative_out_path_still_reclaims_the_system_scope_slice() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection_with_system_scope_lnk(&coll);

    // The capture path stays absolute on purpose: a relative one resolves
    // against each external tool's own install directory.
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .current_dir(td.path())
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args([
            "run",
            coll.to_str().unwrap(),
            "--out",
            "relative-out",
            "--csv",
            "--layout",
            "velo",
            "--only",
            "le",
        ])
        .assert()
        .success();

    let stem = format!("{PINNED_STAMP}_LETriage_results");
    let merged_path = td
        .path()
        .join("relative-out")
        .join(format!("Processed-WS01-{PINNED_STAMP}"))
        .join(format!("FileSystem/{stem}.csv"));
    let merged = std::fs::read_to_string(&merged_path)
        .unwrap_or_else(|e| panic!("expected merged file at {merged_path:?}: {e}"));
    let body: Vec<&str> = merged.lines().skip(1).collect();
    assert_eq!(body.len(), 2, "one row per slice: got {body:?}");
    assert!(
        body.iter().any(|l| l.ends_with(",system")),
        "the system-scope slice must still be reclaimed and merged, not \
         silently replaced: got {body:?}"
    );
    assert!(body.iter().any(|l| l.ends_with(",jdoe")), "got {body:?}");
}

/// The write side and the read side must agree on which directory a
/// *discriminated* dataset's per-user slices live in. The router picks it
/// from the `DatasetSpec` (`OutputLayout::for_velo_dataset`); the merge
/// post-pass is told it by `execute.rs`, from the same spec. If those two
/// ever drifted apart the merge would scan an empty directory and silently
/// produce no merged file at all -- no error, just a missing artifact -- so
/// this drives the real binary over a discriminated per-user dataset and
/// asserts both halves: the slice under `PerUser/<Discriminator>/`, and the
/// merged, `TriageUser`-tagged file at the category root. Every other
/// binary-driving test here uses LETriage, whose single dataset has no
/// discriminator and therefore exercises neither.
#[test]
fn a_discriminated_datasets_per_user_slice_lands_under_its_own_directory_and_still_merges() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_gate_passing_collection(&coll, "WS01");
    // A profile name that also begins with the *other* dataset's
    // discriminator: nothing about this name may change where the file goes
    // or who the merge says wrote it.
    let jump_dir = coll.join(
        "uploads/auto/C%3A/Users/AutomaticDestinations_jdoe/AppData/Roaming/Microsoft/Windows/Recent/CustomDestinations",
    );
    std::fs::create_dir_all(&jump_dir).unwrap();
    std::fs::write(
        jump_dir.join("1b4dd67f29cb1962.customDestinations-ms"),
        build_custom_destinations(),
    )
    .unwrap();

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
            "--layout",
            "velo",
            "--only",
            "jle",
        ])
        .assert()
        .success();

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let stem = format!("{PINNED_STAMP}_JLETriage_results_CustomDestinations");
    let slice = collection.join(format!(
        "FileSystem/PerUser/CustomDestinations/{stem}_AutomaticDestinations_jdoe.csv"
    ));
    assert!(
        slice.is_file(),
        "expected the per-user slice under its dataset's directory at {slice:?}"
    );

    let merged_path = collection.join(format!("FileSystem/{stem}.csv"));
    let merged = std::fs::read_to_string(&merged_path).unwrap_or_else(|e| {
        panic!(
            "expected the merged file at {merged_path:?}: {e} -- the merge \
             must read the same directory the router wrote to"
        )
    });
    let header = merged.lines().next().unwrap_or_default();
    assert!(
        header.ends_with(",TriageUser"),
        "merged header must be the per-user header plus a trailing TriageUser: got {header}"
    );
    let body: Vec<&str> = merged.lines().skip(1).collect();
    assert!(!body.is_empty(), "expected at least one merged row");
    assert!(
        body.iter()
            .all(|l| l.ends_with(",AutomaticDestinations_jdoe")),
        "every row belongs to the profile that produced it, whole name \
         intact: got {body:?}"
    );
}

/// A prefetch file that clears validation and then fails to parse: the magic
/// `PeTool::validate_legacy` checks (`SCCA` at offset 4) is present and the
/// version is a supported 17, but the file stops at the end of the 84-byte
/// header, so `triage_prefetch::format::parse` runs off the end of the
/// file-information block. This is the exact shape the process log used to
/// be blind to -- validation says Supported, parsing says no, and the log
/// recorded only a closing `failed: 1`.
fn build_header_only_prefetch() -> Vec<u8> {
    let mut buf = build_prefetch();
    buf.truncate(84);
    buf
}

/// `write_fixture_collection` plus a second `.pf` that validates and then
/// fails to parse, so one PETriage run produces both a success and a
/// recoverable parse failure.
fn write_fixture_collection_with_unparsable_prefetch(coll: &std::path::Path) {
    write_fixture_collection(coll);
    let pf_dir = coll.join("uploads/auto/C%3A/Windows/Prefetch");
    std::fs::write(
        pf_dir.join("BROKEN.EXE-DEADBEEF.pf"),
        build_header_only_prefetch(),
    )
    .unwrap();
}

/// The process log is the persistent per-file record of what happened to
/// each artifact, so an artifact that validates and then fails to parse must
/// be named there, with the parser's own reason -- not just folded into the
/// closing `failed:` count, which tells an analyst that something failed but
/// never which file or why.
#[test]
fn a_parse_failure_names_the_file_and_the_reason_in_the_process_log() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection_with_unparsable_prefetch(&coll);

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
        // Partial: one of the two prefetch files parsed, the other failed.
        .code(5);

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let body = std::fs::read_to_string(collection.join("process_logs/PETriage.log")).unwrap();

    let failure_line = body
        .lines()
        .find(|l| l.contains("BROKEN.EXE-DEADBEEF.pf"))
        .unwrap_or_else(|| panic!("the failing file must be named in the log:\n{body}"));
    assert!(
        failure_line.contains("file information block"),
        "the line naming the file must carry the parser's own reason: {failure_line}"
    );
    assert!(
        body.contains("parsed: 1") && body.contains("failed: 1"),
        "the closing counts must be unchanged: {body}"
    );

    // The manifest keeps the same failure as a capped summary sample, so a
    // machine-readable record of *why* exists too.
    let manifest = std::fs::read_to_string(out.join("run_manifest.json")).unwrap();
    assert!(
        manifest.contains("BROKEN.EXE-DEADBEEF.pf") && manifest.contains("file information block"),
        "the manifest must carry the parse failure as a reason sample: {manifest}"
    );
}

/// A run must survive an unusable process log. `process_logs/` is
/// pre-created as a regular *file*, so `ProcessLog::open`'s `create_dir_all`
/// fails and the tool runs with no log at all -- including the parse-failure
/// lines the run now wants to write. The run must still produce its parsed
/// output and exit exactly as it does with a working log (5, partial: one
/// prefetch parsed, the other failed), because the manifest, not the log, is
/// the authoritative record.
#[test]
fn an_unwritable_process_log_does_not_change_the_runs_outcome() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection_with_unparsable_prefetch(&coll);

    let out = td.path().join("out");
    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    std::fs::create_dir_all(&collection).unwrap();
    std::fs::write(collection.join("process_logs"), b"not a directory").unwrap();

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
        .code(5);

    assert!(
        collection
            .join(format!("FileSystem/{PINNED_STAMP}_PETriage_results.csv"))
            .is_file(),
        "the parsed output must still be written when the process log cannot be"
    );
    assert!(
        collection.join("process_logs").is_file(),
        "the blocking file must be left alone rather than replaced"
    );
    // The reason still reaches the manifest, which is where it is durable.
    let manifest = std::fs::read_to_string(out.join("run_manifest.json")).unwrap();
    assert!(
        manifest.contains("BROKEN.EXE-DEADBEEF.pf"),
        "the manifest must carry the parse failure even with no log: {manifest}"
    );
}

/// A merge failure must name the dataset and the reason in the process log,
/// for the same reason a parse failure must: `note_merge_failure` increments
/// `failed`, so without a line here the closing count has no explanation.
///
/// The failure planted here is a *directory* occupying the merged file's
/// destination: `merge_per_user` stages its output and then renames it into
/// place, and a rename onto a directory fails under `--overwrite` exactly as
/// it does for every other output in this tool (`OutputLayout::publish`).
/// The run uses `--overwrite`, so the process log itself opens normally and
/// the merge is the only thing that fails.
///
/// It used to plant a stale `PerUser/<stem>_ghost.csv` with a header the
/// current LETriage output does not have, back when `merge_per_user` took
/// its sources from a plain `read_dir` of `PerUser/`. It now takes them from
/// this run's published destinations
/// (`velo::merge::published_per_user_sources`), so a leftover file is no
/// longer a reachable source -- which is the point of
/// [`a_removed_user_does_not_persist_into_the_reruns_merged_output`] and its
/// sibling, and the reason this test needed a different failure.
#[test]
fn a_merge_failure_names_the_dataset_and_the_reason_in_the_process_log() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection(&coll);

    let out = td.path().join("out");
    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let stem = format!("{PINNED_STAMP}_LETriage_results");
    let merged_path = collection.join(format!("FileSystem/{stem}.csv"));
    // Non-empty, so the rename cannot succeed on any platform's
    // rename-onto-an-empty-directory semantics.
    std::fs::create_dir_all(merged_path.join("occupied")).unwrap();

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
            "le",
        ])
        .assert()
        // Partial: the lnk parsed, the merge of its dataset did not.
        .code(5);

    let body = std::fs::read_to_string(collection.join("process_logs/LETriage.log")).unwrap();
    let failure_line = body
        .lines()
        .find(|l| l.starts_with("merge failed:"))
        .unwrap_or_else(|| panic!("the failed merge must be named in the log:\n{body}"));
    let reason = failure_line
        .split_once(&format!("{stem} — "))
        .map(|(_, reason)| reason)
        .unwrap_or_else(|| panic!("the line must name the dataset stem: {failure_line}"));
    // The reason names the output that failed and says something further
    // about it: the rename's own error, whose text is the OS's and so is not
    // spelled out here.
    let named = merged_path.display().to_string();
    assert!(
        reason.contains(&named) && reason.len() > named.len() + 1,
        "the line must carry the merge's own reason -- which output failed, \
         and what the OS said about it: {failure_line}"
    );
    assert!(
        body.contains("failed: 1"),
        "the closing count must still record the failure: {body}"
    );
}

/// A minimal, genuinely valid `automaticDestinations-ms`: an OLE compound
/// file (`cfb`) holding one `DestList` stream -- version 3, one entry, no
/// serialized property store and no numbered LNK stream, which
/// `triage_jumplist::automatic::parse` reads as one entry with `lnk: None`
/// and JLETriage still emits a row for.
///
/// It exists so a synthetic collection can reach JLETriage's *other*
/// dataset. The two-dataset shape is the whole point: a dataset's per-user
/// slices live under `PerUser/<Discriminator>/`, so a write to the
/// `AutomaticDestinations` dataset can be made to fail while the
/// `CustomDestinations` dataset's own directory -- and the category root --
/// stay writable, which is what
/// [`a_failed_finish_publishes_nothing_so_the_previous_merged_csv_survives`]
/// needs.
fn build_automatic_destinations() -> Vec<u8> {
    use std::io::Write;

    let path = "C:\\Users\\jdoe\\file.txt";
    let units: Vec<u16> = path.encode_utf16().collect();
    // Version 3 entry layout: path length (in UTF-16 code units) at +128,
    // path at +130, then a u32 serialized-property-store size (0 here).
    let entry_size = 130 + units.len() * 2;
    let mut entry = vec![0u8; entry_size + 4];
    entry[88..92].copy_from_slice(&1u32.to_le_bytes()); // entry number
    entry[108..112].copy_from_slice(&(-1i32).to_le_bytes()); // pin status
    entry[128..130].copy_from_slice(&(units.len() as u16).to_le_bytes());
    for (i, u) in units.iter().enumerate() {
        entry[130 + i * 2..132 + i * 2].copy_from_slice(&u.to_le_bytes());
    }
    let mut dest_list = vec![0u8; 32];
    dest_list[0..4].copy_from_slice(&3u32.to_le_bytes()); // version
    dest_list[4..8].copy_from_slice(&1u32.to_le_bytes()); // number of entries
    dest_list[16..20].copy_from_slice(&1u32.to_le_bytes()); // last entry number
    dest_list.extend_from_slice(&entry);

    let mut comp = cfb::CompoundFile::create(std::io::Cursor::new(Vec::new())).unwrap();
    let mut stream = comp.create_stream("/DestList").unwrap();
    stream.write_all(&dest_list).unwrap();
    stream.flush().unwrap();
    drop(stream);
    comp.flush().unwrap();
    comp.into_inner().into_inner()
}

/// [`build_custom_destinations`] with `tag` in the first four header bytes.
/// The parser frames categories by footer signature and embedded LNKs by
/// LNK signature, so nothing reads these bytes -- they exist only to make
/// two copies in one collection differ, which `dedupe_by_content` (on by
/// default) otherwise collapses into a single parse, costing the fixture
/// either its system-scope or its per-user slice.
fn build_custom_destinations_tagged(tag: u32) -> Vec<u8> {
    let mut buf = build_custom_destinations();
    buf[0..4].copy_from_slice(&tag.to_le_bytes());
    buf
}

/// The collection for the failed-`finish()` scenario: JLETriage
/// (`Scope::UserElseSystem`) with a system-scope and a per-user
/// `customDestinations-ms`, plus a per-user `automaticDestinations-ms`.
///
/// The paths are chosen for the order `triage_core::discovery` hands them
/// to the parse loop, which is a byte-wise sort (`discover`'s `files.sort()`):
/// `ProgramData/...` < `Users/...`, and under `Recent/`,
/// `AutomaticDestinations/` < `CustomDestinations/`. So the system-scope
/// custom destination is parsed *first* -- opening the category-root
/// destination the reclaim later asks about -- and the automatic
/// destination second, which is where the poisoned write lands.
fn write_fixture_collection_with_jump_lists(coll: &std::path::Path) {
    write_gate_passing_collection(coll, "WS01");
    let uploads = coll.join("uploads/auto/C%3A");

    let system = uploads.join("ProgramData/Microsoft/Windows/Recent/CustomDestinations");
    std::fs::create_dir_all(&system).unwrap();
    std::fs::write(
        system.join("1b4dd67f29cb1962.customDestinations-ms"),
        build_custom_destinations_tagged(1),
    )
    .unwrap();

    let recent = uploads.join("Users/jdoe/AppData/Roaming/Microsoft/Windows/Recent");
    let user_custom = recent.join("CustomDestinations");
    std::fs::create_dir_all(&user_custom).unwrap();
    std::fs::write(
        user_custom.join("2b4dd67f29cb1962.customDestinations-ms"),
        build_custom_destinations_tagged(2),
    )
    .unwrap();

    let user_auto = recent.join("AutomaticDestinations");
    std::fs::create_dir_all(&user_auto).unwrap();
    std::fs::write(
        user_auto.join("5f7b5f1e01b83767.automaticDestinations-ms"),
        build_automatic_destinations(),
    )
    .unwrap();
}

/// Replace the `AutomaticDestinations` dataset's per-user directory with a
/// regular file, so the next run's first write to that dataset fails in
/// `create_dir_all` (`NotADirectory`) -- an `OutputRouter` write failure,
/// which sets the router's `failed` flag and makes `finish()` discard every
/// staged file without publishing any of them.
///
/// Nothing else is touched: the category root and the `CustomDestinations`
/// dataset's own per-user directory stay writable, so the merge post-pass
/// runs for real afterwards rather than failing on the poison itself.
fn poison_automatic_destinations_dir(collection: &std::path::Path) {
    let dir = collection.join("FileSystem/PerUser/AutomaticDestinations");
    assert!(dir.is_dir(), "expected {dir:?} from the first run");
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::write(&dir, b"not a directory").unwrap();
}

/// A rerun whose `finish()` fails publishes **nothing** -- every staged file
/// is deleted -- so the previous run's merged file sitting at the category
/// root is not this run's output and must not be reclaimed into `PerUser/`.
///
/// The caller used to narrow the router's destinations by *existence* after
/// a failed `finish()`, and a previous run's merged file exists at exactly
/// the destination a `Scope::UserElseSystem` tool's system-scope slice is
/// written to. The merge then moved it into `PerUser/` under the reclaim
/// label and folded it back in as a system slice: for CSV that carried the
/// only merged copy off the category root and then failed on the
/// `TriageUser` column the previous merge had already appended.
#[test]
fn a_failed_finish_publishes_nothing_so_the_previous_merged_csv_survives() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection_with_jump_lists(&coll);

    let out = td.path().join("out");
    let base = [
        "run",
        coll.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--layout",
        "velo",
        "--only",
        "jle",
    ];

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(base)
        .assert()
        .success();

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let stem = format!("{PINNED_STAMP}_JLETriage_results_CustomDestinations");
    let merged_path = collection.join(format!("FileSystem/{stem}.csv"));
    let first_merged = std::fs::read_to_string(&merged_path).unwrap();

    poison_automatic_destinations_dir(&collection);

    let mut second = base.to_vec();
    second.push("--overwrite");
    // The poisoned write is what makes `finish()` fail; a successful rerun
    // would mean the poison never took and the rest of this proves nothing.
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(&second)
        .assert()
        .failure();

    let per_user = collection.join("FileSystem/PerUser/CustomDestinations");
    let after = std::fs::read_to_string(&merged_path).unwrap_or_else(|e| {
        let moved: Vec<String> = std::fs::read_dir(&per_user)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        panic!(
            "the previous run's merged file must still be at {merged_path:?} \
             after a rerun that published nothing: {e} -- PerUser/ now holds \
             {moved:?}"
        )
    });
    assert_eq!(
        after, first_merged,
        "a rerun that published nothing must not rewrite the previous \
         merged file's rows"
    );
    let header = after.lines().next().unwrap_or_default();
    assert!(
        header.ends_with(",TriageUser") && header.matches(",TriageUser").count() == 1,
        "merged header must still carry exactly one TriageUser column: got {header}"
    );
}

/// NDJSON counterpart of
/// [`a_failed_finish_publishes_nothing_so_the_previous_merged_csv_survives`].
/// This is the shape that corrupts silently instead of failing: NDJSON has
/// no header to disagree with, so the previous merged file's rows were
/// re-read, relabelled `TriageUser = "system"`, and appended alongside the
/// per-user rows -- more rows, wrongly attributed, and no error anywhere.
#[test]
fn a_failed_finish_publishes_nothing_so_the_previous_merged_ndjson_survives() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection_with_jump_lists(&coll);

    let out = td.path().join("out");
    let base = [
        "run",
        coll.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--json",
        "--layout",
        "velo",
        "--only",
        "jle",
    ];

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(base)
        .assert()
        .success();

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let stem = format!("{PINNED_STAMP}_JLETriage_results_CustomDestinations");
    let merged_path = collection.join(format!("FileSystem/{stem}.json"));
    let first_merged = std::fs::read_to_string(&merged_path).unwrap();

    poison_automatic_destinations_dir(&collection);

    let mut second = base.to_vec();
    second.push("--overwrite");
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(&second)
        .assert()
        .failure();

    let after = std::fs::read_to_string(&merged_path).unwrap_or_else(|e| {
        panic!("the previous run's merged file must still be at {merged_path:?}: {e}")
    });
    let rows: Vec<serde_json::Value> = after
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let mut users: Vec<String> = rows
        .iter()
        .map(|r| {
            r.get("TriageUser")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    users.sort();
    assert_eq!(
        users,
        vec!["jdoe".to_string(), "system".to_string()],
        "one row per source, each still attributed to the source that \
         produced it -- a rerun that published nothing must not re-read the \
         previous merged file, relabel its rows system, and append them \
         again: got {after}"
    );
    assert_eq!(
        after, first_merged,
        "a rerun that published nothing must not rewrite the previous \
         merged file's rows"
    );
}

/// `write_fixture_collection_with_system_scope_lnk` plus a second real user
/// profile, `asmith`. Two users is what lets a rerun remove *one* of them
/// and still be a normal run: with a single profile, removing it removes
/// every per-user slice at once, and `merge_per_user` then declines to merge
/// at all rather than merging a stale one.
///
/// Two trailing bytes, for the same reason
/// `write_fixture_collection_with_system_scope_lnk` adds one: `dedupe_by_content`
/// (default on) would otherwise collapse this file into the jdoe or the
/// system-scope copy and the profile would never appear in the output.
fn write_fixture_collection_with_two_users_and_system_scope_lnk(coll: &std::path::Path) {
    write_fixture_collection_with_system_scope_lnk(coll);
    let recent = coll
        .join("uploads/auto/C%3A")
        .join("Users/asmith/AppData/Roaming/Microsoft/Windows/Recent");
    std::fs::create_dir_all(&recent).unwrap();
    let mut lnk = build_lnk();
    lnk.push(0);
    lnk.push(0);
    std::fs::write(recent.join("asmith.lnk"), lnk).unwrap();
}

/// Every merged CSV row's `TriageUser` value -- the last column -- sorted.
/// The fixtures below hold no quoted commas, so the last field is the last
/// comma-separated token.
fn merged_csv_users(path: &std::path::Path) -> Vec<String> {
    let body = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("expected merged file at {path:?}: {e}"));
    let mut users: Vec<String> = body
        .lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.rsplit(',').next().unwrap().to_string())
        .collect();
    users.sort();
    users
}

/// NDJSON counterpart of [`merged_csv_users`].
fn merged_ndjson_users(path: &std::path::Path) -> Vec<String> {
    let body = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("expected merged file at {path:?}: {e}"));
    let mut users: Vec<String> = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let row: serde_json::Value = serde_json::from_str(l).unwrap();
            row.get("TriageUser")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("every merged row carries TriageUser: got {l}"))
                .to_string()
        })
        .collect();
    users.sort();
    users
}

/// One `--only le` run over `coll`, writing both CSV and NDJSON into `out`
/// under [`PINNED_STAMP`]. The same stamp on both runs of the tests below is
/// the point: it is what makes the rerun land on the previous run's exact
/// filenames, which is the only way a stale slice can be mistaken for this
/// run's output.
fn run_le(coll: &std::path::Path, out: &std::path::Path, overwrite: bool) {
    let mut args = vec![
        "run",
        coll.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--json",
        "--layout",
        "velo",
        "--only",
        "le",
    ];
    if overwrite {
        args.push("--overwrite");
    }
    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(&args)
        .assert()
        .success();
}

/// An artifact class that stopped being collected must not persist into a
/// rerun's merged output.
///
/// `PerUser/` accumulates across runs: `--overwrite` replaces the slices a
/// rerun writes *again*, but the reclaimed system-scope slice of a run whose
/// system artifact is now gone is simply left behind. Selecting merge
/// sources by scanning the directory could not tell that leftover from this
/// run's own output, so it was merged back in as `TriageUser = "system"` --
/// and `write_output_hashes` then attested to a merged file carrying
/// evidence this capture does not contain.
///
/// Reproduced on `Collection-STDC1_umbralabs_dev-2026-03-11T20_27_13Z`
/// before the fix: a rerun that parsed 86 records published a merged file of
/// 97 rows, 11 of them the previous run's system-scope rows.
#[test]
fn a_removed_system_artifact_does_not_persist_into_the_reruns_merged_output() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection_with_two_users_and_system_scope_lnk(&coll);
    let out = td.path().join("out");

    run_le(&coll, &out, false);

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let stem = format!("{PINNED_STAMP}_LETriage_results");
    let merged_csv = collection.join(format!("FileSystem/{stem}.csv"));
    let merged_json = collection.join(format!("FileSystem/{stem}.json"));
    assert_eq!(
        merged_csv_users(&merged_csv),
        vec!["asmith", "jdoe", "system"],
        "the first run must merge both users and the reclaimed system slice"
    );
    assert_eq!(
        merged_ndjson_users(&merged_json),
        vec!["asmith", "jdoe", "system"],
        "the first run must merge both users and the reclaimed system slice"
    );

    // The artifact class stops being collected. Everything else is
    // unchanged, including the stamp, so the rerun writes the same filenames.
    std::fs::remove_dir_all(coll.join("uploads/auto/C%3A/ProgramData")).unwrap();
    run_le(&coll, &out, true);

    assert_eq!(
        merged_csv_users(&merged_csv),
        vec!["asmith", "jdoe"],
        "the previous run's system-scope rows must not survive into a run \
         whose capture no longer holds that artifact"
    );
    assert_eq!(
        merged_ndjson_users(&merged_json),
        vec!["asmith", "jdoe"],
        "the previous run's system-scope rows must not survive into a run \
         whose capture no longer holds that artifact"
    );
}

/// NDJSON and CSV counterpart of the above for a *user* removed from the
/// host between runs: `jdoe`'s slice is never rewritten by the rerun, so the
/// previous run's copy is still sitting in `PerUser/` under jdoe's name. It
/// must not be merged, under that identity or any other.
///
/// Reproduced on `Collection-STDC1_umbralabs_dev-2026-03-11T20_27_13Z`
/// before the fix: a rerun that parsed 95 records published a merged file of
/// 97 rows, the extra two attributed to a profile the capture no longer had.
#[test]
fn a_removed_user_does_not_persist_into_the_reruns_merged_output() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection_with_two_users_and_system_scope_lnk(&coll);
    let out = td.path().join("out");

    run_le(&coll, &out, false);

    let collection = out.join(format!("Processed-WS01-{PINNED_STAMP}"));
    let stem = format!("{PINNED_STAMP}_LETriage_results");
    let merged_csv = collection.join(format!("FileSystem/{stem}.csv"));
    let merged_json = collection.join(format!("FileSystem/{stem}.json"));
    assert_eq!(
        merged_csv_users(&merged_csv),
        vec!["asmith", "jdoe", "system"],
        "the first run must merge both users and the reclaimed system slice"
    );

    std::fs::remove_dir_all(coll.join("uploads/auto/C%3A/Users/jdoe")).unwrap();
    run_le(&coll, &out, true);

    // The system slice is still expected: the reclaim writes it *during*
    // this merge, so it is absent from the router's published set by
    // construction and must be admitted on its own terms.
    assert_eq!(
        merged_csv_users(&merged_csv),
        vec!["asmith", "system"],
        "a profile the capture no longer holds must not reappear from a \
         leftover slice"
    );
    assert_eq!(
        merged_ndjson_users(&merged_json),
        vec!["asmith", "system"],
        "a profile the capture no longer holds must not reappear from a \
         leftover slice"
    );
}

/// The per-collection output-compat writes (the VeloResults copy, the source
/// hash log, the SysInfo report, the sessions, the output hash walk) used to
/// call `die` on failure, which exits before the manifest is written. Over a
/// reused `--out` that left the *previous* run's successful
/// `run_manifest.json` standing beside this run's partial output -- a success
/// record for a run that failed.
///
/// Forced through the CLI by replacing `CaseInfo/<stamp>_OutputHashes.txt`
/// with a directory of the same name, which `write_output_hashes` cannot
/// write over. The run must still exit 4 *and* leave a manifest describing
/// this run.
#[test]
fn a_failed_output_compat_write_still_replaces_the_previous_runs_manifest() {
    let td = TempDir::new().unwrap();
    let coll = td.path().join("Collection-WS01-2026");
    write_fixture_collection(&coll);
    let out = td.path().join("out");
    let args = [
        "run",
        coll.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--csv",
        "--overwrite",
        "--only",
        "pe,le",
    ];

    Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(args)
        .assert()
        .success();
    let first: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("run_manifest.json")).unwrap())
            .unwrap();
    assert_eq!(first["final_exit_status"], 0);

    let hashes = out
        .join(format!("Processed-WS01-{PINNED_STAMP}"))
        .join("CaseInfo")
        .join(format!("{PINNED_STAMP}_OutputHashes.txt"));
    std::fs::remove_file(&hashes).unwrap();
    std::fs::create_dir(&hashes).unwrap();

    let o = Command::cargo_bin("TriageSuite")
        .unwrap()
        .env("TRIAGE_RUN_STAMP", PINNED_STAMP)
        .args(args)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(4),
        "an output failure must still exit 4: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    let second: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out.join("run_manifest.json"))
            .expect("a failed run must still leave a manifest"),
    )
    .unwrap();
    assert_eq!(
        second["final_exit_status"], 4,
        "run_manifest.json still describes the previous successful run"
    );
    assert_ne!(second["run_id"], first["run_id"]);
    // The host still ran: this is a failure of the closing output-compat
    // step, not of the parsing, and the manifest must say what was produced.
    assert_eq!(second["hosts"][0]["host"], "WS01");
    // And it must say *which* host's output is incomplete, and why. Console
    // output is transient; the manifest is the record. Exit 4 over a
    // normal-looking host entry would name the run but not the collection.
    let errors = second["hosts"][0]["output_errors"]
        .as_array()
        .unwrap_or_else(|| panic!("no output_errors on {:?}", second["hosts"][0]));
    assert!(
        errors.iter().any(|e| e
            .as_str()
            .unwrap_or_default()
            .contains("cannot write output hashes")),
        "the failure must be attributed to this host: {errors:?}"
    );
    // A healthy run says nothing, rather than carrying an empty array.
    assert!(
        first["hosts"][0].get("output_errors").is_none(),
        "output_errors must be omitted when empty: {:?}",
        first["hosts"][0]
    );
}
