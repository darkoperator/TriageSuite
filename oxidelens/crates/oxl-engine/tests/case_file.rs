//! The case file: creation, reopening, the refusals around both, and the
//! primitives (actor text, timestamp text) every later write depends on.

mod common;

use common::Holder;
use oxl_engine::{load_file, parse_utc_text, query_sql, sources, Actor, Case, FileOptions};

fn code<T>(result: oxl_engine::Result<T>) -> &'static str {
    match result {
        Ok(_) => "ok",
        Err(e) => e.code(),
    }
}

/// Found on the second real run: removing a 900 MB source left the case
/// file at 2 GB, because DuckDB keeps a file's freed blocks for reuse and
/// never gives them back. The size and the free part are reported, and
/// `compact_to` writes a copy without the free part, the original left as
/// it is. The copy carries on where the case was: its ids continue, so a
/// removed source's row ids are never handed out again.
#[test]
fn a_case_reports_its_free_space_and_compacts_into_a_copy() {
    use oxl_engine::{
        add_note, load_run, mark_suspicious, relations, remove_source, row_place, RowStanding,
    };
    let d = tempfile::tempdir().unwrap();
    let analyst = Actor::Analyst("tester".into());
    let p = d.path().join("a.case.duckdb");
    let mut case = Case::create(&p, "IR-1", &analyst).unwrap();
    let first = load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("one")),
        &analyst,
    )
    .unwrap()
    .source_id;
    let marked = case
        .scalar_i64("SELECT min(_ox_row) FROM evtxtriage_events")
        .unwrap();
    assert!(mark_suspicious(&mut case, &[marked], &analyst).unwrap()[0].done);
    add_note(&mut case, marked, "kept", &analyst).unwrap();
    let loose = load_file(
        &mut case,
        &oxl_fixtures::loose_fw_csv(d.path()),
        FileOptions::default(),
        &analyst,
    )
    .unwrap();
    let loose_last = case.scalar_i64("SELECT max(_ox_row) FROM fw_csv").unwrap();
    remove_source(&mut case, loose.source_id, true, &analyst).unwrap();
    let size = case.size().unwrap();
    assert!(size.bytes > 0 && size.free_bytes <= size.bytes, "{size:?}");

    let out = d.path().join("b.case.duckdb");
    let report = case.compact_to(&out).unwrap();
    assert_eq!(report.bytes_before, std::fs::metadata(&p).unwrap().len());
    assert_eq!(report.bytes_after, std::fs::metadata(&out).unwrap().len());
    let rows = |c: &Case| -> Vec<(String, u64)> {
        relations(c)
            .unwrap()
            .into_iter()
            .map(|r| (r.name, r.rows))
            .collect()
    };
    let history = |c: &Case| c.scalar_i64("SELECT count(*) FROM ox.history").unwrap();
    let (rows_before, history_before) = (rows(&case), history(&case));
    drop(case);

    let mut copy = Case::open(&out).unwrap();
    let info = copy.info().unwrap();
    assert_eq!(
        (info.name.as_str(), info.path.as_path()),
        ("IR-1", out.as_path())
    );
    assert_eq!(rows(&copy), rows_before);
    assert_eq!(history(&copy), history_before);
    assert_eq!(
        copy.scalar_i64(&format!(
            "SELECT count(*) FROM ox.row_mark WHERE row_id = {marked}"
        ))
        .unwrap(),
        1
    );
    // The removed source's place is kept, and its ids are not reused.
    assert_eq!(
        row_place(&copy, loose_last).unwrap().unwrap().standing,
        RowStanding::Removed
    );
    let again = load_run(
        &mut copy,
        &oxl_fixtures::rerun_ws01_prefetch(&d.path().join("two")),
        &analyst,
    )
    .unwrap();
    let newest = copy
        .scalar_i64(&format!(
            "SELECT min(first_row) FROM ox.source_table WHERE source_id = {}",
            again.source_id
        ))
        .unwrap();
    assert!(newest > loose_last, "{newest} <= {loose_last}");
    assert!(again.source_id > first);
    // The original is untouched and still opens.
    drop(copy);
    assert_eq!(Case::open(&p).unwrap().info().unwrap().name, "IR-1");
}

/// Reviewed: DuckDB names a database after its file and compares names
/// without regard to case, so a case called like compaction's own name for
/// the copy still compacts.
#[test]
fn a_case_named_like_the_copy_still_compacts() {
    let d = tempfile::tempdir().unwrap();
    let analyst = Actor::Analyst("tester".into());
    let case = Case::create(&d.path().join("Ox_Compact.duckdb"), "IR-1", &analyst).unwrap();
    let out = d.path().join("copy.case.duckdb");
    case.compact_to(&out).unwrap();
    assert_eq!(Case::open(&out).unwrap().info().unwrap().name, "IR-1");
}

#[test]
fn compacting_never_overwrites_a_file() {
    let d = tempfile::tempdir().unwrap();
    let analyst = Actor::Analyst("tester".into());
    let case = Case::create(&d.path().join("a.case.duckdb"), "IR-1", &analyst).unwrap();
    let taken = d.path().join("taken.duckdb");
    std::fs::write(&taken, b"keep me").unwrap();
    assert_eq!(code(case.compact_to(&taken)), "invalid");
    assert_eq!(std::fs::read(&taken).unwrap(), b"keep me");
    assert_eq!(code(case.compact_to(&d.path().join("copy.txt"))), "invalid");
    assert!(!d.path().join("copy.txt").exists());
    // A log beside the name, which DuckDB would replay into the copy.
    let logged = d.path().join("logged.case.duckdb");
    std::fs::write(d.path().join("logged.case.duckdb.wal"), b"not ours").unwrap();
    assert_eq!(code(case.compact_to(&logged)), "invalid");
    assert!(!logged.exists());
    assert_eq!(
        std::fs::read(d.path().join("logged.case.duckdb.wal")).unwrap(),
        b"not ours"
    );
    // A copy that works leaves nothing beside it but itself (the open
    // case's own log aside).
    case.compact_to(&d.path().join("fine.case.duckdb")).unwrap();
    let mut names: Vec<String> = std::fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "a.case.duckdb.wal")
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "a.case.duckdb",
            "fine.case.duckdb",
            "logged.case.duckdb.wal",
            "taken.duckdb"
        ]
    );
}

#[test]
fn create_then_open_round_trips_info() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.case.duckdb");
    drop(Case::create(&p, "IR-1", &Actor::Analyst("tester".into())).unwrap());
    let info = Case::open(&p).unwrap().info().unwrap();
    assert_eq!(
        (info.name.as_str(), info.format, info.created_by.as_str()),
        ("IR-1", 2, "analyst:tester")
    );
    assert_eq!(info.path, p);
    assert!(
        parse_utc_text(&info.created_at).as_deref() == Some(info.created_at.as_str()),
        "created_at is not the canonical 6-digit Z form: {}",
        info.created_at
    );
}

#[test]
fn create_refuses_an_existing_file_and_a_wrong_suffix() {
    let d = tempfile::tempdir().unwrap();
    let actor = Actor::Analyst("tester".into());

    let existing = d.path().join("taken.duckdb");
    std::fs::write(&existing, b"not a case").unwrap();
    assert_eq!(code(Case::create(&existing, "IR-1", &actor)), "invalid");
    // The refusal must not have touched the file that was already there.
    assert_eq!(std::fs::read(&existing).unwrap(), b"not a case");

    let wrong = d.path().join("a.case.db");
    assert_eq!(code(Case::create(&wrong, "IR-1", &actor)), "invalid");
    assert!(!wrong.exists());
}

#[test]
fn open_missing_is_not_found() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("absent.case.duckdb");
    assert_eq!(code(Case::open(&p)), "not_found");
    assert!(
        !p.exists(),
        "open must not create the file it was asked for"
    );
}

#[test]
fn open_while_open_elsewhere_is_case_locked() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("held.case.duckdb");
    drop(Case::create(&p, "IR-1", &Actor::Analyst("tester".into())).unwrap());

    let holder = Holder::start(&p);
    let outcome = code(Case::open(&p));
    holder.release();
    assert_eq!(outcome, "case_locked");
}

/// Found on the first real run: every command opened the case for writing,
/// so two commands that only read could not run at once.
#[test]
fn a_case_being_read_elsewhere_can_be_read_but_not_written() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("read.case.duckdb");
    drop(Case::create(&p, "IR-1", &Actor::Analyst("tester".into())).unwrap());

    let holder = Holder::start_reading(&p);
    let name = Case::open_read_only(&p)
        .and_then(|case| case.info())
        .map(|info| info.name);
    let query = query_sql(&p, "select 1", 10, 0).map(|_| ());
    let write = code(Case::open(&p));
    holder.release();
    assert_eq!(name.unwrap(), "IR-1");
    query.unwrap();
    assert_eq!(write, "case_locked");
}

#[test]
fn a_case_open_for_writing_elsewhere_cannot_be_read() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("held.case.duckdb");
    drop(Case::create(&p, "IR-1", &Actor::Analyst("tester".into())).unwrap());

    let holder = Holder::start(&p);
    let outcome = code(Case::open_read_only(&p));
    holder.release();
    assert_eq!(outcome, "case_locked");
}

#[test]
fn a_case_opened_for_reading_takes_no_write() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("ro.case.duckdb");
    let actor = Actor::Analyst("tester".into());
    drop(Case::create(&p, "IR-1", &actor).unwrap());
    let fw = oxl_fixtures::loose_fw_csv(d.path());

    let mut case = Case::open_read_only(&p).unwrap();
    assert!(load_file(&mut case, &fw, FileOptions::default(), &actor).is_err());
    drop(case);
    assert!(sources(&Case::open(&p).unwrap()).unwrap().is_empty());
}

/// A process that died after a commit leaves its write-ahead log beside the
/// case. Opening for reading must still show what was committed.
#[test]
fn a_case_left_with_a_write_ahead_log_can_be_read() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("wal.case.duckdb");
    drop(Case::create(&p, "IR-1", &Actor::Analyst("tester".into())).unwrap());
    {
        let conn = duckdb::Connection::open(&p).unwrap();
        conn.execute_batch(
            "PRAGMA disable_checkpoint_on_shutdown; \
             UPDATE ox.meta SET value = 'IR-2' WHERE key = 'name';",
        )
        .unwrap();
    }
    let mut wal = p.clone().into_os_string();
    wal.push(".wal");
    assert!(
        std::path::Path::new(&wal).exists(),
        "the setup left no write-ahead log to read through"
    );

    let info = Case::open_read_only(&p).unwrap().info().unwrap();
    assert_eq!(info.name, "IR-2");
}

#[test]
fn a_newer_format_needs_upgrade() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("future.case.duckdb");
    drop(Case::create(&p, "IR-1", &Actor::Analyst("tester".into())).unwrap());
    {
        let conn = duckdb::Connection::open(&p).unwrap();
        conn.execute("UPDATE ox.meta SET value = '3' WHERE key = 'format'", [])
            .unwrap();
    }
    match Case::open(&p) {
        Err(e) => {
            assert_eq!(e.code(), "needs_upgrade");
            assert!(matches!(
                e,
                oxl_engine::EngineError::NeedsUpgrade {
                    found: 3,
                    expected: 2
                }
            ));
        }
        Ok(_) => panic!("a format-3 case opened under format 2"),
    }
}

/// A case that breaks an invariant the engine relies on is not the caller's
/// mistake: no change to the input fixes it. It has its own code,
/// `internal`, so a caller can tell "fix what you asked" (`invalid`) from
/// "the case or the engine is broken".
#[test]
fn internal_faults_have_their_own_code() {
    use oxl_engine::{columns, marks, page, relations, GridQuery};
    let d = tempfile::tempdir().unwrap();
    let analyst = Actor::Analyst("tester".into());
    let p = d.path().join("c.case.duckdb");
    let mut case = Case::create(&p, "IR-1", &analyst).unwrap();
    let fw = oxl_fixtures::loose_fw_csv(d.path());
    load_file(&mut case, &fw, FileOptions::default(), &analyst).unwrap();
    let row = case.scalar_i64("SELECT min(_ox_row) FROM fw_csv").unwrap();
    drop(case);
    let corrupt = |sql: &str| {
        let conn = duckdb::Connection::open(&p).unwrap();
        conn.execute_batch(sql).unwrap();
    };
    let all = GridQuery {
        relation: "fw_csv".into(),
        ..Default::default()
    };

    corrupt("UPDATE ox.column_catalog SET origin = 'bogus' WHERE name = 'time'");
    let case = Case::open(&p).unwrap();
    assert_eq!(code(columns(&case, "fw_csv")), "internal");
    drop(case);
    corrupt("UPDATE ox.column_catalog SET origin = 'sniffed' WHERE name = 'time'");

    corrupt("UPDATE ox.column_catalog SET role = 'bogus' WHERE name = 'time'");
    let case = Case::open(&p).unwrap();
    assert_eq!(code(columns(&case, "fw_csv")), "internal");
    drop(case);
    corrupt("UPDATE ox.column_catalog SET role = 'data' WHERE name = 'time'");

    corrupt("UPDATE ox.relation SET kind = 'bogus'");
    let case = Case::open(&p).unwrap();
    assert_eq!(code(relations(&case)), "internal");
    drop(case);
    corrupt("UPDATE ox.relation SET kind = 'file'");

    // A mark the engine never writes.
    corrupt(&format!(
        "INSERT INTO ox.row_mark VALUES ({row}, 'bogus', 'analyst:tester', \
         '2026-01-01T00:00:00.000000Z')"
    ));
    let case = Case::open(&p).unwrap();
    assert_eq!(code(page(&case, &all, 50, 0)), "internal");
    assert_eq!(code(marks(&case, None, 50, 0)), "internal");
    drop(case);

    // And an input mistake is still `invalid`.
    let case = Case::open(&p).unwrap();
    assert_eq!(code(page(&case, &all, 0, 0)), "invalid");
}

#[test]
fn a_duckdb_file_that_is_not_a_case_is_invalid() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("plain.duckdb");
    drop(duckdb::Connection::open(&p).unwrap());
    assert_eq!(code(Case::open(&p)), "invalid");
}

#[test]
fn every_ox_table_and_column_has_a_comment() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("c.case.duckdb");
    let case = Case::create(&p, "IR-1", &Actor::Analyst("tester".into())).unwrap();

    let tables = case
        .scalar_i64("SELECT count(*) FROM duckdb_tables() WHERE schema_name = 'ox'")
        .unwrap();
    assert_eq!(tables, 17, "schema ox should hold the 17 format-2 tables");
    let sequences = case
        .scalar_i64("SELECT count(*) FROM duckdb_sequences() WHERE schema_name = 'ox'")
        .unwrap();
    assert_eq!(sequences, 5);

    let bare_tables = case
        .scalar_i64(
            "SELECT count(*) FROM duckdb_tables() \
             WHERE schema_name = 'ox' AND (comment IS NULL OR comment = '')",
        )
        .unwrap();
    assert_eq!(bare_tables, 0, "an ox table has no COMMENT");
    let bare_columns = case
        .scalar_i64(
            "SELECT count(*) FROM duckdb_columns() \
             WHERE schema_name = 'ox' AND (comment IS NULL OR comment = '')",
        )
        .unwrap();
    assert_eq!(bare_columns, 0, "an ox column has no COMMENT");
}

#[test]
fn actor_parse_is_total() {
    let long = "a".repeat(100_000);
    let too_long = format!("agent:{}", "a".repeat(65));
    for bad in [
        "",
        "analyst:",
        "agent:a b",
        "root:x",
        "agent:\u{0}",
        long.as_str(),
        too_long.as_str(),
        "agent",
        ":x",
        "Agent:x",
        "agent:é",
        "agent:x:y",
    ] {
        assert!(Actor::parse(bad).is_err(), "accepted {bad:?}");
        assert_eq!(code(Actor::parse(bad)), "invalid");
    }
    let claude = Actor::parse("agent:claude").unwrap();
    assert_eq!(claude.label(), "agent:claude");
    assert!(claude.is_agent());
    let max = format!("analyst:{}", "Z".repeat(64));
    let analyst = Actor::parse(&max).unwrap();
    assert_eq!(analyst.label(), max);
    assert!(!analyst.is_agent());
    assert_eq!(
        Actor::parse("analyst:j.doe-2_x").unwrap(),
        Actor::Analyst("j.doe-2_x".into())
    );
}

#[test]
fn parse_utc_text_accepts_only_the_z_form() {
    assert_eq!(
        parse_utc_text("2026-01-05T02:13:58.2040000Z").as_deref(),
        Some("2026-01-05T02:13:58.204000Z")
    );
    assert_eq!(
        parse_utc_text("2026-01-05T02:13:58Z").as_deref(),
        Some("2026-01-05T02:13:58.000000Z")
    );
    assert_eq!(
        parse_utc_text("2024-02-29T23:59:59.5Z").as_deref(),
        Some("2024-02-29T23:59:59.500000Z")
    );
    for bad in [
        "2026-01-05 02:13:58",
        "2026-01-05T02:13:58+01:00",
        "2026-13-40T99:99:99Z",
        "",
        "Z",
        "2026-01-05T02:13:58.Z",
        "2026-01-05T02:13:58.12345678Z",
        "2026-01-05T02:13:58z",
        "2026-02-29T00:00:00Z",
        "2026-04-31T00:00:00Z",
        "2026-00-10T00:00:00Z",
        "2026-01-00T00:00:00Z",
        "2026-01-05T24:00:00Z",
        "2026-01-05T23:60:00Z",
        "2026-01-05T23:59:60Z",
        "0000-01-01T00:00:00Z",
        "+026-01-05T02:13:58Z",
        "2026-01-05T02:13:58Z ",
        "２026-01-05T02:13:58Z",
        "2026-01-05T02:13:58.20400é0Z",
    ] {
        assert!(parse_utc_text(bad).is_none(), "accepted {bad:?}");
    }
}
