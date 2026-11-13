//! Case format 2: a step 1 case (format 1) is upgraded in place when it is
//! opened to change it, and still answers every step 1 read when it is
//! opened read-only; reader clones of an open case; the display zone.
//!
//! A format-1 case is built from the format-1 DDL exactly as step 1 shipped
//! it (`fixtures/schema_v1.sql`, `2f6aa9b`), then loaded and annotated
//! through the engine.

mod common;

use common::{agent, analyst, n};
use oxl_engine::{
    add_note, add_tag, display_zone, find, groups, history, load_run, mark_counts, mark_suspicious,
    marks, page, sessions, set_display_zone, Case, FindMode, FindRequest, GridQuery, CASE_FORMAT,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const SCHEMA_V1: &str = include_str!("fixtures/schema_v1.sql");

const SVCH0ST_WS01: &str = "SELECT _ox_row FROM petriage_timeline \
                            WHERE ExecutableName = 'SVCH0ST.EXE' AND _triage_host = 'WS01'";
const CHROME_SRV01: &str = "SELECT _ox_row FROM petriage_timeline \
                            WHERE ExecutableName = 'CHROME.EXE' AND _triage_host = 'SRV01'";
const RCLONE_EVTX: &str = "SELECT _ox_row FROM evtxtriage_events \
                           WHERE PayloadData1 = 'rclone.exe'";

/// The tables whose every row the upgrade must leave as it was.
const KEPT: [&str; 6] = [
    "row_mark",
    "row_tag",
    "note",
    "source",
    "source_dataset",
    "relation",
];

/// A step 1 case at `dir/v1.case.duckdb`, at format 1: the two-host run
/// loaded, rows marked, tagged and noted, and WS01's prefetch superseded by
/// a later run. Closed again; returns its path.
fn step1_case(dir: &Path) -> PathBuf {
    let path = dir.join("v1.case.duckdb");
    let mut case = Case::create_at_format_1(&path, SCHEMA_V1, "IR-1", &analyst()).unwrap();
    assert_eq!(case.format(), 1);
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&dir.join("a")),
        &analyst(),
    )
    .unwrap();
    let moving = n(&case, SVCH0ST_WS01);
    let staying = n(&case, CHROME_SRV01);
    let event = n(&case, RCLONE_EVTX);
    for row in [moving, staying, event] {
        assert!(mark_suspicious(&mut case, &[row], &analyst()).unwrap()[0].done);
    }
    assert!(add_tag(&mut case, &[moving, event], "exfil", &analyst()).unwrap()[0].done);
    add_note(&mut case, moving, "renamed svchost", &analyst()).unwrap();
    add_note(&mut case, event, "rclone to a share", &agent()).unwrap();
    let report = load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_prefetch(&dir.join("b")),
        &analyst(),
    )
    .unwrap();
    assert!(
        !report.superseded.is_empty(),
        "the rerun superseded nothing"
    );
    path
}

/// Every row of `ox.<table>`, as text, in a stable order.
fn dump(conn: &duckdb::Connection, table: &str) -> Vec<String> {
    conn.prepare(&format!(
        "SELECT CAST(t AS VARCHAR) FROM ox.{table} AS t ORDER BY 1"
    ))
    .unwrap()
    .query_map([], |r| r.get(0))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

/// The file at `path` read with DuckDB alone: each kept table's rows.
fn dumps(path: &Path) -> Vec<(String, Vec<String>)> {
    let conn = duckdb::Connection::open(path).unwrap();
    KEPT.iter()
        .map(|t| (t.to_string(), dump(&conn, t)))
        .collect()
}

/// A scalar of the file at `path`, read with DuckDB alone.
fn raw_text(path: &Path, sql: &str) -> Option<String> {
    let conn = duckdb::Connection::open(path).unwrap();
    conn.query_row(sql, [], |r| r.get(0)).ok()
}

/// Every table, view and sequence in schema `ox`, by kind and name.
fn ox_objects(path: &Path) -> Vec<String> {
    let conn = duckdb::Connection::open(path).unwrap();
    conn.prepare(
        "SELECT 'table ' || table_name FROM duckdb_tables() WHERE schema_name = 'ox' \
         UNION ALL SELECT 'view ' || view_name FROM duckdb_views() WHERE schema_name = 'ox' \
         UNION ALL SELECT 'sequence ' || sequence_name FROM duckdb_sequences() \
           WHERE schema_name = 'ox' \
         ORDER BY 1",
    )
    .unwrap()
    .query_map([], |r| r.get(0))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

fn is_case_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[test]
fn the_build_writes_format_2() {
    assert_eq!(CASE_FORMAT, 2);
    let d = tempfile::tempdir().unwrap();
    let case = oxl_fixtures::new_case(d.path());
    assert_eq!(case.format(), 2);
    assert_eq!(case.info().unwrap().format, 2);
    assert!(is_case_id(&case.case_id().unwrap()));
}

#[test]
fn a_step1_case_upgrades_in_place() {
    let d = tempfile::tempdir().unwrap();
    let path = step1_case(d.path());
    let before = dumps(&path);
    assert!(before.iter().all(|(_, rows)| !rows.is_empty()));
    let history_before = raw_text(&path, "SELECT CAST(count(*) AS VARCHAR) FROM ox.history");
    assert_eq!(
        raw_text(&path, "SELECT value FROM ox.meta WHERE key = 'format'").as_deref(),
        Some("1")
    );

    let case = Case::open(&path).unwrap();
    assert_eq!(case.format(), 2);
    assert_eq!(case.info().unwrap().format, 2);
    let id = case.case_id().unwrap();
    assert!(is_case_id(&id), "{id:?}");
    // The run's session was read from where the run is.
    let found = sessions(&case).unwrap();
    assert_eq!(
        found.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["Execution_Analysis"]
    );
    let upgrades = history(&case, Some("case"), 50, 0).unwrap().items;
    assert_eq!(upgrades.len(), 1, "{upgrades:?}");
    let upgrade = &upgrades[0];
    assert_eq!(
        (upgrade.action.as_str(), upgrade.target_id.as_str()),
        ("case_upgrade", id.as_str())
    );
    assert_eq!(upgrade.before, Some(serde_json::json!({"format": 1})));
    assert_eq!(upgrade.after, Some(serde_json::json!({"format": 2})));
    drop(case);

    assert_eq!(dumps(&path), before);
    let history_after = raw_text(&path, "SELECT CAST(count(*) AS VARCHAR) FROM ox.history");
    assert_eq!(
        history_after.unwrap().parse::<i64>().unwrap(),
        history_before.unwrap().parse::<i64>().unwrap() + 1
    );
    // Opened again: already format 2, nothing more is done, and the id stays.
    let case = Case::open(&path).unwrap();
    assert_eq!(case.case_id().unwrap(), id);
    assert_eq!(history(&case, Some("case"), 50, 0).unwrap().items.len(), 1);
}

/// What the case at its present state answers to the step 1 reads.
fn step1_answers(case: &Case) -> serde_json::Value {
    let events = GridQuery {
        relation: "evtxtriage_events".into(),
        ..Default::default()
    };
    let prefetch = GridQuery {
        relation: "petriage_timeline".into(),
        ..Default::default()
    };
    let found = find(
        case,
        &FindRequest {
            value: "svc_backup".into(),
            mode: FindMode::Contains,
            hosts: Vec::new(),
            from: None,
            to: None,
        },
        &mut |_, _| true,
    )
    .unwrap();
    serde_json::json!({
        "events": page(case, &events, 50, 0).unwrap(),
        "prefetch": page(case, &prefetch, 50, 0).unwrap(),
        "groups": groups(case, &events, &["Channel".to_string()], &[], 50, 0).unwrap(),
        "find": found,
        "marks": marks(case, None, 50, 0).unwrap(),
    })
}

#[test]
fn read_only_open_of_format_1_answers_step1_queries() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("v1.case.duckdb");
    let mut case = Case::create_at_format_1(&path, SCHEMA_V1, "IR-1", &analyst()).unwrap();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    let row = n(&case, SVCH0ST_WS01);
    assert!(mark_suspicious(&mut case, &[row], &analyst()).unwrap()[0].done);
    let expected = step1_answers(&case);
    drop(case);

    let case = Case::open_read_only(&path).unwrap();
    assert_eq!(case.format(), 1);
    assert_eq!(case.case_id().unwrap(), "");
    assert_eq!(step1_answers(&case), expected);
    assert_eq!(display_zone(&case).unwrap(), "UTC");
    assert!(sessions(&case).unwrap().is_empty());
    assert_eq!(mark_counts(&case).unwrap().suspicious, 1);
    // A clone reads as the case does.
    let clone = case.try_clone().unwrap();
    assert_eq!(clone.format(), 1);
    assert_eq!(step1_answers(&clone), expected);
    drop((clone, case));
    // Reading changed nothing.
    assert_eq!(
        raw_text(&path, "SELECT value FROM ox.meta WHERE key = 'format'").as_deref(),
        Some("1")
    );
}

#[test]
fn a_failed_upgrade_leaves_format_1() {
    let d = tempfile::tempdir().unwrap();
    let path = step1_case(d.path());
    {
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE VIEW ox.setting AS SELECT 1 AS key")
            .unwrap();
    }
    let before = dumps(&path);
    let objects = ox_objects(&path);
    let history_before = raw_text(&path, "SELECT CAST(count(*) AS VARCHAR) FROM ox.history");

    let e = Case::open(&path).err().expect("the upgrade succeeded");
    assert_eq!(e.code(), "engine", "{e}");

    let case = Case::open_read_only(&path).unwrap();
    assert_eq!(case.format(), 1);
    assert_eq!(case.info().unwrap().format, 1);
    drop(case);
    assert_eq!(dumps(&path), before);
    // Nothing the upgrade created before it failed is left, not even the
    // tables and the sequence it made first.
    assert_eq!(ox_objects(&path), objects);
    assert!(!objects
        .iter()
        .any(|o| o.contains("saved_view") || o.contains("view_seq")));
    assert_eq!(
        raw_text(&path, "SELECT CAST(count(*) AS VARCHAR) FROM ox.history"),
        history_before
    );
    assert_eq!(
        raw_text(&path, "SELECT value FROM ox.meta WHERE key = 'case_id'"),
        None
    );
}

/// Each `ox` table and sequence as DDL would make it: columns with their
/// types, nullability and defaults, constraints, and every comment. Not a
/// sequence's start: DuckDB reports a used sequence's next value there, and
/// the upgrade itself takes a history id.
fn catalog(path: &Path) -> Vec<String> {
    let conn = duckdb::Connection::open(path).unwrap();
    conn.prepare(
        "SELECT 'table ' || table_name || ': ' || coalesce(comment, '') \
           FROM duckdb_tables() WHERE schema_name = 'ox' \
         UNION ALL \
         SELECT 'column ' || table_name || '.' || column_name || ' ' || data_type || \
                CASE WHEN is_nullable THEN '' ELSE ' NOT NULL' END || \
                ' default ' || coalesce(column_default, '-') || ': ' || coalesce(comment, '') \
           FROM duckdb_columns() WHERE schema_name = 'ox' \
         UNION ALL \
         SELECT 'constraint ' || table_name || ' ' || constraint_type || ' ' || \
                CAST(constraint_column_names AS VARCHAR) \
           FROM duckdb_constraints() WHERE schema_name = 'ox' \
         UNION ALL \
         SELECT 'sequence ' || sequence_name || ' by ' || CAST(increment_by AS VARCHAR) \
           FROM duckdb_sequences() WHERE schema_name = 'ox' \
         ORDER BY 1",
    )
    .unwrap()
    .query_map([], |r| r.get(0))
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

/// The upgrade and a new case's DDL are written apart; they must make the
/// same tables, so an upgraded case is a format-2 case like any other.
#[test]
fn an_upgraded_case_has_the_tables_of_a_new_one() {
    let d = tempfile::tempdir().unwrap();
    let fresh = d.path().join("new.case.duckdb");
    drop(Case::create(&fresh, "IR-1", &analyst()).unwrap());
    let upgraded = d.path().join("old.case.duckdb");
    drop(Case::create_at_format_1(&upgraded, SCHEMA_V1, "IR-1", &analyst()).unwrap());
    drop(Case::open(&upgraded).unwrap());
    let expected = catalog(&fresh);
    assert!(expected.iter().any(|l| l.starts_with("table saved_view: ")));
    assert_eq!(catalog(&upgraded), expected);
    // And every one of them has a comment.
    for line in &expected {
        if line.starts_with("table ") || line.starts_with("column ") {
            assert!(!line.ends_with(": "), "no COMMENT: {line}");
        }
    }
}

#[test]
fn a_newer_format_still_needs_upgrade() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("future.case.duckdb");
    drop(Case::create(&path, "IR-1", &analyst()).unwrap());
    {
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute("UPDATE ox.meta SET value = '3' WHERE key = 'format'", [])
            .unwrap();
    }
    for opened in [Case::open(&path), Case::open_read_only(&path)] {
        assert!(matches!(
            opened.err(),
            Some(oxl_engine::EngineError::NeedsUpgrade {
                found: 3,
                expected: 2
            })
        ));
    }
}

#[test]
fn clones_share_the_database() {
    let (_d, mut case) = common::loaded();
    let c2 = case.try_clone().unwrap();
    assert_eq!(c2.format(), 2);
    assert_eq!(c2.case_id().unwrap(), case.case_id().unwrap());
    assert_eq!(c2.path(), case.path());
    let row = n(&case, SVCH0ST_WS01);
    assert!(mark_suspicious(&mut case, &[row], &analyst()).unwrap()[0].done);
    let prefetch = GridQuery {
        relation: "petriage_timeline".into(),
        ..Default::default()
    };
    let seen = page(&c2, &prefetch, 50, 0).unwrap();
    let at = seen.row_ids.iter().position(|&r| r == row).unwrap();
    assert!(seen.marks[at].is_some(), "the clone does not see the mark");

    // The clone pages on another thread while the case writes.
    let (done, finished) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut pages = 0;
        for _ in 0..25 {
            let p = page(&c2, &prefetch, 50, 0).expect("a page through the clone");
            assert_eq!(p.row_ids.len(), 5);
            pages += 1;
        }
        let _ = done.send(pages);
    });
    let other = n(&case, CHROME_SRV01);
    for _ in 0..10 {
        assert!(mark_suspicious(&mut case, &[other], &analyst()).unwrap()[0].done);
        assert!(oxl_engine::clear_suspicious(&mut case, &[other], &analyst()).unwrap()[0].done);
    }
    let pages = finished
        .recv_timeout(Duration::from_secs(120))
        .expect("the reader did not finish (or panicked)");
    assert_eq!(pages, 25);
    reader.join().unwrap();
}

#[test]
fn interrupt_stops_a_long_query() {
    let (_d, case) = common::loaded();
    let clone = case.try_clone().unwrap();
    let handle = clone.interrupt_handle();
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        // Minutes of work uninterrupted.
        let _ = done.send(clone.scalar_i64("SELECT max(range % 7) FROM range(10000000000)"));
    });
    std::thread::sleep(Duration::from_millis(100));
    let first = Instant::now();
    // As the app does: the interrupt is repeated until the statement ends,
    // in case it landed before the statement started.
    let result = loop {
        handle.interrupt();
        match finished.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => break result,
            Err(mpsc::RecvTimeoutError::Timeout) if first.elapsed() < Duration::from_secs(2) => {}
            Err(e) => panic!("the query did not stop within 2 s of the interrupt: {e}"),
        }
    };
    assert!(first.elapsed() < Duration::from_secs(2));
    let e = result.expect_err("the query finished instead of being interrupted");
    assert_eq!(e.code(), "engine");
    assert!(e.to_string().contains("INTERRUPT"), "{e}");
    // The case's own connection was not interrupted.
    assert_eq!(n(&case, "SELECT 1"), 1);
}

#[test]
fn display_zone_round_trips() {
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    assert_eq!(display_zone(&case).unwrap(), "UTC");
    set_display_zone(&mut case, "America/New_York", &analyst()).unwrap();
    assert_eq!(display_zone(&case).unwrap(), "America/New_York");
    let refused = set_display_zone(&mut case, "Europe/Paris", &agent()).unwrap_err();
    assert_eq!(refused.code(), "refused", "{refused}");
    let invalid = set_display_zone(&mut case, "Mars/Olympus", &analyst()).unwrap_err();
    assert_eq!(invalid.code(), "invalid", "{invalid}");
    // Names are exact: a zone in the wrong case is not a zone.
    let invalid = set_display_zone(&mut case, "america/new_york", &analyst()).unwrap_err();
    assert_eq!(invalid.code(), "invalid", "{invalid}");
    assert_eq!(display_zone(&case).unwrap(), "America/New_York");

    let rows = history(&case, Some("case"), 50, 0).unwrap().items;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(
        (
            row.action.as_str(),
            row.target_kind.as_str(),
            row.by.as_str()
        ),
        ("display_zone_set", "case", "analyst:tester")
    );
    assert_eq!(row.target_id, case.case_id().unwrap());
    assert_eq!(row.before, Some(serde_json::json!({"display_zone": "UTC"})));
    assert_eq!(
        row.after,
        Some(serde_json::json!({"display_zone": "America/New_York"}))
    );
    // Setting what is already set changes nothing and records nothing.
    set_display_zone(&mut case, "America/New_York", &analyst()).unwrap();
    assert_eq!(history(&case, Some("case"), 50, 0).unwrap().items.len(), 1);

    // It is the case's: another opening of the file reads it.
    let path = case.path().to_path_buf();
    drop(case);
    let case = Case::open_read_only(&path).unwrap();
    assert_eq!(display_zone(&case).unwrap(), "America/New_York");
}

#[test]
fn a_display_zone_the_engine_never_wrote_is_internal() {
    let d = tempfile::tempdir().unwrap();
    let path = oxl_fixtures::new_case(d.path()).path().to_path_buf();
    {
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO ox.setting VALUES \
             ('display_zone', 'Mars/Olympus', 'analyst:x', '2026-01-01T00:00:00.000000Z')",
        )
        .unwrap();
    }
    let case = Case::open_read_only(&path).unwrap();
    assert_eq!(display_zone(&case).unwrap_err().code(), "internal");
}
