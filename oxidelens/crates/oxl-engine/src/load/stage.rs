//! From a staged relation to a case table, and from case tables to the views
//! an analyst queries.
//!
//! A load first exposes its input as a relation in a staging schema (for a
//! run, the suite's own rendered views). `materialize` copies it into
//! `src_<id>`, adding the three columns every loaded row carries:
//!
//! - `_ox_row`: a case-wide unique id from `ox.row_seq`. Annotations hang off it.
//! - `_ox_hash`: a digest of the row's content (host, identity and every
//!   column's text, see `row_hash_sql`), deliberately blind to the run id and
//!   the output path,
//!   so that the same event read by a later run of the same host has the same
//!   hash. Supersession matches rows on it.
//! - `_ox_occ`: which occurrence of that hash the row is, so that two
//!   byte-identical rows stay two distinguishable rows.
//!
//! `rebuild_relations` then derives every view in `main` from the case
//! tables alone, so it can be rerun after any change to what is active.

use crate::error::{EngineError, Result};
use duckdb::{params, Connection};
use std::collections::{BTreeMap, BTreeSet};
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
                    {hash} AS _ox_hash
             FROM {staged}
         )
         ORDER BY _ox_row;",
        schema = quote_ident(&source_schema(source_id)),
        hash = row_hash_sql(hash_columns),
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

/// The SQL expression for a row's `_ox_hash`: the SHA-256 of the JSON array
/// `[[name, text], ...]` over `columns` in the order given, `text` being
/// `null` for a NULL cell.
///
/// DuckDB writes the JSON, so every quote, backslash and control character
/// in a cell is escaped and no cell can imitate the array's structure: text
/// cannot shift between columns, and NULL, the empty string and any text
/// (`"null"` and NUL included) all encode differently. Naming each column
/// means moving a value from one column to the next changes the hash. The
/// names are literals because a CREATE TABLE AS cannot bind parameters;
/// quote_literal escapes them.
fn row_hash_sql(columns: &[HashColumn]) -> String {
    let pairs: Vec<String> = columns
        .iter()
        .map(|c| {
            format!(
                "[{}, CAST({} AS VARCHAR)]",
                quote_literal(&c.name),
                c.text_expr
            )
        })
        .collect();
    format!("sha256(CAST(to_json([{}]) AS VARCHAR))", pairs.join(", "))
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
        // a superseded or removed source had); the raw view still shows it,
        // as NULL.
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

/// The column of `relation`'s tables that names each row's host, or `None`
/// when it has none (a loose file loaded without one). Every live source of
/// a relation names its metadata columns alike (a run that does not is
/// refused), so the relation's own record holds for every table.
pub(crate) fn host_column(conn: &Connection, relation: &str) -> Result<Option<String>> {
    Ok(conn.query_row(
        "SELECT host_column FROM ox.relation WHERE name = ?",
        params![relation],
        |r| r.get(0),
    )?)
}

/// The `(source id, SELECT)` arms of relation `name`: one per non-removed
/// source table with at least one active host, in source order.
fn active_arms(conn: &Connection, name: &str) -> Result<Vec<(i64, String)>> {
    let host_column = host_column(conn, name)?;
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
        if let Some(host_column) = host_column.as_deref().filter(|_| !hosts.is_empty()) {
            // A view cannot bind parameters, so the host values are
            // literals, escaped by quote_literal.
            let list: Vec<String> = hosts.iter().map(|h| quote_literal(h)).collect();
            sql.push_str(&format!(
                " WHERE {} IN ({})",
                quote_ident(host_column),
                list.join(", ")
            ));
        }
        arms.push((source_id, sql));
    }
    Ok(arms)
}

/// The physical columns of every relation's view `main."<name>"`, lowercased
/// (DuckDB resolves identifiers case-insensitively), in one statement. A
/// relation with no view has no entry.
///
/// A view is the `UNION ALL BY NAME` of its arms, each a `SELECT *`, so its
/// columns are those of the tables `active_arms` picks, and this reads them
/// by the same rule: a non-removed source's table with at least one active
/// host. Describing each view says the same and costs a statement per view.
pub(crate) fn view_columns(conn: &Connection) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT st.relation, lower(c.column_name) \
         FROM ox.source_table st \
         JOIN ox.source s ON s.id = st.source_id AND NOT s.removed \
         JOIN duckdb_columns() c \
           ON c.database_name = current_database() \
          AND c.schema_name = st.table_schema AND c.table_name = st.relation \
         WHERE EXISTS (SELECT 1 FROM ox.source_dataset sd \
                       WHERE sd.source_id = st.source_id AND sd.relation = st.relation \
                         AND sd.active)",
    )?;
    let pairs = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for pair in pairs {
        let (relation, column) = pair?;
        out.entry(relation).or_default().insert(column);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hash of one row with columns `a` and `b` whose text is given as
    /// SQL expressions.
    fn hash(conn: &Connection, a: &str, b: &str) -> String {
        let columns = [
            HashColumn {
                name: "a".into(),
                text_expr: "a".into(),
            },
            HashColumn {
                name: "b".into(),
                text_expr: "b".into(),
            },
        ];
        conn.query_row(
            &format!(
                "SELECT {} FROM (SELECT {a} AS a, {b} AS b)",
                row_hash_sql(&columns)
            ),
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn null_empty_and_lookalike_text_hash_apart() {
        let conn = Connection::open_in_memory().unwrap();
        let values = [
            "CAST(NULL AS VARCHAR)",
            "''",
            "'null'",
            "chr(0)",
            "'\"'",
            "'\\'",
        ];
        let mut seen = std::collections::HashSet::new();
        for v in values {
            assert!(seen.insert(hash(&conn, v, "'1'")), "{v} collided");
        }
        // Text moved between columns across what would be a separator.
        assert_ne!(
            hash(&conn, "'x\", \"b\"'", "'y'"),
            hash(&conn, "'x'", "'\", \"b\"y'")
        );
        assert_ne!(hash(&conn, "'x'", "'y'"), hash(&conn, "'y'", "'x'"));
    }
}
