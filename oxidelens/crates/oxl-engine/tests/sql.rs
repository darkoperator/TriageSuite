//! The escape hatch: one read-only SELECT over the case, and the schema
//! description that tells an analyst or an agent what there is to select.
//! Everything that is not one SELECT, and every SELECT that reaches outside
//! the case, is refused.

mod common;

use common::{analyst, loaded, Holder};
use oxl_engine::{
    describe, load_file, query_sql, query_sql_within, Case, FileOptions, CELL_CAP, MAX_LIMIT,
    SQL_TEXT_MAX, SQL_TIME_LIMIT, SQL_TIME_LIMIT_MAX,
};
use oxl_fixtures::loose_fw_csv;
use std::path::PathBuf;
use std::time::Duration;

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
    // A page of nothing is a mistake, not an empty page.
    let e = query_sql(&path, "select 1", 0, 0).unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");
    assert_eq!(
        query_sql(&path, "select 1", MAX_LIMIT, 0).unwrap().rows,
        vec![vec![s("1")]]
    );
    // An offset past what DuckDB's OFFSET holds is the caller's input too.
    let e = query_sql(&path, "select 1", 10, u64::MAX).unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");
    assert!(query_sql(&path, "select 1", 10, i64::MAX as u64)
        .unwrap()
        .rows
        .is_empty());
}

/// Found on the first real run: a typo in a column name came back as
/// `engine`, exit 10, the code for a fault in the tool itself, so a caller
/// could not tell its own mistake from a broken build.
#[test]
fn a_mistake_in_the_query_is_invalid_with_duckdbs_reason() {
    let (_d, path) = closed();
    for (sql, says) in [
        // Found by the binder, before a row is read.
        ("select * from no_such_relation", "no_such_relation"),
        ("select NoSuchColumn from evtxtriage_events", "NoSuchColumn"),
        ("select no_such_function(1)", "no_such_function"),
        ("select 1 + 'a'::BLOB", "BLOB"),
        // Found while the rows are read.
        (
            "select CAST(Channel AS INTEGER) from evtxtriage_events",
            "Security",
        ),
        ("select 9223372036854775807::BIGINT + 1", "overflow"),
        (
            "select regexp_matches(Channel, '(') from evtxtriage_events",
            "(",
        ),
    ] {
        let e = query_sql(&path, sql, 10, 0).unwrap_err();
        assert_eq!(e.code(), "invalid", "{sql}: {e}");
        let message = e.to_string();
        assert!(
            message.to_lowercase().contains(&says.to_lowercase()),
            "{sql}: the reason does not name {says:?}: {message}"
        );
        // DuckDB points into the SQL it ran, which wraps the caller's; a
        // pointer into text the caller never wrote would mislead.
        for wrapper in ["DESCRIBE", "ox_c0", "ox_q", "LINE 1"] {
            assert!(!message.contains(wrapper), "{sql}: {message}");
        }
    }
    // Only DuckDB's trailing pointer is cut: text that merely looks like one
    // inside a quoted value stays.
    let e = query_sql(
        &path,
        "select CAST(Channel || chr(10) || 'LINE 7: x' AS INTEGER) from evtxtriage_events",
        10,
        0,
    )
    .unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");
    assert!(e.to_string().contains("LINE 7: x"), "{e}");
}

/// A known limit, kept on purpose: a view of the case's own that no longer
/// binds fails with the same kinds of error as a caller's typo, and `query`
/// reports it the same way. DuckDB's reason then names the table the view
/// lost, which the caller never wrote. Telling the two apart was tried and
/// dropped (see `query_failure`); this pins what the caller sees instead.
#[test]
fn a_view_of_a_damaged_case_fails_with_duckdbs_reason() {
    let (_d, path) = closed();
    let damage = "CREATE TABLE ox.scratch (a INTEGER); \
                  CREATE VIEW main.broken AS SELECT a FROM ox.scratch; \
                  DROP TABLE ox.scratch;";
    duckdb::Connection::open(&path)
        .unwrap()
        .execute_batch(damage)
        .unwrap();
    let e = query_sql(&path, "select * from broken", 10, 0).unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");
    assert!(e.to_string().contains("scratch"), "{e}");
    // The rest of the case reads as before.
    assert!(query_sql(&path, "select count(*) from evtxtriage_events", 10, 0).is_ok());
}

/// Billions of row pairs and no row out: runs for minutes unless stopped.
const RUNAWAY: &str = "select a.range from range(300000) a, range(300000) b \
                       where a.range * b.range = -1";

/// Found on the first real run: nothing bounded how long a query ran, so
/// a cross join an agent wrote by mistake held the case open for minutes.
#[test]
fn a_query_that_runs_past_its_time_limit_is_stopped_and_invalid() {
    let (_d, path) = closed();
    let started = std::time::Instant::now();
    let e = query_sql_within(&path, RUNAWAY, 10, 0, Duration::from_secs(1)).unwrap_err();
    let took = started.elapsed();
    assert_eq!(e.code(), "invalid", "{e}");
    assert!(e.to_string().contains("1 s"), "{e}");
    assert!(
        took < Duration::from_secs(10),
        "stopped only after {took:?}"
    );

    // A query inside its limit runs, with the default limit and a set one.
    assert!(query_sql(&path, "select count(*) from evtxtriage_events", 10, 0).is_ok());
    assert!(query_sql_within(
        &path,
        "select count(*) from evtxtriage_events",
        10,
        0,
        Duration::from_secs(1)
    )
    .is_ok());
    // Stopping one query does not leave the next one stopped.
    assert!(query_sql_within(&path, "select 1", 10, 0, Duration::from_secs(1)).is_ok());
}

/// Reviewed: binding cannot be interrupted, and how long a bind takes grows
/// with the SQL's length, so the length is capped: that bounds how far past
/// its time a query can run.
#[test]
fn a_query_longer_than_the_cap_is_invalid() {
    let (_d, path) = closed();
    let padded = |chars: usize| {
        let head = "select 1 as one -- ";
        format!("{head}{}", "x".repeat(chars - head.chars().count()))
    };
    assert!(query_sql(&path, &padded(SQL_TEXT_MAX), 10, 0).is_ok());
    let e = query_sql(&path, &padded(SQL_TEXT_MAX + 1), 10, 0).unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");
    assert!(e.to_string().contains("100000"), "{e}");
    // The cap counts characters, not bytes.
    let e = query_sql(&path, &format!("{}é", padded(SQL_TEXT_MAX)), 10, 0).unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");
}

#[test]
fn a_time_limit_outside_its_range_is_invalid() {
    let (_d, path) = closed();
    for limit in [
        Duration::ZERO,
        Duration::from_millis(999),
        SQL_TIME_LIMIT_MAX + Duration::from_secs(1),
    ] {
        let e = query_sql_within(&path, "select 1", 10, 0, limit).unwrap_err();
        assert_eq!(e.code(), "invalid", "{limit:?}: {e}");
    }
    assert!(query_sql_within(&path, "select 1", 10, 0, SQL_TIME_LIMIT_MAX).is_ok());
    assert!(SQL_TIME_LIMIT <= SQL_TIME_LIMIT_MAX);
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
fn table_references_outside_the_case_are_refused() {
    let (_d, path) = closed();
    // The case path split at its last dot, as a schema and a table: DuckDB
    // would rebuild the path from both parts and scan the file.
    let text = path.to_str().unwrap();
    let stem = text.strip_suffix(".duckdb").unwrap().replace('"', "\"\"");
    for sql in [
        format!("select * from \"{stem}\".\"duckdb\""),
        format!("select * from \"{stem}\".duckdb"),
        "select * from one.duckdb".to_string(),
        "select * from elsewhere.ox.source".to_string(),
        "select * from memory.main.t".to_string(),
        "select * from secrets.source".to_string(),
        "select * from src_.x".to_string(),
        "select * from src_1a.x".to_string(),
        "select * from main.\"a/b\"".to_string(),
        "select * from (select * from one.duckdb) as t".to_string(),
        "with c as (select 1) select * from c, one.duckdb".to_string(),
    ] {
        let e = query_sql(&path, &sql, 10, 0).unwrap_err();
        assert_eq!(e.code(), "invalid", "{sql}: {e}");
    }
    for (sql, expected) in [
        ("select count(*) from main.evtxtriage_events", "11"),
        ("select count(*) from MAIN.evtxtriage_events", "11"),
        ("select count(*) from ox.source", "1"),
        ("select count(*) from ox_case.evtxtriage_events", "11"),
        ("select count(*) from ox_case.main.evtxtriage_events", "11"),
        ("select count(*) from Ox_Case.ox.source", "1"),
        (
            "select count(*) > 0 from information_schema.tables where table_schema = 'ox'",
            "true",
        ),
    ] {
        let r = query_sql(&path, sql, 10, 0).unwrap_or_else(|e| panic!("{sql}: {e}"));
        assert_eq!(r.rows, vec![vec![s(expected)]], "{sql}");
    }
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

/// Every refusal is `invalid` and says why, in words that name what the
/// caller wrote or what the rule is, so a caller can act on the message.
#[test]
fn sql_refusals_name_their_reason() {
    let (_d, path) = closed();
    for (sql, fragment) in [
        ("drop table ox.meta", "a query is one SELECT statement"),
        ("select 1; select 2", "not 2"),
        ("insert into ox.tag values ('a','b',now())", "one SELECT"),
        ("copy (select 1) to 'x.csv'", "one SELECT"),
        ("attach 'x.duckdb' as y", "one SELECT"),
        ("install httpfs", "one SELECT"),
        ("pragma database_list", "one SELECT"),
        ("set enable_external_access = true", "one SELECT"),
        ("create view v as select 1", "one SELECT"),
        ("", "not 0"),
        ("select getenv('PATH')", "may not call getenv"),
        ("select read_text('x')", "may not call read_text"),
        ("select * from read_csv('x')", "the table function read_csv"),
        ("select * from glob('*')", "the table function glob"),
        (
            "select * from query('select 1')",
            "the table function query",
        ),
        (
            "select * from duckdb_tables()",
            "may not read from the table function duckdb_tables",
        ),
        (
            "select * from generate_subscripts([1], 1)",
            "may not read from the table function generate_subscripts",
        ),
        (
            "select * from main.\"a/b\"",
            "a name with a path character reads a file",
        ),
        (
            "select * from one.duckdb",
            "only the case's own schemas may be named",
        ),
        (
            "select * from memory.main.t",
            "only the case's own catalog may be named",
        ),
        (
            "select * from secrets.source",
            "only the case's own schemas may be named",
        ),
        // A parameter has no value to take: DuckDB says so and the query is
        // the caller's mistake.
        ("select ?", "parameter"),
        ("select $1", "parameter"),
        // The sequence the notes draw their ids from is the case's, not the
        // query's: reading it would advance it, so it is refused by name.
        ("select nextval('ox.note_seq')", "nextval"),
        ("select nextval('ox.note_seq') from range(3)", "nextval"),
    ] {
        let e = query_sql(&path, sql, 10, 0).unwrap_err();
        assert_eq!(e.code(), "invalid", "{sql}: {e}");
        assert!(
            e.to_string()
                .to_lowercase()
                .contains(&fragment.to_lowercase()),
            "{sql}: expected {fragment:?} in: {e}"
        );
    }
}

/// A page of an ordered result is a slice of one order: pages drawn at
/// different times from the same query stay descending and contiguous.
#[test]
fn ordered_results_stay_ordered_across_pages() {
    let (_d, path) = closed();
    let sql = "SELECT * FROM range(100000) t(i) ORDER BY i DESC";
    let page = |number: u64| {
        let r = query_sql(&path, sql, 1000, number * 1000).unwrap();
        let values: Vec<i64> = r
            .rows
            .iter()
            .map(|row| row[0].as_deref().unwrap().parse().unwrap())
            .collect();
        (values, r.truncated)
    };
    for (number, first) in [(0u64, 99_999i64), (1, 98_999), (99, 999)] {
        let (values, truncated) = page(number);
        assert_eq!(values.len(), 1000, "page {number}");
        assert_eq!(values[0], first, "page {number}");
        assert!(
            values.windows(2).all(|w| w[0] == w[1] + 1),
            "page {number} is not descending and contiguous"
        );
        assert_eq!(truncated, number != 99, "page {number}");
    }
    assert!(page(100).0.is_empty());
}
