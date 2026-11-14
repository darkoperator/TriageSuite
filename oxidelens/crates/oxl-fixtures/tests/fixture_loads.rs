//! The fixtures are only useful if the suite's own view renderer accepts
//! them: every later task loads them through `render_sql`.

use std::path::Path;
use triage_core::output::duckdb::inventory::{Inventory, Status};
use triage_core::output::duckdb::render::render_sql;

fn inventory(root: &Path) -> Inventory {
    let text = std::fs::read_to_string(root.join("duckdb").join("datasets.json")).unwrap();
    let mut inv: Inventory = serde_json::from_str(&text).unwrap();
    for dataset in &inv.datasets {
        for file in &dataset.files {
            assert!(
                file.path.is_relative(),
                "{} is stored absolute",
                file.path.display()
            );
        }
    }
    inv.out_root = root.to_path_buf();
    inv
}

fn count(conn: &duckdb::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn views(root: &Path) -> duckdb::Connection {
    let conn = duckdb::Connection::open_in_memory().unwrap();
    conn.execute_batch(&render_sql(&inventory(root))).unwrap();
    conn
}

#[test]
fn the_two_host_fixture_renders_views_duckdb_accepts() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(d.path());
    let inv = inventory(&root);
    assert_eq!(
        (inv.run_id.as_str(), inv.status),
        ("20260101000000-aaaa", Status::Ok)
    );

    let conn = views(&root);
    assert_eq!(count(&conn, "select count(*) from evtxtriage_events"), 11);
    assert_eq!(count(&conn, "select count(*) from petriage_timeline"), 5);
    assert_eq!(
        count(&conn, "select count(*) from ext_hayabusa_csv_timeline"),
        5
    );

    // The declared types reach the typed view; the bad cell stays visible as text.
    assert_eq!(
        count(
            &conn,
            "select count(*) from evtxtriage_events \
             where TimeCreated is null and TimeCreated__text is not null"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "select count(*) from evtxtriage_events where EventId > 4000"
        ),
        6
    );
    assert_eq!(
        count(&conn, "select sum(RunCount) from petriage_timeline"),
        427
    );
    assert_eq!(
        count(
            &conn,
            "select count(distinct _triage_host) from evtxtriage_events"
        ),
        2
    );
    assert_eq!(
        count(
            &conn,
            "select count(*) from evtxtriage_events where _triage_host = 'WS01'"
        ),
        8
    );
}

#[test]
fn the_rerun_has_five_ws01_prefetch_rows() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::rerun_ws01_prefetch(d.path());
    let inv = inventory(&root);
    assert_eq!(inv.run_id, "20260102000000-bbbb");
    assert_eq!(inv.datasets.len(), 1);
    let conn = views(&root);
    assert_eq!(count(&conn, "select count(*) from petriage_timeline"), 5);
    assert_eq!(
        count(
            &conn,
            "select count(*) from petriage_timeline where ExecutableName = 'PSEXESVC.EXE'"
        ),
        1
    );
}

#[test]
fn the_empty_dataset_has_a_header_and_no_rows() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::empty_dataset(d.path());
    assert_eq!(inventory(&root).run_id, "20260103000000-cccc");
    let conn = views(&root);
    assert_eq!(count(&conn, "select count(*) from rbtriage_main"), 0);
}

#[test]
fn the_fixtures_coexist_in_one_directory() {
    let d = tempfile::tempdir().unwrap();
    let a = oxl_fixtures::two_hosts(d.path());
    let b = oxl_fixtures::rerun_ws01_prefetch(d.path());
    let c = oxl_fixtures::empty_dataset(d.path());
    let f = oxl_fixtures::loose_fw_csv(d.path());
    assert!(a != b && b != c && a != c);
    assert_eq!(
        std::fs::read_to_string(&f).unwrap().lines().count(),
        4,
        "fw.csv is a header and three rows"
    );
    let case = oxl_fixtures::new_case(d.path());
    assert_eq!(case.info().unwrap().name, "T");
}

/// A logon row: `EventId`, `Computer`, `PayloadData1`, `PayloadData2`,
/// `RemoteHost`.
type Logon = (i64, String, String, Option<String>, Option<String>);

/// The logon rows of `two_hosts_logons` are WS01's, in `Security`, with the
/// account in `PayloadData1` and the logon type in `PayloadData2`; the rest
/// of the run is `two_hosts`'s, which itself is unchanged.
#[test]
fn the_logons_run_adds_three_security_rows_to_ws01() {
    let d = tempfile::tempdir().unwrap();
    let plain = views(&oxl_fixtures::two_hosts(&d.path().join("a")));
    let root = oxl_fixtures::two_hosts_logons(&d.path().join("b"));
    assert_eq!(inventory(&root).run_id, "20260101000000-aaaa");
    let conn = views(&root);
    assert_eq!(count(&plain, "select count(*) from evtxtriage_events"), 11);
    assert_eq!(count(&conn, "select count(*) from evtxtriage_events"), 14);
    assert_eq!(
        count(
            &conn,
            "select count(*) from evtxtriage_events where _triage_host = 'WS01'"
        ),
        11
    );
    for table in ["petriage_timeline", "ext_hayabusa_csv_timeline"] {
        let sql = format!("select count(*) from {table}");
        assert_eq!(count(&conn, &sql), count(&plain, &sql), "{table}");
    }
    let logons: Vec<Logon> = conn
        .prepare(
            "select EventId, Computer, PayloadData1, PayloadData2, RemoteHost \
             from evtxtriage_events where PayloadData2 is not null \
             order by TimeCreated",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        logons,
        [
            (
                4624,
                "WS01".to_string(),
                r"Target: ACME\jdoe".to_string(),
                Some("LogonType 10".to_string()),
                Some("203.0.113.7".to_string())
            ),
            (
                4625,
                "WS01".to_string(),
                r"Target: ACME\jdoe".to_string(),
                Some("LogonType 3".to_string()),
                None
            ),
            (
                4624,
                "WS01".to_string(),
                "Target: -".to_string(),
                Some("LogonType 3".to_string()),
                None
            ),
        ]
    );
    assert_eq!(
        count(
            &conn,
            "select count(*) from evtxtriage_events \
             where Channel = 'Security' and _triage_host = 'WS01' and PayloadData2 is not null"
        ),
        3
    );
}

/// A row that differs between two files: its line number, and each cell
/// that differs as (column, before, after).
type ChangedRow<'a> = (usize, Vec<(&'a str, &'a str, &'a str)>);

/// The later WS01 event run holds WS01's eight rows, one of them with its
/// `MapDescription` changed: seven rows match `two_hosts`'s exactly.
#[test]
fn the_event_rerun_changes_one_ws01_row() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::rerun_ws01_events(d.path());
    let inv = inventory(&root);
    assert_eq!(inv.run_id, "20260108000000-gggg");
    assert_eq!(inv.datasets.len(), 1);
    let conn = views(&root);
    assert_eq!(count(&conn, "select count(*) from evtxtriage_events"), 8);
    assert_eq!(
        count(
            &conn,
            "select count(*) from evtxtriage_events where _triage_host <> 'WS01'"
        ),
        0
    );
    // The files themselves, compared whole: same header, same rows in the
    // same order, and one row that differs in one cell, its MapDescription.
    let old = oxl_fixtures::two_hosts(&d.path().join("old"));
    let read = |root: &Path, folder: &str| -> Vec<String> {
        std::fs::read_to_string(root.join(folder).join("EventLogs").join("evtx.csv"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    };
    let before = read(&old, "Processed-WS01-20260101000000");
    let after = read(&root, "Processed-WS01-20260108000000");
    assert_eq!(after.len(), before.len());
    assert_eq!(after[0], before[0], "the header changed");
    let header: Vec<&str> = before[0].split(',').collect();
    let changed: Vec<ChangedRow> = before
        .iter()
        .zip(&after)
        .enumerate()
        .skip(1)
        .filter(|(_, (b, a))| b != a)
        .map(|(at, (b, a))| {
            let (b, a): (Vec<&str>, Vec<&str>) = (b.split(',').collect(), a.split(',').collect());
            assert_eq!(a.len(), b.len(), "row {at} changed its cell count");
            let cells = header
                .iter()
                .zip(b.iter().zip(&a))
                .filter(|(_, (b, a))| b != a)
                .map(|(name, (b, a))| (*name, *b, *a))
                .collect();
            (at, cells)
        })
        .collect();
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(
        changed[0].1,
        [(
            "MapDescription",
            "Process creation",
            "Process creation (edited)"
        )]
    );
    assert!(before[changed[0].0].contains(",rclone.exe,"));
}

#[test]
fn the_untimed_file_has_a_zoneless_time_and_a_blank_one() {
    let d = tempfile::tempdir().unwrap();
    let path = oxl_fixtures::untimed_file(d.path());
    assert_eq!(path.file_name().unwrap(), "notes.csv");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "When,What\n2026-01-05 10:00:00,disk wiped\n,blank\n"
    );
}

#[test]
fn the_hostile_values_cover_review_focus_3() {
    let values = oxl_fixtures::hostile_record_values();
    for needle in [
        "=HYPERLINK(\"http://x\")",
        "<img src=x onerror=1>",
        "\u{202E}",
        "|",
        "`",
    ] {
        assert!(
            values.iter().any(|v| v.contains(needle)),
            "no value holds {needle:?}"
        );
    }
    assert!(values.iter().any(|v| v.chars().count() == 5_000));
}
