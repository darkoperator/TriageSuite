//! The case file: creation, reopening, the refusals around both, and the
//! primitives (actor text, timestamp text) every later write depends on.

use oxl_engine::{parse_utc_text, Actor, Case};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

fn code<T>(result: oxl_engine::Result<T>) -> &'static str {
    match result {
        Ok(_) => "ok",
        Err(e) => e.code(),
    }
}

#[test]
fn create_then_open_round_trips_info() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.case.duckdb");
    drop(Case::create(&p, "IR-1", &Actor::Analyst("tester".into())).unwrap());
    let info = Case::open(&p).unwrap().info().unwrap();
    assert_eq!(
        (info.name.as_str(), info.format, info.created_by.as_str()),
        ("IR-1", 1, "analyst:tester")
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

    let mut holder = Command::new(env!("CARGO"));
    holder
        .args([
            "run",
            "-q",
            "-p",
            "oxl-fixtures",
            "--example",
            "hold_case",
            "--",
        ])
        .arg(&p);
    // Cargo gives this test process its per-package variables, and a nested
    // cargo would pass them on to build scripts. ring's tracks
    // CARGO_MANIFEST_DIR, so inheriting it rebuilds ring and, through it,
    // DuckDB on every run -- and then again for the next ordinary build.
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy();
        let per_package = [
            "CARGO_MANIFEST_",
            "CARGO_PKG_",
            "CARGO_CRATE_",
            "CARGO_BIN_",
        ]
        .iter()
        .any(|prefix| key.starts_with(prefix))
            || key == "CARGO_PRIMARY_PACKAGE"
            || key == "CARGO_TARGET_TMPDIR";
        if per_package {
            holder.env_remove(key.as_ref());
        }
    }
    let mut child = holder
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(
        line.trim(),
        "held",
        "hold_case did not report holding the case"
    );

    let outcome = code(Case::open(&p));

    // Closing stdin is what lets the holder exit.
    drop(child.stdin.take());
    let status = child.wait().unwrap();
    assert_eq!(outcome, "case_locked");
    assert!(status.success(), "hold_case exited with {status}");
}

#[test]
fn a_newer_format_needs_upgrade() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("future.case.duckdb");
    drop(Case::create(&p, "IR-1", &Actor::Analyst("tester".into())).unwrap());
    {
        let conn = duckdb::Connection::open(&p).unwrap();
        conn.execute("UPDATE ox.meta SET value = '2' WHERE key = 'format'", [])
            .unwrap();
    }
    match Case::open(&p) {
        Err(e) => {
            assert_eq!(e.code(), "needs_upgrade");
            assert!(matches!(
                e,
                oxl_engine::EngineError::NeedsUpgrade {
                    found: 2,
                    expected: 1
                }
            ));
        }
        Ok(_) => panic!("a format-2 case opened under format 1"),
    }
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
    assert_eq!(tables, 14, "schema ox should hold the 14 format-1 tables");
    let sequences = case
        .scalar_i64("SELECT count(*) FROM duckdb_sequences() WHERE schema_name = 'ox'")
        .unwrap();
    assert_eq!(sequences, 4);

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
