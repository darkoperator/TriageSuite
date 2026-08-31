//! Chromium `downloads` parsing.
//!
//! The columns asserted here are the ones the previously-rejected tool never
//! selected at all — end time, received bytes, MIME type, referrer, tab URL and
//! the redirect chain — plus the orphan-chain and in-progress completeness
//! cases.

#![cfg(unix)]

mod support;

use rusqlite::Connection;
use std::path::Path;
use support::{column, profile_dir, read_output, rows, run};
use tempfile::TempDir;

/// A downloads table with, deliberately:
///   * a completed download that arrived through a two-hop redirect,
///   * an in-progress download whose `end_time` is 0,
///   * a `downloads_url_chains` group whose parent row was deleted.
fn write_downloads(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE downloads(
            id INTEGER PRIMARY KEY, guid TEXT, current_path TEXT, target_path TEXT,
            start_time INTEGER, received_bytes INTEGER, total_bytes INTEGER,
            state INTEGER, danger_type INTEGER, interrupt_reason INTEGER, hash BLOB,
            end_time INTEGER, opened INTEGER, last_access_time INTEGER, referrer TEXT,
            site_url TEXT, tab_url TEXT, tab_referrer_url TEXT, by_ext_id TEXT,
            by_ext_name TEXT, etag TEXT, last_modified TEXT, mime_type TEXT,
            original_mime_type TEXT);
         CREATE TABLE downloads_url_chains(id INTEGER, chain_index INTEGER, url TEXT);

         INSERT INTO downloads VALUES
           (1,'guid-1','C:\\Users\\a\\Downloads\\tool.exe.crdownload',
            'C:\\Users\\a\\Downloads\\tool.exe',
            13344473600000000, 1024, 1024, 1, 7, 0, X'00ff',
            13344473660000000, 1, 13344473700000000, 'https://ref.test/page',
            'https://site.test/', 'https://tab.test/', 'https://tabref.test/',
            'ext-id-1','Some Extension','\"abc\"','Wed, 21 Oct 2015 07:28:00 GMT',
            'application/x-msdownload','application/octet-stream'),
           (2,'guid-2','C:\\Users\\a\\Downloads\\big.iso.crdownload',
            'C:\\Users\\a\\Downloads\\big.iso',
            13344473800000000, 500, 9999, 0, 0, 0, X'',
            0, 0, 0, '', '', '', '', '', '', '', '', 'application/octet-stream','');

         INSERT INTO downloads_url_chains VALUES
           (1,0,'https://origin.test/start'),
           (1,1,'https://cdn.test/hop'),
           (1,2,'https://final.test/tool.exe'),
           (2,0,'https://slow.test/big.iso'),
           (77,0,'https://deleted.test/gone.zip');",
    )
    .unwrap();
}

fn setup() -> (TempDir, std::path::PathBuf) {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    write_downloads(&dir.join("History"));
    let out = td.path().join("out");
    run(td.path(), &out);
    (td, out)
}

#[test]
fn every_download_and_every_orphan_chain_reaches_the_output() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Downloads.csv");
    assert_eq!(rows(&csv_text).len(), 3, "{csv_text}");

    let types = column(&csv_text, "Record Type");
    assert_eq!(types.iter().filter(|t| *t == "Download").count(), 2);
    assert_eq!(types.iter().filter(|t| *t == "Orphan URL Chain").count(), 1);
}

/// A chain group whose parent download row was deleted still holds the URLs,
/// and an INNER JOIN would have thrown them away.
#[test]
fn an_orphaned_url_chain_is_kept_and_explained() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Downloads.csv");
    let line = csv_text
        .lines()
        .find(|l| l.contains("deleted.test"))
        .expect("the orphan chain must survive");
    assert!(line.contains("Orphan URL Chain"), "{line}");
    assert!(line.contains("no matching downloads row"), "{line}");
}

/// The whole point of reading `downloads_url_chains`: the target path says
/// nothing about where the file really came from.
#[test]
fn the_full_redirect_chain_is_preserved_in_order() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Downloads.csv");
    let chains = column(&csv_text, "URL Chain");
    assert!(
        chains.iter().any(|c| c
            == "https://origin.test/start -> https://cdn.test/hop -> https://final.test/tool.exe"),
        "{chains:?}"
    );
    assert!(
        column(&csv_text, "Download URL")
            .iter()
            .any(|u| u == "https://origin.test/start"),
        "the first hop is the originating URL"
    );
}

/// These are precisely the columns the rejected external tool never selected.
#[test]
fn the_columns_the_previous_tool_omitted_are_all_populated() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Downloads.csv");
    for (name, expected) in [
        ("End Time", "2023-11-14T22:14:20.0000000Z"),
        ("Received Bytes", "1024"),
        ("MIME Type", "application/x-msdownload"),
        ("Original MIME Type", "application/octet-stream"),
        ("Referrer", "https://ref.test/page"),
        ("Tab URL", "https://tab.test/"),
        ("Opened", "True"),
        ("ETag", "\"abc\""),
        ("Last Modified Header", "Wed, 21 Oct 2015 07:28:00 GMT"),
        ("Hash (SHA-256)", "00ff"),
        ("By Extension", "Some Extension (ext-id-1)"),
    ] {
        assert!(
            column(&csv_text, name).iter().any(|v| v == expected),
            "{name} should contain {expected:?}: {:?}",
            column(&csv_text, name)
        );
    }
}

#[test]
fn state_and_danger_type_are_decoded() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Downloads.csv");
    assert!(column(&csv_text, "State").iter().any(|s| s == "Complete"));
    assert!(column(&csv_text, "State")
        .iter()
        .any(|s| s == "In Progress"));
    assert!(column(&csv_text, "Danger Type")
        .iter()
        .any(|d| d == "Dangerous Host"));
}

/// An unfinished download has `end_time = 0`. The row stays and the cell is
/// empty — never the 1601 epoch, and never filtered out.
#[test]
fn an_in_progress_download_keeps_an_empty_end_time() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Downloads.csv");
    let mut reader = csv::Reader::from_reader(csv_text.as_bytes());
    let headers = reader.headers().unwrap().clone();
    let target_at = headers.iter().position(|h| h == "Target Path").unwrap();
    let end_at = headers.iter().position(|h| h == "End Time").unwrap();
    let row = reader
        .records()
        .map(|r| r.unwrap())
        .find(|r| r[target_at].contains("big.iso"))
        .expect("the in-progress download must be present");
    assert_eq!(&row[end_at], "", "unset must be empty, not an epoch");
}

/// A download has up to three instants, and each is its own timeline event.
#[test]
fn the_timeline_fans_out_one_row_per_download_timestamp() {
    let (_td, out) = setup();
    let csv_text = read_output(&out, "BrowserTriage_Output_Timeline.csv");
    let kinds = column(&csv_text, "Timestamp Type");
    assert_eq!(
        kinds.iter().filter(|k| *k == "Download Started").count(),
        2,
        "both downloads started"
    );
    assert_eq!(
        kinds.iter().filter(|k| *k == "Download Completed").count(),
        1,
        "only one finished, and the unfinished one contributes no row"
    );
    assert_eq!(
        kinds
            .iter()
            .filter(|k| *k == "Download Last Accessed")
            .count(),
        1
    );
    assert!(
        column(&csv_text, "Value")
            .iter()
            .any(|v| v.contains("tool.exe")),
        "the timeline pivots on the file on disk"
    );
}
