//! Append a dynamic dataset (runtime columns) to `<basename>.csv` and, when a
//! JSON dir is given, `<basename>.json` (NDJSON). Files are opened in append
//! mode so multiple source DBs that resolve to the same identity + basename
//! merge; the CSV header is written only when the file is newly created.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

/// Write `rows` (already rendered to strings) under `basename` to whichever
/// output roots are configured. Files are opened in append mode so multiple
/// source DBs that resolve to the same identity + basename merge; the CSV
/// header is written only when the file is newly created. Returns the number
/// of data rows written (independent of how many sinks are active).
pub fn write_dataset(
    csv_dir: Option<&Path>,
    json_dir: Option<&Path>,
    basename: &str,
    columns: &[String],
    rows: &[Vec<String>],
) -> std::io::Result<u64> {
    // ── CSV ──────────────────────────────────────────────────────────────
    if let Some(csv_dir) = csv_dir {
        fs::create_dir_all(csv_dir)?;
        let csv_path = csv_dir.join(format!("{basename}.csv"));
        let needs_header = fs::metadata(&csv_path)
            .map(|m| m.len() == 0)
            .unwrap_or(true);
        let csv_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&csv_path)?;
        let mut w = csv::Writer::from_writer(csv_file);
        if needs_header {
            w.write_record(columns)?;
        }
        for row in rows {
            w.write_record(row)?;
        }
        w.flush()?;
    }

    // ── NDJSON ───────────────────────────────────────────────────────────
    if let Some(jdir) = json_dir {
        fs::create_dir_all(jdir)?;
        let json_path = jdir.join(format!("{basename}.json"));
        let mut jf = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&json_path)?;
        for row in rows {
            let mut obj = serde_json::Map::new();
            for (c, v) in columns.iter().zip(row.iter()) {
                obj.insert(c.clone(), serde_json::Value::String(v.clone()));
            }
            writeln!(
                jf,
                "{}",
                serde_json::to_string(&serde_json::Value::Object(obj))?
            )?;
        }
    }

    Ok(rows.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_header_once_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let csv = dir.path().join("csv");
        let json = dir.path().join("json");
        let cols = vec!["ID".to_string(), "Name".to_string()];

        let n = write_dataset(
            Some(&csv),
            Some(&json),
            "Firefox_FormHistory",
            &cols,
            &[vec!["1".into(), "a".into()]],
        )
        .unwrap();
        assert_eq!(n, 1);

        write_dataset(
            Some(&csv),
            Some(&json),
            "Firefox_FormHistory",
            &cols,
            &[vec!["2".into(), "b".into()]],
        )
        .unwrap();

        let text = fs::read_to_string(csv.join("Firefox_FormHistory.csv")).unwrap();
        assert_eq!(text, "ID,Name\n1,a\n2,b\n");

        let jtext = fs::read_to_string(json.join("Firefox_FormHistory.json")).unwrap();
        let lines: Vec<&str> = jtext.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], r#"{"ID":"1","Name":"a"}"#);
    }

    #[test]
    fn json_only_when_no_csv_dir() {
        let dir = tempfile::tempdir().unwrap();
        let json = dir.path().join("json");
        let cols = vec!["ID".to_string()];
        let n = write_dataset(None, Some(&json), "X_Y", &cols, &[vec!["1".into()]]).unwrap();
        assert_eq!(n, 1);
        let jtext = std::fs::read_to_string(json.join("X_Y.json")).unwrap();
        assert_eq!(jtext.trim(), r#"{"ID":"1"}"#);
        // No CSV dir created.
        assert!(!dir.path().join("csv").exists());
    }
}
