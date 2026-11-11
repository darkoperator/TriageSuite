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
//! 2. **Only computing functions.** The parse tree is walked and refused
//!    when any of these holds:
//!    - It reads from a table function (in FROM at any depth: top level,
//!      subquery, CTE, lateral join) that is not on the allowlist (ruling
//!      R17). Each allowed function computes rows from its arguments alone:
//!      - `range` and `generate_series`: a sequence of numbers or timestamps
//!        between two bounds;
//!      - `unnest`: the elements of a list or struct value it is given;
//!      - `generate_subscripts`: the indexes of a list it is given.
//!
//!      Everything else is refused, which includes the file readers, the
//!      statement-in-a-value indirections (`query`, `query_table`,
//!      `json_execute_serialized_sql`), the logging, profiling and
//!      checkpoint controls, `arrow_scan` (raw pointers) and the catalog
//!      functions. A table function a DuckDB upgrade adds is refused until
//!      someone allows it.
//!    - It calls, in any position, a name starting `read_`, `parquet_` or
//!      `arrow_scan`, or one of `getenv`, `glob`, `sniff_csv`, `query`,
//!      `query_table`, `json_execute_serialized_sql`, `enable_logging`,
//!      `disable_logging`, `truncate_duckdb_logs`, `enable_profiling`,
//!      `disable_profiling`, `checkpoint`, `force_checkpoint`. They read
//!      files or the environment, hide a statement in a value, or change the
//!      instance's state. A scalar of one of these names is refused like the
//!      table function.
//!    - It names a table with a path character (`.`, `/`, `\`, `:`), which
//!      DuckDB would answer with a replacement scan of that file.
//!
//!    This layer exists because the next one has a gap: DuckDB keeps an
//!    attached database's own files readable with external access off, so
//!    without it `read_blob` of the case file or its write-ahead log would
//!    return raw pages, free blocks of deleted notes and removed sources
//!    included.
//! 3. **A separate, locked instance.** A second in-memory instance with
//!    extension auto-install and auto-load off and no temporary directory
//!    (nothing spills to disk; a query needing more than `SQL_MEMORY_LIMIT`
//!    fails) attaches the case `READ_ONLY`, then sets
//!    `enable_external_access = false` (no file, network or environment
//!    access) and `lock_configuration = true` (no `SET` can undo it). The
//!    lockdown is probed before anything runs; if a probe succeeds or a
//!    setting did not take, nothing does.
//! 4. **Never a writer.** The case is only ever attached read-only.
//!
//! What a query reaches, then: the case's tables and views by name,
//! scalar functions (scalars such as `current_setting` included), and
//! the allowed table functions. No catalog table functions
//! (`duckdb_tables()` and the like; `describe` is how a caller learns the
//! schema), no file (the case file's own bytes included), no network and no
//! environment.
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

/// The most memory one query may use. Nothing spills to disk, so a query
/// that needs more fails rather than writing a temporary file.
const SQL_MEMORY_LIMIT: &str = "2GiB";

/// What the locked instance must report for each setting, as
/// `duckdb_settings()` spells it, checked after the lockdown.
const LOCKED_SETTINGS: [(&str, &str); 5] = [
    ("temp_directory", ""),
    ("max_temp_directory_size", "0 bytes"),
    ("memory_limit", "2.0 GiB"),
    ("enable_external_access", "false"),
    ("lock_configuration", "true"),
];

/// The only table functions a query may use in a FROM position (ruling
/// R17): each computes rows from its arguments alone. The module doc says
/// why each is safe.
const ALLOWED_TABLE_FUNCTIONS: [&str; 4] =
    ["range", "generate_series", "unnest", "generate_subscripts"];

/// Name prefixes no call may have, in any position: file readers and the
/// pointer-taking Arrow scans.
const REFUSED_PREFIXES: [&str; 3] = ["read_", "parquet_", "arrow_scan"];

/// Functions no call may name, in any position, besides `REFUSED_PREFIXES`:
/// they read files or the environment, run a statement hidden in a value,
/// or change the instance's state. A table function is already refused by
/// the allowlist; these are refused by name as well so that a scalar of the
/// same name, now or after an upgrade, is refused too.
const REFUSED_FUNCTIONS: [&str; 13] = [
    "getenv",
    "glob",
    "sniff_csv",
    "query",
    "query_table",
    "json_execute_serialized_sql",
    "enable_logging",
    "disable_logging",
    "truncate_duckdb_logs",
    "enable_profiling",
    "disable_profiling",
    "checkpoint",
    "force_checkpoint",
];

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
    if let Some(why) = reaches_outside(&v["statements"][0]) {
        return Err(EngineError::Invalid(why));
    }
    let one = serde_json::json!({ "error": false, "statements": [v["statements"][0].clone()] });
    Ok(parser.query_row(
        "SELECT json_deserialize_sql(?::JSON)",
        params![one.to_string()],
        |r| r.get(0),
    )?)
}

/// Why the parsed statement `node` would reach outside the case, if it
/// would: the first table function it uses that is not allowed, the first
/// call it makes to a refused name (both in any case and under any schema),
/// or the first table it names with a path character.
fn reaches_outside(node: &Value) -> Option<String> {
    match node {
        Value::Object(fields) => {
            if let Some(Value::String(name)) = fields.get("function_name") {
                if is_refused_call(name) {
                    return Some(format!(
                        "a query may not call {}: it reaches outside the case",
                        name.to_ascii_lowercase()
                    ));
                }
            }
            if fields.get("type").and_then(Value::as_str) == Some("TABLE_FUNCTION") {
                // Fail closed on a table function whose name the parse does
                // not spell out where it is expected.
                let name = fields
                    .get("function")
                    .and_then(|f| f.get("function_name"))
                    .and_then(Value::as_str);
                match name {
                    Some(name) if is_allowed_table_function(name) => {}
                    Some(name) => {
                        return Some(format!(
                            "a query may not read from the table function {}; \
                             only {} are allowed",
                            name.to_ascii_lowercase(),
                            ALLOWED_TABLE_FUNCTIONS.join(", ")
                        ))
                    }
                    None => return Some("a query may not use that table function".to_string()),
                }
            }
            if fields.get("type").and_then(Value::as_str) == Some("BASE_TABLE") {
                if let Some(Value::String(table)) = fields.get("table_name") {
                    if table.contains(['.', '/', '\\', ':']) {
                        return Some(format!(
                            "a query may not name the table {table:?}: \
                             a name with a path character reads a file"
                        ));
                    }
                }
            }
            fields.values().find_map(reaches_outside)
        }
        Value::Array(items) => items.iter().find_map(reaches_outside),
        _ => None,
    }
}

/// `name` (any case) is on `ALLOWED_TABLE_FUNCTIONS`.
fn is_allowed_table_function(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ALLOWED_TABLE_FUNCTIONS.contains(&name.as_str())
}

/// `name` (any case) is refused wherever it is called.
fn is_refused_call(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    REFUSED_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
        || REFUSED_FUNCTIONS.contains(&name.as_str())
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
    // DuckDB's default temporary directory is `.tmp` under the process's
    // current directory, which may be an evidence folder, and it stays
    // writable with external access off. With none, nothing spills.
    conn.execute_batch(&format!(
        "SET autoinstall_known_extensions = false; SET autoload_known_extensions = false; \
         SET allow_community_extensions = false; \
         SET temp_directory = ''; SET max_temp_directory_size = '0B'; \
         SET memory_limit = {};",
        quote_literal(SQL_MEMORY_LIMIT)
    ))?;
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
        "SET temp_directory = '.tmp'".to_string(),
        format!("SELECT * FROM read_blob({})", quote_literal(existing)),
        format!("SELECT * FROM glob({})", quote_literal(existing)),
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
    let mut setting = conn.prepare("SELECT value FROM duckdb_settings() WHERE name = ?")?;
    for (name, expected) in LOCKED_SETTINGS {
        let value: Option<String> =
            setting
                .query_row(params![name], |r| r.get(0))
                .or_else(|e| match e {
                    duckdb::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
        if value.as_deref() != Some(expected) {
            return Err(EngineError::Invalid(format!(
                "the query connection is not locked down ({name} is {value:?}, not {expected:?})"
            )));
        }
    }
    Ok(())
}

/// DuckDB's out-of-memory failure, which here means the query needs more
/// than `SQL_MEMORY_LIMIT`, as `Invalid`; any other error as it is.
fn out_of_memory(e: duckdb::Error) -> EngineError {
    if e.to_string().contains("Out of Memory Error") {
        EngineError::Invalid(format!(
            "the query needs more memory than a query may use here ({SQL_MEMORY_LIMIT}); \
             narrow it or aggregate it further"
        ))
    } else {
        e.into()
    }
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
    let mut stmt = conn.prepare(&sql).map_err(out_of_memory)?;
    let mut rows = stmt
        .query(params_from_iter(page_params))
        .map_err(out_of_memory)?;
    while let Some(r) = rows.next().map_err(out_of_memory)? {
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

    const NO_SPILL: &str =
        "SET temp_directory = ''; SET max_temp_directory_size = '0B'; SET memory_limit = '2GiB';";
    const LOCK: &str = "SET enable_external_access = false; SET lock_configuration = true;";

    #[test]
    fn the_probe_passes_a_locked_connection() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(NO_SPILL).unwrap();
        conn.execute_batch(LOCK).unwrap();
        let exe = std::env::current_exe().unwrap();
        probe_lockdown(&conn, exe.to_str().unwrap()).unwrap();
    }

    #[test]
    fn the_probe_catches_a_lockdown_that_can_still_spill() {
        // Locked, but with DuckDB's default temporary directory and limit.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(LOCK).unwrap();
        let exe = std::env::current_exe().unwrap();
        let e = probe_lockdown(&conn, exe.to_str().unwrap()).unwrap_err();
        assert!(e.to_string().contains("temp_directory"), "{e}");
        // And each spill setting on its own is checked.
        for (keep_default, setting) in [
            ("SET temp_directory = '';", "max_temp_directory_size"),
            ("SET max_temp_directory_size = '0B';", "temp_directory"),
        ] {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(keep_default).unwrap();
            conn.execute_batch("SET memory_limit = '2GiB';").unwrap();
            conn.execute_batch(LOCK).unwrap();
            let e = probe_lockdown(&conn, exe.to_str().unwrap()).unwrap_err();
            assert!(e.to_string().contains(setting), "{e}");
        }
    }

    /// Every table function and table macro the pinned DuckDB has, as
    /// `duckdb_functions()` lists them. An upgrade that adds or removes one
    /// fails `every_table_function_is_classified` until this list, and the
    /// allowlist if the new one computes from its arguments alone, are
    /// brought up to date.
    const KNOWN_TABLE_FUNCTIONS: &[&str] = &[
        "arrow_scan",
        "arrow_scan_dumb",
        "checkpoint",
        "disable_logging",
        "disable_profiling",
        "duckdb_approx_database_count",
        "duckdb_columns",
        "duckdb_connection_count",
        "duckdb_constraints",
        "duckdb_coordinate_systems",
        "duckdb_databases",
        "duckdb_dependencies",
        "duckdb_extensions",
        "duckdb_external_file_cache",
        "duckdb_functions",
        "duckdb_indexes",
        "duckdb_keywords",
        "duckdb_log_contexts",
        "duckdb_logs",
        "duckdb_logs_parsed",
        "duckdb_memory",
        "duckdb_optimizers",
        "duckdb_prepared_statements",
        "duckdb_profiling_settings",
        "duckdb_schemas",
        "duckdb_secret_types",
        "duckdb_secrets",
        "duckdb_sequences",
        "duckdb_settings",
        "duckdb_table_sample",
        "duckdb_tables",
        "duckdb_temporary_files",
        "duckdb_types",
        "duckdb_variables",
        "duckdb_views",
        "enable_logging",
        "enable_profiling",
        "force_checkpoint",
        "generate_series",
        "glob",
        "histogram",
        "histogram_values",
        "json_each",
        "json_execute_serialized_sql",
        "json_tree",
        "pragma_collations",
        "pragma_database_size",
        "pragma_metadata_info",
        "pragma_platform",
        "pragma_show",
        "pragma_storage_info",
        "pragma_table_info",
        "pragma_user_agent",
        "pragma_version",
        "query",
        "query_table",
        "range",
        "read_blob",
        "read_csv",
        "read_csv_auto",
        "read_duckdb",
        "read_json",
        "read_json_auto",
        "read_json_objects",
        "read_json_objects_auto",
        "read_ndjson",
        "read_ndjson_auto",
        "read_ndjson_objects",
        "read_text",
        "repeat",
        "repeat_row",
        "seq_scan",
        "sniff_csv",
        "summary",
        "test_all_types",
        "test_vector_types",
        "truncate_duckdb_logs",
        "unnest",
        "which_secret",
    ];

    #[test]
    fn every_table_function_is_classified() {
        let conn = Connection::open_in_memory().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT function_name FROM duckdb_functions() \
                 WHERE function_type IN ('table', 'table_macro') ORDER BY 1",
            )
            .unwrap();
        let present: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            present, KNOWN_TABLE_FUNCTIONS,
            "the pinned DuckDB's table functions changed: classify the difference"
        );
        // Through the gate itself, not its helpers: the parser does not
        // check arity, so an empty argument list stands for any call.
        for name in &present {
            let sql = format!("SELECT * FROM {}()", quote_ident(name));
            let outcome = the_select(&sql);
            if is_allowed_table_function(name) {
                outcome.unwrap_or_else(|e| panic!("{sql}: {e}"));
            } else {
                assert_eq!(outcome.map_err(|e| e.code()), Err("invalid"), "{sql}");
            }
        }
    }

    #[test]
    fn the_allowed_table_functions_run_anywhere_and_others_nowhere() {
        for fine in [
            "select * from range(3)",
            "select * from Generate_Series(1, 3) as g(n)",
            "select * from (select * from range(3)) as t",
            "with c as (select * from unnest([1, 2])) select * from c",
            "select * from range(2) as r, lateral unnest([r.range]) as u(x)",
        ] {
            the_select(fine).unwrap_or_else(|e| panic!("{fine}: {e}"));
        }
        for bad in [
            "select * from duckdb_tables()",
            "select * from main.duckdb_settings()",
            "select * from (select * from json_each('[1]')) as t",
            "with c as (select * from repeat_row(1, num_rows := 2)) select * from c",
            "select * from range(2) as r, lateral summary(r) as s",
            "select * from range(2) where 1 in (select 1 from pragma_version())",
        ] {
            assert_eq!(
                the_select(bad).map_err(|e| e.code()),
                Err("invalid"),
                "{bad}"
            );
        }
        // A scalar of a refused name is refused too.
        assert_eq!(
            the_select("select getenv('PATH')").map_err(|e| e.code()),
            Err("invalid")
        );
    }

    #[test]
    fn file_and_indirection_functions_are_refused_by_the_parse() {
        for bad in [
            "select * from read_blob('c.duckdb')",
            "select * from READ_TEXT('c.duckdb.wal')",
            "select * from main.read_csv('x')",
            "select * from system.main.Read_Csv_Auto('x')",
            "select * from parquet_metadata('x')",
            "select * from glob('*')",
            "select * from sniff_csv('x')",
            "select * from query('select 1')",
            "select * from query_table('evtxtriage_events')",
            "select * from json_execute_serialized_sql('{}')",
            "select * from enable_logging()",
            "select * from enable_profiling()",
            "select * from force_checkpoint()",
            "select * from arrow_scan(0, 0, 0)",
            "select * from 'c.duckdb'",
            "select * from \"evidence/x.csv\"",
            "select (select count(*) from read_text('x')) as n",
            "with c as (select * from glob('*')) select * from c",
            "select 1 where exists (select 1 from query('select 2'))",
            "select read_blob is null from (select 1 as read_blob) t where length(read_text('x')) > 0",
        ] {
            let e = the_select(bad).unwrap_err();
            assert_eq!(e.code(), "invalid", "{bad}: {e}");
        }
        for fine in [
            "select 1 as read_count, 2 as query",
            "select query from (select 'x' as query) as read_csv",
            "select count(*) as read_count from ox.source",
            "select * from ox.meta",
        ] {
            the_select(fine).unwrap_or_else(|e| panic!("{fine}: {e}"));
        }
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
