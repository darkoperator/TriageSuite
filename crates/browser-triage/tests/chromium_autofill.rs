//! Chromium `Web Data` -> `autofill`.
//!
//! Driven by the epoch trap: these two timestamps are unix seconds while every
//! other Chromium table uses WebKit microseconds. Reading them as WebKit puts
//! every row in 1601 — wrong, but plausible enough to pass a casual review.

#![cfg(unix)]

mod support;

use rusqlite::Connection;
use std::path::Path;
use support::{column, profile_dir, read_output, rows, run};
use tempfile::TempDir;

fn write_web_data(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE autofill(name TEXT, value TEXT, value_lower TEXT,
                               date_created INTEGER, date_last_used INTEGER, count INTEGER);
         -- 1700000000 is unix seconds for 2023-11-14T22:13:20Z.
         INSERT INTO autofill VALUES
           ('email','user@example.test','user@example.test',1700000000,1700003600,4),
           ('search','how to disable defender','how to disable defender',1700000000,1700000000,1),
           ('never_used','typed once','typed once',1700000000,0,1);",
    )
    .unwrap();
}

fn setup() -> (TempDir, std::path::PathBuf) {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    write_web_data(&dir.join("Web Data"));
    let out = td.path().join("out");
    run(td.path(), &out);
    (td, out)
}

#[test]
fn every_autofill_entry_reaches_the_output() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Autofill.csv");
    assert_eq!(rows(&csv_text).len(), 3, "{csv_text}");
}

/// The trap. If these were read as WebKit microseconds every row would land in
/// 1601 rather than 2023.
#[test]
fn autofill_timestamps_are_unix_seconds_not_webkit_microseconds() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Autofill.csv");
    assert!(
        column(&csv_text, "First Used")
            .iter()
            .any(|t| t == "2023-11-14T22:13:20.0000000Z"),
        "unix seconds must decode to 2023: {csv_text}"
    );
    assert!(
        !csv_text.contains("1601-"),
        "a 1601 timestamp means the WebKit epoch was applied by mistake: {csv_text}"
    );
}

#[test]
fn field_name_value_and_use_count_are_preserved() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Autofill.csv");
    assert!(column(&csv_text, "Field Name").iter().any(|f| f == "email"));
    assert!(column(&csv_text, "Value")
        .iter()
        .any(|v| v == "user@example.test"));
    assert!(column(&csv_text, "Use Count").iter().any(|c| c == "4"));
}

/// An entry never used again has `date_last_used = 0`. The row stays, the cell
/// is empty.
#[test]
fn an_unused_entry_keeps_an_empty_last_used() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Autofill.csv");
    let mut reader = csv::Reader::from_reader(csv_text.as_bytes());
    let headers = reader.headers().unwrap().clone();
    let name_at = headers.iter().position(|h| h == "Field Name").unwrap();
    let last_at = headers.iter().position(|h| h == "Last Used").unwrap();
    let row = reader
        .records()
        .map(|r| r.unwrap())
        .find(|r| &r[name_at] == "never_used")
        .expect("the never-used entry must still be emitted");
    assert_eq!(&row[last_at], "");
}

#[test]
fn the_timeline_indexes_both_autofill_instants() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Timeline.csv");
    let kinds = column(&csv_text, "Timestamp Type");
    assert_eq!(
        kinds.iter().filter(|k| *k == "Autofill First Used").count(),
        3
    );
    assert_eq!(
        kinds.iter().filter(|k| *k == "Autofill Last Used").count(),
        2,
        "the never-used entry contributes no last-used row"
    );
    assert!(column(&csv_text, "Value")
        .iter()
        .any(|v| v == "email=user@example.test"));
}
