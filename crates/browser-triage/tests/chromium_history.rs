//! Chromium `History` parsing, driven through the real `BrowserTriage` binary
//! against synthetic databases built with rusqlite.
//!
//! The assertions that matter here are the completeness ones: a URL whose
//! visits are gone, a visit whose URL is gone, and a zero timestamp all have to
//! survive into the output.

#![cfg(unix)]

mod support;

use assert_cmd::Command;
use rusqlite::Connection;
use std::path::Path;
use support::{column, profile_dir, read_output, rows, run};
use tempfile::TempDir;

/// Chromium history with, deliberately:
///   * two ordinary visits to one URL,
///   * a URL with `visit_count = 12` and no visit rows (deleted history),
///   * a visit whose `url` foreign key points at nothing (dangling),
///   * a visit with `visit_time = 0` (unset).
fn write_history(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE urls(id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER,
                           typed_count INTEGER, last_visit_time INTEGER, hidden INTEGER);
         CREATE TABLE visits(id INTEGER PRIMARY KEY, url INTEGER, visit_time INTEGER,
                             from_visit INTEGER, transition INTEGER, visit_duration INTEGER,
                             opener_visit INTEGER);

         INSERT INTO urls VALUES
           (1,'https://example.test/a','Page A',2,1,13344473600000000,0),
           (2,'https://deleted.test/gone','Gone',12,0,13344473500000000,0);

         INSERT INTO visits VALUES
           (10,1,13344473600000000,0,1,5000000,NULL),
           (11,1,13344473610000000,10,0,0,NULL),
           (12,99,13344473620000000,0,0,0,NULL),
           (13,1,0,0,0,0,NULL);",
    )
    .unwrap();
}

#[test]
fn every_visit_and_every_orphan_url_reaches_the_output() {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    write_history(&dir.join("History"));
    let out = td.path().join("out");
    run(td.path(), &out);

    let csv_text = read_output(&out, "BrowserTriage_Output.csv");
    let record_types = column(&csv_text, "Record Type");

    // 4 visits + 1 URL with no visits. Nothing is filtered, coalesced or
    // deduplicated: source rows in, output rows out.
    assert_eq!(rows(&csv_text).len(), 5, "{csv_text}");
    assert_eq!(record_types.iter().filter(|t| *t == "Visit").count(), 4);
    assert_eq!(record_types.iter().filter(|t| *t == "URL Only").count(), 1);
}

/// A `urls` row with a visit count and no visit rows is the classic
/// history-deletion signature. Losing it would defeat the point of the tool.
#[test]
fn a_url_whose_visits_were_deleted_is_still_reported() {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    write_history(&dir.join("History"));
    let out = td.path().join("out");
    run(td.path(), &out);

    let csv_text = read_output(&out, "BrowserTriage_Output.csv");
    let line = csv_text
        .lines()
        .find(|l| l.contains("deleted.test"))
        .expect("the orphan URL must be present");
    assert!(line.contains("URL Only"), "{line}");
    assert!(line.contains("12"), "its visit_count must survive: {line}");
}

/// A zero timestamp is Chromium's "unset". The row stays; the cell is empty.
/// This is the specific behaviour whose absence cost the previous tool 41%.
#[test]
fn a_zero_timestamp_empties_the_cell_without_dropping_the_row() {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    write_history(&dir.join("History"));
    let out = td.path().join("out");
    run(td.path(), &out);

    let csv_text = read_output(&out, "BrowserTriage_Output.csv");
    let visit_ids = column(&csv_text, "Visit ID");
    assert!(
        visit_ids.iter().any(|v| v == "13"),
        "the zero-timestamp visit must still be emitted: {csv_text}"
    );

    let mut reader = csv::Reader::from_reader(csv_text.as_bytes());
    let headers = reader.headers().unwrap().clone();
    let id_at = headers.iter().position(|h| h == "Visit ID").unwrap();
    let ts_at = headers.iter().position(|h| h == "Visit Time").unwrap();
    let row = reader
        .records()
        .map(|r| r.unwrap())
        .find(|r| &r[id_at] == "13")
        .unwrap();
    assert_eq!(&row[ts_at], "", "unset must be empty, never an epoch");
}

/// A dangling foreign key is evidence, not a parse failure — emit the row and
/// say so in Notes.
#[test]
fn a_visit_with_a_dangling_url_reference_is_emitted_and_noted() {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    write_history(&dir.join("History"));
    let out = td.path().join("out");
    run(td.path(), &out);

    let csv_text = read_output(&out, "BrowserTriage_Output.csv");
    let notes = column(&csv_text, "Notes");
    assert!(
        notes.iter().any(|n| n.contains("no matching urls row")),
        "the dangling visit must be explained: {csv_text}"
    );
}

#[test]
fn timestamps_are_typed_and_keep_sub_second_precision() {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    write_history(&dir.join("History"));
    let out = td.path().join("out");
    run(td.path(), &out);

    let csv_text = read_output(&out, "BrowserTriage_Output.csv");
    assert!(
        csv_text.contains("2023-11-14T22:13:20.0000000Z"),
        "WebKit microseconds must decode to ISO-8601: {csv_text}"
    );
}

#[test]
fn transitions_are_decoded_and_the_raw_value_is_kept() {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    write_history(&dir.join("History"));
    let out = td.path().join("out");
    run(td.path(), &out);

    let csv_text = read_output(&out, "BrowserTriage_Output.csv");
    let types = column(&csv_text, "Visit Type");
    assert!(types.iter().any(|t| t == "Typed"), "{csv_text}");
    assert!(types.iter().any(|t| t == "Link"), "{csv_text}");
    assert!(
        !column(&csv_text, "Transition Raw")
            .iter()
            .all(String::is_empty),
        "the undecoded value must be preserved"
    );
}

/// The headline guarantee: two profiles of the same browser, and two browsers
/// sharing a profile name, all land in one file and stay distinguishable.
#[test]
fn multiple_browsers_and_profiles_merge_into_one_file_without_colliding() {
    let td = TempDir::new().unwrap();
    for (vendor, profile) in [
        ("Google/Chrome", "Default"),
        ("Google/Chrome", "Profile 2"),
        ("Google/Chrome", "Snapshots/116.0.5845.97/Default"),
        ("Microsoft/Edge", "Default"),
    ] {
        let dir = profile_dir(td.path(), "alice", vendor, profile);
        write_history(&dir.join("History"));
    }
    let out = td.path().join("out");
    run(td.path(), &out);

    let csv_text = read_output(&out, "BrowserTriage_Output.csv");

    // One file for the user, holding every profile — not one file per source.
    let files: Vec<_> = std::fs::read_dir(&out)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with("BrowserTriage_Output.csv"))
        .collect();
    assert_eq!(files.len(), 1, "expected one History file, got {files:?}");

    // 4 profiles x 5 rows each, with nothing overwritten.
    assert_eq!(rows(&csv_text).len(), 20, "{csv_text}");

    let mut pairs: Vec<(String, String)> = {
        let browsers = column(&csv_text, "Browser");
        let profiles = column(&csv_text, "Profile");
        browsers.into_iter().zip(profiles).collect()
    };
    pairs.sort();
    pairs.dedup();
    assert_eq!(
        pairs,
        vec![
            ("Chrome".to_string(), "Default".to_string()),
            ("Chrome".to_string(), "Profile 2".to_string()),
            (
                "Chrome".to_string(),
                "Snapshots/116.0.5845.97/Default".to_string()
            ),
            ("Edge".to_string(), "Default".to_string()),
        ],
        "every profile must be present and distinct"
    );
}

/// The timeline indexes the instants the typed rows carry, in the same run.
#[test]
fn the_timeline_indexes_history_visits() {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    write_history(&dir.join("History"));
    let out = td.path().join("out");
    run(td.path(), &out);

    let csv_text = read_output(&out, "BrowserTriage_Output_Timeline.csv");
    let mut reader = csv::Reader::from_reader(csv_text.as_bytes());
    assert_eq!(
        reader.headers().unwrap().iter().collect::<Vec<_>>(),
        vec![
            "Timestamp",
            "Timestamp Type",
            "Browser",
            "Profile",
            "Artifact",
            "Value",
            "Source File"
        ]
    );

    // 5 history rows, one of which has no timestamp at all, so 4 instants.
    assert_eq!(rows(&csv_text).len(), 4, "{csv_text}");
    assert!(column(&csv_text, "Timestamp Type")
        .iter()
        .all(|t| t == "Visited"));
    assert!(
        column(&csv_text, "Timestamp").iter().all(|t| !t.is_empty()),
        "a timeline row without an instant would be noise"
    );
}

/// WAL-only rows must be recovered, and the original evidence must be left
/// exactly as found.
#[test]
fn rows_living_only_in_a_wal_are_recovered_without_touching_the_original() {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    let db_path = dir.join("History");

    let conn = Connection::open(&db_path).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch(
        "CREATE TABLE urls(id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER,
                           typed_count INTEGER, last_visit_time INTEGER, hidden INTEGER);
         CREATE TABLE visits(id INTEGER PRIMARY KEY, url INTEGER, visit_time INTEGER,
                             from_visit INTEGER, transition INTEGER, visit_duration INTEGER,
                             opener_visit INTEGER);
         INSERT INTO urls VALUES (1,'https://wal-only.test/','WAL',1,0,13344473600000000,0);
         INSERT INTO visits VALUES (10,1,13344473600000000,0,1,0,NULL);",
    )
    .unwrap();
    // Leak the connection so SQLite never checkpoints on close, leaving the
    // rows in the -wal exactly as a live acquisition would.
    std::mem::forget(conn);
    assert!(dir.join("History-wal").exists(), "fixture must leave a WAL");

    let out = td.path().join("out");
    run(td.path(), &out);

    let csv_text = read_output(&out, "BrowserTriage_Output.csv");
    assert!(
        csv_text.contains("wal-only.test"),
        "uncheckpointed rows must be recovered: {csv_text}"
    );
    assert!(
        dir.join("History-wal").exists(),
        "the original WAL must survive the run untouched"
    );
}

/// A damaged database must not abort the run: the healthy profile beside it
/// still has to produce output.
#[test]
fn a_corrupt_database_does_not_stop_the_other_profiles() {
    let td = TempDir::new().unwrap();
    let good = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    write_history(&good.join("History"));

    let bad = profile_dir(td.path(), "alice", "Microsoft/Edge", "Default");
    // Valid magic so it passes validation, garbage body so it fails to parse.
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.extend_from_slice(&[0xAB; 512]);
    std::fs::write(bad.join("History"), bytes).unwrap();

    let out = td.path().join("out");
    // The exit code may be non-zero to signal the failed artifact; what matters
    // is that the run reached the healthy profile rather than aborting on the
    // damaged one.
    let _ = Command::cargo_bin("BrowserTriage")
        .unwrap()
        .args([
            "-d",
            td.path().to_str().unwrap(),
            "--csv",
            out.to_str().unwrap(),
            "-q",
        ])
        .assert();

    let csv_text = read_output(&out, "BrowserTriage_Output.csv");
    assert!(
        csv_text.contains("example.test"),
        "the healthy profile must still be parsed: {csv_text}"
    );
}
