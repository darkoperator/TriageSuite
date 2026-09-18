//! The inventory's column names must be the names a query will actually see,
//! so this replays DuckDB 1.5.4's own header handling. Each expectation here
//! was measured against that version, not read from documentation.

use std::io::Write;
use triage_core::output::duckdb::header::{duckdb_names, read_header};

fn write(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).expect("create");
    f.write_all(bytes).expect("write");
    path
}

/// DuckDB strips a UTF-8 BOM before the first header name. A reader that
/// does not would produce a column called "\u{feff}KeyName" that matches
/// nothing an override or a query names.
#[test]
fn a_utf8_bom_is_stripped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write(dir.path(), "bom.csv", b"\xef\xbb\xbfA,B\n1,2\n");
    assert_eq!(read_header(&path).expect("header"), vec!["A", "B"]);
}

/// A header field may be quoted and contain a newline. Reading the first
/// *line* would split it into two bogus columns; only parsing the first CSV
/// *record* is correct.
#[test]
fn a_multiline_quoted_header_field_is_one_column() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write(dir.path(), "ml.csv", b"A,\"multi\nline\",C\n1,2,3\n");
    assert_eq!(
        read_header(&path).expect("header"),
        vec!["A", "multi\nline", "C"]
    );
}

/// Measured against DuckDB 1.5.4: an empty name becomes `column<index>`
/// (0-based), and a name colliding case-insensitively with an earlier one is
/// suffixed. `A,,C,a` -> `A`, `column1`, `C`, `a_1`.
#[test]
fn empty_and_case_colliding_names_match_duckdbs_renaming() {
    let raw: Vec<String> = ["A", "", "C", "a"].iter().map(|s| s.to_string()).collect();
    assert_eq!(duckdb_names(&raw), vec!["A", "column1", "C", "a_1"]);
}

/// The suffix search must keep going when the suffixed name is itself taken,
/// or two columns would end up with the same name.
#[test]
fn a_taken_suffix_advances_to_the_next_free_one() {
    let raw: Vec<String> = ["A", "A_1", "a"].iter().map(|s| s.to_string()).collect();
    assert_eq!(duckdb_names(&raw), vec!["A", "A_1", "a_2"]);
}

/// An empty file has no header at all. That is a fact about the file, not an
/// error to propagate: the caller records it and drops the file from the
/// view while keeping its dataset's other files.
#[test]
fn an_empty_file_has_no_header() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write(dir.path(), "empty.csv", b"");
    assert!(read_header(&path).expect("header").is_empty());
}
