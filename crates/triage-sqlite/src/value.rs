//! Typed SQLite cell values and the `Database` query/introspection API.

use rusqlite::types::ValueRef;

use crate::db::Database;

/// One cell value read from a query, normalized to the five SQLite storage
/// classes. Tools match on this rather than juggling rusqlite generics.
#[derive(Debug, Clone, PartialEq)]
pub enum SqliteValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl SqliteValue {
    fn from_ref(v: ValueRef<'_>) -> SqliteValue {
        match v {
            ValueRef::Null => SqliteValue::Null,
            ValueRef::Integer(i) => SqliteValue::Integer(i),
            ValueRef::Real(f) => SqliteValue::Real(f),
            ValueRef::Text(t) => SqliteValue::Text(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Blob(b) => SqliteValue::Blob(b.to_vec()),
        }
    }

    /// Convenience: the text of a `Text` cell, else `None`.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            SqliteValue::Text(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Convenience: the integer of an `Integer` cell, else `None`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            SqliteValue::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// Convenience: the bytes of a `Blob` cell, else `None`.
    pub fn as_blob(&self) -> Option<&[u8]> {
        match self {
            SqliteValue::Blob(b) => Some(b.as_slice()),
            _ => None,
        }
    }
}

impl Database {
    /// True if a table (or view) with this exact name exists.
    pub fn table_exists(&self, name: &str) -> Result<bool, rusqlite::Error> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','view') AND name = ?1",
            [name],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Run a query and collect every row as a `Vec<SqliteValue>` (column order
    /// follows the SELECT). Suitable for the small Timeline tables; not a
    /// streaming API.
    pub fn query(&self, sql: &str) -> Result<Vec<Vec<SqliteValue>>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(sql)?;
        let col_count = stmt.column_count();
        let rows = stmt.query_map([], |row| {
            let mut cells = Vec::with_capacity(col_count);
            for i in 0..col_count {
                cells.push(SqliteValue::from_ref(row.get_ref(i)?));
            }
            Ok(cells)
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Like `query`, but also returns the result column names (the SQL aliases),
    /// in SELECT order. Used by the map engine to build dynamic CSV headers.
    pub fn query_with_columns(
        &self,
        sql: &str,
    ) -> Result<(Vec<String>, Vec<Vec<SqliteValue>>), rusqlite::Error> {
        let mut stmt = self.conn.prepare(sql)?;
        let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let col_count = stmt.column_count();
        let rows = stmt.query_map([], |row| {
            let mut cells = Vec::with_capacity(col_count);
            for i in 0..col_count {
                cells.push(SqliteValue::from_ref(row.get_ref(i)?));
            }
            Ok(cells)
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok((columns, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::path::Path;

    fn make(path: &Path) {
        let c = Connection::open(path).unwrap();
        c.execute_batch(
            "CREATE TABLE t(i INTEGER, r REAL, s TEXT, b BLOB, n);
             INSERT INTO t VALUES (7, 1.5, 'hi', x'00ff', NULL);",
        )
        .unwrap();
    }

    #[test]
    fn query_returns_typed_values_in_column_order() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("q.db");
        make(&p);
        let db = Database::open(&p).unwrap();
        let rows = db.query("SELECT i, r, s, b, n FROM t").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], SqliteValue::Integer(7));
        assert_eq!(rows[0][1], SqliteValue::Real(1.5));
        assert_eq!(rows[0][2], SqliteValue::Text("hi".into()));
        assert_eq!(rows[0][3], SqliteValue::Blob(vec![0x00, 0xff]));
        assert_eq!(rows[0][4], SqliteValue::Null);
    }

    #[test]
    fn table_exists_detects_presence_and_absence() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("e.db");
        make(&p);
        let db = Database::open(&p).unwrap();
        assert!(db.table_exists("t").unwrap());
        assert!(!db.table_exists("Activity_PackageId").unwrap());
    }

    #[test]
    fn query_with_columns_returns_aliases_and_rows() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.db");
        make(&p);
        let db = Database::open(&p).unwrap();
        let (cols, rows) = db
            .query_with_columns("SELECT i AS ID, s AS 'Display Name' FROM t")
            .unwrap();
        assert_eq!(cols, vec!["ID".to_string(), "Display Name".to_string()]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], SqliteValue::Integer(7));
        assert_eq!(rows[0][1], SqliteValue::Text("hi".into()));
    }
}
