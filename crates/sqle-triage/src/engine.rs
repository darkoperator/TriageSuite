//! Decide which maps apply to a database and run their queries.

use std::path::Path;

use sha1::{Digest, Sha1};
use triage_sqlite::Database;

use crate::map::SqlMap;
use crate::render::render_cell;

/// One dataset's worth of output: a basename, its column headers (SQL aliases),
/// and rendered string rows.
#[derive(Debug, Clone)]
pub struct DatasetResult {
    pub basename: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Does `map` apply to this database? With `hunt`, identification is purely the
/// IdentifyQuery == IdentifyValue check (filename ignored). Without `hunt`, the
/// DB filename must also match the map's `FileName`.
pub fn map_applies(db: &Database, map: &SqlMap, db_filename: &str, hunt: bool) -> bool {
    if !hunt && !filename_matches(&map.file_name, db_filename) {
        return false;
    }
    if map.identify_query.trim().is_empty() {
        return false;
    }
    match db.query(&map.identify_query) {
        Ok(rows) => {
            let got = rows
                .first()
                .and_then(|r| r.first())
                .map(|v| render_cell(v, false))
                .unwrap_or_default();
            got.trim() == map.identify_value.trim()
        }
        Err(_) => false, // missing table / bad query -> not applicable
    }
}

/// Case-insensitive filename match. `FileName` is usually a single token; we
/// also accept a leading/trailing `*` wildcard and `;`/`|`-delimited lists.
fn filename_matches(pattern: &str, name: &str) -> bool {
    let name_l = name.to_ascii_lowercase();
    for part in pattern.split([';', '|']) {
        let p = part.trim().to_ascii_lowercase();
        if p.is_empty() {
            continue;
        }
        if let Some(stripped) = p.strip_prefix('*') {
            if name_l.ends_with(stripped.trim_start_matches('*')) {
                return true;
            }
        } else if let Some(stripped) = p.strip_suffix('*') {
            if name_l.starts_with(stripped) {
                return true;
            }
        } else if name_l == p {
            return true;
        }
    }
    false
}

/// Run every applicable map's queries against `db`, returning one
/// `DatasetResult` per (map, query). A query that errors warns and is skipped.
pub fn run_maps(
    db: &Database,
    maps: &[(String, SqlMap)],
    db_filename: &str,
    hunt: bool,
    noblob: bool,
) -> Vec<DatasetResult> {
    let mut out = Vec::new();
    for (map_file, map) in maps {
        if !map_applies(db, map, db_filename, hunt) {
            continue;
        }
        for q in &map.queries {
            match db.query_with_columns(&q.query) {
                Ok((columns, rows)) => {
                    let rendered: Vec<Vec<String>> = rows
                        .iter()
                        .map(|r| r.iter().map(|v| render_cell(v, noblob)).collect())
                        .collect();
                    out.push(DatasetResult {
                        basename: format!("{}_{}", map.csv_prefix, q.base_file_name),
                        columns,
                        rows: rendered,
                    });
                }
                Err(e) => {
                    eprintln!("SQLETriage: map {map_file} query '{}' failed: {e}", q.name);
                }
            }
        }
    }
    out
}

/// SHA-1 of a file's contents, for `--dedupe`. Returns `None` on read error.
pub fn file_sha1(path: &Path) -> Option<[u8; 20]> {
    let bytes = std::fs::read(path).ok()?;
    let mut h = Sha1::new();
    h.update(&bytes);
    Some(h.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::parse_map;
    use rusqlite::Connection;
    use std::path::Path;

    fn build_db(path: &Path) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(
            "CREATE TABLE moz_formhistory(id INTEGER, value TEXT);
             INSERT INTO moz_formhistory VALUES (1,'a'),(2,'b');",
        )
        .unwrap();
    }

    const MAP: &str = r#"
CSVPrefix: Firefox
FileName: formhistory.sqlite
IdentifyQuery: SELECT count(*) FROM sqlite_master WHERE type='table' AND (name='moz_formhistory');
IdentifyValue: 1
Queries:
    -
        Name: Firefox Form History
        Query: |
               SELECT id AS ID, value AS Value FROM moz_formhistory ORDER BY id ASC
        BaseFileName: FormHistory
"#;

    #[test]
    fn identifies_and_runs_map() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("formhistory.sqlite");
        build_db(&p);
        let db = Database::open(&p).unwrap();
        let map = parse_map(MAP).unwrap();
        let maps = vec![("Firefox_FormHistory.smap".to_string(), map)];

        let res = run_maps(&db, &maps, "formhistory.sqlite", true, false);
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].basename, "Firefox_FormHistory");
        assert_eq!(res[0].columns, vec!["ID".to_string(), "Value".to_string()]);
        assert_eq!(res[0].rows.len(), 2);
        assert_eq!(res[0].rows[0], vec!["1".to_string(), "a".to_string()]);
    }

    #[test]
    fn non_hunt_requires_filename_match() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("formhistory.sqlite");
        build_db(&p);
        let db = Database::open(&p).unwrap();
        let maps = vec![("m.smap".to_string(), parse_map(MAP).unwrap())];
        assert!(run_maps(&db, &maps, "other.db", false, false).is_empty());
        assert_eq!(
            run_maps(&db, &maps, "formhistory.sqlite", false, false).len(),
            1
        );
    }

    #[test]
    fn identify_mismatch_excludes_map() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("empty.db");
        let c = Connection::open(&p).unwrap();
        c.execute_batch("CREATE TABLE other(x);").unwrap();
        drop(c);
        let db = Database::open(&p).unwrap();
        let maps = vec![("m.smap".to_string(), parse_map(MAP).unwrap())];
        assert!(run_maps(&db, &maps, "formhistory.sqlite", true, false).is_empty());
    }
}
