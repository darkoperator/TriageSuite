//! Chromium `keyword_search_terms`.
//!
//! The regression that drives this suite: Chromium renamed `lower_term` to
//! `normalized_term`, and hardcoding either name makes the whole query fail on
//! half the profiles in the wild — silently, because a per-table failure is a
//! warning rather than an error. Both spellings are therefore tested.

#![cfg(unix)]

mod support;

use rusqlite::Connection;
use std::path::Path;
use support::{column, profile_dir, read_output, rows, run};
use tempfile::TempDir;

/// `normalized_term` is the modern spelling (Chrome ~110+).
fn write_modern(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE urls(id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER,
                           typed_count INTEGER, last_visit_time INTEGER, hidden INTEGER);
         CREATE TABLE visits(id INTEGER PRIMARY KEY, url INTEGER, visit_time INTEGER,
                             from_visit INTEGER, transition INTEGER, visit_duration INTEGER,
                             opener_visit INTEGER);
         CREATE TABLE keyword_search_terms(keyword_id INTEGER, url_id INTEGER,
                                           term TEXT, normalized_term TEXT);

         INSERT INTO urls VALUES
           (1,'https://www.google.com/search?q=mimikatz','mimikatz - Google',2,0,13344473600000000,0);
         INSERT INTO visits VALUES
           (10,1,13344473600000000,0,5,0,NULL),
           (11,1,13344473700000000,0,5,0,NULL);
         INSERT INTO keyword_search_terms VALUES
           (2,1,'MimiKatz','mimikatz'),
           (2,999,'orphaned search','orphaned search');",
    )
    .unwrap();
}

/// `lower_term` is the historical spelling.
fn write_legacy(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE urls(id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER,
                           typed_count INTEGER, last_visit_time INTEGER, hidden INTEGER);
         CREATE TABLE visits(id INTEGER PRIMARY KEY, url INTEGER, visit_time INTEGER,
                             from_visit INTEGER, transition INTEGER, visit_duration INTEGER,
                             opener_visit INTEGER);
         CREATE TABLE keyword_search_terms(keyword_id INTEGER, url_id INTEGER,
                                           lower_term TEXT, term TEXT);

         INSERT INTO urls VALUES
           (1,'https://duckduckgo.com/?q=psexec','psexec',1,0,13344473600000000,0);
         INSERT INTO visits VALUES (10,1,13344473600000000,0,5,0,NULL);
         INSERT INTO keyword_search_terms VALUES (3,1,'psexec','PsExec');",
    )
    .unwrap();
}

fn setup(writer: fn(&Path)) -> (TempDir, std::path::PathBuf) {
    let td = TempDir::new().unwrap();
    let dir = profile_dir(td.path(), "alice", "Google/Chrome", "Default");
    writer(&dir.join("History"));
    let out = td.path().join("out");
    run(td.path(), &out);
    (td, out)
}

/// The regression: this produced zero rows against a real Chrome profile
/// because the query named a column that no longer exists.
#[test]
fn the_modern_normalized_term_column_is_read() {
    let (_td, out) = setup(write_modern);
    let csv_text = read_output(&out, "BrowserTriage_Output_KeywordSearches.csv");
    assert!(!rows(&csv_text).is_empty(), "no rows parsed: {csv_text}");
    assert!(column(&csv_text, "Search Term")
        .iter()
        .any(|t| t == "MimiKatz"));
    assert!(column(&csv_text, "Search Term (Lower)")
        .iter()
        .any(|t| t == "mimikatz"));
}

#[test]
fn the_legacy_lower_term_column_is_still_read() {
    let (_td, out) = setup(write_legacy);
    let csv_text = read_output(&out, "BrowserTriage_Output_KeywordSearches.csv");
    assert!(column(&csv_text, "Search Term")
        .iter()
        .any(|t| t == "PsExec"));
    assert!(column(&csv_text, "Search Term (Lower)")
        .iter()
        .any(|t| t == "psexec"));
}

/// Each visit to a search-results URL is a separate execution of that search,
/// which is the event an analyst is counting.
#[test]
fn one_row_is_emitted_per_search_execution() {
    let (_td, out) = setup(write_modern);
    let csv_text = read_output(&out, "BrowserTriage_Output_KeywordSearches.csv");
    let terms = column(&csv_text, "Search Term");
    assert_eq!(
        terms.iter().filter(|t| *t == "MimiKatz").count(),
        2,
        "two visits to the results page means two searches: {csv_text}"
    );
}

/// A term whose URL row is gone is exactly the case worth surfacing: the search
/// outlived the history that would otherwise prove it happened.
#[test]
fn a_term_whose_history_was_cleared_is_kept_and_explained() {
    let (_td, out) = setup(write_modern);
    let csv_text = read_output(&out, "BrowserTriage_Output_KeywordSearches.csv");
    let line = csv_text
        .lines()
        .find(|l| l.contains("orphaned search"))
        .expect("the orphaned term must survive");
    assert!(line.contains("no matching urls row"), "{line}");
    assert!(line.contains("outlived its visit rows"), "{line}");
}

#[test]
fn the_search_provider_is_projected_from_the_url() {
    let (_td, out) = setup(write_modern);
    let csv_text = read_output(&out, "BrowserTriage_Output_KeywordSearches.csv");
    assert!(column(&csv_text, "Search Engine Host")
        .iter()
        .any(|h| h == "www.google.com"));
}

/// The bare term is what makes a timeline searchable, so it is indexed
/// separately from the History row covering the same visit.
#[test]
fn searches_are_indexed_on_the_timeline_by_term() {
    let (_td, out) = setup(write_modern);
    let csv_text = read_output(&out, "BrowserTriage_Output_Timeline.csv");
    let mut reader = csv::Reader::from_reader(csv_text.as_bytes());
    let headers = reader.headers().unwrap().clone();
    let kind_at = headers.iter().position(|h| h == "Timestamp Type").unwrap();
    let value_at = headers.iter().position(|h| h == "Value").unwrap();
    let searches: Vec<String> = reader
        .records()
        .map(|r| r.unwrap())
        .filter(|r| &r[kind_at] == "Search")
        .map(|r| r[value_at].to_string())
        .collect();
    assert_eq!(searches.iter().filter(|v| *v == "MimiKatz").count(), 2);
}
