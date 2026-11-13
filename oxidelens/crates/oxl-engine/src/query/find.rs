//! Find everywhere: one value looked up in every relation and host of the
//! case at once, with each hit attributed to the dataset, column and host
//! it was found in.
//!
//! Each relation is searched with one query in two stages. The first keeps
//! the rows with a cell that can match: for `Contains` the search box's own
//! unscoped term, for `Exact` a cell equal to the value. The second
//! unpivots only those rows into one row per cell and keeps the cells that
//! match, which is what attributes a hit to a column. An exact find asks
//! for equality from the start because a value common inside cells (`_`
//! is in most paths) would otherwise keep nearly every row for the second
//! stage.
//! A cell matches the way a filter leaf does: `eq` (`Exact`) or `contains`
//! (`Contains`), without regard to case, on the cell's text.
//!
//! The searched columns are the data columns: the search box's searchable
//! columns without the host column, since a hit there would only restate
//! the host the hit is already attributed to.

use super::fan::in_order;
use super::group::{as_count, first_last_sql, primary_time_semantics, typed_primary_time};
use super::{resolve_hosts, scope_of, subjects, GridQuery, Scope, Subject, ROW_ALIAS};
use crate::case::Case;
use crate::catalog::{ColumnInfo, ColumnRole, TimeSemantics};
use crate::error::{EngineError, Result};
use crate::filter::{compile_filter, literal_eq_sql, null_safe, FilterNode, Leaf, Op, SqlFragment};
use crate::search::{compile_search, Search, SearchExpr, SearchOptions, Term};
use crate::time::parse_utc_text;
use duckdb::types::Value as SqlValue;
use duckdb::{params_from_iter, Connection};
use std::collections::BTreeSet;
use triage_core::output::duckdb::sql::quote_ident;

/// Characters in one find value.
pub const MAX_VALUE: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum FindMode {
    /// The cell's whole text is the value, without regard to case.
    Exact,
    /// The cell's text contains the value literally, without regard to case.
    Contains,
}

/// A find across every relation. The app sends one inside a
/// `MergedSource`; unknown fields are refused.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct FindRequest {
    pub value: String,
    pub mode: FindMode,
    /// Empty means every host; otherwise relations without a host column
    /// are not searched.
    #[serde(default)]
    pub hosts: Vec<String>,
    /// UTC text bounding the rows' primary time, inclusive. Either bound
    /// leaves out the relations that have no primary time.
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FindHit {
    pub relation: String,
    pub column: String,
    /// `None` for a relation with no host column, or a row with no host.
    pub host: Option<String>,
    /// Rows whose cell in `column` matches.
    pub count: u64,
    /// The earliest and latest primary time among those rows, rendered with
    /// a `Z`; `time_semantics` says whether they really are UTC.
    pub first: Option<String>,
    pub last: Option<String>,
    /// How the relation's primary time is zoned; `None` without one. An
    /// `unknown` time was compared with `from`/`to` as written.
    pub time_semantics: Option<TimeSemantics>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FindResult {
    /// Ordered by relation, column, host.
    pub hits: Vec<FindHit>,
    /// The relations searched, in the order they were.
    pub searched: Vec<String>,
    /// The searched relations without a hit.
    pub no_hit: Vec<String>,
    /// Hosts of the searched relations (narrowed to the request's hosts)
    /// that no hit came from, sorted. A host only a relation left out of
    /// the search has (by time span or host, or not reached before a
    /// cancel) is not listed: it was never looked at.
    pub hosts_not_seen: Vec<String>,
    /// The progress callback stopped the search before every relation was.
    pub cancelled: bool,
}

/// Find `req.value` in every relation in scope. Several relations are
/// searched at a time; `progress` is called once per relation, in relation
/// order, as soon as it and every relation before it is done, and returning
/// false cancels the rest. `Invalid` for an empty value, one over
/// `MAX_VALUE` characters, or a `from`/`to` that is not UTC text.
pub fn find(
    case: &Case,
    req: &FindRequest,
    progress: &mut dyn FnMut(&str, &[FindHit]) -> bool,
) -> Result<FindResult> {
    let req = &resolved(case, req)?;
    let subjects = subjects(case)?;
    let targets = targets(case, req, &subjects)?;
    let mut out = FindResult {
        hits: Vec::new(),
        searched: Vec::new(),
        no_hit: Vec::new(),
        hosts_not_seen: Vec::new(),
        cancelled: false,
    };
    let finished = in_order(
        case.conn(),
        &targets,
        |conn, target| relation_hits(conn, req, target),
        |target, hits| {
            let name = &target.scope.relation.name;
            if hits.is_empty() {
                out.no_hit.push(name.clone());
            }
            out.searched.push(name.clone());
            let more = progress(name, &hits);
            out.hits.extend(hits);
            more
        },
    )?;
    out.cancelled = !finished;
    // `subjects` is sorted by name and each relation's hits by column and
    // host, so the hits are already in their documented order.
    out.hosts_not_seen = hosts_not_seen(&targets, &out.searched, &req.hosts, &out.hits);
    Ok(out)
}

/// The hosts the search looked at and found nothing on: those of the
/// relations it searched (`searched`, so a relation it left out, by time
/// span or host, or never reached after a cancel, adds none), narrowed to
/// `wanted` when that is not empty, less the hosts a hit came from. Sorted.
fn hosts_not_seen(
    targets: &[Target],
    searched: &[String],
    wanted: &[String],
    hits: &[FindHit],
) -> Vec<String> {
    let seen: BTreeSet<&str> = hits.iter().filter_map(|h| h.host.as_deref()).collect();
    let looked_at: BTreeSet<&str> = targets
        .iter()
        .filter(|t| searched.contains(&t.scope.relation.name))
        .flat_map(|t| t.scope.relation.hosts.iter().map(String::as_str))
        .collect();
    looked_at
        .into_iter()
        .filter(|h| wanted.is_empty() || wanted.iter().any(|w| w == h))
        .filter(|h| !seen.contains(h))
        .map(str::to_string)
        .collect()
}

/// `req` with its hosts as the case spells them (`resolve_hosts`).
pub(super) fn resolved(case: &Case, req: &FindRequest) -> Result<FindRequest> {
    Ok(FindRequest {
        hosts: resolve_hosts(case, &req.hosts)?,
        ..req.clone()
    })
}

/// One relation a find searches: its scope, narrowed to the request's hosts
/// and time span, and the columns searched in it.
pub(super) struct Target {
    pub scope: Scope,
    pub searched: Vec<ColumnInfo>,
}

/// The relations `req` searches, from `subjects`, in order. Checks the
/// request first, so an invalid one fails before any search runs.
pub(super) fn targets(case: &Case, req: &FindRequest, subjects: &[Subject]) -> Result<Vec<Target>> {
    if req.value.is_empty() {
        return Err(EngineError::Invalid("find needs a value".into()));
    }
    if req.value.chars().count() > MAX_VALUE {
        return Err(EngineError::Invalid(format!(
            "a find value is at most {MAX_VALUE} characters"
        )));
    }
    let from = req.from.as_deref().map(utc_bound).transpose()?;
    let to = req.to.as_deref().map(utc_bound).transpose()?;
    let timed = from.is_some() || to.is_some();

    let mut out = Vec::new();
    for subject in subjects {
        if !req.hosts.is_empty() && subject.relation.host_column.is_none() {
            continue;
        }
        let mut scope = scope_of(
            case,
            subject,
            &GridQuery {
                relation: subject.relation.name.clone(),
                hosts: req.hosts.clone(),
                ..Default::default()
            },
        )?;
        if timed {
            let Some(t) = typed_primary_time(&scope.relation.primary_time, &scope.columns) else {
                continue;
            };
            let t = format!("{ROW_ALIAS}.{}", quote_ident(t));
            for (bound, cmp) in [(&from, ">="), (&to, "<=")] {
                if let Some(bound) = bound {
                    scope.where_sql =
                        format!("({}) AND {t} {cmp} CAST(? AS TIMESTAMP)", scope.where_sql);
                    scope.params.push(SqlValue::Text(bound.clone()));
                }
            }
        }
        let searched = scope
            .columns
            .iter()
            .filter(|c| c.role == ColumnRole::Data)
            .cloned()
            .collect();
        out.push(Target { scope, searched });
    }
    Ok(out)
}

fn utc_bound(text: &str) -> Result<String> {
    parse_utc_text(text).ok_or_else(|| {
        EngineError::Invalid(format!(
            "{text:?} is not UTC text (YYYY-MM-DDTHH:MM:SS[.f]Z)"
        ))
    })
}

/// The name each unpivoted cell's text goes under; `_ox_` names are the
/// engine's, so no catalogued column has it.
const CELL: &str = "_ox_v";

/// A subquery holding one row per matching cell of `target`: `_ox_row`,
/// `_ox_h` (the host), the relation's typed primary time under its own
/// name when it has one, `_ox_i` (the cell's index in `target.searched`)
/// and the cell's text. `None` when the relation has no searched column.
/// Its parameters are pushed onto `params` in order.
pub(super) fn cells_sql(
    req: &FindRequest,
    target: &Target,
    params: &mut Vec<SqlValue>,
) -> Result<Option<String>> {
    if target.searched.is_empty() {
        return Ok(None);
    }
    let scope = &target.scope;
    let host = match &scope.relation.host_column {
        Some(h) => format!("CAST({ROW_ALIAS}.{} AS VARCHAR)", quote_ident(h)),
        None => "CAST(NULL AS VARCHAR)".into(),
    };
    let time = typed_primary_time(&scope.relation.primary_time, &scope.columns)
        .map(|t| format!(", {ROW_ALIAS}.{}", quote_ident(t)))
        .unwrap_or_default();
    let cells: Vec<String> = target
        .searched
        .iter()
        .map(|c| {
            format!(
                "CAST({ROW_ALIAS}.{} AS VARCHAR)",
                quote_ident(&c.text_column)
            )
        })
        .collect();

    // Stage one: the rows with a cell that can match.
    let rows = match req.mode {
        FindMode::Contains => compile_search(
            &Search::Terms(SearchExpr {
                any: vec![Term {
                    text: req.value.clone(),
                    column: None,
                }],
                ..Default::default()
            }),
            SearchOptions::default(),
            &target.searched,
        )?,
        FindMode::Exact => {
            let mut params = Vec::new();
            let tests: Vec<String> = cells
                .iter()
                .map(|cell| null_safe(&literal_eq_sql(cell, &req.value, false, &mut params)))
                .collect();
            SqlFragment {
                sql: format!("({})", tests.join(" OR ")),
                params,
            }
        }
    };
    // Stage two: which of their cells match.
    let cell = ColumnInfo {
        name: CELL.into(),
        role: ColumnRole::Data,
        text_column: CELL.into(),
        typed_column: None,
        sql_type: None,
        time_semantics: None,
        origin: None,
    };
    let test = compile_filter(
        &FilterNode::Leaf(Leaf {
            column: CELL.into(),
            op: match req.mode {
                FindMode::Exact => Op::Eq,
                FindMode::Contains => Op::Contains,
            },
            value: serde_json::Value::String(req.value.clone()),
            case_sensitive: false,
        }),
        &[cell],
    )?;

    params.extend(scope.params.iter().cloned());
    params.extend(rows.params);
    params.extend(test.params);
    Ok(Some(format!(
        "(SELECT * FROM (\
           SELECT {ROW_ALIAS}._ox_row, {host} AS _ox_h{time}, \
           unnest(range({n})) AS _ox_i, unnest([{cells}]) AS {cell_ident} \
           FROM {from} WHERE ({where_sql}) AND {rows_sql}\
         ) AS ox_u WHERE {test_sql})",
        n = cells.len(),
        cells = cells.join(", "),
        cell_ident = quote_ident(CELL),
        from = scope.from_sql,
        where_sql = scope.where_sql,
        rows_sql = rows.sql,
        test_sql = test.sql,
    )))
}

/// The hits of `req` in one relation, ordered by column and host.
fn relation_hits(conn: &Connection, req: &FindRequest, target: &Target) -> Result<Vec<FindHit>> {
    let mut params = Vec::new();
    let Some(cells) = cells_sql(req, target, &mut params)? else {
        return Ok(Vec::new());
    };
    let scope = &target.scope;
    // The cells keep the typed primary time under its own name, so the
    // group span reads it the way every other query does.
    let (first, last) = first_last_sql(&scope.relation.primary_time, &scope.columns);
    let sql = format!(
        "SELECT {ROW_ALIAS}._ox_i, {ROW_ALIAS}._ox_h, count(*), {first}, {last} \
         FROM {cells} AS {ROW_ALIAS} GROUP BY {ROW_ALIAS}._ox_i, {ROW_ALIAS}._ox_h"
    );
    let time_semantics = primary_time_semantics(&scope.relation.primary_time, &scope.columns);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let index: i64 = row.get(0)?;
        let column = usize::try_from(index)
            .ok()
            .and_then(|i| target.searched.get(i))
            .ok_or_else(|| {
                EngineError::Internal(format!("find produced an unknown cell index {index}"))
            })?;
        out.push(FindHit {
            relation: scope.relation.name.clone(),
            column: column.name.clone(),
            host: row.get(1)?,
            count: as_count(row.get(2)?),
            first: row.get(3)?,
            last: row.get(4)?,
            time_semantics,
        });
    }
    out.sort_by(|a, b| {
        (&a.column, a.host.is_none(), &a.host).cmp(&(&b.column, b.host.is_none(), &b.host))
    });
    Ok(out)
}
