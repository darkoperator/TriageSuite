//! From a staged relation to a case table, and from case tables to the views
//! an analyst queries.
//!
//! A load first exposes its input as a relation in a staging schema (for a
//! run, the suite's own rendered views). `materialize` copies it into
//! `src_<id>`, adding the three columns every loaded row carries:
//!
//! - `_ox_row`: a case-wide unique id from `ox.row_seq`. Annotations hang off it.
//! - `_ox_hash`: a digest of the row's content (host, identity and every
//!   column's text), deliberately blind to the run id and the output path,
//!   so that the same event read by a later run of the same host has the same
//!   hash. Supersession matches rows on it.
//! - `_ox_occ`: which occurrence of that hash the row is, so that two
//!   byte-identical rows stay two distinguishable rows.
//!
//! `rebuild_relations` then derives every view in `main` from the case
//! tables alone, so it can be rerun after any change to what is active.

use crate::error::{EngineError, Result};
use duckdb::{params, Connection};
use triage_core::output::duckdb::inventory::MetadataColumns;
use triage_core::output::duckdb::sql::{quote_ident, quote_literal};

/// The prefix of the columns the engine adds; a loaded relation may not
/// bring its own.
const RESERVED_PREFIX: &str = "_ox_";

/// One part of a row's content hash: a column's name and a SQL expression
/// for its text.
pub(crate) struct HashColumn {
    pub name: String,
    /// A SQL expression over the staged relation yielding the column's text,
    /// already quoted (typically `quote_ident` of the text column).
    pub text_expr: String,
}

/// The schema holding source `source_id`'s tables.
pub(crate) fn source_schema(source_id: i64) -> String {
    format!("src_{source_id}")
}

/// Copy `staged` (a schema-qualified, quoted relation) into
/// `src_<source_id>."<relation>"` with `_ox_row`, `_ox_hash` and `_ox_occ`
/// added. Returns the row count and the first and last `_ox_row`; both ids
/// are 0 when the table has no rows (the caller records them as NULL).
pub(crate) fn materialize(
    conn: &Connection,
    source_id: i64,
    relation: &str,
    staged: &str,
    hash_columns: &[HashColumn],
) -> Result<(u64, i64, i64)> {
    let mut stmt = conn.prepare(&format!(
        "SELECT column_name FROM (DESCRIBE SELECT * FROM {staged})"
    ))?;
    let names = stmt.query_map([], |r| r.get::<_, String>(0))?;
    for name in names {
        let name = name?;
        // DuckDB resolves identifiers case-insensitively, so `_OX_ROW` would
        // collide with the engine's `_ox_row` just as surely.
        if name.to_lowercase().starts_with(RESERVED_PREFIX) {
            return Err(EngineError::Invalid(format!(
                "{relation} has a column {name:?}; names starting {RESERVED_PREFIX} \
                 are reserved for the engine"
            )));
        }
    }
    if hash_columns.is_empty() {
        return Err(EngineError::Invalid(format!(
            "{relation}: no columns to hash"
        )));
    }

    // Each part names its column, so that moving a value from one column to
    // the next changes the hash. chr(0) stands for a NULL cell, which a CSV
    // cannot write, and chr(31) (unit separator) joins the parts. The column
    // names are literals because a CREATE TABLE AS cannot bind parameters;
    // quote_literal escapes them.
    let parts: Vec<String> = hash_columns
        .iter()
        .map(|c| {
            format!(
                "{} || coalesce(CAST({} AS VARCHAR), chr(0))",
                quote_literal(&format!("{}=", c.name)),
                c.text_expr
            )
        })
        .collect();
    let target = format!(
        "{}.{}",
        quote_ident(&source_schema(source_id)),
        quote_ident(relation)
    );
    conn.execute_batch(&format!(
        "CREATE SCHEMA IF NOT EXISTS {schema};
         CREATE TABLE {target} AS
         SELECT *, row_number() OVER (PARTITION BY _ox_hash ORDER BY _ox_row) AS _ox_occ
         FROM (
             SELECT *,
                    nextval('ox.row_seq') AS _ox_row,
                    sha256(concat_ws(chr(31), {parts})) AS _ox_hash
             FROM {staged}
         )
         ORDER BY _ox_row;",
        schema = quote_ident(&source_schema(source_id)),
        parts = parts.join(", "),
    ))?;

    let (rows, first, last): (i64, Option<i64>, Option<i64>) = conn.query_row(
        &format!("SELECT count(*), min(_ox_row), max(_ox_row) FROM {target}"),
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok((
        u64::try_from(rows).unwrap_or_default(),
        first.unwrap_or(0),
        last.unwrap_or(0),
    ))
}

/// Recreate, for every relation in `ox.relation`, the view `main."<name>"`
/// over its active source tables and the all-text view `main."raw_<name>"`,
/// which shows every catalogued column's text under the column's own name.
///
/// Each source's table is one arm of a `UNION ALL BY NAME`, filtered to the
/// hosts that are active for it in `ox.source_dataset`; a removed source
/// contributes no arm. A relation with no active arm gets no views.
pub(crate) fn rebuild_relations(conn: &Connection) -> Result<()> {
    let relations: Vec<String> = conn
        .prepare("SELECT name FROM ox.relation ORDER BY name")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;

    for name in relations {
        let arms = active_arms(conn, &name)?;
        let view = format!("main.{}", quote_ident(&name));
        let raw_view = format!("main.{}", quote_ident(&format!("raw_{name}")));
        if arms.is_empty() {
            conn.execute_batch(&format!(
                "DROP VIEW IF EXISTS {raw_view}; DROP VIEW IF EXISTS {view};"
            ))?;
            continue;
        }
        let union: Vec<String> = arms.into_iter().map(|(_, sql)| sql).collect();
        conn.execute_batch(&format!(
            "CREATE OR REPLACE VIEW {view} AS\n{};",
            union.join("\nUNION ALL BY NAME\n")
        ))?;

        let columns: Vec<(String, String)> = conn
            .prepare(
                "SELECT name, text_column FROM ox.column_catalog \
                 WHERE relation = ? ORDER BY ordinal",
            )?
            .query_map(params![name], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        if columns.is_empty() {
            // Nothing catalogued says which column holds which text.
            conn.execute_batch(&format!("DROP VIEW IF EXISTS {raw_view};"))?;
            continue;
        }
        // A catalogued column can be missing from every active arm (one only
        // a superseded or removed source had, or a run's renamed host
        // column); the raw view still shows it, as NULL.
        let present: Vec<String> = conn
            .prepare(&format!(
                "SELECT lower(column_name) FROM (DESCRIBE SELECT * FROM {view})"
            ))?
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let mut select: Vec<String> = columns
            .iter()
            .map(|(name, text)| {
                let source = if present.contains(&text.to_lowercase()) {
                    quote_ident(text)
                } else {
                    "CAST(NULL AS VARCHAR)".to_string()
                };
                format!("{source} AS {}", quote_ident(name))
            })
            .collect();
        select.extend(["_ox_row", "_ox_hash", "_ox_occ"].map(String::from));
        conn.execute_batch(&format!(
            "CREATE OR REPLACE VIEW {raw_view} AS SELECT {} FROM {view};",
            select.join(", ")
        ))?;
    }
    Ok(())
}

/// The column of source `source_id`'s table for `relation` that names each
/// row's host, or `None` when it has none (a loose file loaded without one).
///
/// A run's is the host metadata column its own inventory recorded, not the
/// relation's `host_column`: the suite renames that column (`_triage_host_1`
/// ...) when a dataset already has one of the name, so two runs feeding one
/// relation need not agree on it. A loose file has no inventory; its host
/// column is the relation's.
pub(crate) fn host_column_of(
    conn: &Connection,
    source_id: i64,
    relation: &str,
) -> Result<Option<String>> {
    let recorded: Option<String> = conn
        .query_row(
            "SELECT metadata_columns_json FROM ox.inventory_dataset \
             WHERE source_id = ? AND view = ?",
            params![source_id, relation],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            duckdb::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    if let Some(json) = recorded {
        let meta: MetadataColumns = serde_json::from_str(&json).map_err(|e| {
            EngineError::Invalid(format!(
                "the case's inventory of source {source_id} for {relation} is unreadable: {e}"
            ))
        })?;
        return Ok(Some(meta.host));
    }
    Ok(conn.query_row(
        "SELECT host_column FROM ox.relation WHERE name = ?",
        params![relation],
        |r| r.get(0),
    )?)
}

/// The `(source id, SELECT)` arms of relation `name`: one per non-removed
/// source table with at least one active host, in source order, each
/// filtered on its own source's host column.
fn active_arms(conn: &Connection, name: &str) -> Result<Vec<(i64, String)>> {
    let tables: Vec<(i64, String)> = conn
        .prepare(
            "SELECT st.source_id, st.table_schema FROM ox.source_table st \
             JOIN ox.source s ON s.id = st.source_id \
             WHERE st.relation = ? AND NOT s.removed ORDER BY st.source_id",
        )?
        .query_map(params![name], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;

    let mut arms = Vec::new();
    for (source_id, schema) in tables {
        // A loose file loaded without a host has one row here with a NULL
        // host and no host column to filter on.
        let hosts: Vec<Option<String>> = conn
            .prepare(
                "SELECT host FROM ox.source_dataset \
                 WHERE source_id = ? AND relation = ? AND active ORDER BY host",
            )?
            .query_map(params![source_id, name], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        if hosts.is_empty() {
            continue;
        }
        let hosts: Vec<String> = hosts.into_iter().flatten().collect();
        let mut sql = format!(
            "SELECT * FROM {}.{}",
            quote_ident(&schema),
            quote_ident(name)
        );
        let host_column = if hosts.is_empty() {
            None
        } else {
            host_column_of(conn, source_id, name)?
        };
        if let Some(host_column) = host_column {
            // A view cannot bind parameters, so the host values are
            // literals, escaped by quote_literal.
            let list: Vec<String> = hosts.iter().map(|h| quote_literal(h)).collect();
            sql.push_str(&format!(
                " WHERE {} IN ({})",
                quote_ident(&host_column),
                list.join(", ")
            ));
        }
        arms.push((source_id, sql));
    }
    Ok(arms)
}
