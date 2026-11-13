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
//!    - It names a table that is not the case's own. DuckDB answers a name
//!      it cannot find with a replacement scan of the file that the whole
//!      qualified name spells (`one.duckdb`, `"C:\dir\one"."duckdb"`), so:
//!      - a path character (`.`, `/`, `\`, `:`) in any part (catalog,
//!        schema or table) is refused;
//!      - a catalog qualifier must be one of these, in any case:
//!        - `ox_case`: the case itself, as it is attached;
//!        - `system`: DuckDB's built-in, read-only catalog;
//!        - `temp`: this instance's temporary catalog, which nothing can
//!          write to on this path, so it is always empty.
//!      - a schema qualifier must be one of these (or one of the catalogs
//!        above, since `a.b` is `catalog.table` to the binder as well), in
//!        any case:
//!        - `main` and `ox`: the relation views and the case tables;
//!        - `src_<digits>`: loaded data, which is case data like the views
//!          over it, superseded rows included, and is read the same way;
//!        - `information_schema` and `pg_catalog`: DuckDB's read-only
//!          catalog views. They describe the case's schema, and also the
//!          instance: `pg_catalog.pg_settings` lists every setting.
//!
//!      Any other qualifier is refused rather than looked up.
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
//!    setting did not take, nothing does (`internal`).
//! 4. **Never a writer.** The case is only ever attached read-only, and a
//!    probe that tries to create a table in it must fail.
//!
//! Three limits of the above:
//!
//! - `SQL_MEMORY_LIMIT` is per DuckDB instance, and each call opens its own
//!   instance, so calls running at once can together hold more than it.
//!   Some allocations DuckDB does not track at all (one huge string from
//!   `repeat`, for one).
//! - The parse gate (layers 1 and 2) checks the caller's text only. A view
//!   stored in the case runs as it is stored, with only the instance's
//!   lockdown (layer 3) behind it. The engine writes every view a case it
//!   made holds, so this is a trust boundary: the case files are the
//!   analyst's own. A case file from elsewhere is as trusted as its author.
//! - A timestamp at the top level of a result is rendered as R1 UTC text; one
//!   nested inside a STRUCT, LIST or MAP is rendered as DuckDB casts it to
//!   text.
//!
//! What a query reaches, then: the case's tables and views by name (in
//! `main`, `ox` and `src_<n>`), scalar functions, the allowed table
//! functions, and DuckDB's own system views and settings. That last group
//! is wider than the case: the system views reached by bare name
//! (`duckdb_databases` and the like are views, and the allowlist covers
//! table functions only), the `information_schema` and
//! `pg_catalog` views (`pg_settings` lists every setting) and scalars such
//! as `current_setting` return the instance's configuration, which includes
//! the case file's full path and paths under the OS user's profile (the
//! `secret_directory` setting, for one). Whatever they return comes back
//! inside the CLI's untrusted wrapper like every other result. Refusing
//! them by name is left for the step that adds the agent bridge.
//!
//! One residual of the qualifier rule: a case file whose name spells an
//! allowed qualifier can be read as a table from its own folder, by a
//! replacement scan of that name.
//!
//! What a query does not reach: the catalog table functions
//! (`duckdb_tables()` and the like; `describe` is how a caller learns the
//! schema), any file's contents through a reader (the case file's own bytes
//! included), the network and the environment.
//!
//! The case is attached as the default database, so the relations' views
//! are in `main`, the case tables in `ox` and loaded data in `src_<n>`, just
//! as they are to the engine itself. Attaching takes the file's lock, so the
//! caller must not hold the case open read-write meanwhile.

use super::page::clip_text;
use super::{check_offset, ISO_FORMAT};
use crate::case::{check_format, is_lock_conflict};
use crate::error::{EngineError, Result};
use crate::listing::check_limit;
use crate::query::CELL_CAP;
use duckdb::types::Value as SqlValue;
use duckdb::{params, params_from_iter, Connection};
use serde_json::Value;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use triage_core::output::duckdb::sql::{quote_ident, quote_literal};

/// The alias the case is attached under.
const CASE_ALIAS: &str = "ox_case";

/// The most memory one query may use. Nothing spills to disk, so a query
/// that needs more fails rather than writing a temporary file.
pub const SQL_MEMORY_LIMIT: &str = "2GiB";

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

/// The catalogs a table reference may name. The module doc says why.
const ALLOWED_CATALOGS: [&str; 3] = [CASE_ALIAS, "system", "temp"];

/// The schemas a table reference may name, besides `src_<digits>`. The
/// module doc says why.
const ALLOWED_SCHEMAS: [&str; 4] = ["main", "ox", "information_schema", "pg_catalog"];

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
/// one SELECT (with DuckDB's message) or a `limit` of 0 or over `MAX_LIMIT`;
/// `CaseLocked` when another connection holds the file.
pub fn query_sql(case_path: &Path, sql: &str, limit: u32, offset: u64) -> Result<SqlResult> {
    query_sql_within(case_path, sql, limit, offset, SQL_TIME_LIMIT)
}

/// The time a query may run unless its caller sets another.
pub const SQL_TIME_LIMIT: Duration = Duration::from_secs(30);
/// The most time a caller may give a query.
pub const SQL_TIME_LIMIT_MAX: Duration = Duration::from_secs(600);

/// The least time a caller may give a query.
const SQL_TIME_LIMIT_MIN: Duration = Duration::from_secs(1);

/// Characters in one query's SQL. Binding cannot be interrupted, and takes
/// about a millisecond per relation the SQL names per source that relation
/// holds (each relation's view has one arm per active source), so this cap
/// is one of the two things that bound how far past its time a query can
/// run; the other, the number of sources, is the case's. On a 32-thread
/// machine, binding the slowest query this long found (a union of some
/// 2,400 references to the widest view of a one-run case) took about 3 s.
/// It leaves room for an `IN` list of a thousand hashes.
pub const SQL_TEXT_MAX: usize = 100_000;

/// When a query's time is up, and the limit it was given.
struct Budget {
    deadline: Instant,
    limit: Duration,
}

impl Budget {
    /// The error a query that ran out of time ends with.
    fn spent(&self) -> EngineError {
        EngineError::Invalid(format!(
            "the query ran past its time limit of {} and was stopped; narrow it, or give it \
             more time (at most {})",
            seconds(self.limit),
            seconds(SQL_TIME_LIMIT_MAX)
        ))
    }
}

/// `query_sql` with a time limit of the caller's, `SQL_TIME_LIMIT_MIN` to
/// `SQL_TIME_LIMIT_MAX` (`Invalid` outside it, as is SQL over
/// `SQL_TEXT_MAX` characters). A query still running when its time is up
/// is interrupted and is `Invalid`, as one needing more than
/// `SQL_MEMORY_LIMIT` is: it asked for more than a query may have here.
///
/// The time counts from when the case is open; parsing happens before, on
/// its own instance, and attaching and probing the case cost a small fixed
/// time. A query can run past its time by one bind of its SQL: DuckDB does
/// not stop a bind when interrupted (measured: an interrupted 1.6 s bind
/// took 1.6 s), so a bind under way at the deadline finishes, and only then
/// is the query stopped. `SQL_TEXT_MAX` bounds that bind.
pub fn query_sql_within(
    case_path: &Path,
    sql: &str,
    limit: u32,
    offset: u64,
    time_limit: Duration,
) -> Result<SqlResult> {
    check_limit(limit)?;
    check_offset(offset)?;
    if !(SQL_TIME_LIMIT_MIN..=SQL_TIME_LIMIT_MAX).contains(&time_limit) {
        return Err(EngineError::Invalid(format!(
            "a query's time limit is {} to {}, not {}",
            seconds(SQL_TIME_LIMIT_MIN),
            seconds(SQL_TIME_LIMIT_MAX),
            seconds(time_limit)
        )));
    }
    let chars = sql.chars().count();
    if chars > SQL_TEXT_MAX {
        return Err(EngineError::Invalid(format!(
            "a query is at most {SQL_TEXT_MAX} characters, not {chars}"
        )));
    }
    let select = the_select(sql)?;
    let conn = locked(case_path)?;
    let budget = Budget {
        deadline: Instant::now() + time_limit,
        limit: time_limit,
    };
    // Declared after `conn`, so on a panic it is dropped, and its thread
    // stopped, before the connection it interrupts is closed.
    let watchdog = Watchdog::start(&conn, budget.deadline);
    let result = run_select(&conn, &select, limit, offset, &budget);
    timed(watchdog.stop(), &budget, result)
}

/// `result` as the call reports it: the time-limit error when the watchdog
/// `fired` and the query failed because DuckDB was interrupted, and as it
/// is otherwise. A query that finished, or failed on its own, as its time
/// ran out keeps its own outcome.
fn timed(fired: bool, budget: &Budget, result: Result<SqlResult>) -> Result<SqlResult> {
    match result {
        Err(EngineError::Db(e)) if fired && e.to_string().starts_with("INTERRUPT Error") => {
            Err(budget.spent())
        }
        other => other,
    }
}

/// `d` as the CLI spells a time limit: whole seconds, or tenths below that.
fn seconds(d: Duration) -> String {
    if d.subsec_nanos() == 0 {
        format!("{} s", d.as_secs())
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}

/// Interrupts a connection's running statement once a time limit passes.
///
/// DuckDB clears an interrupt when the next statement starts, and a call
/// runs several (the DESCRIBE that names the columns, then the page), so
/// once the time is up the watchdog keeps interrupting until it is stopped:
/// a statement started after the first interrupt is interrupted too.
///
/// Its thread is stopped and joined when it is stopped or dropped, so it
/// never outlives the call and never interrupts a connection being closed.
struct Watchdog {
    stop: Option<mpsc::Sender<()>>,
    fired: Arc<AtomicBool>,
    /// Set by the thread as it returns; what a test reads to see that the
    /// thread is gone once the watchdog is.
    #[cfg_attr(not(test), allow(dead_code))]
    finished: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Watchdog {
    /// How often an expired watchdog interrupts again.
    const REPEAT: Duration = Duration::from_millis(50);

    /// Watch `conn` until `deadline`. The deadline is fixed by the caller, so
    /// however late the thread first runs, the time counts from the call.
    fn start(conn: &Connection, deadline: Instant) -> Watchdog {
        let handle = conn.interrupt_handle();
        let (stop, stopped) = mpsc::channel::<()>();
        let fired = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let (flag, done) = (Arc::clone(&fired), Arc::clone(&finished));
        let thread = std::thread::spawn(move || {
            let left = deadline.saturating_duration_since(Instant::now());
            // A message or a dropped sender both mean the call is over.
            if stopped.recv_timeout(left) == Err(RecvTimeoutError::Timeout) {
                flag.store(true, Ordering::SeqCst);
                loop {
                    handle.interrupt();
                    if stopped.recv_timeout(Self::REPEAT) != Err(RecvTimeoutError::Timeout) {
                        break;
                    }
                }
            }
            done.store(true, Ordering::SeqCst);
        });
        Watchdog {
            stop: Some(stop),
            fired,
            finished,
            thread: Some(thread),
        }
    }

    /// Stop watching; whether the time ran out first.
    fn stop(mut self) -> bool {
        self.halt();
        self.fired.load(Ordering::SeqCst)
    }

    /// End the thread and wait for it.
    fn halt(&mut self) {
        // Dropping the sender ends the thread's wait at once.
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.halt();
    }
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
        EngineError::Internal(format!("the query parser returned invalid JSON: {e}"))
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
/// or the first table it names that is not the case's own.
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
                if let Some(why) = foreign_table(fields) {
                    return Some(why);
                }
            }
            fields.values().find_map(reaches_outside)
        }
        Value::Array(items) => items.iter().find_map(reaches_outside),
        _ => None,
    }
}

/// Why the base-table reference `fields` is not one of the case's own, if
/// it is not: a part with a path character, or a catalog or schema
/// qualifier outside `ALLOWED_CATALOGS` and `ALLOWED_SCHEMAS`. DuckDB builds
/// a replacement-scan path from the whole qualified name (`one.duckdb`,
/// `"C:\dir\one"."duckdb"`), so every part is checked. A two-part name is
/// `schema.table` or `catalog.table` to the binder, so its qualifier may be
/// either.
fn foreign_table(fields: &serde_json::Map<String, Value>) -> Option<String> {
    let part = |key: &str| fields.get(key).and_then(Value::as_str).unwrap_or_default();
    let (catalog, schema, table) = (
        part("catalog_name"),
        part("schema_name"),
        part("table_name"),
    );
    let shown = [catalog, schema, table]
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(".");
    let refuse = |why: &str| Some(format!("a query may not name the table {shown:?}: {why}"));
    if [catalog, schema, table]
        .iter()
        .any(|p| p.contains(['.', '/', '\\', ':']))
    {
        return refuse("a name with a path character reads a file");
    }
    if !catalog.is_empty() && !is_allowed_catalog(catalog) {
        return refuse("only the case's own catalog may be named");
    }
    if !schema.is_empty() && !is_allowed_schema(schema) && !is_allowed_catalog(schema) {
        return refuse("only the case's own schemas may be named");
    }
    None
}

/// `name` (any case) is a catalog a query may name.
fn is_allowed_catalog(name: &str) -> bool {
    ALLOWED_CATALOGS
        .iter()
        .any(|c| c.eq_ignore_ascii_case(name))
}

/// `name` (any case) is a schema a query may name: one of
/// `ALLOWED_SCHEMAS`, or `src_` followed by digits only.
fn is_allowed_schema(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ALLOWED_SCHEMAS.contains(&lower.as_str())
        || lower
            .strip_prefix("src_")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
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

/// Prove the lockdown holds on `conn`: each attempt to undo it, to reach
/// outside the process or to write to the case must fail, else `Internal`
/// (no query the caller writes fixes a lockdown that did not take).
/// `existing` names a file that exists and is not attached, so that reading
/// it fails only because access is off.
///
/// The ATTACH probe names a file in a directory made for this call alone,
/// so the path is not one another process can predict and prepare, and
/// whatever a successful attach left there goes with the directory. The
/// write probe creates a table in the case as it is attached (`ox_case`);
/// it fails only because the attach is read-only.
fn probe_lockdown(conn: &Connection, existing: &str) -> Result<()> {
    let probe_dir = tempfile::TempDir::new()?;
    let probe_file = probe_dir.path().join("probe.duckdb");
    let probe_path = probe_file.to_string_lossy();
    let probes = [
        "SET enable_external_access = true".to_string(),
        "SET lock_configuration = false".to_string(),
        "SET temp_directory = '.tmp'".to_string(),
        format!("SELECT * FROM read_blob({})", quote_literal(existing)),
        format!("SELECT * FROM glob({})", quote_literal(existing)),
        format!("ATTACH {} AS ox_probe", quote_literal(&probe_path)),
        format!("CREATE TABLE {CASE_ALIAS}.main.ox_probe_write (x INTEGER)"),
    ];
    for probe in &probes {
        if conn.execute_batch(probe).is_ok() {
            let _ = conn.execute_batch("DETACH ox_probe");
            let _ = conn.execute_batch(&format!(
                "DROP TABLE IF EXISTS {CASE_ALIAS}.main.ox_probe_write"
            ));
            return Err(EngineError::Internal(format!(
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
            return Err(EngineError::Internal(format!(
                "the query connection is not locked down ({name} is {value:?}, not {expected:?})"
            )));
        }
    }
    Ok(())
}

/// The kinds of DuckDB error, as its messages begin (`Binder Error: …`),
/// that a mistake in the query's own text or in what it asks of the data
/// causes: a name that does not exist, a type that does not fit, a value
/// that does not convert or overflows, a bad regex, a function DuckDB does
/// not have here. Any other kind (`INTERNAL`, `IO`, `Connection`, and
/// `Not implemented`, `Invalid type` and `Unknown Type`, which DuckDB's own
/// internals raise as often as a query does) is a fault on this side and
/// stays `engine`.
const CALLER_MISTAKES: [&str; 13] = [
    "Binder",
    "Catalog",
    "Conversion",
    "Decimal",
    "Divide by Zero",
    "Invalid Input",
    "Mismatch Type",
    "Missing Extension",
    "Out of Range",
    "Parameter Not Allowed",
    "Parameter Not Resolved",
    "Parser",
    "Syntax",
];

/// A failure of the caller's SELECT, as the error it is: `Invalid` with
/// DuckDB's own reason when the query asked for something it cannot have
/// (`CALLER_MISTAKES`, or more memory than `SQL_MEMORY_LIMIT`), and the
/// engine error it is otherwise.
///
/// A view of the case that no longer binds fails with the same `Binder` and
/// `Catalog` errors as a caller's misspelt name, and is reported the same
/// way here; DuckDB's reason then names a table the caller never wrote.
/// Telling the two apart was tried and dropped: checking only the views a
/// query reached means redoing DuckDB's name resolution (CTE scope and
/// order, lazily bound CTEs, catalog search), and checking every view means
/// binds that no interrupt stops (DuckDB does not look for one while
/// binding: an interrupted 1.6 s bind took 1.6 s), which could hold the case
/// past the query's time limit. The engine rebuilds its views in the same
/// transaction as every change, so such a view means a damaged case, which
/// every other command, reading through the engine's own path, reports as
/// `engine`.
fn query_failure(e: duckdb::Error) -> EngineError {
    let message = e.to_string();
    if message.starts_with("Out of Memory Error") {
        return EngineError::Invalid(format!(
            "the query needs more memory than a query may use here ({SQL_MEMORY_LIMIT}); \
             narrow it or aggregate it further"
        ));
    }
    let kind = message.split_once(" Error: ").map(|(kind, _)| kind);
    if kind.is_some_and(|kind| CALLER_MISTAKES.contains(&kind)) {
        return EngineError::Invalid(format!("the query failed: {}", without_pointer(&message)));
    }
    e.into()
}

/// `message` without the `LINE n:` pointer DuckDB ends it with, which points
/// into the SQL it ran (the caller's SELECT wrapped in the engine's own),
/// not into anything the caller wrote. Only a trailing block of that shape
/// is cut: a blank line, `LINE <digits>:`, then a caret line at the end.
fn without_pointer(message: &str) -> &str {
    let Some(at) = message.rfind("\n\nLINE ") else {
        return message;
    };
    let pointer = &message[at + "\n\nLINE ".len()..];
    let numbered = pointer
        .split_once(':')
        .is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let caret = pointer
        .rsplit_once('\n')
        .is_some_and(|(_, last)| last.trim() == "^");
    if numbered && caret {
        message[..at].trim_end()
    } else {
        message
    }
}

/// `select` (from `the_select`) on `conn`: up to `limit` rows from `offset`,
/// every cell as text, within `budget`.
fn run_select(
    conn: &Connection,
    select: &str,
    limit: u32,
    offset: u64,
    budget: &Budget,
) -> Result<SqlResult> {
    // The binder alone names and types the columns, so an empty result
    // still has them.
    let mut columns = Vec::new();
    let mut types = Vec::new();
    {
        let mut stmt = conn
            .prepare(&format!("DESCRIBE {select}"))
            .map_err(query_failure)?;
        let mut rows = stmt.query([]).map_err(query_failure)?;
        while let Some(r) = rows.next().map_err(query_failure)? {
            columns.push(r.get::<_, String>(0)?);
            types.push(r.get::<_, String>(1)?);
        }
    }
    // The page binds the SELECT again, and a bind runs to its end however
    // often it is interrupted: once the first used up the time, the second
    // is not begun.
    if Instant::now() >= budget.deadline {
        return Err(budget.spent());
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
    let mut stmt = conn.prepare(&sql).map_err(query_failure)?;
    let mut rows = stmt
        .query(params_from_iter(page_params))
        .map_err(query_failure)?;
    while let Some(r) = rows.next().map_err(query_failure)? {
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

    fn failure(message: &str) -> duckdb::Error {
        duckdb::Error::DuckDBFailure(duckdb::ffi::Error::new(1), Some(message.to_string()))
    }

    fn classify(message: &str) -> EngineError {
        query_failure(failure(message))
    }

    /// Only a mistake in the query is the caller's: a fault on this side,
    /// or a message that names no kind, stays `engine`.
    #[test]
    fn only_the_callers_mistakes_are_invalid() {
        for message in [
            "Binder Error: Referenced column \"x\" not found",
            "Catalog Error: Table with name t does not exist!",
            "Conversion Error: Could not convert string 'a' to INT32",
            "Out of Range Error: Overflow in addition",
            "Invalid Input Error: Regex compile error",
        ] {
            let e = classify(message);
            assert_eq!(e.code(), "invalid", "{message}");
            assert!(e.to_string().contains(message), "{e}");
        }
        assert!(classify("Out of Memory Error: x")
            .to_string()
            .contains(SQL_MEMORY_LIMIT));
        for message in [
            "INTERNAL Error: Attempted to access index 3",
            "IO Error: Could not read from file",
            "Connection Error: closed",
            "FATAL Error: database invalidated",
            "something without a kind",
            // A kind is the text before the first " Error: ", whole.
            "Binderish Error: x",
            "IO Error: Binder Error: x",
            // Reviewed: these come from DuckDB's own internals as often as
            // from a query, so they are not taken as the caller's.
            "Not implemented Error: Unimplemented type for cast",
            "Invalid type Error: Invalid Type [unknown]",
            "Unknown Type Error: x",
        ] {
            assert_eq!(classify(message).code(), "engine", "{message}");
        }
    }

    #[test]
    fn only_a_trailing_pointer_is_cut() {
        for (message, reason) in [
            (
                "Binder Error: no x\n\nLINE 1: SELECT x\n               ^",
                "Binder Error: no x",
            ),
            ("Binder Error: no x", "Binder Error: no x"),
            (
                "Conversion Error: 'a\nLINE 2: b' to INT32\n\nLINE 1: SELECT y\n       ^",
                "Conversion Error: 'a\nLINE 2: b' to INT32",
            ),
            // Not a pointer: no caret line after it, or no line number.
            (
                "Conversion Error: 'a\n\nLINE 2: b' to INT32",
                "Conversion Error: 'a\n\nLINE 2: b' to INT32",
            ),
            (
                "Conversion Error: 'a\n\nLINE x: b'\n ^",
                "Conversion Error: 'a\n\nLINE x: b'\n ^",
            ),
        ] {
            assert_eq!(without_pointer(message), reason, "{message:?}");
        }
    }

    /// Reviewed: only DuckDB's interrupt, after the watchdog fired, is a
    /// time-out; an error the query met on its own stays what it was, even
    /// when the time ran out before the call returned.
    #[test]
    fn only_an_interrupt_after_the_time_ran_out_is_a_time_out() {
        let limit = Budget {
            deadline: Instant::now(),
            limit: Duration::from_secs(3),
        };
        let interrupted = || Err(EngineError::Db(failure("INTERRUPT Error: Interrupted!")));
        let mistake = || {
            Err(EngineError::Invalid(
                "the query failed: Binder Error: x".into(),
            ))
        };
        let fault = || Err(EngineError::Db(failure("IO Error: disk")));
        let code = |r: Result<SqlResult>| r.map(|_| "ok").unwrap_or_else(|e| e.code());
        let message = |r: Result<SqlResult>| r.err().map(|e| e.to_string()).unwrap_or_default();

        assert!(message(timed(true, &limit, interrupted())).contains("time limit of 3 s"));
        assert_eq!(code(timed(true, &limit, interrupted())), "invalid");
        assert!(message(timed(true, &limit, mistake())).contains("Binder Error"));
        assert_eq!(code(timed(true, &limit, fault())), "engine");
        // Not fired: an interrupt is not ours to explain.
        assert_eq!(code(timed(false, &limit, interrupted())), "engine");
        let done = Ok(SqlResult {
            columns: vec![],
            types: vec![],
            rows: vec![],
            truncated: false,
            cells_clipped: false,
        });
        assert_eq!(code(timed(true, &limit, done)), "ok");
    }

    /// Reviewed: the time counts from when the call fixed its deadline, not
    /// from whenever the watchdog's thread first runs.
    #[test]
    fn a_watchdog_counts_from_its_deadline() {
        let conn = Connection::open_in_memory().unwrap();
        let watchdog = Watchdog::start(&conn, Instant::now() - Duration::from_secs(1));
        let fired = Arc::clone(&watchdog.fired);
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            fired.load(Ordering::SeqCst),
            "a passed deadline did not fire"
        );
        assert!(watchdog.stop());
    }

    /// Reviewed: DuckDB does not stop a bind when interrupted, and a call
    /// binds the caller's SELECT twice (to name the columns, then to page
    /// it). When the first bind ran past the deadline the second is not
    /// begun.
    #[test]
    fn no_page_is_begun_once_the_time_is_up() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CALL enable_logging('QueryLog');")
            .unwrap();
        let budget = Budget {
            deadline: Instant::now() - Duration::from_millis(1),
            limit: Duration::from_secs(1),
        };
        let e = run_select(&conn, "SELECT 42 AS answer", 10, 0, &budget).unwrap_err();
        assert!(e.to_string().contains("time limit of 1 s"), "{e}");
        conn.execute_batch("CALL disable_logging();").unwrap();
        let ran: Vec<String> = conn
            .prepare("SELECT message FROM duckdb_logs WHERE type = 'QueryLog'")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert!(ran.iter().any(|s| s.starts_with("DESCRIBE")), "{ran:#?}");
        assert!(!ran.iter().any(|s| s.contains("OFFSET")), "{ran:#?}");
    }

    /// Reviewed: a watchdog that is dropped, as on a panic, stops its thread
    /// before it returns, so nothing interrupts a connection being closed.
    #[test]
    fn a_dropped_watchdog_stops_its_thread() {
        let conn = Connection::open_in_memory().unwrap();
        let watchdog = Watchdog::start(&conn, Instant::now() + Duration::from_millis(1));
        let fired = Arc::clone(&watchdog.fired);
        let finished = Arc::clone(&watchdog.finished);
        std::thread::sleep(Duration::from_millis(100));
        assert!(fired.load(Ordering::SeqCst), "the watchdog never fired");
        drop(watchdog);
        assert!(
            finished.load(Ordering::SeqCst),
            "dropping the watchdog left its thread running"
        );
        drop(conn);
    }

    #[test]
    fn the_probe_catches_an_open_connection() {
        let conn = Connection::open_in_memory().unwrap();
        let exe = std::env::current_exe().unwrap();
        let e = probe_lockdown(&conn, exe.to_str().unwrap()).unwrap_err();
        // Not the caller's mistake: no change to the query fixes it.
        assert_eq!(e.code(), "internal", "{e}");
    }

    /// The case must be attached read-only: a write to it is probed, and
    /// must fail, though every setting is as it should be.
    #[test]
    fn the_probe_catches_a_case_attached_for_writing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("w.duckdb");
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(NO_SPILL).unwrap();
        conn.execute_batch(&format!(
            "ATTACH {} AS {CASE_ALIAS}; USE {CASE_ALIAS};",
            quote_literal(file.to_str().unwrap())
        ))
        .unwrap();
        conn.execute_batch(LOCK).unwrap();
        let exe = std::env::current_exe().unwrap();
        let e = probe_lockdown(&conn, exe.to_str().unwrap()).unwrap_err();
        assert_eq!(e.code(), "internal", "{e}");
        assert!(e.to_string().contains("CREATE TABLE"), "{e}");
        drop(conn);

        // Attached read-only, as `locked` attaches the case, it passes.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(NO_SPILL).unwrap();
        conn.execute_batch(&format!(
            "ATTACH {} AS {CASE_ALIAS} (READ_ONLY); USE {CASE_ALIAS};",
            quote_literal(file.to_str().unwrap())
        ))
        .unwrap();
        conn.execute_batch(LOCK).unwrap();
        probe_lockdown(&conn, exe.to_str().unwrap()).unwrap();
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
