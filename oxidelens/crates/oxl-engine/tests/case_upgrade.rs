//! Case format 3: a step 1 case (format 1) and a step 2 case (format 2) are
//! upgraded in place, `1 → 2 → 3` in one transaction, when opened to change
//! them, and a format-1 case still answers every step 1 read when it is
//! opened read-only; the `ox.row_state` view; reader clones of an open
//! case; the display zone.
//!
//! A format-1 case is built from the format-1 DDL exactly as step 1 shipped
//! it (`fixtures/schema_v1.sql`, `2f6aa9b`), a format-2 case from that and
//! `upgrade_1_2.sql` (`Case::create_at_format_2`), then loaded and annotated
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
fn the_build_writes_format_3() {
    assert_eq!(CASE_FORMAT, 3);
    let d = tempfile::tempdir().unwrap();
    let case = oxl_fixtures::new_case(d.path());
    assert_eq!(case.format(), 3);
    assert_eq!(case.info().unwrap().format, 3);
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
    assert_eq!(case.format(), 3);
    assert_eq!(case.info().unwrap().format, 3);
    let id = case.case_id().unwrap();
    assert!(is_case_id(&id), "{id:?}");
    // The run's sessions were read from where the run is, in each host's
    // `Processed-*/Sessions/` as the suite writes them, and merged by name.
    let found = sessions(&case).unwrap();
    assert_eq!(
        found.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["Execution_Analysis"]
    );
    let fed: Vec<(&str, &str)> = found[0]
        .relations
        .iter()
        .map(|r| (r.relation.as_str(), r.host.as_str()))
        .collect();
    assert_eq!(
        fed,
        [
            ("evtxtriage_events", "SRV01"),
            ("evtxtriage_events", "WS01"),
            ("petriage_timeline", "SRV01"),
            ("petriage_timeline", "WS01"),
        ]
    );
    // One history row per step, newest first.
    let upgrades = history(&case, Some("case"), None, 50, 0).unwrap().items;
    let steps: Vec<_> = upgrades
        .iter()
        .map(|u| {
            (
                u.action.as_str(),
                u.target_id.as_str(),
                u.before.clone(),
                u.after.clone(),
            )
        })
        .collect();
    let format = |n: u32| Some(serde_json::json!({ "format": n }));
    assert_eq!(
        steps,
        [
            ("case_upgrade", id.as_str(), format(2), format(3)),
            ("case_upgrade", id.as_str(), format(1), format(2)),
        ]
    );
    drop(case);

    assert_eq!(dumps(&path), before);
    let history_after = raw_text(&path, "SELECT CAST(count(*) AS VARCHAR) FROM ox.history");
    assert_eq!(
        history_after.unwrap().parse::<i64>().unwrap(),
        history_before.unwrap().parse::<i64>().unwrap() + 2
    );
    // Opened again: already format 3, nothing more is done, and the id stays.
    let case = Case::open(&path).unwrap();
    assert_eq!(case.case_id().unwrap(), id);
    assert_eq!(
        history(&case, Some("case"), None, 50, 0)
            .unwrap()
            .items
            .len(),
        2
    );
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

/// Each `ox` table, view and sequence as DDL would make it: columns with
/// their types, nullability and defaults, constraints, a view's definition,
/// and every comment. Not a sequence's start: DuckDB reports a used
/// sequence's next value there, and the upgrade itself takes a history id.
fn catalog(path: &Path) -> Vec<String> {
    let conn = duckdb::Connection::open(path).unwrap();
    conn.prepare(
        "SELECT 'table ' || table_name || ': ' || coalesce(comment, '') \
           FROM duckdb_tables() WHERE schema_name = 'ox' \
         UNION ALL \
         SELECT 'view ' || view_name || ': ' || coalesce(comment, '') \
           FROM duckdb_views() WHERE schema_name = 'ox' \
         UNION ALL \
         SELECT 'definition ' || view_name || ' ' || sql \
           FROM duckdb_views() WHERE schema_name = 'ox' \
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

/// The upgrades and a new case's DDL are written apart; they must make the
/// same tables, views and sequences, so a case upgraded from either older
/// format is a format-3 case like any other.
#[test]
fn format_3_new_and_upgraded_catalogs_match() {
    let d = tempfile::tempdir().unwrap();
    let fresh = d.path().join("new.case.duckdb");
    drop(Case::create(&fresh, "IR-1", &analyst()).unwrap());
    let from_1 = d.path().join("v1.case.duckdb");
    drop(Case::create_at_format_1(&from_1, SCHEMA_V1, "IR-1", &analyst()).unwrap());
    let from_2 = d.path().join("v2.case.duckdb");
    let v2 = Case::create_at_format_2(&from_2, "IR-1", &analyst()).unwrap();
    assert_eq!(v2.format(), 2);
    drop(v2);
    // Before the upgrade, the format-2 file has none of format 3.
    assert!(!catalog(&from_2)
        .iter()
        .any(|l| l.starts_with("table entry")));
    for path in [&from_1, &from_2] {
        assert_eq!(Case::open(path).unwrap().format(), 3);
    }
    let expected = catalog(&fresh);
    for object in [
        "table saved_view: ",
        "table entry: ",
        "table evidence: ",
        "table host: ",
        "table account: ",
        "table ioc: ",
        "table entry_host: ",
        "table entry_account: ",
        "table entry_ioc: ",
        "view row_state: ",
        "sequence entry_seq ",
        "sequence host_seq ",
        "sequence account_seq ",
        "sequence ioc_seq ",
    ] {
        assert!(
            expected.iter().any(|l| l.starts_with(object)),
            "{object} is missing from a new case"
        );
    }
    assert!(expected
        .iter()
        .any(|l| l.starts_with("column history.target_kind ")
            && l.contains("entry, evidence, host, account, ioc")));
    assert_eq!(catalog(&from_1), expected, "upgraded from format 1");
    assert_eq!(catalog(&from_2), expected, "upgraded from format 2");
    // And every one of them has a comment.
    for line in &expected {
        if line.starts_with("table ") || line.starts_with("view ") || line.starts_with("column ") {
            assert!(!line.ends_with(": "), "no COMMENT: {line}");
        }
    }
}

/// A case of a format newer than this build is refused, opened either way,
/// with a message that says so (the CLI's exit code is pinned in
/// `oxl-cli/tests/cli_case_load.rs`).
#[test]
fn a_format_3_case_refuses_older_binaries_politely() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("future.case.duckdb");
    drop(Case::create(&path, "IR-1", &analyst()).unwrap());
    {
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute("UPDATE ox.meta SET value = '4' WHERE key = 'format'", [])
            .unwrap();
    }
    for opened in [Case::open(&path), Case::open_read_only(&path)] {
        let e = opened.err().expect("a format-4 case opened");
        assert!(
            matches!(
                e,
                oxl_engine::EngineError::NeedsUpgrade {
                    found: 4,
                    expected: 3
                }
            ),
            "{e:?}"
        );
        assert_eq!(e.code(), "needs_upgrade");
        assert!(e.to_string().contains("newer format"), "{e}");
    }
    // Refusing it changed nothing.
    assert_eq!(
        raw_text(&path, "SELECT value FROM ox.meta WHERE key = 'format'").as_deref(),
        Some("4")
    );
}

/// The rows of `ox.run_session`, as text, in a stable order.
fn session_rows(path: &Path) -> Vec<String> {
    let conn = duckdb::Connection::open(path).unwrap();
    dump(&conn, "run_session")
}

/// A step 2 case at `dir/v2.case.duckdb`: the two-host run loaded, and a
/// second run, WS01's later prefetch, carrying a session `Before` when it
/// was loaded and a session `After` written beside it since. Closed again;
/// returns its path and the two runs' source ids.
fn step2_case(dir: &Path) -> (PathBuf, i64, i64) {
    let path = dir.join("v2.case.duckdb");
    let mut case = Case::create_at_format_2(&path, "IR-1", &analyst()).unwrap();
    let first = load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&dir.join("a")),
        &analyst(),
    )
    .unwrap()
    .source_id;
    let rerun = oxl_fixtures::rerun_ws01_prefetch(&dir.join("b"));
    oxl_fixtures::write_session(&rerun, "Before", &[r"C:\cases\before.csv"]);
    let second = load_run(&mut case, &rerun, &analyst()).unwrap().source_id;
    oxl_fixtures::write_session(&rerun, "After", &[r"C:\cases\after.csv"]);
    assert_eq!(case.format(), 2);
    (path, first, second)
}

/// The rows of `rows` (from `session_rows`) that belong to `source`.
fn of_source(rows: &[String], source: i64) -> Vec<String> {
    let prefix = format!("{{'source_id': {source}, ");
    rows.iter()
        .filter(|r| r.starts_with(&prefix))
        .cloned()
        .collect()
}

/// Cases upgraded to format 2 before `9a0dd89` lost their sessions: the
/// `2 → 3` step reads them again for each run that has none, and leaves a
/// run that has some exactly as it is.
#[test]
fn upgrade_from_2_restores_lost_sessions() {
    let d = tempfile::tempdir().unwrap();
    let (path, first, second) = step2_case(d.path());
    let kept = session_rows(&path);
    let first_rows = of_source(&kept, first);
    assert_eq!(first_rows.len(), 1, "{kept:?}");
    assert_eq!(of_source(&kept, second).len(), 1, "{kept:?}");
    {
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute("DELETE FROM ox.run_session WHERE source_id = ?", [first])
            .unwrap();
    }
    assert!(of_source(&session_rows(&path), first).is_empty());

    let case = Case::open(&path).unwrap();
    assert_eq!(case.format(), 3);
    let names: Vec<(i64, String)> = sessions(&case)
        .unwrap()
        .into_iter()
        .map(|s| (s.source_id, s.name))
        .collect();
    assert_eq!(
        names,
        [
            (first, "Execution_Analysis".to_string()),
            (second, "Before".to_string())
        ]
    );
    drop(case);
    // The lost run's rows are back as its load wrote them; the other run's
    // are exactly what they were, though a re-read would now find `After`.
    let after = session_rows(&path);
    assert_eq!(of_source(&after, first), first_rows);
    assert_eq!(after, kept);
}

/// Make the history insert of the `n`th upgrade step from now fail, by
/// taking the id `ox.history_seq` will hand that step: the step's last
/// statement, so everything before it in the transaction has been done.
fn fault_history_step(path: &Path, n: i64) {
    let conn = duckdb::Connection::open(path).unwrap();
    let next: i64 = conn
        .query_row("SELECT nextval('ox.history_seq')", [], |r| r.get(0))
        .unwrap();
    conn.execute(
        "INSERT INTO ox.history VALUES (?, '2026-01-01T00:00:00.000000Z', 'analyst:x', \
         'case', 'x', 'fault', NULL, NULL)",
        [next + n],
    )
    .unwrap();
}

/// A fault in the `2 → 3` step, after its tables, its session re-read and
/// its format change, leaves the file as it was: at format 2 for a step 2
/// case, and at format 1, with nothing of format 2 either, for a step 1
/// case whose `1 → 2` step had succeeded in the same transaction.
#[test]
fn upgrade_is_one_transaction() {
    let d = tempfile::tempdir().unwrap();
    for sub in ["one", "two"] {
        std::fs::create_dir(d.path().join(sub)).unwrap();
    }

    let (v2, first, _) = step2_case(&d.path().join("two"));
    {
        let conn = duckdb::Connection::open(&v2).unwrap();
        conn.execute("DELETE FROM ox.run_session WHERE source_id = ?", [first])
            .unwrap();
    }
    fault_history_step(&v2, 1);
    let objects = ox_objects(&v2);
    let sessions_before = session_rows(&v2);
    let history_before = raw_text(&v2, "SELECT CAST(count(*) AS VARCHAR) FROM ox.history");
    let e = Case::open(&v2).err().expect("the upgrade succeeded");
    assert_eq!(e.code(), "engine", "{e}");
    assert_eq!(
        raw_text(&v2, "SELECT value FROM ox.meta WHERE key = 'format'").as_deref(),
        Some("2")
    );
    assert_eq!(ox_objects(&v2), objects);
    assert!(!objects.iter().any(|o| o.contains("entry")));
    assert_eq!(session_rows(&v2), sessions_before);
    assert!(of_source(&sessions_before, first).is_empty());
    assert_eq!(
        raw_text(&v2, "SELECT CAST(count(*) AS VARCHAR) FROM ox.history"),
        history_before
    );
    assert_eq!(Case::open_read_only(&v2).unwrap().format(), 2);

    let v1 = step1_case(&d.path().join("one"));
    // Step 1 → 2 takes the next id; the fault is on step 2 → 3's.
    fault_history_step(&v1, 2);
    let before = dumps(&v1);
    let objects = ox_objects(&v1);
    let history_before = raw_text(&v1, "SELECT CAST(count(*) AS VARCHAR) FROM ox.history");
    let e = Case::open(&v1).err().expect("the upgrade succeeded");
    assert_eq!(e.code(), "engine", "{e}");
    assert_eq!(
        raw_text(&v1, "SELECT value FROM ox.meta WHERE key = 'format'").as_deref(),
        Some("1")
    );
    assert_eq!(ox_objects(&v1), objects);
    assert!(!objects
        .iter()
        .any(|o| o.contains("saved_view") || o.contains("run_session") || o.contains("entry")));
    assert_eq!(dumps(&v1), before);
    assert_eq!(
        raw_text(&v1, "SELECT CAST(count(*) AS VARCHAR) FROM ox.history"),
        history_before
    );
    assert_eq!(
        raw_text(&v1, "SELECT value FROM ox.meta WHERE key = 'case_id'"),
        None
    );
}

/// Insert an `ox.entry` row: `id`, created by `by`, at `status` (reviewed
/// by `analyst:lead` when `reviewed`), deleted or not.
fn insert_entry(conn: &duckdb::Connection, id: i64, by: &str, status: &str, deleted: bool) {
    let (reviewed_by, reviewed_at) = if status == "reviewed" {
        (Some("analyst:lead"), Some("2026-01-02T00:00:00.000000Z"))
    } else {
        (None, None)
    };
    conn.execute(
        "INSERT INTO ox.entry (id, \"time\", time_source, title, description, tactic, \
         technique_ids, status, created_by, created_at, modified_by, modified_at, \
         reviewed_by, reviewed_at, deleted) \
         VALUES (?, NULL, NULL, 'entry', '', NULL, '[]', ?, ?, \
         '2026-01-01T00:00:00.000000Z', ?, '2026-01-01T00:00:00.000000Z', ?, ?, ?)",
        duckdb::params![id, status, by, by, reviewed_by, reviewed_at, deleted],
    )
    .unwrap();
}

/// Attach row `row` to entry `entry` as evidence.
fn insert_evidence(conn: &duckdb::Connection, entry: i64, row: i64) {
    conn.execute(
        "INSERT INTO ox.evidence (entry_id, row_id, relation, host, row_hash, \
         snapshot_json, attached_by, attached_at) \
         VALUES (?, ?, 'evtxtriage_events', 'WS01', 'h', '{}', 'analyst:x', \
         '2026-01-01T00:00:00.000000Z')",
        [entry, row],
    )
    .unwrap();
}

/// Mark row `row` suspicious, set by `by`.
fn insert_mark(conn: &duckdb::Connection, row: i64, by: &str) {
    conn.execute(
        "INSERT INTO ox.row_mark VALUES (?, 'suspicious', ?, '2026-01-01T00:00:00.000000Z')",
        duckdb::params![row, by],
    )
    .unwrap();
}

/// Ruling S2: one row per marked row; `malicious` (evidence on a live entry
/// an analyst made or reviewed) over `agent_malicious` (evidence only on
/// live unreviewed agent entries) over `suspicious` (`ox.row_mark`); `by`
/// is the entry's creator or the mark's setter.
#[test]
fn row_state_holds_one_mark_per_row_by_precedence() {
    let d = tempfile::tempdir().unwrap();
    let path = oxl_fixtures::new_case(d.path()).path().to_path_buf();
    let conn = duckdb::Connection::open(&path).unwrap();
    // Entries: 1 analyst draft, 2 agent unreviewed, 3 agent reviewed by an
    // analyst, 4 analyst reviewed but deleted, 5 another agent's
    // unreviewed, 6 agent unreviewed but deleted.
    insert_entry(&conn, 1, "analyst:ann", "draft", false);
    insert_entry(&conn, 2, "agent:bot", "unreviewed", false);
    insert_entry(&conn, 3, "agent:bot", "reviewed", false);
    insert_entry(&conn, 4, "analyst:ann", "reviewed", true);
    insert_entry(&conn, 5, "agent:other", "unreviewed", false);
    insert_entry(&conn, 6, "agent:gone", "unreviewed", true);
    // 101: suspicious only, set by an agent.
    insert_mark(&conn, 101, "agent:bot");
    // 102: suspicious, and evidence on an agent's unreviewed entry.
    insert_mark(&conn, 102, "analyst:sam");
    insert_evidence(&conn, 2, 102);
    // 103: suspicious, evidence on an agent's unreviewed entry and on an
    // analyst's draft.
    insert_mark(&conn, 103, "analyst:sam");
    insert_evidence(&conn, 2, 103);
    insert_evidence(&conn, 1, 103);
    // 104: evidence only on an agent's entry an analyst has reviewed.
    insert_evidence(&conn, 3, 104);
    // 105: suspicious, evidence only on a deleted analyst entry.
    insert_mark(&conn, 105, "analyst:sam");
    insert_evidence(&conn, 4, 105);
    // 106: evidence only on a deleted entry: no state at all.
    insert_evidence(&conn, 4, 106);
    // 107: evidence on two agents' unreviewed entries: one row, naming the
    // first entry's creator.
    insert_evidence(&conn, 5, 107);
    insert_evidence(&conn, 2, 107);
    // 108: evidence on an agent's live unreviewed entry and on a deleted
    // analyst one: the agent's.
    insert_evidence(&conn, 4, 108);
    insert_evidence(&conn, 2, 108);
    // 109: suspicious, evidence only on a deleted agent entry.
    insert_evidence(&conn, 6, 109);
    insert_mark(&conn, 109, "analyst:sam");

    let states: Vec<(i64, String, String)> = conn
        .prepare("SELECT row_id, mark, \"by\" FROM ox.row_state ORDER BY row_id, mark")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let expected: Vec<(i64, String, String)> = [
        (101, "suspicious", "agent:bot"),
        (102, "agent_malicious", "agent:bot"),
        (103, "malicious", "analyst:ann"),
        (104, "malicious", "agent:bot"),
        (105, "suspicious", "analyst:sam"),
        (107, "agent_malicious", "agent:bot"),
        (108, "agent_malicious", "agent:bot"),
        (109, "suspicious", "analyst:sam"),
    ]
    .into_iter()
    .map(|(r, m, b)| (r, m.to_string(), b.to_string()))
    .collect();
    assert_eq!(states, expected);
}

#[test]
fn clones_share_the_database() {
    let (_d, mut case) = common::loaded();
    let c2 = case.try_clone().unwrap();
    assert_eq!(c2.format(), CASE_FORMAT);
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

    let rows = history(&case, Some("case"), None, 50, 0).unwrap().items;
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
    assert_eq!(
        history(&case, Some("case"), None, 50, 0)
            .unwrap()
            .items
            .len(),
        1
    );

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
