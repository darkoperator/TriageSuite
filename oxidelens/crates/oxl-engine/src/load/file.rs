//! Loading one loose CSV or TSV file.
//!
//! A file lands in the case the way a run's dataset does: a table in
//! `src_<id>` with the engine's row columns, rows in `ox.source_table` and
//! `ox.source_dataset`, a relation, catalogued columns and one `source_load`
//! history row. Every column is read as text, so the cell text is what the
//! file wrote; the types DuckDB detects are offered as `<col>__typed`
//! companions, never applied to the original.
//!
//! The dialect is stated, never sniffed (a one-row file gives a sniffer
//! nothing to go on): `,` or tab by extension, `"` quoting with `""` as the
//! escape, and a header row.

use super::run::{canonical_path, discard};
use super::stage::{materialize, rebuild_relations, source_schema, HashColumn};
use super::{LoadReport, LoadedRelation};
use crate::actor::Actor;
use crate::case::Case;
use crate::catalog::{catalog_file, FileColumn, SqlType, TimeSemantics};
use crate::error::{EngineError, Result};
use crate::time::now_iso;
use duckdb::{params, Connection};
use std::path::Path;
use triage_core::output::duckdb::sql::{quote_ident, quote_literal, NameAllocator};

/// The column a host assigned at load time is stored in.
const HOST_COLUMN: &str = "_triage_host";

/// What an analyst may say about a file at load time.
#[derive(Default)]
pub struct FileOptions {
    /// The relation's name; derived from the file name when absent.
    pub name: Option<String>,
    /// The host the file's rows describe.
    pub host: Option<String>,
}

/// The relation name for a file called `file_name`: lower case, every
/// character outside `[a-z0-9_]` an underscore, a leading digit prefixed
/// with `_`, `file` when nothing is left, and `_2`, `_3`... until it is not
/// in `taken` (compared without regard to case).
pub fn relation_name_for_file(file_name: &str, taken: &[String]) -> String {
    let mut base = sanitise(file_name);
    if base.is_empty() {
        base.push_str("file");
    }
    if base.starts_with(|c: char| c.is_ascii_digit()) {
        base.insert(0, '_');
    }
    let is_taken = |name: &str| taken.iter().any(|t| t.eq_ignore_ascii_case(name));
    let mut candidate = base.clone();
    let mut n = 2u64;
    while is_taken(&candidate) {
        candidate = format!("{base}_{n}");
        n += 1;
    }
    candidate
}

fn sanitise(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '_' => c,
            'A'..='Z' => c.to_ascii_lowercase(),
            _ => '_',
        })
        .collect()
}

/// Load the CSV or TSV file at `path` into `case`.
///
/// Refused when the file's canonical path is already a live source;
/// `Invalid` for any extension but `.csv` and `.tsv`. Any failure leaves the
/// case exactly as it was.
pub fn load_file(
    case: &mut Case,
    path: &Path,
    opts: FileOptions,
    actor: &Actor,
) -> Result<LoadReport> {
    let delim = match extension(path).as_deref() {
        Some("csv") => ',',
        Some("tsv") => '\t',
        Some(other) => {
            return Err(EngineError::Invalid(format!(
                "cannot load a .{other} file: only .csv and .tsv are supported"
            )))
        }
        None => {
            return Err(EngineError::Invalid(format!(
                "{} has no extension: only .csv and .tsv are supported",
                path.display()
            )))
        }
    };
    if !path.is_file() {
        return Err(EngineError::NotFound(format!(
            "no file at {}",
            path.display()
        )));
    }
    let canonical = canonical_path(path)?;

    let conn = case.conn_mut();
    let mut source_id = None;
    let tx = conn.transaction()?;
    match load_in(&tx, &canonical, delim, &opts, actor, &mut source_id) {
        Ok(report) => {
            tx.commit()?;
            Ok(report)
        }
        Err(e) => {
            drop(tx);
            discard(conn, source_id);
            Err(e)
        }
    }
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// DuckDB's `read_csv` with the dialect stated outright. `path_sql` is the
/// SQL for the path: `?` in a query, so the path is a bound parameter, or a
/// quoted literal inside the CREATE TABLE AS, which cannot bind.
fn read_csv(path_sql: &str, delim: char, tail: &str) -> String {
    format!(
        "read_csv({path_sql}, delim = {}, quote = '\"', escape = '\"', header = true{tail})",
        quote_literal(&delim.to_string())
    )
}

/// `(name, type)` of every column `read_csv` yields for `path`, which is
/// bound as a parameter.
fn describe(
    conn: &Connection,
    path: &str,
    delim: char,
    tail: &str,
) -> Result<Vec<(String, String)>> {
    let source = read_csv("?", delim, tail);
    Ok(conn
        .prepare(&format!(
            "SELECT column_name, column_type FROM (DESCRIBE SELECT * FROM {source})"
        ))?
        .query_map(params![path], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?)
}

/// The engine's type for a type DuckDB detected, or `None` to stay text.
fn sniffed_type(detected: &str) -> Option<SqlType> {
    match detected {
        t if t.starts_with("TIMESTAMP") => Some(SqlType::Timestamp),
        "BOOLEAN" => Some(SqlType::Boolean),
        "UBIGINT" | "UHUGEINT" => Some(SqlType::UBigInt),
        "BIGINT" | "INTEGER" | "SMALLINT" | "TINYINT" | "UINTEGER" | "USMALLINT" | "UTINYINT"
        | "HUGEINT" => Some(SqlType::BigInt),
        _ => None,
    }
}

fn load_in(
    conn: &Connection,
    canonical: &Path,
    delim: char,
    opts: &FileOptions,
    actor: &Actor,
    source_id: &mut Option<i64>,
) -> Result<LoadReport> {
    let path = canonical.display().to_string();
    let file_name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let live: i64 = conn.query_row(
        "SELECT count(*) FROM ox.source WHERE kind = 'file' AND path = ? AND NOT removed",
        params![path],
        |r| r.get(0),
    )?;
    if live > 0 {
        return Err(EngineError::Refused(format!(
            "{path} is already in this case"
        )));
    }
    let relation = choose_relation(conn, &file_name, opts.name.as_deref())?;

    // Names as DuckDB reports them (it already makes empty and repeated ones
    // distinct), then as the case will hold them.
    let reported = describe(conn, &path, delim, ", all_varchar = true")?;
    let detected = describe(conn, &path, delim, "")?;
    if reported.len() != detected.len() {
        return Err(EngineError::Invalid(format!(
            "{path}: the header and the detected columns disagree"
        )));
    }
    let mut warnings = Vec::new();
    let mut allocator = NameAllocator::new(opts.host.iter().map(|_| HOST_COLUMN.to_string()));
    let names: Vec<String> = reported
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            let wanted = if name.is_empty() {
                format!("column{i}")
            } else if name.to_lowercase().starts_with("_ox_") {
                format!("x{name}")
            } else {
                name.clone()
            };
            let given = allocator.allocate(&wanted);
            if given != *name {
                warnings.push(format!(
                    "column {name:?} renamed {given:?}: names starting _ox_ are reserved \
                     and every name must be unique"
                ));
            }
            given
        })
        .collect();

    let name_list: Vec<String> = names.iter().map(|n| quote_literal(n)).collect();
    let source = read_csv(
        &quote_literal(&path),
        delim,
        &format!(", all_varchar = true, names = [{}]", name_list.join(", ")),
    );
    let staged = match &opts.host {
        Some(host) => format!(
            "(SELECT *, {} AS {} FROM {source})",
            quote_literal(host),
            quote_ident(HOST_COLUMN)
        ),
        None => format!("(SELECT * FROM {source})"),
    };

    let id: i64 = conn.query_row("SELECT nextval('ox.source_seq')", [], |r| r.get(0))?;
    *source_id = Some(id);
    let loaded_at = now_iso();
    let loaded_by = actor.label();
    conn.execute(
        "INSERT INTO ox.source \
         (id, kind, path, run_id, generation, loaded_at, loaded_by, manifest_json, removed) \
         VALUES (?, 'file', ?, NULL, NULL, ?, ?, NULL, false)",
        params![id, path, loaded_at, loaded_by],
    )?;

    // Every column, by name; the host column is one of them.
    let mut hashed: Vec<String> = names.clone();
    if opts.host.is_some() {
        hashed.push(HOST_COLUMN.to_string());
    }
    hashed.sort();
    let hash_columns: Vec<HashColumn> = hashed
        .into_iter()
        .map(|name| HashColumn {
            text_expr: quote_ident(&name),
            name,
        })
        .collect();
    let (rows, first, last) = materialize(conn, id, &relation, &staged, &hash_columns)?;
    let (first, last) = if rows == 0 {
        (None, None)
    } else {
        (Some(first), Some(last))
    };
    let row_count = i64::try_from(rows).unwrap_or(i64::MAX);
    conn.execute(
        "INSERT INTO ox.source_table \
         (source_id, relation, table_schema, first_row, last_row, rows) \
         VALUES (?, ?, ?, ?, ?, ?)",
        params![id, relation, source_schema(id), first, last, row_count],
    )?;
    conn.execute(
        "INSERT INTO ox.source_dataset (source_id, relation, host, rows, active) \
         VALUES (?, ?, ?, ?, true)",
        params![id, relation, opts.host, row_count],
    )?;
    conn.execute(
        "INSERT INTO ox.relation \
         (name, kind, tool, primary_time, evidence_path_column, host_column) \
         VALUES (?, 'file', NULL, NULL, NULL, ?)",
        params![relation, opts.host.as_ref().map(|_| HOST_COLUMN)],
    )?;

    let mut columns: Vec<FileColumn> = Vec::new();
    for ((_, detected_type), name) in detected.iter().zip(&names) {
        let sniffed = match sniffed_type(detected_type) {
            Some(SqlType::Timestamp) => {
                let semantics = time_semantics(conn, id, &relation, name)?;
                // DuckDB detects a value ending in `Z` as a zoned timestamp
                // too. Such a column is typed only when every value is UTC;
                // an offset such as +02:00 would be applied by the cast, and
                // step 1 shows times as written, so that column stays text.
                let zoned = detected_type.contains("TIME ZONE") || detected_type == "TIMESTAMPTZ";
                if zoned && semantics != TimeSemantics::Utc {
                    None
                } else {
                    Some((SqlType::Timestamp, Some(semantics)))
                }
            }
            Some(t) => Some((t, None)),
            None => None,
        };
        columns.push(FileColumn {
            name: name.clone(),
            sniffed,
        });
    }
    if opts.host.is_some() {
        columns.push(FileColumn {
            name: HOST_COLUMN.to_string(),
            sniffed: None,
        });
    }
    catalog_file(conn, &relation, &columns)?;

    let after = serde_json::json!({
        "kind": "file",
        "path": path,
        "relations": [{
            "name": relation,
            "hosts": opts.host.iter().collect::<Vec<_>>(),
            "rows": rows,
        }],
    });
    conn.execute(
        "INSERT INTO ox.history \
         (id, \"at\", \"by\", target_kind, target_id, action, before_json, after_json) \
         VALUES (nextval('ox.history_seq'), ?, ?, 'source', ?, 'source_load', NULL, ?)",
        params![loaded_at, loaded_by, id.to_string(), after.to_string()],
    )?;
    rebuild_relations(conn)?;

    Ok(LoadReport {
        source_id: id,
        relations: vec![LoadedRelation {
            name: relation,
            hosts: opts.host.iter().cloned().collect(),
            rows,
        }],
        superseded: Vec::new(),
        warnings,
    })
}

/// `Utc` when every non-empty value of the timestamp column `column` ends in
/// `Z`, else `Unknown`: a bare local time says nothing about its zone.
fn time_semantics(
    conn: &Connection,
    source_id: i64,
    relation: &str,
    column: &str,
) -> Result<TimeSemantics> {
    let not_z: i64 = conn.query_row(
        &format!(
            "SELECT count(*) FROM {}.{} WHERE {col} <> '' AND NOT ends_with({col}, 'Z')",
            quote_ident(&source_schema(source_id)),
            quote_ident(relation),
            col = quote_ident(column)
        ),
        [],
        |r| r.get(0),
    )?;
    Ok(if not_z == 0 {
        TimeSemantics::Utc
    } else {
        TimeSemantics::Unknown
    })
}

/// The relation this file feeds: `requested` (made safe, and refused when
/// something has it), or a name derived from the file's that nothing has.
fn choose_relation(conn: &Connection, file_name: &str, requested: Option<&str>) -> Result<String> {
    let existing: Vec<String> = conn
        .prepare("SELECT name FROM ox.relation")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    // Each relation also owns the view `raw_<name>`.
    let mut taken: Vec<String> = existing
        .iter()
        .flat_map(|n| [n.clone(), format!("raw_{n}")])
        .collect();
    let clashes = |name: &str| {
        existing
            .iter()
            .any(|n| n.eq_ignore_ascii_case(&format!("raw_{name}")))
    };
    if let Some(requested) = requested {
        let name = relation_name_for_file(requested, &[]);
        if matches!(name.as_str(), "ox" | "main")
            || taken.iter().any(|t| t.eq_ignore_ascii_case(&name))
            || clashes(&name)
        {
            return Err(EngineError::Refused(format!(
                "the name {name} conflicts with a relation or a reserved name in the case"
            )));
        }
        return Ok(name);
    }
    loop {
        let name = relation_name_for_file(file_name, &taken);
        if !clashes(&name) {
            return Ok(name);
        }
        taken.push(name);
    }
}
