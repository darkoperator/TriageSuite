//! The time pivot and the merged row list.
//!
//! `around` counts what happened on one host in a window around a moment,
//! in every relation that has a host and a time. `merged_rows` lists the
//! rows of such a window, or of a find, across every relation at once, in
//! time order, each with a one-line summary: the rows a pivot opens.
//!
//! The merged list is one query: each relation contributes a self-contained
//! part with the same columns (row id, relation, time, host, summary, mark),
//! the parts are joined with `UNION ALL`, and the union is ordered and paged
//! in SQL, so a page never holds more than it shows.

use super::find::{cells_sql, targets, FindRequest};
use super::group::{as_count, typed_primary_time};
use super::page::{mark_sql, read_mark, MAX_LIMIT};
use super::{scope, GridQuery, MarkState, Scope, ISO_FORMAT, ROW_ALIAS};
use crate::annot::locate;
use crate::case::Case;
use crate::catalog::appmap::summary_for;
use crate::catalog::{self, ColumnInfo, ColumnRole, SqlType};
use crate::error::{EngineError, Result};
use crate::time::parse_utc_text;
use duckdb::types::Value as SqlValue;
use duckdb::{params, params_from_iter};
use triage_core::output::duckdb::sql::quote_ident;

#[derive(Debug, Clone, serde::Serialize)]
pub struct AroundCount {
    pub relation: String,
    pub count: u64,
}

/// Rows on `host` whose primary time is within `at ± window_seconds`, per
/// relation, for every relation with a host column and a primary time, in
/// relation order; zero counts included. `Invalid` when `at` is not UTC
/// text or the window does not fit the timestamp range.
pub fn around(case: &Case, host: &str, at: &str, window_seconds: u64) -> Result<Vec<AroundCount>> {
    let mut out = Vec::new();
    for scope in around_scopes(case, host, at, window_seconds)? {
        let n: i64 = case.conn().query_row(
            &format!(
                "SELECT count(*) FROM {} WHERE {}",
                scope.from_sql, scope.where_sql
            ),
            params_from_iter(&scope.params),
            |r| r.get(0),
        )?;
        out.push(AroundCount {
            relation: scope.relation.name,
            count: as_count(n),
        });
    }
    Ok(out)
}

/// The host and UTC primary time of row `row_id`, the anchor of a pivot
/// from it. `NotFound` when no relation holds the row; `Invalid` when the
/// row has no host or no usable time.
pub fn anchor_of(case: &Case, row_id: i64) -> Result<(String, String)> {
    let conn = case.conn();
    let not_found = || EngineError::NotFound(format!("no row {row_id} in the case"));
    let Some((_, name, _)) = locate(conn, row_id)? else {
        return Err(not_found());
    };
    // Through the view: a row of a superseded run or a removed source has
    // nothing around it to pivot to. A removed source's relation may be gone.
    let scope = match scope(
        case,
        &GridQuery {
            relation: name,
            ..Default::default()
        },
    ) {
        Ok(scope) => scope,
        Err(EngineError::NotFound(_)) => return Err(not_found()),
        Err(e) => return Err(e),
    };
    let relation = &scope.relation;
    let host = match &relation.host_column {
        Some(h) => format!("CAST({ROW_ALIAS}.{} AS VARCHAR)", quote_ident(h)),
        None => "CAST(NULL AS VARCHAR)".into(),
    };
    let time = match typed_primary_time(&relation.primary_time, &scope.columns) {
        Some(t) => format!("strftime({ROW_ALIAS}.{}, '{ISO_FORMAT}')", quote_ident(t)),
        None => "CAST(NULL AS VARCHAR)".into(),
    };
    let found: Option<(Option<String>, Option<String>)> = conn
        .prepare(&format!(
            "SELECT {host}, {time} FROM {} WHERE {ROW_ALIAS}._ox_row = ?",
            scope.from_sql
        ))?
        .query_map(params![row_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .next()
        .transpose()?;
    let (host, time) = found.ok_or_else(not_found)?;
    let host = host.filter(|h| !h.is_empty()).ok_or_else(|| {
        EngineError::Invalid(format!(
            "row {row_id} of {} has no host to pivot on",
            relation.name
        ))
    })?;
    let time = time.ok_or_else(|| {
        EngineError::Invalid(format!(
            "row {row_id} of {} has no usable {} time to pivot on",
            relation.name,
            relation.primary_time.as_deref().unwrap_or("primary")
        ))
    })?;
    Ok((host, time))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MergedRow {
    pub row_id: i64,
    pub relation: String,
    /// The row's primary time, UTC; `None` when it has none.
    pub time: Option<String>,
    pub host: Option<String>,
    pub summary: String,
    pub mark: Option<MarkState>,
}

#[derive(Debug, Clone)]
pub enum MergedSource {
    /// The rows a find counts.
    Find(FindRequest),
    /// The rows `around` counts.
    Around {
        host: String,
        at: String,
        window_seconds: u64,
    },
}

/// Characters of a merged row's summary.
const SUMMARY_CAP: usize = 300;
/// Values a summary takes when the relation has no `SUMMARY` entry.
const FALLBACK_VALUES: usize = 4;

/// Up to `limit` rows of `source` from `offset`, across every relation it
/// covers: time ascending, untimed last, then relation name, then row id.
/// The flag says more rows follow. `Invalid` when `limit` is over
/// `MAX_LIMIT`, or for what `find` or `around` would refuse.
pub fn merged_rows(
    case: &Case,
    source: &MergedSource,
    limit: u32,
    offset: u64,
) -> Result<(Vec<MergedRow>, bool)> {
    if limit > MAX_LIMIT {
        return Err(EngineError::Invalid(format!(
            "a page is at most {MAX_LIMIT} rows, not {limit}"
        )));
    }
    let mut parts = Vec::new();
    let mut params = Vec::new();
    match source {
        MergedSource::Find(req) => {
            let relations = catalog::relations(case)?;
            for target in targets(case, req, &relations)? {
                let mut cell_params = Vec::new();
                let Some(cells) = cells_sql(req, &target, &mut cell_params)? else {
                    continue;
                };
                let matched =
                    format!("{ROW_ALIAS}._ox_row IN (SELECT ox_c._ox_row FROM {cells} AS ox_c)");
                parts.push(part_sql(&target.scope, &matched, &mut params));
                params.extend(cell_params);
            }
        }
        MergedSource::Around {
            host,
            at,
            window_seconds,
        } => {
            for scope in around_scopes(case, host, at, *window_seconds)? {
                parts.push(part_sql(&scope, &scope.where_sql, &mut params));
                params.extend(scope.params);
            }
        }
    }
    if parts.is_empty() {
        return Ok((Vec::new(), false));
    }

    // Fetching one more than asked says whether the page is the last one.
    let sql = format!(
        "SELECT _ox_row, _ox_rel, strftime(_ox_t, '{ISO_FORMAT}'), _ox_h, _ox_s, _ox_m \
         FROM ({}) AS ox_merged \
         ORDER BY _ox_t ASC NULLS LAST, _ox_rel ASC, _ox_row ASC LIMIT ? OFFSET ?",
        parts.join(" UNION ALL ")
    );
    params.push(SqlValue::BigInt(i64::from(limit) + 1));
    params.push(SqlValue::UBigInt(offset));

    let mut stmt = case.conn().prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;
    let mut out = Vec::new();
    let mut truncated = false;
    while let Some(row) = rows.next()? {
        if out.len() == limit as usize {
            truncated = true;
            break;
        }
        out.push(MergedRow {
            row_id: row.get(0)?,
            relation: row.get(1)?,
            time: row.get(2)?,
            host: row.get(3)?,
            summary: row.get(4)?,
            mark: read_mark(row.get(5)?)?,
        });
    }
    Ok((out, truncated))
}

/// One relation's part of the merged union: its rows that satisfy
/// `where_sql`, as row id, relation, time, host, summary and mark. Pushes
/// the relation name, which the part binds before `where_sql`'s own
/// parameters; the caller pushes those next.
fn part_sql(scope: &Scope, where_sql: &str, params: &mut Vec<SqlValue>) -> String {
    let relation = &scope.relation;
    let time = match typed_primary_time(&relation.primary_time, &scope.columns) {
        Some(t) => format!("CAST({ROW_ALIAS}.{} AS TIMESTAMP)", quote_ident(t)),
        None => "CAST(NULL AS TIMESTAMP)".into(),
    };
    let host = match &relation.host_column {
        Some(h) => format!("CAST({ROW_ALIAS}.{} AS VARCHAR)", quote_ident(h)),
        None => "CAST(NULL AS VARCHAR)".into(),
    };
    params.push(SqlValue::Text(relation.name.clone()));
    format!(
        "SELECT {ROW_ALIAS}._ox_row AS _ox_row, CAST(? AS VARCHAR) AS _ox_rel, \
         {time} AS _ox_t, {host} AS _ox_h, {summary} AS _ox_s, {mark} AS _ox_m \
         FROM {from} WHERE {where_sql}",
        summary = summary_sql(&relation.name, &scope.columns),
        mark = mark_sql(),
        from = scope.from_sql,
    )
}

/// SQL for a row's summary: the `SUMMARY` columns' non-empty texts, or, for
/// a relation without an entry (or none of whose listed columns it has),
/// the first four non-empty values among its columns that are neither
/// metadata nor timestamps, in catalog order; joined by ` · ` and cut to
/// `SUMMARY_CAP` characters.
fn summary_sql(relation: &str, all: &[ColumnInfo]) -> String {
    let listed: Vec<&ColumnInfo> = summary_for(relation)
        .unwrap_or_default()
        .iter()
        .filter_map(|name| all.iter().find(|c| c.name == *name))
        .collect();
    let (columns, take) = if listed.is_empty() {
        let plain: Vec<&ColumnInfo> = all
            .iter()
            .filter(|c| c.role == ColumnRole::Data && c.sql_type != Some(SqlType::Timestamp))
            .collect();
        (plain, Some(FALLBACK_VALUES))
    } else {
        (listed, None)
    };
    if columns.is_empty() {
        return "CAST('' AS VARCHAR)".into();
    }
    let texts: Vec<String> = columns
        .iter()
        .map(|c| {
            format!(
                "CAST({ROW_ALIAS}.{} AS VARCHAR)",
                quote_ident(&c.text_column)
            )
        })
        .collect();
    let mut kept = format!(
        "list_filter([{}], lambda ox_x: ox_x <> '')",
        texts.join(", ")
    );
    if let Some(n) = take {
        kept = format!("list_slice({kept}, 1, {n})");
    }
    format!("left(array_to_string({kept}, ' · '), {SUMMARY_CAP})")
}

/// The scope of every relation with a host column and a typed primary time,
/// in relation order, narrowed to `host` and to the rows whose time is in
/// `at ± window_seconds`. The window is computed in SQL; `Invalid` when `at`
/// is not UTC text or the window does not fit the timestamp range.
fn around_scopes(case: &Case, host: &str, at: &str, window_seconds: u64) -> Result<Vec<Scope>> {
    let at = parse_utc_text(at).ok_or_else(|| {
        EngineError::Invalid(format!("{at:?} is not UTC text (YYYY-MM-DDTHH:MM:SS[.f]Z)"))
    })?;
    let out_of_range = || {
        EngineError::Invalid(format!(
            "a window of {window_seconds} seconds around {at} is outside the timestamp range"
        ))
    };
    let seconds = i64::try_from(window_seconds).map_err(|_| out_of_range())?;
    let window_params = || {
        [
            SqlValue::Text(at.clone()),
            SqlValue::BigInt(seconds),
            SqlValue::Text(at.clone()),
            SqlValue::BigInt(seconds),
        ]
    };
    // The same bounds every count uses, computed once on their own so that
    // a window past the timestamp range is `Invalid` rather than an engine
    // error from the first count.
    case.conn()
        .query_row(
            &format!("SELECT ({LOW}) <= ({HIGH})"),
            params_from_iter(window_params()),
            |r| r.get::<_, Option<bool>>(0),
        )
        .map_err(|_| out_of_range())?;

    let mut out = Vec::new();
    for relation in catalog::relations(case)? {
        if relation.host_column.is_none() {
            continue;
        }
        let mut scope = scope(
            case,
            &GridQuery {
                relation: relation.name.clone(),
                hosts: vec![host.to_string()],
                ..Default::default()
            },
        )?;
        let Some(t) = typed_primary_time(&scope.relation.primary_time, &scope.columns) else {
            continue;
        };
        scope.where_sql = format!(
            "({}) AND {ROW_ALIAS}.{} BETWEEN {LOW} AND {HIGH}",
            scope.where_sql,
            quote_ident(t)
        );
        scope.params.extend(window_params());
        out.push(scope);
    }
    Ok(out)
}

/// The window's bounds: the anchor and the window's seconds bound in turn.
const LOW: &str = "CAST(? AS TIMESTAMP) - INTERVAL (?) SECOND";
const HIGH: &str = "CAST(? AS TIMESTAMP) + INTERVAL (?) SECOND";
