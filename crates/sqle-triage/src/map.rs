//! Parse SQLECmd `.smap` map files. A map describes how to identify a SQLite
//! database (FileName + IdentifyQuery/IdentifyValue) and the SQL queries to run
//! against it. Each query's result columns become CSV headers at runtime.

use include_dir::{include_dir, Dir};
use serde::Deserialize;

/// The bundled SQLECmd map corpus, embedded at compile time from the
/// workspace-root `resources/sqlite-maps/` directory (refreshed by `--sync`).
static MAPS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../resources/sqlite-maps");

/// Parse every bundled `.smap`. Maps that fail to parse are reported to stderr
/// (filename + error) and skipped — a single bad map never aborts the run.
pub fn load_bundled_maps() -> Vec<(String, SqlMap)> {
    let mut out = Vec::new();
    for f in MAPS_DIR.files() {
        let name = f
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        let Some(text) = f.contents_utf8() else {
            eprintln!("SQLETriage: map {name}: not valid UTF-8, skipping");
            continue;
        };
        match parse_map(text) {
            Ok(m) => out.push((name, m)),
            Err(e) => eprintln!("SQLETriage: map {name}: parse error: {e}"),
        }
    }
    out
}

/// A parsed SQLECmd map. Informational fields (Author, Email, Id, Version) are
/// tolerated but unused; unknown fields are ignored so future corpus additions
/// do not break loading.
#[derive(Debug, Clone, Deserialize)]
pub struct SqlMap {
    #[serde(rename = "Description", default)]
    pub description: String,
    #[serde(rename = "CSVPrefix", default)]
    pub csv_prefix: String,
    #[serde(rename = "FileName", default)]
    pub file_name: String,
    #[serde(rename = "IdentifyQuery", default)]
    pub identify_query: String,
    #[serde(rename = "IdentifyValue", default, deserialize_with = "de_stringish")]
    pub identify_value: String,
    #[serde(rename = "Queries", default)]
    pub queries: Vec<SqlQuery>,
}

/// One query within a map. `BaseFileName` (older maps: `BindToTable`) names the
/// output file component.
#[derive(Debug, Clone, Deserialize)]
pub struct SqlQuery {
    #[serde(rename = "Name", default)]
    pub name: String,
    #[serde(rename = "Query", default)]
    pub query: String,
    #[serde(rename = "BaseFileName", alias = "BindToTable", default)]
    pub base_file_name: String,
}

/// Deserialize a YAML scalar (string, number, or bool) into a String. SQLECmd
/// `IdentifyValue` is usually a bare number (e.g. `5`) which YAML parses as an
/// integer; we normalize everything to its string form for comparison.
fn de_stringish<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_yaml::Value::deserialize(d)?;
    Ok(match v {
        serde_yaml::Value::String(s) => s,
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        _ => String::new(),
    })
}

/// Parse one `.smap` YAML document.
pub fn parse_map(yaml: &str) -> Result<SqlMap, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
Description: Firefox Form History database
Author: Heather Mahalik
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
    fn parses_core_fields_and_query() {
        let m = parse_map(SAMPLE).unwrap();
        assert_eq!(m.csv_prefix, "Firefox");
        assert_eq!(m.file_name, "formhistory.sqlite");
        assert_eq!(m.identify_value, "1"); // numeric YAML scalar -> "1"
        assert_eq!(m.queries.len(), 1);
        assert_eq!(m.queries[0].base_file_name, "FormHistory");
        assert!(m.queries[0].query.contains("moz_formhistory"));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let y = "Description: x\nCSVPrefix: P\nFileName: f\nIdentifyQuery: SELECT 1\nIdentifyValue: 1\nSomethingNew: hello\nQueries: []\n";
        let m = parse_map(y).unwrap();
        assert_eq!(m.csv_prefix, "P");
        assert!(m.queries.is_empty());
    }

    #[test]
    fn bundled_corpus_loads_many_maps() {
        let maps = load_bundled_maps();
        assert!(
            maps.len() >= 50,
            "expected the bundled corpus, got {}",
            maps.len()
        );
        assert!(
            maps.iter().any(|(_, m)| m.csv_prefix == "ChromiumBrowser"),
            "ChromiumBrowser map should be in the corpus"
        );
        for (name, m) in &maps {
            assert!(!m.queries.is_empty(), "map {name} has no queries");
        }
    }
}
