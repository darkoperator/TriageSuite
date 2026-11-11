//! The escape hatch: one read-only SELECT of the caller's own over the case,
//! and a description of what there is to select. All of the lockdown lives
//! in this file.
//!
//! The SQL text comes from outside, the data it reads is evidence an
//! attacker may have written, and an agent issuing it may be steered by that
//! evidence. So this path must be unable to change the case, read or write
//! any other file, reach the network or undo its own configuration. Each of
//! these layers is enough against a write on its own:
//!
//! 1. **One SELECT only.** DuckDB's own parser checks the text, on an
//!    in-memory instance with nothing attached: `json_serialize_sql`
//!    serializes SELECT statements only, so any other statement fails, and
//!    the parse must hold exactly one statement, so a second one after `;`
//!    (also one behind a comment) is refused. What runs is DuckDB's
//!    re-rendering of that one parsed SELECT, so comments and trailing text
//!    never reach execution.
//! 2. **A separate, locked instance.** A second in-memory instance with
//!    extension auto-install and auto-load off attaches the case
//!    `READ_ONLY`, then sets `enable_external_access = false` (no file,
//!    network or environment access) and `lock_configuration = true` (no
//!    `SET` can undo it). The lockdown is probed before anything runs; if a
//!    probe succeeds, nothing does.
//! 3. **Never a writer.** The case is only ever attached read-only.
//!
//! DuckDB keeps an attached database's own file readable with external
//! access off, so `read_blob` of the case file's path succeeds. That reads
//! nothing the query could not already select, and every other path fails.
//!
//! The case is attached as the default database, so the relations' views
//! are in `main`, the case tables in `ox` and loaded data in `src_<n>`, just
//! as they are to the engine itself. Attaching takes the file's lock, so the
//! caller must not hold the case open read-write meanwhile.

use super::page::clip_text;
use super::{ISO_FORMAT, MAX_LIMIT};
use crate::case::{check_format, is_lock_conflict};
use crate::error::{EngineError, Result};
use crate::query::CELL_CAP;
use duckdb::types::Value as SqlValue;
use duckdb::{params, params_from_iter, Connection};
use serde_json::Value;
use std::path::Path;
use triage_core::output::duckdb::sql::{quote_ident, quote_literal};

/// The alias the case is attached under.
const CASE_ALIAS: &str = "ox_case";

/// The columns' DuckDB type names whose values render as R1 UTC text.
const TIMESTAMP_TYPES: [&str; 5] = [
    "TIMESTAMP",
    "TIMESTAMP_S",
    "TIMESTAMP_MS",
    "TIMESTAMP_NS",
    "TIMESTAMP WITH TIME ZONE",
];

#[derive(Debug, Clone, serde::Serialize)]
pub struct SqlResult {
    pub columns: Vec<String>,
    /// Each column's DuckDB type name.
    pub types: Vec<String>,
    /// Each cell as text; a timestamp as UTC `YYYY-MM-DDTHH:MM:SS.ffffffZ`.
    pub rows: Vec<Vec<Option<String>>>,
    /// More rows exist past this page.
    pub truncated: bool,
    /// Some cell was longer than `CELL_CAP` characters and was cut.
    pub cells_clipped: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DescribedColumn {
    pub name: String,
    pub sql_type: String,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Described {
    pub schema: String,
    pub name: String,
    /// `table` or `view`.
    pub kind: String,
    pub comment: Option<String>,
    pub rows: Option<u64>,
    pub columns: Vec<DescribedColumn>,
}

/// Run one SELECT over the case at `case_path`: up to `limit` rows from
/// `offset`. Opens its own locked connection; `case_path` must not be open
/// read-write in this process. `Invalid` for anything that is not exactly
/// one SELECT (with DuckDB's message) or a `limit` over `MAX_LIMIT`;
/// `CaseLocked` when another connection holds the file.
pub fn query_sql(case_path: &Path, sql: &str, limit: u32, offset: u64) -> Result<SqlResult> {
    if limit > MAX_LIMIT {
        return Err(EngineError::Invalid(format!(
            "a query returns at most {MAX_LIMIT} rows at a time, not {limit}"
        )));
    }
    let select = the_select(sql)?;
    let conn = locked(case_path)?;
    run_select(&conn, &select, limit, offset)
}

/// The relation views in `main` and the case tables in `ox`, with their
/// comments, columns and row counts; only those named `name` when given
/// (`NotFound` when none is). Loaded data in `src_<n>` is not listed: the
/// views are how it is read. Opens its own locked connection, as
/// `query_sql` does.
pub fn describe(case_path: &Path, name: Option<&str>) -> Result<Vec<Described>> {
    let conn = locked(case_path)?;
    let mut out = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT schema_name, table_name, 'table', comment FROM duckdb_tables() \
             WHERE database_name = current_database() AND schema_name IN ('main', 'ox') \
               AND (?::VARCHAR IS NULL OR table_name = ?) \
             UNION ALL \
             SELECT schema_name, view_name, 'view', comment FROM duckdb_views() \
             WHERE database_name = current_database() AND schema_name IN ('main', 'ox') \
               AND NOT internal AND (?::VARCHAR IS NULL OR view_name = ?) \
             ORDER BY 1, 2",
        )?;
        let mut rows = stmt.query(params![name, name, name, name])?;
        while let Some(r) = rows.next()? {
            out.push(Described {
                schema: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                comment: non_empty(r.get(3)?),
                rows: None,
                columns: Vec::new(),
            });
        }
    }
    if out.is_empty() {
        if let Some(name) = name {
            return Err(EngineError::NotFound(format!(
                "no relation or case table named {name:?}"
            )));
        }
    }
    let mut columns = conn.prepare(
        "SELECT column_name, data_type, comment FROM duckdb_columns() \
         WHERE database_name = current_database() AND schema_name = ? AND table_name = ? \
         ORDER BY column_index",
    )?;
    for d in &mut out {
        let mut rows = columns.query(params![d.schema, d.name])?;
        while let Some(r) = rows.next()? {
            d.columns.push(DescribedColumn {
                name: r.get(0)?,
                sql_type: r.get(1)?,
                comment: non_empty(r.get(2)?),
            });
        }
        let n: i64 = conn.query_row(
            &format!(
                "SELECT count(*) FROM {}.{}",
                quote_ident(&d.schema),
                quote_ident(&d.name)
            ),
            [],
            |r| r.get(0),
        )?;
        d.rows = u64::try_from(n).ok();
    }
    Ok(out)
}

fn non_empty(text: Option<String>) -> Option<String> {
    text.filter(|t| !t.is_empty())
}

/// The one SELECT in `sql`, as DuckDB re-renders it; `Invalid` for anything
/// else. Runs on its own in-memory instance with nothing attached.
fn the_select(sql: &str) -> Result<String> {
    let parser = Connection::open_in_memory()?;
    let parsed: String =
        parser.query_row("SELECT json_serialize_sql(?::VARCHAR)", params![sql], |r| {
            r.get(0)
        })?;
    let v: Value = serde_json::from_str(&parsed).map_err(|e| {
        EngineError::Invalid(format!("the query parser returned invalid JSON: {e}"))
    })?;
    if v["error"] != Value::Bool(false) {
        return Err(EngineError::Invalid(format!(
            "a query is one SELECT statement: {}",
            v["error_message"].as_str().unwrap_or("not a SELECT")
        )));
    }
    let n = v["statements"].as_array().map_or(0, Vec::len);
    if n != 1 {
        return Err(EngineError::Invalid(format!(
            "a query is exactly one SELECT statement, not {n}"
        )));
    }
    let one = serde_json::json!({ "error": false, "statements": [v["statements"][0].clone()] });
    Ok(parser.query_row(
        "SELECT json_deserialize_sql(?::JSON)",
        params![one.to_string()],
        |r| r.get(0),
    )?)
}

/// A fresh in-memory instance with the case at `case_path` attached
/// read-only as the default database, locked down and probed.
fn locked(case_path: &Path) -> Result<Connection> {
    if !case_path.try_exists()? {
        return Err(EngineError::NotFound(format!(
            "no case file at {}",
            case_path.display()
        )));
    }
    // ATTACH cannot bind its path, so it is embedded as a quoted literal;
    // a lossy conversion could name a different file, so none is made.
    let file = case_path.to_str().ok_or_else(|| {
        EngineError::Invalid(format!(
            "the case path {} is not valid Unicode",
            case_path.display()
        ))
    })?;
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(
        "SET autoinstall_known_extensions = false; SET autoload_known_extensions = false; \
         SET allow_community_extensions = false;",
    )?;
    conn.execute_batch(&format!(
        "ATTACH {} AS {CASE_ALIAS} (READ_ONLY)",
        quote_literal(file)
    ))
    .map_err(|e| {
        if is_lock_conflict(&e) {
            EngineError::CaseLocked(case_path.to_path_buf())
        } else {
            e.into()
        }
    })?;
    conn.execute_batch(&format!("USE {CASE_ALIAS}"))?;
    check_format(&conn, case_path)?;
    conn.execute_batch("SET enable_external_access = false; SET lock_configuration = true;")?;
    // DuckDB keeps an attached database's own file readable with external
    // access off, so the case cannot be the file the probe tries to read:
    // the running executable is one that certainly exists and is not it.
    let exe = std::env::current_exe()?;
    probe_lockdown(&conn, &exe.to_string_lossy())?;
    Ok(conn)
}

/// Prove the lockdown holds on `conn`: each attempt to undo it or to reach
/// outside the process must fail, else `Invalid`. `existing` names a file
/// that exists and is not attached, so that reading it fails only because
/// access is off.
fn probe_lockdown(conn: &Connection, existing: &str) -> Result<()> {
    let probe_file =
        std::env::temp_dir().join(format!("oxidelens-sql-probe-{}.duckdb", std::process::id()));
    let probe_path = probe_file.to_string_lossy();
    let probes = [
        "SET enable_external_access = true".to_string(),
        "SET lock_configuration = false".to_string(),
        format!("SELECT * FROM read_blob({})", quote_literal(existing)),
        format!("SELECT * FROM glob({})", quote_literal(existing)),
        "SELECT getenv('PATH')".to_string(),
        format!("ATTACH {} AS ox_probe", quote_literal(&probe_path)),
    ];
    for probe in &probes {
        if conn.execute_batch(probe).is_ok() {
            let _ = conn.execute_batch("DETACH ox_probe");
            let _ = std::fs::remove_file(&probe_file);
            return Err(EngineError::Invalid(format!(
                "the query connection is not locked down ({probe} succeeded)"
            )));
        }
    }
    Ok(())
}

/// `select` (from `the_select`) on `conn`: up to `limit` rows from `offset`,
/// every cell as text.
fn run_select(conn: &Connection, select: &str, limit: u32, offset: u64) -> Result<SqlResult> {
    // The binder alone names and types the columns, so an empty result
    // still has them.
    let mut columns = Vec::new();
    let mut types = Vec::new();
    {
        let mut stmt = conn.prepare(&format!("DESCRIBE {select}"))?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            columns.push(r.get::<_, String>(0)?);
            types.push(r.get::<_, String>(1)?);
        }
    }

    // The result's columns are renamed by position, which reaches each one
    // even where the query gave two the same name. Each text is cut a
    // character past the cap in SQL, as a page's are.
    let aliases: Vec<String> = (0..columns.len())
        .map(|i| quote_ident(&format!("ox_c{i}")))
        .collect();
    let cells: Vec<String> = aliases
        .iter()
        .zip(&types)
        .map(|(c, t)| {
            let text = if TIMESTAMP_TYPES.contains(&t.as_str()) {
                format!(
                    "CASE WHEN isfinite(TRY_CAST({c} AS TIMESTAMP)) \
                     THEN strftime(TRY_CAST({c} AS TIMESTAMP), '{ISO_FORMAT}') \
                     ELSE CAST({c} AS VARCHAR) END"
                )
            } else {
                format!("CAST({c} AS VARCHAR)")
            };
            format!("left({text}, {})", CELL_CAP + 1)
        })
        .collect();
    let sql = format!(
        "SELECT {} FROM ({select}) AS ox_q({}) LIMIT ? OFFSET ?",
        cells.join(", "),
        aliases.join(", ")
    );
    let page_params = [
        SqlValue::BigInt(i64::from(limit) + 1),
        SqlValue::UBigInt(offset),
    ];

    let mut out = SqlResult {
        columns,
        types,
        rows: Vec::new(),
        truncated: false,
        cells_clipped: false,
    };
    let width = out.columns.len();
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(page_params))?;
    while let Some(r) = rows.next()? {
        if out.rows.len() == limit as usize {
            out.truncated = true;
            break;
        }
        let mut row = Vec::with_capacity(width);
        for i in 0..width {
            let (text, clipped) = clip_text(r.get(i)?);
            out.cells_clipped |= clipped;
            row.push(text);
        }
        out.rows.push(row);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_catches_an_open_connection() {
        let conn = Connection::open_in_memory().unwrap();
        let exe = std::env::current_exe().unwrap();
        let e = probe_lockdown(&conn, exe.to_str().unwrap()).unwrap_err();
        assert_eq!(e.code(), "invalid", "{e}");
    }

    #[test]
    fn the_probe_catches_a_locked_configuration_with_access_on() {
        // No SET gets through, so only the file probes can notice.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("SET lock_configuration = true;")
            .unwrap();
        let exe = std::env::current_exe().unwrap();
        let e = probe_lockdown(&conn, exe.to_str().unwrap()).unwrap_err();
        assert!(e.to_string().contains("read_blob"), "{e}");
    }

    #[test]
    fn the_probe_passes_a_locked_connection() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("SET enable_external_access = false; SET lock_configuration = true;")
            .unwrap();
        let exe = std::env::current_exe().unwrap();
        probe_lockdown(&conn, exe.to_str().unwrap()).unwrap();
    }

    #[test]
    fn only_one_select_parses_and_it_is_re_rendered() {
        let one = the_select("select 1 as a -- and then\n").unwrap();
        assert!(!one.contains("--"), "{one}");
        for bad in [
            "",
            ";",
            "select 1; select 2",
            "select 1; /* */ delete from t",
            "delete from t",
            "explain select 1",
            "\u{0}",
            "select '",
        ] {
            assert_eq!(
                the_select(bad).map_err(|e| e.code()),
                Err("invalid"),
                "{bad:?}"
            );
        }
    }
}
