//! Reading a published CSV's header back, exactly as DuckDB will see it.
//!
//! Two things make this more than `read_line`. A header field may be quoted
//! and contain a newline, so the first *record* has to be parsed as CSV. And
//! DuckDB renames empty and colliding names, so the inventory has to replay
//! that renaming or it would advertise column names no query can use.

use std::path::Path;

/// The raw header names of `path`, with a UTF-8 BOM stripped from the first.
///
/// An empty vector means the file has no header record at all -- an empty
/// file. That is a fact to record, not an error: the caller drops that one
/// file from its view and keeps the dataset's others.
pub fn read_header(path: &Path) -> std::io::Result<Vec<String>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_path(path)?;
    let mut record = csv::StringRecord::new();
    if !reader.read_record(&mut record).map_err(to_io)? {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = record.iter().map(|s| s.to_string()).collect();
    if let Some(first) = names.first_mut() {
        if let Some(stripped) = first.strip_prefix('\u{feff}') {
            *first = stripped.to_string();
        }
    }
    Ok(names)
}

fn to_io(error: csv::Error) -> std::io::Error {
    // `csv::ErrorKind` has no `Display` impl of its own; capture the
    // message from the original error before `into_kind` consumes it.
    let message = error.to_string();
    match error.into_kind() {
        csv::ErrorKind::Io(io) => io,
        _ => std::io::Error::other(message),
    }
}

/// Replay DuckDB 1.5.4's header renaming over `raw`.
///
/// An empty name becomes `column<0-based index>`. A name that collides
/// case-insensitively with one already taken gets `_1`, `_2`, ... until it
/// does not. Both measured, not assumed; see `tests/duckdb_header.rs`.
pub fn duckdb_names(raw: &[String]) -> Vec<String> {
    let mut taken: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(raw.len());
    for (index, name) in raw.iter().enumerate() {
        let base = if name.is_empty() {
            format!("column{index}")
        } else {
            name.clone()
        };
        let mut candidate = base.clone();
        let mut suffix = 1u32;
        while !taken.insert(candidate.to_lowercase()) {
            candidate = format!("{base}_{suffix}");
            suffix = suffix.saturating_add(1);
        }
        out.push(candidate);
    }
    out
}
