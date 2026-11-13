//! The column catalog: every logical column of every relation, where its
//! text lives and where its typed value lives.
//!
//! Every later query resolves a column through here, never through the
//! physical tables. A column the suite typed is two physical columns, the
//! typed one under the column's name and its text as `<col>__text`. A column
//! the app or an analyst types gets a companion `<col>__typed`, filled with
//! `TRY_CAST` from the text, in every source table of the relation; the
//! original column is never altered, so the cell text is always what the
//! source wrote.

pub mod appmap;
mod populate;
mod zones;

pub use zones::{host_zones, HostZone};

pub use triage_core::output::duckdb::types::{SqlType, TimeSemantics};

pub(crate) use populate::{
    catalog_dataset, catalog_file, check_sql_type, check_typing, FileColumn,
};

use crate::actor::Actor;
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::load::stage::rebuild_relations;
use crate::time::now_iso;
use appmap::{primary_time_for, rule_for, AppRule};
use duckdb::{params, params_from_iter, Connection};
use std::collections::{BTreeMap, BTreeSet};
use triage_core::output::duckdb::sql::{quote_ident, NameAllocator};

/// Who typed a column. Serialised as `ox.column_catalog.origin` spells it
/// (`as_str`), which a test pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "kebab-case")]
pub enum TypeOrigin {
    Suite,
    AppMap,
    Sniffed,
    Analyst,
}

/// What `ox.column_catalog.origin` holds for a column nobody typed.
const UNTYPED: &str = "text";

impl TypeOrigin {
    /// The spelling stored in `ox.column_catalog.origin`.
    pub fn as_str(&self) -> &'static str {
        match self {
            TypeOrigin::Suite => "suite",
            TypeOrigin::AppMap => "app-map",
            TypeOrigin::Sniffed => "sniffed",
            TypeOrigin::Analyst => "analyst",
        }
    }

    /// The inverse of `as_str`; `Ok(None)` for an untyped column.
    fn parse(text: &str) -> Result<Option<TypeOrigin>> {
        if text == UNTYPED {
            return Ok(None);
        }
        [
            TypeOrigin::Suite,
            TypeOrigin::AppMap,
            TypeOrigin::Sniffed,
            TypeOrigin::Analyst,
        ]
        .into_iter()
        .find(|o| o.as_str() == text)
        .map(Some)
        .ok_or_else(|| corrupt(format!("type origin {text:?}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Dataset,
    File,
}

impl RelationKind {
    fn parse(text: &str) -> Result<RelationKind> {
        match text {
            "dataset" => Ok(RelationKind::Dataset),
            "file" => Ok(RelationKind::File),
            other => Err(corrupt(format!("relation kind {other:?}"))),
        }
    }
}

/// What a column holds: the source's own data, or one of the metadata
/// columns the suite injects into a run's datasets (or the engine adds to a
/// loose file loaded with a host). Metadata names can carry a collision
/// suffix (`_triage_host_1`), so the role, not the name, says which is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ColumnRole {
    Data,
    RunId,
    Host,
    Identity,
    OutputFile,
}

impl ColumnRole {
    const ALL: [ColumnRole; 5] = [
        ColumnRole::Data,
        ColumnRole::RunId,
        ColumnRole::Host,
        ColumnRole::Identity,
        ColumnRole::OutputFile,
    ];

    /// The spelling stored in `ox.column_catalog.role`.
    pub fn as_str(&self) -> &'static str {
        match self {
            ColumnRole::Data => "data",
            ColumnRole::RunId => "run_id",
            ColumnRole::Host => "host",
            ColumnRole::Identity => "identity",
            ColumnRole::OutputFile => "output_file",
        }
    }

    fn parse(text: &str) -> Result<ColumnRole> {
        ColumnRole::ALL
            .into_iter()
            .find(|r| r.as_str() == text)
            .ok_or_else(|| corrupt(format!("column role {text:?}")))
    }
}

/// One logical column of a relation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ColumnInfo {
    pub name: String,
    pub role: ColumnRole,
    /// The physical column holding the original cell text.
    pub text_column: String,
    /// The physical column holding the typed value; `None` when untyped.
    pub typed_column: Option<String>,
    #[cfg_attr(feature = "ts", ts(as = "Option<crate::ts::SqlTypeTs>"))]
    pub sql_type: Option<SqlType>,
    #[cfg_attr(feature = "ts", ts(as = "Option<crate::ts::TimeSemanticsTs>"))]
    pub time_semantics: Option<TimeSemantics>,
    /// `None` when untyped.
    pub origin: Option<TypeOrigin>,
}

/// One queryable relation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RelationInfo {
    pub name: String,
    pub kind: RelationKind,
    pub tool: Option<String>,
    /// Rows in its view: the active rows of every non-removed source.
    pub rows: u64,
    /// The hosts with active rows, sorted.
    pub hosts: Vec<String>,
    /// The catalogued timestamp column the timeline uses.
    pub primary_time: Option<String>,
    pub evidence_path_column: Option<String>,
    pub host_column: Option<String>,
}

/// Every relation in the case, sorted by name.
pub fn relations(case: &Case) -> Result<Vec<RelationInfo>> {
    read_relations(case.conn(), None)
}

/// The relation `name`; `NotFound` when the case has none.
pub fn relation(case: &Case, name: &str) -> Result<RelationInfo> {
    read_relation(case.conn(), name)
}

/// The catalogued columns of `relation`, in display order. Physical
/// companions (`__text`, `__typed`) and the engine's `_ox_*` columns are not
/// columns of their own: they are reached through a column's `text_column`
/// and `typed_column`.
pub fn columns(case: &Case, relation: &str) -> Result<Vec<ColumnInfo>> {
    let conn = case.conn();
    require_relation(conn, relation)?;
    read_columns(conn, relation)
}

/// The comments DuckDB holds on the columns of `relation`'s view, by column
/// name; a column without one has no entry. `NotFound` for an unknown
/// relation. Read on the case's own connection, which any open serves.
pub fn column_comments(case: &Case, relation: &str) -> Result<BTreeMap<String, String>> {
    let conn = case.conn();
    require_relation(conn, relation)?;
    Ok(conn
        .prepare(
            "SELECT column_name, comment FROM duckdb_columns() \
             WHERE database_name = current_database() AND schema_name = 'main' \
               AND table_name = ? AND comment IS NOT NULL AND comment <> ''",
        )?
        .query_map(params![relation], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?)
}

/// Type `column` of `relation` as `sql_type`, in a companion column filled
/// from its text. An agent may not; a timestamp needs semantics `Utc` or
/// `Unknown` (step 1 has no zone conversion, so `Local` is `Invalid`). A
/// column the suite typed keeps its type: declaring one is `Invalid`.
pub fn declare_type(
    case: &mut Case,
    relation: &str,
    column: &str,
    sql_type: SqlType,
    semantics: Option<TimeSemantics>,
    actor: &Actor,
) -> Result<()> {
    if actor.is_agent() {
        return Err(EngineError::Refused(
            "an agent may not declare a column's type; an analyst must".to_string(),
        ));
    }
    match (sql_type, semantics) {
        (SqlType::Timestamp, Some(TimeSemantics::Utc | TimeSemantics::Unknown)) => {}
        (SqlType::Timestamp, Some(TimeSemantics::Local)) => {
            return Err(EngineError::Invalid(
                "local time semantics are not supported yet: declare utc or unknown".to_string(),
            ))
        }
        (SqlType::Timestamp, None) => {
            return Err(EngineError::Invalid(
                "a timestamp needs its time semantics: utc or unknown".to_string(),
            ))
        }
        (_, Some(_)) => {
            return Err(EngineError::Invalid(format!(
                "time semantics apply only to a timestamp, not to {}",
                sql_type.sql()
            )))
        }
        (_, None) => {}
    }
    let tx = case.conn_mut().transaction()?;
    declare_in(&tx, relation, column, sql_type, semantics, actor)?;
    tx.commit()?;
    Ok(())
}

fn declare_in(
    conn: &Connection,
    relation: &str,
    column: &str,
    sql_type: SqlType,
    semantics: Option<TimeSemantics>,
    actor: &Actor,
) -> Result<()> {
    let info = read_relation(conn, relation)?;
    let before = read_column(conn, relation, column)?;
    // The suite's typing is what its documented queries read; the case
    // never re-points such a column at another physical column.
    if before.origin == Some(TypeOrigin::Suite) {
        return Err(EngineError::Invalid(format!(
            "{relation}.{column} is already typed by the suite; its type cannot be redeclared"
        )));
    }
    if info.primary_time.as_deref() == Some(column) && sql_type != SqlType::Timestamp {
        // The type does not fit the column's role: the input is wrong.
        return Err(EngineError::Invalid(format!(
            "{column} is the primary time of {relation}; choose another primary time first"
        )));
    }

    // Past the suite check, a typed column is always a companion: one this
    // column already has is replaced under the same name; otherwise a new
    // one is named.
    let reuse = match &before.typed_column {
        Some(companion) => {
            for schema in source_schemas(conn, relation)? {
                if physical_columns(conn, &schema, relation)?.contains(&companion.to_lowercase()) {
                    conn.execute_batch(&format!(
                        "ALTER TABLE {} DROP COLUMN {};",
                        table(&schema, relation),
                        quote_ident(companion)
                    ))?;
                }
            }
            Some(companion.clone())
        }
        _ => None,
    };
    set_typing(
        conn,
        relation,
        column,
        reuse,
        (sql_type, semantics),
        TypeOrigin::Analyst,
    )?;
    sync_companions(conn, relation)?;
    // As a load would: a relation with no primary time, and none an analyst
    // chose (none counts), takes one by the load's rule, its listed primary
    // time or else its first catalogued timestamp; usually the one just
    // declared. The history row says what it became.
    default_primary_time(conn, relation)?;
    let after = read_column(conn, relation, column)?;
    let with_primary_time = |mut json: serde_json::Value, primary: Option<String>| {
        json["primary_time"] = serde_json::json!(primary);
        json
    };
    record(
        conn,
        actor,
        relation,
        "declare_type",
        with_primary_time(column_json(&before), info.primary_time.clone()),
        with_primary_time(
            column_json(&after),
            read_relation(conn, relation)?.primary_time,
        ),
    )?;
    rebuild_relations(conn)
}

/// Make `column` (a catalogued timestamp) the time `relation`'s timeline
/// runs on, or clear it with `None`. An agent may not: the choice outlives
/// later loads and changes the analyst's histogram, default sort and pivots.
pub fn set_primary_time(
    case: &mut Case,
    relation: &str,
    column: Option<&str>,
    actor: &Actor,
) -> Result<()> {
    if actor.is_agent() {
        return Err(EngineError::Refused(
            "an agent may not choose a relation's primary time; an analyst must".to_string(),
        ));
    }
    let tx = case.conn_mut().transaction()?;
    let before = read_relation(&tx, relation)?.primary_time;
    if let Some(column) = column {
        let info = read_column(&tx, relation, column)?;
        if info.sql_type != Some(SqlType::Timestamp) {
            return Err(EngineError::Invalid(format!(
                "{relation}.{column} is not a timestamp; declare its type first"
            )));
        }
    }
    tx.execute(
        "UPDATE ox.relation SET primary_time = ?, primary_time_chosen = true WHERE name = ?",
        params![column, relation],
    )?;
    record(
        &tx,
        actor,
        relation,
        "set_primary_time",
        serde_json::json!({ "primary_time": before }),
        serde_json::json!({ "primary_time": column }),
    )?;
    tx.commit()?;
    Ok(())
}

/// Give every source table of `relation` each companion column the catalog
/// names and the table lacks, filled from the table's text. A table without
/// the text column gets none; the union view fills it with NULL.
fn sync_companions(conn: &Connection, relation: &str) -> Result<()> {
    let companions: Vec<ColumnInfo> = read_columns(conn, relation)?
        .into_iter()
        .filter(|c| c.typed_column.as_ref().is_some_and(|t| *t != c.name))
        .collect();
    if companions.is_empty() {
        return Ok(());
    }
    for schema in source_schemas(conn, relation)? {
        let present = physical_columns(conn, &schema, relation)?;
        for column in &companions {
            let (Some(typed), Some(sql_type)) = (&column.typed_column, column.sql_type) else {
                continue;
            };
            if present.contains(&typed.to_lowercase())
                || !present.contains(&column.text_column.to_lowercase())
            {
                continue;
            }
            let rule = match column.origin {
                Some(TypeOrigin::AppMap) => rule_for(relation, &column.name),
                _ => None,
            };
            let text = quote_ident(&column.text_column);
            let cast = format!("TRY_CAST({text} AS {})", sql_type.sql());
            let fill = match rule {
                Some(AppRule::UtcWhenTextEndsInZ) => {
                    format!("CASE WHEN ends_with({text}, 'Z') THEN {cast} END")
                }
                None => cast,
            };
            let target = table(&schema, relation);
            conn.execute_batch(&format!(
                "ALTER TABLE {target} ADD COLUMN {typed} {ty};
                 UPDATE {target} SET {typed} = {fill};",
                typed = quote_ident(typed),
                ty = sql_type.sql(),
            ))?;
        }
    }
    Ok(())
}

/// Give `relation` its default primary time unless an analyst ever chose
/// one for this relation row (choosing none is a choice too): the
/// `PRIMARY_TIME` entry once that is a catalogued timestamp, else, when it
/// has none, its first catalogued timestamp. An automatic choice is not
/// final: a relation that took its first timestamp, as one loaded before
/// its entry existed did, moves to the entry's column once that is typed.
/// The choice is read from the row itself, not from history, so a relation
/// recreated after its last source was removed starts from the default
/// again.
fn default_primary_time(conn: &Connection, relation: &str) -> Result<()> {
    let chosen: bool = conn.query_row(
        "SELECT primary_time_chosen FROM ox.relation WHERE name = ?",
        params![relation],
        |r| r.get(0),
    )?;
    if chosen {
        return Ok(());
    }
    let current = read_relation(conn, relation)?.primary_time;
    let timestamps: Vec<String> = read_columns(conn, relation)?
        .into_iter()
        .filter(|c| c.sql_type == Some(SqlType::Timestamp))
        .map(|c| c.name)
        .collect();
    let listed =
        primary_time_for(relation).and_then(|p| timestamps.iter().find(|t| t.as_str() == p));
    let pick = match (listed, &current) {
        (Some(listed), _) => Some(listed),
        // An automatic choice with no listed column to move to stays.
        (None, Some(_)) => return Ok(()),
        (None, None) => timestamps.first(),
    };
    if pick == current.as_ref() {
        return Ok(());
    }
    if let Some(column) = pick {
        conn.execute(
            "UPDATE ox.relation SET primary_time = ? WHERE name = ?",
            params![column, relation],
        )?;
    }
    Ok(())
}

/// Record `column`'s typing in the catalog: its companion is `reuse` (the
/// name it already has) or a newly allocated one. The physical companion is
/// filled afterwards by `sync_companions`.
fn set_typing(
    conn: &Connection,
    relation: &str,
    column: &str,
    reuse: Option<String>,
    (sql_type, semantics): (SqlType, Option<TimeSemantics>),
    origin: TypeOrigin,
) -> Result<()> {
    let typed = match reuse {
        Some(name) => name,
        None => allocate_companion(conn, relation, column)?,
    };
    conn.execute(
        "UPDATE ox.column_catalog \
         SET typed_column = ?, sql_type = ?, time_semantics = ?, origin = ? \
         WHERE relation = ? AND name = ?",
        params![
            typed,
            sql_type.sql(),
            semantics.map(|s| s.as_str()),
            origin.as_str(),
            relation,
            column
        ],
    )?;
    Ok(())
}

/// A free name for `column`'s companion: `<column>__typed` unless that
/// collides with a physical column of some source table or a name the
/// catalog already uses for the relation.
fn allocate_companion(conn: &Connection, relation: &str, column: &str) -> Result<String> {
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for schema in source_schemas(conn, relation)? {
        taken.extend(physical_columns(conn, &schema, relation)?);
    }
    for c in read_columns(conn, relation)? {
        taken.insert(c.name);
        taken.insert(c.text_column);
        taken.extend(c.typed_column);
    }
    Ok(NameAllocator::new(taken).allocate(&format!("{column}__typed")))
}

/// The schemas holding a table for `relation`, one per non-removed source.
fn source_schemas(conn: &Connection, relation: &str) -> Result<Vec<String>> {
    Ok(conn
        .prepare(
            "SELECT st.table_schema FROM ox.source_table st \
             JOIN ox.source s ON s.id = st.source_id \
             WHERE st.relation = ? AND NOT s.removed ORDER BY st.source_id",
        )?
        .query_map(params![relation], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?)
}

/// The column names of `schema.relation`, lowercased: DuckDB resolves
/// identifiers case-insensitively, so that is how they collide.
pub(crate) fn physical_columns(
    conn: &Connection,
    schema: &str,
    relation: &str,
) -> Result<BTreeSet<String>> {
    Ok(conn
        .prepare(
            "SELECT lower(column_name) FROM duckdb_columns() \
             WHERE schema_name = ? AND table_name = ?",
        )?
        .query_map(params![schema, relation], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?)
}

fn table(schema: &str, relation: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(relation))
}

fn require_relation(conn: &Connection, relation: &str) -> Result<()> {
    let present: i64 = conn.query_row(
        "SELECT count(*) FROM ox.relation WHERE name = ?",
        params![relation],
        |r| r.get(0),
    )?;
    if present == 0 {
        return Err(EngineError::NotFound(format!("no relation {relation}")));
    }
    Ok(())
}

fn read_relation(conn: &Connection, name: &str) -> Result<RelationInfo> {
    read_relations(conn, Some(name))?
        .pop()
        .ok_or_else(|| EngineError::NotFound(format!("no relation {name}")))
}

/// The case's relations, or only the one named `only`, sorted by name. Two
/// statements however many relations there are: every pivot reads them all,
/// and a statement costs about a millisecond whatever it returns.
fn read_relations(conn: &Connection, only: Option<&str>) -> Result<Vec<RelationInfo>> {
    // The datasets that count: active ones of sources not removed.
    const ACTIVE: &str = "ox.source_dataset sd JOIN ox.source s ON s.id = sd.source_id \
                          WHERE sd.active AND NOT s.removed";
    let (one_relation, one_dataset) = match only {
        Some(_) => ("WHERE r.name = ?", "AND sd.relation = ?"),
        None => ("", ""),
    };

    let mut hosts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut stmt = conn.prepare(&format!(
        "SELECT DISTINCT sd.relation, sd.host FROM {ACTIVE} AND sd.host IS NOT NULL \
         {one_dataset} ORDER BY sd.relation, sd.host"
    ))?;
    let pairs = stmt.query_map(params_from_iter(only), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    for pair in pairs {
        let (relation, host) = pair?;
        hosts.entry(relation).or_default().push(host);
    }

    type Row = (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
    );
    let rows: Vec<Row> = conn
        .prepare(&format!(
            "SELECT r.name, r.kind, r.tool, r.primary_time, r.evidence_path_column, \
                    r.host_column, CAST(coalesce(a.rows, 0) AS BIGINT) \
             FROM ox.relation r LEFT JOIN \
                  (SELECT sd.relation, sum(sd.rows) AS rows FROM {ACTIVE} GROUP BY sd.relation) a \
                  ON a.relation = r.name \
             {one_relation} ORDER BY r.name"
        ))?
        .query_map(params_from_iter(only), |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?
        .collect::<std::result::Result<_, _>>()?;
    rows.into_iter()
        .map(
            |(name, kind, tool, primary_time, evidence_path_column, host_column, rows)| {
                Ok(RelationInfo {
                    kind: RelationKind::parse(&kind)?,
                    tool,
                    rows: u64::try_from(rows).unwrap_or_default(),
                    hosts: hosts.remove(&name).unwrap_or_default(),
                    primary_time,
                    evidence_path_column,
                    host_column,
                    name,
                })
            },
        )
        .collect()
}

type CatalogRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
);

/// What `catalog_row` reads, in its order.
const CATALOG_COLUMNS: &str =
    "name, text_column, typed_column, sql_type, time_semantics, origin, role";

/// The names of `relation`'s catalogued columns, in display order, as the
/// catalog spells them; none for a relation it does not have yet.
pub(crate) fn column_names(conn: &Connection, relation: &str) -> Result<Vec<String>> {
    Ok(conn
        .prepare("SELECT name FROM ox.column_catalog WHERE relation = ? ORDER BY ordinal")?
        .query_map(params![relation], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?)
}

fn read_columns(conn: &Connection, relation: &str) -> Result<Vec<ColumnInfo>> {
    let rows: Vec<CatalogRow> = conn
        .prepare(&format!(
            "SELECT {CATALOG_COLUMNS} FROM ox.column_catalog WHERE relation = ? ORDER BY ordinal"
        ))?
        .query_map(params![relation], catalog_row)?
        .collect::<std::result::Result<_, _>>()?;
    rows.into_iter().map(decode_column).collect()
}

/// The catalogued columns of every relation, each list in display order, in
/// one statement. A relation with no catalogued column has no entry.
pub(crate) fn all_columns(conn: &Connection) -> Result<BTreeMap<String, Vec<ColumnInfo>>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {CATALOG_COLUMNS}, relation FROM ox.column_catalog ORDER BY relation, ordinal"
    ))?;
    let rows = stmt.query_map([], |r| Ok((catalog_row(r)?, r.get::<_, String>(7)?)))?;
    let mut out: BTreeMap<String, Vec<ColumnInfo>> = BTreeMap::new();
    for row in rows {
        let (column, relation) = row?;
        out.entry(relation)
            .or_default()
            .push(decode_column(column)?);
    }
    Ok(out)
}

/// One catalogued column; `NotFound` when `relation` has no such column.
fn read_column(conn: &Connection, relation: &str, column: &str) -> Result<ColumnInfo> {
    let row = conn
        .query_row(
            &format!(
                "SELECT {CATALOG_COLUMNS} FROM ox.column_catalog WHERE relation = ? AND name = ?"
            ),
            params![relation, column],
            catalog_row,
        )
        .map_err(|e| match e {
            duckdb::Error::QueryReturnedNoRows => {
                EngineError::NotFound(format!("{relation} has no column {column}"))
            }
            other => other.into(),
        })?;
    decode_column(row)
}

fn catalog_row(r: &duckdb::Row<'_>) -> duckdb::Result<CatalogRow> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
    ))
}

fn decode_column(row: CatalogRow) -> Result<ColumnInfo> {
    let (name, text_column, typed_column, sql_type, semantics, origin, role) = row;
    let sql_type = sql_type
        .map(|t| SqlType::from_sql(&t).ok_or_else(|| corrupt(format!("SQL type {t:?}"))))
        .transpose()?;
    let time_semantics = semantics
        .map(|s| TimeSemantics::parse(&s).ok_or_else(|| corrupt(format!("time semantics {s:?}"))))
        .transpose()?;
    Ok(ColumnInfo {
        name,
        role: ColumnRole::parse(&role)?,
        text_column,
        typed_column,
        sql_type,
        time_semantics,
        origin: TypeOrigin::parse(&origin)?,
    })
}

/// A catalog value the engine never writes: the case is damaged, not the
/// caller's input.
fn corrupt(what: String) -> EngineError {
    EngineError::Internal(format!("the case catalog holds an unknown {what}"))
}

fn column_json(c: &ColumnInfo) -> serde_json::Value {
    serde_json::json!({
        "name": c.name,
        "text_column": c.text_column,
        "typed_column": c.typed_column,
        "sql_type": c.sql_type.map(|t| t.sql()),
        "time_semantics": c.time_semantics.map(|s| s.as_str()),
        "origin": c.origin.map(|o| o.as_str()),
    })
}

/// One history row for a change to `relation`, in the caller's transaction.
fn record(
    conn: &Connection,
    actor: &Actor,
    relation: &str,
    action: &str,
    before: serde_json::Value,
    after: serde_json::Value,
) -> Result<()> {
    conn.execute(
        "INSERT INTO ox.history \
         (id, \"at\", \"by\", target_kind, target_id, action, before_json, after_json) \
         VALUES (nextval('ox.history_seq'), ?, ?, 'relation', ?, ?, ?, ?)",
        params![
            now_iso(),
            actor.label(),
            relation,
            action,
            before.to_string(),
            after.to_string()
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json<T: serde::Serialize>(value: T) -> String {
        serde_json::to_value(value)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The app reads the catalog as JSON, and the spellings it gets are the
    /// ones the case stores: the derived names must not drift from the
    /// `as_str` the catalog reads back with `parse`.
    #[test]
    fn serialised_spellings_are_the_stored_ones() {
        for origin in [
            TypeOrigin::Suite,
            TypeOrigin::AppMap,
            TypeOrigin::Sniffed,
            TypeOrigin::Analyst,
        ] {
            assert_eq!(json(origin), origin.as_str());
            assert_eq!(TypeOrigin::parse(&json(origin)).unwrap(), Some(origin));
        }
        for role in ColumnRole::ALL {
            assert_eq!(json(role), role.as_str());
            assert_eq!(ColumnRole::parse(&json(role)).unwrap(), role);
        }
        for kind in [RelationKind::Dataset, RelationKind::File] {
            assert_eq!(RelationKind::parse(&json(kind)).unwrap(), kind);
        }
    }

    /// A column's type and time semantics reach the app as the catalog
    /// stores them in `ox.column_catalog.sql_type` and `.time_semantics`,
    /// which `decode_column` reads back with `from_sql` and `parse`.
    #[test]
    fn column_types_serialise_as_the_catalog_stores_them() {
        for t in SqlType::ALL {
            assert_eq!(SqlType::from_sql(&json(t)), Some(t));
        }
        for s in TimeSemantics::ALL {
            assert_eq!(TimeSemantics::parse(&json(s)), Some(s));
        }
    }
}
