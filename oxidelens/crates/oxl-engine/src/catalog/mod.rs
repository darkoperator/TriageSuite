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

pub use triage_core::output::duckdb::types::{SqlType, TimeSemantics};

use crate::actor::Actor;
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::load::stage::rebuild_relations;
use crate::time::now_iso;
use appmap::{primary_time_for, rule_for, AppRule, APP_TYPES};
use duckdb::{params, Connection};
use std::collections::BTreeSet;
use triage_core::output::duckdb::inventory::Dataset;
use triage_core::output::duckdb::sql::{quote_ident, NameAllocator};

/// Who typed a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// One logical column of a relation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    /// The physical column holding the original cell text.
    pub text_column: String,
    /// The physical column holding the typed value; `None` when untyped.
    pub typed_column: Option<String>,
    pub sql_type: Option<SqlType>,
    pub time_semantics: Option<TimeSemantics>,
    /// `None` when untyped.
    pub origin: Option<TypeOrigin>,
}

/// One queryable relation.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    let conn = case.conn();
    let names: Vec<String> = conn
        .prepare("SELECT name FROM ox.relation ORDER BY name")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    names.iter().map(|name| read_relation(conn, name)).collect()
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
        return Err(EngineError::Refused(format!(
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
    let after = read_column(conn, relation, column)?;
    record(
        conn,
        actor,
        relation,
        "declare_type",
        column_json(&before),
        column_json(&after),
    )?;
    rebuild_relations(conn)
}

/// Make `column` (a catalogued timestamp) the time `relation`'s timeline
/// runs on, or clear it with `None`.
pub fn set_primary_time(
    case: &mut Case,
    relation: &str,
    column: Option<&str>,
    actor: &Actor,
) -> Result<()> {
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

/// Catalogue a dataset a load just materialized, inside the load's
/// transaction: its columns, the app's typing of them, the companions every
/// source table of the relation needs, and its default primary time. The
/// app's typing is not an analyst's change, so it writes no history.
pub(crate) fn catalog_dataset(conn: &Connection, dataset: &Dataset) -> Result<()> {
    let relation = dataset.view.as_str();
    record_columns(conn, dataset)?;
    apply_app_types(conn, relation)?;
    sync_companions(conn, relation)?;
    default_primary_time(conn, relation)
}

/// Refuse `dataset` when the suite's typing of one of its columns differs
/// from what the catalog already holds for its relation: a column the case
/// has as suite-typed must be typed the same way (SQL type, text column,
/// time semantics), and a column the case has as anything else must not be
/// suite-typed. A relation's column has one typing, and every source table
/// of the relation is read through it. Columns the relation does not have
/// yet are not checked.
pub(crate) fn check_typing(conn: &Connection, dataset: &Dataset) -> Result<()> {
    let relation = dataset.view.as_str();
    let meta = &dataset.metadata_columns;
    let named: BTreeSet<&String> = [&meta.run_id, &meta.host, &meta.identity, &meta.output_file]
        .into_iter()
        .chain(&dataset.columns)
        .collect();
    for column in read_columns(conn, relation)? {
        if !named.contains(&column.name) {
            continue;
        }
        let theirs = dataset.effective_types.get(&column.name);
        let agrees = match (column.origin, theirs) {
            (Some(TypeOrigin::Suite), Some(t)) => {
                SqlType::from_sql(&t.sql_type) == column.sql_type
                    && t.text_column == column.text_column
                    && t.time_semantics.as_deref().map(parse_semantics)
                        == column.time_semantics.map(Some)
            }
            (Some(TypeOrigin::Suite), None) | (_, Some(_)) => false,
            (_, None) => true,
        };
        if !agrees {
            let ours = match column.origin {
                Some(TypeOrigin::Suite) => format!(
                    "the suite typed it {} (text in {}{})",
                    column.sql_type.map_or("?", |t| t.sql()),
                    column.text_column,
                    column
                        .time_semantics
                        .map(|s| format!(", {}", s.as_str()))
                        .unwrap_or_default()
                ),
                _ => "the suite left it as text".to_string(),
            };
            let new = match theirs {
                Some(t) => format!(
                    "this run types it {} (text in {}{})",
                    t.sql_type,
                    t.text_column,
                    t.time_semantics
                        .as_deref()
                        .map(|s| format!(", {s}"))
                        .unwrap_or_default()
                ),
                None => "this run leaves it as text".to_string(),
            };
            return Err(EngineError::Invalid(format!(
                "{relation}.{}: in the case {ours}, but {new}; a relation's columns keep \
                 one typing across sources",
                column.name
            )));
        }
    }
    Ok(())
}

/// One column of a loose file, as the loader found it.
pub(crate) struct FileColumn {
    pub name: String,
    /// What type detection found, with the time semantics for a timestamp;
    /// `None` leaves the column as text.
    pub sniffed: Option<(SqlType, Option<TimeSemantics>)>,
}

/// Catalogue a loose file's table, inside the load's transaction, once its
/// source table is recorded: its columns in order, a companion for each one
/// whose type was sniffed (origin `sniffed`), and its default primary time.
/// Like the app's typing, this writes no history.
pub(crate) fn catalog_file(
    conn: &Connection,
    relation: &str,
    columns: &[FileColumn],
) -> Result<()> {
    for (i, column) in columns.iter().enumerate() {
        conn.execute(
            "INSERT INTO ox.column_catalog \
             (relation, name, ordinal, text_column, typed_column, sql_type, time_semantics, origin) \
             VALUES (?, ?, ?, ?, NULL, NULL, NULL, ?)",
            params![
                relation,
                column.name,
                i32::try_from(i + 1).unwrap_or(i32::MAX),
                column.name,
                UNTYPED
            ],
        )?;
    }
    // Named only after every column is catalogued, so no companion takes a
    // name a later column of the file has.
    for column in columns {
        let Some((sql_type, semantics)) = column.sniffed else {
            continue;
        };
        set_typing(
            conn,
            relation,
            &column.name,
            None,
            (sql_type, semantics),
            TypeOrigin::Sniffed,
        )?;
    }
    sync_companions(conn, relation)?;
    default_primary_time(conn, relation)
}

/// Add a catalog row for every column of `dataset` its relation does not
/// have yet: the metadata columns first, then the dataset's own. An earlier
/// source's row for a column stands.
fn record_columns(conn: &Connection, dataset: &Dataset) -> Result<()> {
    let relation = dataset.view.as_str();
    let mut known: BTreeSet<String> = read_columns(conn, relation)?
        .into_iter()
        .map(|c| c.name)
        .collect();
    let mut ordinal: i32 = conn.query_row(
        "SELECT coalesce(max(ordinal), 0) FROM ox.column_catalog WHERE relation = ?",
        params![relation],
        |r| r.get(0),
    )?;
    let meta = &dataset.metadata_columns;
    let named = [&meta.run_id, &meta.host, &meta.identity, &meta.output_file]
        .into_iter()
        .chain(&dataset.columns);
    for column in named {
        if !known.insert(column.clone()) {
            continue;
        }
        ordinal += 1;
        let (text_column, typed_column, sql_type, semantics, origin) =
            match dataset.effective_types.get(column) {
                Some(t) => {
                    // Checked by the loader before anything was written.
                    let sql_type = SqlType::from_sql(&t.sql_type).ok_or_else(|| {
                        EngineError::Invalid(format!(
                            "{relation}: column {column:?} declares SQL type {:?}",
                            t.sql_type
                        ))
                    })?;
                    let semantics = t
                        .time_semantics
                        .as_deref()
                        .map(|s| {
                            parse_semantics(s).ok_or_else(|| {
                                EngineError::Invalid(format!(
                                    "{relation}: column {column:?} declares time semantics {s:?}"
                                ))
                            })
                        })
                        .transpose()?;
                    (
                        t.text_column.clone(),
                        Some(column.clone()),
                        Some(sql_type.sql()),
                        semantics.map(|s| s.as_str()),
                        TypeOrigin::Suite.as_str(),
                    )
                }
                None => (column.clone(), None, None, None, UNTYPED),
            };
        conn.execute(
            "INSERT INTO ox.column_catalog \
             (relation, name, ordinal, text_column, typed_column, sql_type, time_semantics, origin) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                relation,
                column,
                ordinal,
                text_column,
                typed_column,
                sql_type,
                semantics,
                origin
            ],
        )?;
    }
    Ok(())
}

/// Catalogue the `APP_TYPES` entries of `relation` whose column nobody has
/// typed yet. A column the suite or an analyst typed keeps that typing.
fn apply_app_types(conn: &Connection, relation: &str) -> Result<()> {
    for app in APP_TYPES.iter().filter(|t| t.relation == relation) {
        let untyped = read_columns(conn, relation)?
            .into_iter()
            .any(|c| c.name == app.column && c.origin.is_none());
        if !untyped {
            continue;
        }
        set_typing(
            conn,
            relation,
            app.column,
            None,
            (app.sql_type, app.rule.time_semantics()),
            TypeOrigin::AppMap,
        )?;
    }
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

/// Set `relation`'s primary time when it has none and no analyst ever chose
/// one for this relation row (choosing none is a choice too): the
/// `PRIMARY_TIME` entry if that is a catalogued timestamp, else the first
/// catalogued timestamp. The choice is read from the row itself, not from
/// history, so a relation recreated after its last source was removed
/// starts from the default again.
fn default_primary_time(conn: &Connection, relation: &str) -> Result<()> {
    if read_relation(conn, relation)?.primary_time.is_some() {
        return Ok(());
    }
    let chosen: bool = conn.query_row(
        "SELECT primary_time_chosen FROM ox.relation WHERE name = ?",
        params![relation],
        |r| r.get(0),
    )?;
    if chosen {
        return Ok(());
    }
    let timestamps: Vec<String> = read_columns(conn, relation)?
        .into_iter()
        .filter(|c| c.sql_type == Some(SqlType::Timestamp))
        .map(|c| c.name)
        .collect();
    let pick = primary_time_for(relation)
        .and_then(|p| timestamps.iter().find(|t| t.as_str() == p))
        .or_else(|| timestamps.first());
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
fn physical_columns(conn: &Connection, schema: &str, relation: &str) -> Result<BTreeSet<String>> {
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
    type Row = (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Row = conn
        .query_row(
            "SELECT kind, tool, primary_time, evidence_path_column, host_column \
             FROM ox.relation WHERE name = ?",
            params![name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .map_err(|e| match e {
            duckdb::Error::QueryReturnedNoRows => {
                EngineError::NotFound(format!("no relation {name}"))
            }
            other => other.into(),
        })?;
    let (kind, tool, primary_time, evidence_path_column, host_column) = row;
    let active = "FROM ox.source_dataset sd JOIN ox.source s ON s.id = sd.source_id \
                  WHERE sd.relation = ? AND sd.active AND NOT s.removed";
    let rows: i64 = conn.query_row(
        &format!("SELECT CAST(coalesce(sum(sd.rows), 0) AS BIGINT) {active}"),
        params![name],
        |r| r.get(0),
    )?;
    let hosts: Vec<String> = conn
        .prepare(&format!(
            "SELECT DISTINCT sd.host {active} AND sd.host IS NOT NULL ORDER BY sd.host"
        ))?
        .query_map(params![name], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(RelationInfo {
        name: name.to_string(),
        kind: RelationKind::parse(&kind)?,
        tool,
        rows: u64::try_from(rows).unwrap_or_default(),
        hosts,
        primary_time,
        evidence_path_column,
        host_column,
    })
}

type CatalogRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

const CATALOG_SELECT: &str = "SELECT name, text_column, typed_column, sql_type, time_semantics, \
                              origin FROM ox.column_catalog WHERE relation = ?";

fn read_columns(conn: &Connection, relation: &str) -> Result<Vec<ColumnInfo>> {
    let rows: Vec<CatalogRow> = conn
        .prepare(&format!("{CATALOG_SELECT} ORDER BY ordinal"))?
        .query_map(params![relation], catalog_row)?
        .collect::<std::result::Result<_, _>>()?;
    rows.into_iter().map(decode_column).collect()
}

/// One catalogued column; `NotFound` when `relation` has no such column.
fn read_column(conn: &Connection, relation: &str, column: &str) -> Result<ColumnInfo> {
    let row = conn
        .query_row(
            &format!("{CATALOG_SELECT} AND name = ?"),
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
    ))
}

fn decode_column(row: CatalogRow) -> Result<ColumnInfo> {
    let (name, text_column, typed_column, sql_type, semantics, origin) = row;
    let sql_type = sql_type
        .map(|t| SqlType::from_sql(&t).ok_or_else(|| corrupt(format!("SQL type {t:?}"))))
        .transpose()?;
    let time_semantics = semantics
        .map(|s| parse_semantics(&s).ok_or_else(|| corrupt(format!("time semantics {s:?}"))))
        .transpose()?;
    Ok(ColumnInfo {
        name,
        text_column,
        typed_column,
        sql_type,
        time_semantics,
        origin: TypeOrigin::parse(&origin)?,
    })
}

fn parse_semantics(text: &str) -> Option<TimeSemantics> {
    [
        TimeSemantics::Utc,
        TimeSemantics::Local,
        TimeSemantics::Unknown,
    ]
    .into_iter()
    .find(|s| s.as_str() == text)
}

fn corrupt(what: String) -> EngineError {
    EngineError::Invalid(format!("the case catalog holds an unknown {what}"))
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
