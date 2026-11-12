//! The time pivot and the merged row list.
//!
//! `around` counts what happened on one host in a window around a moment,
//! in every relation that has a host and a time. `merged_rows` lists the
//! rows of such a window, or of a find, across every relation at once, in
//! time order, each with a one-line summary: the rows a pivot opens.
//!
//! The merged list is ordered and paged by one query: each relation
//! contributes a self-contained part with the same columns (row id,
//! relation, time), the parts are joined with `UNION ALL`, and the union is
//! ordered and paged in SQL, so a page never holds more than it shows. What
//! a row shows besides (host, summary, mark) is then read for the rows of
//! the page alone, from each relation the page has rows of: working it out
//! for every row of the pivot before paging cost several times the paging.

use super::fan::in_order;
use super::find::{cells_sql, targets, FindRequest};
use super::group::{as_count, primary_time_semantics, typed_primary_time};
use super::page::{mark_sql, read_mark};
use super::{scope, scope_of, subjects, GridQuery, MarkState, Scope, ISO_FORMAT, ROW_ALIAS};
use crate::annot::locate;
use crate::case::Case;
use crate::catalog::appmap::summary_for;
use crate::catalog::{ColumnInfo, ColumnRole, SqlType, TimeSemantics};
use crate::error::{EngineError, Result};
use crate::listing::{check_limit, fetch_limit, Listing};
use crate::time::parse_utc_text;
use duckdb::types::Value as SqlValue;
use duckdb::{params, params_from_iter, Connection};
use std::collections::BTreeMap;
use triage_core::output::duckdb::sql::quote_ident;

#[derive(Debug, Clone, serde::Serialize)]
pub struct AroundCount {
    pub relation: String,
    pub count: u64,
    /// How the relation's primary time is zoned. An `unknown` time was
    /// compared with the window as written.
    #[serde(serialize_with = "crate::catalog::serialize_semantics")]
    pub time_semantics: Option<TimeSemantics>,
}

/// Rows on `host` whose primary time is within `at ± window_seconds`, per
/// relation, for every relation with a host column and a primary time, in
/// relation order; zero counts included. `Invalid` when `at` is not UTC
/// text or the window does not fit the timestamp range.
pub fn around(case: &Case, host: &str, at: &str, window_seconds: u64) -> Result<Vec<AroundCount>> {
    let scopes = around_scopes(case, host, at, window_seconds)?;
    let mut out = Vec::new();
    in_order(
        case.conn(),
        &scopes,
        |conn, scope| -> Result<i64> {
            Ok(conn.query_row(
                &format!(
                    "SELECT count(*) FROM {} WHERE {}",
                    scope.from_sql, scope.where_sql
                ),
                params_from_iter(&scope.params),
                |r| r.get(0),
            )?)
        },
        |scope, n| {
            out.push(AroundCount {
                time_semantics: primary_time_semantics(
                    &scope.relation.primary_time,
                    &scope.columns,
                ),
                relation: scope.relation.name.clone(),
                count: as_count(n),
            });
            true
        },
    )?;
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
    /// The row's primary time, rendered with a `Z`; `None` when it has none.
    /// Only `time_semantics` says whether it really is UTC.
    pub time: Option<String>,
    /// How the relation's primary time is zoned; `None` without one. Rows
    /// with an `unknown` time sort by their written value.
    #[serde(serialize_with = "crate::catalog::serialize_semantics")]
    pub time_semantics: Option<TimeSemantics>,
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
/// `Invalid` when `limit` is 0 or over `MAX_LIMIT`, or for what `find` or
/// `around` would refuse.
pub fn merged_rows(
    case: &Case,
    source: &MergedSource,
    limit: u32,
    offset: u64,
) -> Result<Listing<MergedRow>> {
    check_limit(limit)?;
    let mut parts = Vec::new();
    let mut params = Vec::new();
    // The scope of each part, by its relation's name.
    let mut scopes: BTreeMap<String, Scope> = BTreeMap::new();
    match source {
        MergedSource::Find(req) => {
            let subjects = subjects(case)?;
            for target in targets(case, req, &subjects)? {
                let mut cell_params = Vec::new();
                let Some(cells) = cells_sql(req, &target, &mut cell_params)? else {
                    continue;
                };
                let matched =
                    format!("{ROW_ALIAS}._ox_row IN (SELECT ox_c._ox_row FROM {cells} AS ox_c)");
                parts.push(part_sql(&target.scope, &matched, &mut params));
                params.extend(cell_params);
                scopes.insert(target.scope.relation.name.clone(), target.scope);
            }
        }
        MergedSource::Around {
            host,
            at,
            window_seconds,
        } => {
            for scope in around_scopes(case, host, at, *window_seconds)? {
                parts.push(part_sql(&scope, &scope.where_sql, &mut params));
                params.extend(scope.params.iter().cloned());
                scopes.insert(scope.relation.name.clone(), scope);
            }
        }
    }
    if parts.is_empty() {
        return Ok(Listing {
            items: Vec::new(),
            truncated: false,
        });
    }

    // Which rows the page holds, in order.
    let sql = format!(
        "SELECT _ox_row, _ox_rel, strftime(_ox_t, '{ISO_FORMAT}') \
         FROM ({}) AS ox_merged \
         ORDER BY _ox_t ASC NULLS LAST, _ox_rel ASC, _ox_row ASC LIMIT ? OFFSET ?",
        parts.join(" UNION ALL ")
    );
    params.push(SqlValue::BigInt(fetch_limit(limit)));
    params.push(SqlValue::UBigInt(offset));
    let keys: Vec<(i64, String, Option<String>)> = case
        .conn()
        .prepare(&sql)?
        .query_map(params_from_iter(params), |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<std::result::Result<_, _>>()?;
    let page = Listing::from_fetched(keys, limit);

    // What those rows show, read from each relation the page has rows of.
    let mut on_page: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
    for (row_id, relation, _) in &page.items {
        on_page.entry(relation).or_default().push(*row_id);
    }
    let wanted: Vec<(&Scope, Vec<i64>)> = on_page
        .into_iter()
        .filter_map(|(relation, ids)| Some((scopes.get(relation)?, ids)))
        .collect();
    let mut shown: BTreeMap<i64, Shown> = BTreeMap::new();
    in_order(
        case.conn(),
        &wanted,
        |conn, (scope, ids)| shown_rows(conn, scope, ids),
        |_, rows| {
            shown.extend(rows);
            true
        },
    )?;

    let items = page
        .items
        .into_iter()
        .map(|(row_id, relation, time)| {
            let scope = scopes.get(&relation);
            let (host, summary, mark) = shown.remove(&row_id).ok_or_else(|| {
                EngineError::Invalid(format!(
                    "row {row_id} of {relation} was listed but could not be read back"
                ))
            })?;
            Ok(MergedRow {
                row_id,
                time_semantics: scope
                    .and_then(|s| primary_time_semantics(&s.relation.primary_time, &s.columns)),
                relation,
                time,
                host,
                summary,
                mark,
            })
        })
        .collect::<Result<_>>()?;
    Ok(Listing {
        items,
        truncated: page.truncated,
    })
}

/// One relation's part of the merged union: its rows that satisfy
/// `where_sql`, as row id, relation and time, which is all that ordering
/// and paging them takes. Pushes the relation name, which the part binds
/// before `where_sql`'s own parameters; the caller pushes those next.
fn part_sql(scope: &Scope, where_sql: &str, params: &mut Vec<SqlValue>) -> String {
    let relation = &scope.relation;
    let time = match typed_primary_time(&relation.primary_time, &scope.columns) {
        Some(t) => format!("CAST({ROW_ALIAS}.{} AS TIMESTAMP)", quote_ident(t)),
        None => "CAST(NULL AS TIMESTAMP)".into(),
    };
    params.push(SqlValue::Text(relation.name.clone()));
    format!(
        "SELECT {ROW_ALIAS}._ox_row AS _ox_row, CAST(? AS VARCHAR) AS _ox_rel, \
         {time} AS _ox_t FROM {from} WHERE {where_sql}",
        from = scope.from_sql,
    )
}

/// What a merged row shows besides its time: host, summary and mark.
type Shown = (Option<String>, String, Option<MarkState>);

/// The rows `ids` of `scope`'s relation as the merged list shows them.
fn shown_rows(conn: &Connection, scope: &Scope, ids: &[i64]) -> Result<Vec<(i64, Shown)>> {
    let relation = &scope.relation;
    let host = match &relation.host_column {
        Some(h) => format!("CAST({ROW_ALIAS}.{} AS VARCHAR)", quote_ident(h)),
        None => "CAST(NULL AS VARCHAR)".into(),
    };
    let sql = format!(
        "SELECT {ROW_ALIAS}._ox_row, {host}, {summary}, {mark} \
         FROM {from} WHERE {ROW_ALIAS}._ox_row IN ({slots})",
        summary = summary_sql(&relation.name, &scope.columns),
        mark = mark_sql(),
        from = scope.from_sql,
        slots = vec!["?"; ids.len()].join(", "),
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(ids))?;
    let mut out = Vec::with_capacity(ids.len());
    while let Some(row) = rows.next()? {
        out.push((
            row.get(0)?,
            (row.get(1)?, row.get(2)?, read_mark(row.get(3)?)?),
        ));
    }
    Ok(out)
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
    for subject in subjects(case)? {
        if subject.relation.host_column.is_none() {
            continue;
        }
        let mut scope = scope_of(
            case,
            &subject,
            &GridQuery {
                relation: subject.relation.name.clone(),
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
