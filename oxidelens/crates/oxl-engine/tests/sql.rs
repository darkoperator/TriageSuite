//! The escape hatch: one read-only SELECT over the case, and the schema
//! description that tells an analyst or an agent what there is to select.
//! Everything that is not one SELECT, and every SELECT that reaches outside
//! the case, is refused.

mod common;

use common::{analyst, loaded, Holder};
use oxl_engine::{describe, load_file, query_sql, Case, FileOptions, CELL_CAP, MAX_LIMIT};
use oxl_fixtures::loose_fw_csv;
use std::path::PathBuf;

/// A loaded case, closed so that `query_sql` and `describe` can attach it.
fn closed() -> (tempfile::TempDir, PathBuf) {
    let (d, case) = loaded();
    let path = case.path().to_path_buf();
    drop(case);
    (d, path)
}

fn s(text: &str) -> Option<String> {
    Some(text.to_string())
}

#[test]
fn a_select_runs_and_pages() {
    let (_d, path) = closed();
    let r = query_sql(
        &path,
        "select Channel, count(*) as n from evtxtriage_events group by all order by n desc",
        10,
        0,
    )
    .unwrap();
    assert_eq!(
        (r.columns.clone(), r.rows.clone(), r.truncated),
        (
            vec!["Channel".to_string(), "n".to_string()],
            vec![vec![s("Security"), s("8")], vec![s("Sysmon"), s("3")]],
            false
        )
    );
    assert_eq!(r.types, vec!["VARCHAR".to_string(), "BIGINT".to_string()]);
    assert!(!r.cells_clipped);
    assert!(
        query_sql(&path, "select * from evtxtriage_events", 5, 0)
            .unwrap()
            .truncated
    );

    // Pages are slices of one order: the second row alone, then past the end.
    let ordered = "select Channel from evtxtriage_events group by all order by Channel";
    let second = query_sql(&path, ordered, 1, 1).unwrap();
    assert_eq!(
        (second.rows, second.truncated),
        (vec![vec![s("Sysmon")]], false)
    );
    let past = query_sql(&path, ordered, 10, 50).unwrap();
    assert_eq!((past.rows.len(), past.truncated), (0, false));
    assert_eq!(past.columns, vec!["Channel".to_string()]);
}

#[test]
fn a_page_over_the_maximum_is_invalid() {
    let (_d, path) = closed();
    let e = query_sql(&path, "select 1", MAX_LIMIT + 1, 0).unwrap_err();
    assert_eq!(e.code(), "invalid");
    assert_eq!(
        query_sql(&path, "select 1", MAX_LIMIT, 0).unwrap().rows,
        vec![vec![s("1")]]
    );
}

#[test]
fn everything_that_is_not_one_select_is_refused() {
    let (_d, path) = closed();
    for bad in [
        "drop table ox.meta",
        "select 1; select 2",
        "insert into ox.tag values ('a','b',now())",
        "copy (select 1) to 'x.csv'",
        "attach 'x.duckdb' as y",
        "install httpfs",
        "pragma database_list",
        "set enable_external_access = true",
        "",
        "create view v as select 1",
        "select * from read_csv('C:/Windows/win.ini')",
        "select getenv('PATH')",
        "select * from glob('*')",
    ] {
        assert!(query_sql(&path, bad, 10, 0).is_err(), "{bad}");
    }
    // A statement that is not a SELECT is refused by the parser, with its
    // message, before anything is attached.
    let e = query_sql(&path, "drop table ox.meta", 10, 0).unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");
    let e = query_sql(&path, "select 1; select 2", 10, 0).unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");
    // A second statement hidden after a comment is still a second statement,
    // and a comment after the one SELECT never reaches execution.
    assert!(query_sql(&path, "select 1; -- harmless\ndrop table ox.meta", 10, 0).is_err());
    assert_eq!(
        query_sql(&path, "select 1 as one -- trailing", 10, 0)
            .unwrap()
            .rows,
        vec![vec![s("1")]]
    );
    // None of it wrote anything.
    let case = Case::open(&path).unwrap();
    assert_eq!(common::n(&case, "SELECT count(*) FROM ox.meta"), 4);
    assert_eq!(common::n(&case, "SELECT count(*) FROM ox.tag"), 0);
    assert!(!path.with_file_name("x.duckdb").exists());
}

#[test]
fn file_and_indirection_functions_are_refused_wherever_they_appear() {
    let (d, path) = closed();
    let beside = d.path().join("beside.csv");
    std::fs::write(&beside, "a\n1\n").unwrap();
    let case = path.to_str().unwrap().replace('\'', "''");
    let file = beside.to_str().unwrap().replace('\'', "''");
    let calls = [
        format!("read_blob('{case}')"),
        format!("read_text('{case}.wal')"),
        format!("read_csv('{file}')"),
        "read_csv('x')".to_string(),
        "glob('*')".to_string(),
        "query('select 1')".to_string(),
        "query_table('evtxtriage_events')".to_string(),
    ];
    for call in &calls {
        // The same call with its function name in mixed case: rEaD_BlOb(...).
        let open = call.find('(').unwrap();
        let name: String = call[..open]
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if i % 2 == 1 {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect();
        let mixed = format!("{name}{}", &call[open..]);
        for sql in [
            format!("select * from {call}"),
            format!("select * from main.{call}"),
            format!("select * from {mixed}"),
            format!("select (select count(*) from {call}) as n"),
            format!("select * from (select * from {call}) as t"),
            format!("with c as (select * from {call}) select * from c"),
        ] {
            let e = query_sql(&path, &sql, 10, 0).unwrap_err();
            assert_eq!(e.code(), "invalid", "{sql}: {e}");
        }
    }
    // A table named by a path is a replacement scan of that file.
    for sql in [
        format!("select * from '{case}'"),
        format!("select * from '{file}'"),
    ] {
        let e = query_sql(&path, &sql, 10, 0).unwrap_err();
        assert_eq!(e.code(), "invalid", "{sql}: {e}");
    }
    // Names that only look like those functions still run.
    let r = query_sql(
        &path,
        "select count(*) as read_count, 'x' as query from evtxtriage_events",
        10,
        0,
    )
    .unwrap();
    assert_eq!(
        (r.columns, r.rows),
        (
            vec!["read_count".to_string(), "query".to_string()],
            vec![vec![s("11"), s("x")]]
        )
    );
    // The allowed table functions run, a lateral one included.
    let r = query_sql(
        &path,
        "select count(*) from range(3) as r, lateral unnest([r.range, r.range]) as u(x)",
        10,
        0,
    )
    .unwrap();
    assert_eq!(r.rows, vec![vec![s("6")]]);
    // A catalog table function is not on the allowlist.
    let e = query_sql(&path, "select * from duckdb_tables()", 10, 0).unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");
}

#[test]
fn the_case_is_still_writable_and_loadable_afterwards() {
    let (d, path) = closed();
    query_sql(&path, "select count(*) from evtxtriage_events", 10, 0).unwrap();
    describe(&path, None).unwrap();
    let mut case = Case::open(&path).unwrap();
    let fw = loose_fw_csv(d.path());
    let report = load_file(&mut case, &fw, FileOptions::default(), &analyst()).unwrap();
    let rel = report.relations[0].name.clone();
    drop(case);
    // And the new relation is what the next query sees.
    let r = query_sql(&path, &format!("select count(*) from \"{rel}\""), 10, 0).unwrap();
    assert_eq!(r.rows, vec![vec![s("3")]]);
}

#[test]
fn a_case_held_elsewhere_is_case_locked() {
    let (_d, path) = closed();
    let holder = Holder::start(&path);
    let query = query_sql(&path, "select 1", 10, 0).map(|_| ());
    let schema = describe(&path, None).map(|_| ());
    holder.release();
    assert_eq!(query.unwrap_err().code(), "case_locked");
    assert_eq!(schema.unwrap_err().code(), "case_locked");
}

#[test]
fn a_missing_case_is_not_found() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("absent.case.duckdb");
    assert_eq!(
        query_sql(&path, "select 1", 10, 0).unwrap_err().code(),
        "not_found"
    );
    assert_eq!(describe(&path, None).unwrap_err().code(), "not_found");
    assert!(!path.exists());
}

#[test]
fn case_tables_are_readable() {
    let (_d, path) = closed();
    let r = query_sql(&path, "select count(*) from ox.source", 10, 0).unwrap();
    assert_eq!(r.rows, vec![vec![s("1")]]);
}

#[test]
fn cells_are_clipped_and_timestamps_are_iso_text() {
    let (_d, path) = closed();
    let r = query_sql(
        &path,
        &format!(
            "select repeat('é', {}) as long, NULL as nothing",
            CELL_CAP + 5
        ),
        10,
        0,
    )
    .unwrap();
    assert!(r.cells_clipped);
    assert_eq!(
        r.rows[0][0].as_deref().map(|t| t.chars().count()),
        Some(CELL_CAP)
    );
    assert_eq!(r.rows[0][1], None);

    let r = query_sql(
        &path,
        "select min(TimeCreated) as t from evtxtriage_events",
        10,
        0,
    )
    .unwrap();
    assert!(!r.cells_clipped);
    assert_eq!(r.rows, vec![vec![s("2026-01-05T02:13:58.204000Z")]]);
}

#[test]
fn describe_lists_relations_and_case_tables_with_comments() {
    let (_d, path) = closed();
    let d = describe(&path, None).unwrap();
    assert!(d
        .iter()
        .any(|t| t.schema == "main" && t.name == "evtxtriage_events" && t.rows == Some(11)));
    assert!(d
        .iter()
        .filter(|t| t.schema == "ox")
        .all(|t| t.comment.is_some() && t.columns.iter().all(|c| c.comment.is_some())));
    assert!(d.iter().all(|t| !t.schema.starts_with("src_")));
    assert_eq!(describe(&path, Some("petriage_timeline")).unwrap().len(), 1);

    // Every format-1 case table is there, and the views are views.
    assert_eq!(d.iter().filter(|t| t.schema == "ox").count(), 14);
    assert!(d
        .iter()
        .filter(|t| t.schema == "ox")
        .all(|t| t.kind == "table"));
    let events = d
        .iter()
        .find(|t| t.schema == "main" && t.name == "evtxtriage_events")
        .unwrap();
    assert_eq!(
        (events.kind.as_str(), events.comment.as_deref()),
        ("view", None)
    );
    let time = events
        .columns
        .iter()
        .find(|c| c.name == "TimeCreated")
        .unwrap();
    assert_eq!(time.sql_type, "TIMESTAMP");
    assert!(d
        .iter()
        .any(|t| t.schema == "main" && t.name == "raw_evtxtriage_events" && t.rows == Some(11)));
    let meta = d
        .iter()
        .find(|t| t.schema == "ox" && t.name == "meta")
        .unwrap();
    assert_eq!(meta.rows, Some(4));

    assert_eq!(
        describe(&path, Some("no_such_relation"))
            .unwrap_err()
            .code(),
        "not_found"
    );
}
