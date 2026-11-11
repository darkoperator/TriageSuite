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
