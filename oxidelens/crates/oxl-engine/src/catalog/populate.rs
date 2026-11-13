//! Load-time population of the catalog: what a load writes into it for the
//! run dataset or loose file it just materialized, inside the load's
//! transaction, and the checks a run's typing must pass first. None of it
//! is an analyst's change, so none of it writes history.

use super::appmap::app_types_for;
use super::{
    default_primary_time, read_columns, set_typing, sync_companions, ColumnRole, SqlType,
    TimeSemantics, TypeOrigin, UNTYPED,
};
use crate::error::{EngineError, Result};
use duckdb::{params, Connection};
use std::collections::{BTreeMap, BTreeSet};
use triage_core::output::duckdb::inventory::{Dataset, EffectiveType};

/// `sql_type`, the SQL type an inventory declares for `relation`'s column
/// `column`, as the engine's type; `Invalid` for a spelling TriageSuite
/// never writes. The spelling is pasted into a `TRY_CAST`, so a tampered
/// inventory is untrusted SQL text: only the suite's own spellings get
/// through. The run loader checks every declaration with it before
/// anything is written, and the catalog reads each one through it again.
pub(crate) fn check_sql_type(relation: &str, column: &str, sql_type: &str) -> Result<SqlType> {
    SqlType::from_sql(sql_type).ok_or_else(|| {
        EngineError::Invalid(format!(
            "{relation}: column {column:?} declares SQL type {sql_type:?}, which TriageSuite \
             never writes"
        ))
    })
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
/// yet are not checked. Names are compared without regard to case, as
/// DuckDB resolves them: `eventid` in a run is the case's `EventId`.
pub(crate) fn check_typing(conn: &Connection, dataset: &Dataset) -> Result<()> {
    let relation = dataset.view.as_str();
    let meta = &dataset.metadata_columns;
    let named: BTreeSet<String> = [&meta.run_id, &meta.host, &meta.identity, &meta.output_file]
        .into_iter()
        .chain(&dataset.columns)
        .map(|name| name.to_lowercase())
        .collect();
    let typings: BTreeMap<String, &EffectiveType> = dataset
        .effective_types
        .iter()
        .map(|(name, t)| (name.to_lowercase(), t))
        .collect();
    for column in read_columns(conn, relation)? {
        let name = column.name.to_lowercase();
        if !named.contains(&name) {
            continue;
        }
        let theirs = typings.get(&name).copied();
        let agrees = match (column.origin, theirs) {
            (Some(TypeOrigin::Suite), Some(t)) => {
                SqlType::from_sql(&t.sql_type) == column.sql_type
                    && t.text_column.to_lowercase() == column.text_column.to_lowercase()
                    && t.time_semantics.as_deref().map(TimeSemantics::parse)
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
    /// `Host` for the column holding the host assigned at load time.
    pub role: ColumnRole,
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
             (relation, name, ordinal, text_column, typed_column, sql_type, time_semantics, \
              origin, role) \
             VALUES (?, ?, ?, ?, NULL, NULL, NULL, ?, ?)",
            params![
                relation,
                column.name,
                i32::try_from(i + 1).unwrap_or(i32::MAX),
                column.name,
                UNTYPED,
                column.role.as_str()
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
/// source's row for a column stands, whatever the case of its name: DuckDB
/// resolves `eventid` and `EventId` to one column, so the catalog keeps one
/// (`check_typing` has made sure they agree).
fn record_columns(conn: &Connection, dataset: &Dataset) -> Result<()> {
    let relation = dataset.view.as_str();
    let earlier: BTreeSet<String> = read_columns(conn, relation)?
        .into_iter()
        .map(|c| c.name.to_lowercase())
        .collect();
    let mut known: BTreeSet<String> = BTreeSet::new();
    let mut ordinal: i32 = conn.query_row(
        "SELECT coalesce(max(ordinal), 0) FROM ox.column_catalog WHERE relation = ?",
        params![relation],
        |r| r.get(0),
    )?;
    let meta = &dataset.metadata_columns;
    let named = [
        (&meta.run_id, ColumnRole::RunId),
        (&meta.host, ColumnRole::Host),
        (&meta.identity, ColumnRole::Identity),
        (&meta.output_file, ColumnRole::OutputFile),
    ]
    .into_iter()
    .chain(dataset.columns.iter().map(|c| (c, ColumnRole::Data)));
    for (column, role) in named {
        if earlier.contains(&column.to_lowercase()) || !known.insert(column.clone()) {
            continue;
        }
        ordinal += 1;
        let (text_column, typed_column, sql_type, semantics, origin) =
            match dataset.effective_types.get(column) {
                Some(t) => {
                    // Checked by the loader before anything was written.
                    let sql_type = check_sql_type(relation, column, &t.sql_type)?;
                    let semantics = t
                        .time_semantics
                        .as_deref()
                        .map(|s| {
                            TimeSemantics::parse(s).ok_or_else(|| {
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
             (relation, name, ordinal, text_column, typed_column, sql_type, time_semantics, \
              origin, role) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                relation,
                column,
                ordinal,
                text_column,
                typed_column,
                sql_type,
                semantics,
                origin,
                role.as_str()
            ],
        )?;
    }
    Ok(())
}

/// Catalogue the `APP_TYPES` entries of `relation` whose column nobody has
/// typed yet. A column the suite or an analyst typed keeps that typing.
fn apply_app_types(conn: &Connection, relation: &str) -> Result<()> {
    for app in app_types_for(relation) {
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
