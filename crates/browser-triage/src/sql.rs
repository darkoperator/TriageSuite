//! Total cell accessors and schema-tolerant projection.
//!
//! Two jobs, both in service of the completeness contract:
//!
//! * **No accessor can fail.** `SqliteValue::as_text` returns `None` for an
//!   `Integer`, so a column that normally holds text but holds a number in one
//!   row would silently become an empty cell. The accessors here always produce
//!   a value, so a cell can never cause a row to be skipped.
//! * **A renamed or absent column must not fail the artifact.** Chromium's
//!   schemas change across milestones; [`projection`] pads the missing ones with
//!   `NULL` so the result set keeps a fixed shape on every browser version.

use triage_sqlite::{Database, SqliteValue};

/// The Unicode replacement character, which `String::from_utf8_lossy` in
/// `triage-sqlite` substitutes for undecodable bytes.
const REPLACEMENT: char = '\u{FFFD}';

/// Total text accessor: every storage class renders to something.
///
/// `Null` is the empty string, numbers are stringified, and a blob is decoded
/// lossily. Deliberately never `Option`: an unexpected storage class is not a
/// reason to lose the row.
pub fn text(value: &SqliteValue) -> String {
    match value {
        SqliteValue::Null => String::new(),
        SqliteValue::Integer(i) => i.to_string(),
        SqliteValue::Real(f) => f.to_string(),
        SqliteValue::Text(s) => s.clone(),
        SqliteValue::Blob(b) => String::from_utf8_lossy(b).into_owned(),
    }
}

/// Total byte accessor for a column declared BLOB.
///
/// SQLite's type affinity is advisory, so a column declared BLOB can hand back
/// `Text` — and on a real Chrome profile most `encrypted_value` cells do.
/// Matching `Blob` alone silently treated 698 of 1899 encrypted cookies as
/// unencrypted, so this accepts every storage class that can carry bytes.
pub fn bytes(value: &SqliteValue) -> &[u8] {
    match value {
        SqliteValue::Blob(b) => b.as_slice(),
        SqliteValue::Text(s) => s.as_bytes(),
        _ => &[],
    }
}

/// Integer accessor that also accepts a numeric string, which SQLite's dynamic
/// typing allows in any column. `None` renders as an empty cell.
pub fn int(value: &SqliteValue) -> Option<i64> {
    match value {
        SqliteValue::Integer(i) => Some(*i),
        SqliteValue::Real(f) => Some(*f as i64),
        SqliteValue::Text(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub fn real(value: &SqliteValue) -> Option<f64> {
    match value {
        SqliteValue::Real(f) => Some(*f),
        SqliteValue::Integer(i) => Some(*i as f64),
        SqliteValue::Text(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `"True"` / `"False"`, or empty when the cell is null — rather than
/// defaulting a null to `False`, which would assert something we do not know.
pub fn bool_str(value: &SqliteValue) -> String {
    match int(value) {
        Some(0) => "False".to_string(),
        Some(_) => "True".to_string(),
        None => String::new(),
    }
}

/// A cell whose safe index is beyond the row's length reads as `Null` rather
/// than panicking. Rows always have the projected arity, so this is defensive.
pub fn cell(row: &[SqliteValue], index: usize) -> &SqliteValue {
    row.get(index).unwrap_or(&SqliteValue::Null)
}

/// Lowercased column names of `table`, empty when the table is absent.
///
/// `PRAGMA table_info` goes through the ordinary query API, so schema
/// introspection needs no addition to `triage-sqlite`.
pub fn columns(db: &Database, table: &str) -> Vec<String> {
    let sql = format!("PRAGMA table_info('{}')", table.replace('\'', "''"));
    match db.query(&sql) {
        // column 1 of table_info is the column name
        Ok(rows) => rows
            .iter()
            .map(|row| text(cell(row, 1)).to_ascii_lowercase())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// A fixed-arity SELECT list: each wanted column when it exists, `NULL AS <col>`
/// when it does not.
///
/// This is what lets one query serve every browser version — the caller indexes
/// by position and a column added in Chrome 114 simply reads as null on Chrome
/// 80. `wanted` is always `&'static str` from this crate, never data, so there
/// is no injection surface.
pub fn projection(cols: &[String], wanted: &[&str]) -> String {
    wanted
        .iter()
        .map(|name| {
            if cols.iter().any(|c| c == &name.to_ascii_lowercase()) {
                format!("\"{name}\"")
            } else {
                format!("NULL AS \"{name}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The first candidate column that actually exists, quoted and optionally
/// qualified with a table alias; `NULL` when none of them do.
///
/// For columns Chromium has *renamed* rather than added or removed —
/// `lower_term` became `normalized_term`, `secure` became `is_secure`. A
/// projection cannot help there, because naming either spelling makes the whole
/// statement fail on profiles carrying the other. Getting this wrong is silent:
/// the statement errors, the table is skipped with a warning, and the dataset
/// comes out empty.
pub fn alternatives(cols: &[String], candidates: &[&str], alias: Option<&str>) -> String {
    for name in candidates {
        if cols.iter().any(|c| c == &name.to_ascii_lowercase()) {
            return match alias {
                Some(a) => format!("{a}.\"{name}\""),
                None => format!("\"{name}\""),
            };
        }
    }
    "NULL".to_string()
}

/// Like [`projection`] but qualified with a table alias, for joined queries.
pub fn projection_aliased(cols: &[String], wanted: &[&str], alias: &str) -> String {
    wanted
        .iter()
        .map(|name| {
            if cols.iter().any(|c| c == &name.to_ascii_lowercase()) {
                format!("{alias}.\"{name}\"")
            } else {
                format!("NULL AS \"{name}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Accumulates the per-row `Notes` column: the mechanism that makes "never drop
/// a row" auditable. Empty on a clean row.
#[derive(Debug, Default, Clone)]
pub struct Notes(Vec<String>);

impl Notes {
    pub fn new() -> Self {
        Notes(Vec::new())
    }

    pub fn push(&mut self, note: impl Into<String>) {
        self.0.push(note.into());
    }

    /// Record undecodable bytes, which `triage-sqlite` has already replaced with
    /// U+FFFD by the time we see them. A legitimate U+FFFD in source data would
    /// false-positive here; surfacing that is better than hiding real mojibake.
    pub fn note_if_lossy(&mut self, field: &str, value: &str) {
        if value.contains(REPLACEMENT) {
            self.push(format!("{field}: undecodable bytes replaced"));
        }
    }

    pub fn into_string(self) -> String {
        self.0.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_total_across_every_storage_class() {
        assert_eq!(text(&SqliteValue::Null), "");
        assert_eq!(text(&SqliteValue::Integer(42)), "42");
        assert_eq!(text(&SqliteValue::Text("hi".into())), "hi");
        assert_eq!(text(&SqliteValue::Blob(b"ab".to_vec())), "ab");
    }

    /// SQLite's dynamic typing lets an integer live in a text column and vice
    /// versa; neither may cost us the row.
    #[test]
    fn int_accepts_numeric_text_and_rejects_the_rest() {
        assert_eq!(int(&SqliteValue::Integer(7)), Some(7));
        assert_eq!(int(&SqliteValue::Text(" 7 ".into())), Some(7));
        assert_eq!(int(&SqliteValue::Text("seven".into())), None);
        assert_eq!(int(&SqliteValue::Null), None);
    }

    /// A null boolean is unknown, not false.
    #[test]
    fn bool_str_distinguishes_null_from_false() {
        assert_eq!(bool_str(&SqliteValue::Integer(1)), "True");
        assert_eq!(bool_str(&SqliteValue::Integer(0)), "False");
        assert_eq!(bool_str(&SqliteValue::Null), "");
    }

    #[test]
    fn projection_pads_absent_columns_to_keep_arity_fixed() {
        let cols = vec!["creation_utc".to_string(), "host_key".to_string()];
        assert_eq!(
            projection(&cols, &["creation_utc", "host_key", "last_update_utc"]),
            "\"creation_utc\", \"host_key\", NULL AS \"last_update_utc\""
        );
    }

    #[test]
    fn projection_matches_column_names_case_insensitively() {
        let cols = vec!["visit_time".to_string()];
        assert_eq!(projection(&cols, &["Visit_Time"]), "\"Visit_Time\"");
    }

    /// The renamed-column case that silently emptied a whole dataset before
    /// this helper existed.
    #[test]
    fn alternatives_picks_whichever_spelling_the_schema_uses() {
        let modern = vec!["normalized_term".to_string()];
        let legacy = vec!["lower_term".to_string()];
        let neither: Vec<String> = vec![];

        let candidates = ["normalized_term", "lower_term"];
        assert_eq!(
            alternatives(&modern, &candidates, None),
            "\"normalized_term\""
        );
        assert_eq!(alternatives(&legacy, &candidates, None), "\"lower_term\"");
        assert_eq!(alternatives(&neither, &candidates, None), "NULL");
        assert_eq!(
            alternatives(&modern, &candidates, Some("k")),
            "k.\"normalized_term\""
        );
    }

    #[test]
    fn projection_aliased_qualifies_present_columns_and_pads_absent_ones() {
        let cols = vec!["id".to_string()];
        assert_eq!(
            projection_aliased(&cols, &["id", "gone"], "v"),
            "v.\"id\", NULL AS \"gone\""
        );
    }

    #[test]
    fn cell_beyond_the_row_reads_as_null() {
        let row = vec![SqliteValue::Integer(1)];
        assert_eq!(cell(&row, 0), &SqliteValue::Integer(1));
        assert_eq!(cell(&row, 5), &SqliteValue::Null);
    }

    #[test]
    fn notes_are_empty_on_a_clean_row_and_joined_otherwise() {
        assert_eq!(Notes::new().into_string(), "");
        let mut notes = Notes::new();
        notes.push("a");
        notes.push("b");
        assert_eq!(notes.into_string(), "a; b");
    }

    #[test]
    fn lossy_text_is_surfaced_rather_than_silently_kept() {
        let mut notes = Notes::new();
        notes.note_if_lossy("Title", "clean");
        assert_eq!(notes.clone().into_string(), "");
        notes.note_if_lossy("Title", "bro\u{FFFD}ken");
        assert!(notes.into_string().contains("Title"));
    }
}
