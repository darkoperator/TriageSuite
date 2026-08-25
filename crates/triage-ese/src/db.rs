//! `Database`: open an ESE file read-only (temp copy; revisions above the
//! supported ceiling rejected up front) and introspect its tables/columns via
//! `ese_parser_lib`.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use chrono::{DateTime, Utc};
use ese_parser_lib::ese_parser::EseParser;
use ese_parser_lib::ese_trait::{
    ESE_MoveFirst, ESE_MoveNext, ESE_coltypBit, ESE_coltypCurrency, ESE_coltypDateTime,
    ESE_coltypIEEEDouble, ESE_coltypIEEESingle, ESE_coltypLong, ESE_coltypLongLong,
    ESE_coltypLongText, ESE_coltypShort, ESE_coltypText, ESE_coltypUnsignedByte,
    ESE_coltypUnsignedLong, ESE_coltypUnsignedLongLong, ESE_coltypUnsignedShort, EseDb,
};
use triage_core::timestamp::WinTimestamp;

use crate::header;

const CACHE_SIZE: usize = 10;

/// Errors from the ESE layer. `UnsupportedRevision` is the rev-300 wall.
#[derive(Debug)]
pub enum EseError {
    /// Format revision newer than any parser supports (e.g. 300).
    UnsupportedRevision(u32),
    /// I/O or "not an ESE file".
    Io(std::io::Error),
    /// Parser-level failure (wraps ese_parser_lib's error string).
    Parse(String),
}

impl std::fmt::Display for EseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EseError::UnsupportedRevision(r) => write!(
                f,
                "unsupported ESE format revision {r} — newer than any available parser"
            ),
            EseError::Io(e) => write!(f, "{e}"),
            EseError::Parse(s) => write!(f, "{s}"),
        }
    }
}
impl std::error::Error for EseError {}

/// One column's metadata.
#[derive(Debug, Clone)]
pub struct EseColumn {
    pub name: String,
    pub id: u32,
    pub col_type: u32,
    pub code_page: u16,
}

/// An open ESE database (read-only, on a temp copy).
pub struct Database {
    parser: EseParser<BufReader<File>>,
    _work: tempfile::TempDir,
    dirty: bool,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("dirty", &self.dirty)
            .finish_non_exhaustive()
    }
}

impl Database {
    /// Open `path` honoring evidence integrity: reject rev > MAX_SUPPORTED_REVISION
    /// up front, copy the database to a temp dir, and load the copy. The temp set is
    /// removed on drop; evidence is never opened in place.
    pub fn open(path: &Path) -> Result<Database, EseError> {
        let rev = header::format_revision(path).map_err(EseError::Io)?;
        if rev > header::MAX_SUPPORTED_REVISION {
            return Err(EseError::UnsupportedRevision(rev));
        }
        let work = tempfile::tempdir().map_err(EseError::Io)?;
        let copy = work.path().join(path.file_name().unwrap_or_default());
        std::fs::copy(path, &copy).map_err(EseError::Io)?;
        let parser = EseParser::load_from_path(CACHE_SIZE, &copy)
            .map_err(|e| EseError::Parse(e.to_string()))?;
        let dirty = parser.is_dirty();
        Ok(Database {
            parser,
            _work: work,
            dirty,
        })
    }

    /// Whether the database header indicates an unclean shutdown.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// All table names.
    pub fn tables(&self) -> Result<Vec<String>, EseError> {
        self.parser
            .get_tables()
            .map_err(|e| EseError::Parse(e.to_string()))
    }

    /// True if a table with this exact name exists.
    pub fn table_exists(&self, name: &str) -> bool {
        self.tables()
            .map(|ts| ts.iter().any(|t| t == name))
            .unwrap_or(false)
    }

    /// Columns of a table.
    pub fn columns(&self, table: &str) -> Result<Vec<EseColumn>, EseError> {
        let cols = self
            .parser
            .get_columns(table)
            .map_err(|e| EseError::Parse(e.to_string()))?;
        Ok(cols
            .into_iter()
            .map(|c| EseColumn {
                name: c.name,
                id: c.id,
                col_type: c.typ,
                code_page: c.cp,
            })
            .collect())
    }

    /// Read every row of `table` as `EseValue`s in the table's column order.
    pub fn rows(&self, table: &str) -> Result<Vec<Vec<EseValue>>, EseError> {
        let cols = self.columns(table)?;
        let tid = self
            .parser
            .open_table(table)
            .map_err(|e| EseError::Parse(e.to_string()))?;
        let mut out = Vec::new();
        let mut has = self
            .parser
            .move_row(tid, ESE_MoveFirst)
            .map_err(|e| EseError::Parse(e.to_string()))?;
        while has {
            let mut row = Vec::with_capacity(cols.len());
            for c in &cols {
                row.push(self.read_cell(tid, c));
            }
            out.push(row);
            has = self
                .parser
                .move_row(tid, ESE_MoveNext)
                .map_err(|e| EseError::Parse(e.to_string()))?;
        }
        self.parser.close_table(tid);
        Ok(out)
    }

    // The ese_parser_lib coltype constants (e.g. `ESE_coltypText`) are not
    // SCREAMING_CASE, which the lint flags when they're used as match patterns.
    #[allow(non_upper_case_globals)]
    fn read_cell(&self, tid: u64, col: &EseColumn) -> EseValue {
        match col.col_type {
            ESE_coltypDateTime => match self.parser.get_column_date(tid, col.id) {
                Ok(Some(d)) => EseValue::DateTime(date_to_wints(d)),
                _ => EseValue::Null,
            },
            ESE_coltypText | ESE_coltypLongText => {
                match self.parser.get_column_str(tid, col.id, col.code_page) {
                    Ok(Some(s)) => EseValue::Text(s.trim_end_matches('\0').to_owned()),
                    _ => EseValue::Null,
                }
            }
            other => match self.parser.get_column(tid, col.id) {
                Ok(Some(b)) => decode_scalar(other, &b),
                _ => EseValue::Null,
            },
        }
    }
}

/// One ESE cell value, normalized.
#[derive(Debug, Clone, PartialEq)]
pub enum EseValue {
    Null,
    Int(i64),
    UInt(u64),
    Double(f64),
    Text(String),
    DateTime(WinTimestamp),
    Bytes(Vec<u8>),
}

impl EseValue {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            EseValue::Int(i) => Some(*i),
            EseValue::UInt(u) => Some(*u as i64),
            _ => None,
        }
    }
    pub fn as_text(&self) -> Option<&str> {
        match self {
            EseValue::Text(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            EseValue::Bytes(b) => Some(b),
            _ => None,
        }
    }
    pub fn as_timestamp(&self) -> Option<&WinTimestamp> {
        match self {
            EseValue::DateTime(t) => Some(t),
            _ => None,
        }
    }
}

fn date_to_wints(d: DateTime<Utc>) -> WinTimestamp {
    WinTimestamp::from_unix_nanos(d.timestamp(), d.timestamp_subsec_nanos())
}

#[allow(non_upper_case_globals)]
fn decode_scalar(col_type: u32, b: &[u8]) -> EseValue {
    match col_type {
        ESE_coltypBit | ESE_coltypUnsignedByte if !b.is_empty() => EseValue::UInt(b[0] as u64),
        ESE_coltypShort if b.len() == 2 => EseValue::Int(i16::from_le_bytes([b[0], b[1]]) as i64),
        ESE_coltypUnsignedShort if b.len() == 2 => {
            EseValue::UInt(u16::from_le_bytes([b[0], b[1]]) as u64)
        }
        ESE_coltypLong if b.len() == 4 => {
            EseValue::Int(i32::from_le_bytes(b.try_into().unwrap()) as i64)
        }
        ESE_coltypUnsignedLong if b.len() == 4 => {
            EseValue::UInt(u32::from_le_bytes(b.try_into().unwrap()) as u64)
        }
        ESE_coltypLongLong | ESE_coltypCurrency if b.len() == 8 => {
            EseValue::Int(i64::from_le_bytes(b.try_into().unwrap()))
        }
        ESE_coltypUnsignedLongLong if b.len() == 8 => {
            EseValue::UInt(u64::from_le_bytes(b.try_into().unwrap()))
        }
        ESE_coltypIEEEDouble if b.len() == 8 => {
            EseValue::Double(f64::from_le_bytes(b.try_into().unwrap()))
        }
        ESE_coltypIEEESingle if b.len() == 4 => {
            EseValue::Double(f32::from_le_bytes(b.try_into().unwrap()) as f64)
        }
        _ => EseValue::Bytes(b.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn find_srudb(collection: &str) -> Option<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test captures");
        if !root.exists() {
            return None;
        }
        let mut stack = vec![root];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.file_name().and_then(|n| n.to_str()) == Some("SRUDB.dat")
                    && p.to_string_lossy().contains(collection)
                {
                    return Some(p);
                }
            }
        }
        None
    }

    #[test]
    fn opens_client_srudb_and_lists_tables() {
        let Some(p) = find_srudb("Collection-STCL1") else {
            return;
        };
        let db = Database::open(&p).expect("open rev-110 SRUDB");
        let tables = db.tables().unwrap();
        assert!(db.table_exists("SruDbIdMapTable"), "tables: {tables:?}");
        assert!(tables.iter().any(|t| t.contains("973F5D5C")));
    }

    #[test]
    fn rev_300_not_rejected_on_revision() {
        let Some(p) = find_srudb("Collection-STDC1") else {
            return;
        };
        // rev-300 is supported now; opening may still fail/skip because this SRUDB is
        // dirty, but it must never be rejected as an unsupported revision.
        if let Err(EseError::UnsupportedRevision(r)) = Database::open(&p) {
            panic!("rev {r} should no longer be rejected on revision grounds")
        }
    }

    #[test]
    fn reads_id_map_rows_typed() {
        let Some(p) = find_srudb("Collection-STCL1") else {
            return;
        };
        let db = Database::open(&p).unwrap();
        let rows = db.rows("SruDbIdMapTable").unwrap();
        assert!(!rows.is_empty());
        let cols = db.columns("SruDbIdMapTable").unwrap();
        let idx = |n: &str| cols.iter().position(|c| c.name == n).unwrap();
        let r0 = &rows[0];
        assert!(matches!(r0[idx("IdType")], EseValue::UInt(_)));
        assert!(matches!(r0[idx("IdIndex")], EseValue::Int(_)));
        assert!(matches!(
            r0[idx("IdBlob")],
            EseValue::Bytes(_) | EseValue::Null
        ));
    }

    #[test]
    fn datetime_column_reads_plausible_timestamp() {
        let Some(p) = find_srudb("Collection-STCL1") else {
            return;
        };
        let db = Database::open(&p).unwrap();
        // Find the 973F5D5C network-usage table (has a TimeStamp column)
        let tables = db.tables().unwrap();
        let net_table = tables.iter().find(|t| t.contains("973F5D5C")).cloned();
        let Some(net_table) = net_table else { return };
        let cols = db.columns(&net_table).unwrap();
        let ts_idx = cols.iter().position(|c| c.name == "TimeStamp");
        let Some(ts_idx) = ts_idx else { return };
        let rows = db.rows(&net_table).unwrap();
        // Find first row with a non-Null timestamp
        for row in &rows {
            if let EseValue::DateTime(ts) = &row[ts_idx] {
                let rendered = ts.to_string();
                // Expect a year in the 2020s range; rendered as ISO-8601
                assert!(
                    rendered.starts_with("202") || rendered.starts_with("201"),
                    "timestamp looks implausible: {rendered}"
                );
                return;
            }
        }
        // If all null, that's acceptable (no assert needed)
    }
}
