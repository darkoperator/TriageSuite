//! Stacking: the values of one or more columns counted rarest-first, with
//! how many hosts each appears on. The frequency analysis analysts otherwise
//! do in a spreadsheet pivot.

use super::group::{as_count, first_last_sql, key_sql, MAX_BY};
use super::page::clip_text;
use super::{find_column, marked_sql, scope, GridQuery, ROW_ALIAS};
use crate::case::Case;
use crate::catalog::{self, ColumnInfo};
use crate::error::{EngineError, Result};
use crate::listing::check_limit;
use duckdb::params_from_iter;
use duckdb::types::Value as SqlValue;
use triage_core::output::duckdb::sql::quote_ident;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackOrder {
    Rarest,
    Commonest,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StackRow {
    /// One entry per `by` column, cut to `CELL_CAP` characters; `None` for
    /// an empty cell.
    pub values: Vec<Option<String>>,
    pub count: u64,
    /// Distinct hosts the combination appears on; 0 without a host column.
    pub hosts: u64,
    pub first: Option<String>,
    pub last: Option<String>,
    pub suspicious: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Stack {
    pub by: Vec<String>,
    /// Distinct hosts among the filtered rows; 0 without a host column.
    pub hosts_total: u64,
    /// Distinct value combinations in the whole stack, not the page.
    pub distinct: u64,
    /// Rows the whole stack counts, not the page.
    pub rows_counted: u64,
    pub rows: Vec<StackRow>,
    /// More combinations follow this page.
    pub truncated: bool,
    /// Some value on this page was longer than `CELL_CAP` characters and
    /// was cut.
    pub cells_clipped: bool,
}

/// Up to `limit` combinations from `offset` of `by` (1 to 4 columns) over
/// the rows `q` selects, ordered by count (ties by the values ascending,
/// empty last). `Invalid` for a bad column count or a `limit` of 0 or over
/// `MAX_LIMIT`; `NotFound` for a column the relation lacks.
pub fn stack(
    case: &Case,
    q: &GridQuery,
    by: &[String],
    order: StackOrder,
    limit: u32,
    offset: u64,
) -> Result<Stack> {
    check_limit(limit)?;
    if by.is_empty() || by.len() > MAX_BY {
        return Err(EngineError::Invalid(format!(
            "stack needs 1 to {MAX_BY} columns (got {})",
            by.len()
        )));
    }
    let scope = scope(case, q)?;
    let all = catalog::columns(case, &q.relation)?;
    let by_cols: Vec<&ColumnInfo> = by
        .iter()
        .map(|name| find_column(&all, &scope.relation.name, name))
        .collect::<Result<_>>()?;

    let keys: Vec<String> = by_cols.iter().map(|c| key_sql(c)).collect();
    let key_list = keys.join(", ");
    let host = match &scope.relation.host_column {
        Some(h) => format!("count(DISTINCT {ROW_ALIAS}.{})", quote_ident(h)),
        None => "CAST(0 AS BIGINT)".to_string(),
    };
    let (first, last) = first_last_sql(&scope.relation.primary_time, &all);
    let from = &scope.from_sql;
    let where_sql = &scope.where_sql;
    let conn = case.conn();

    // Totals over the whole stack, independent of the page asked for.
    let (rows_counted, hosts_total): (i64, i64) = conn.query_row(
        &format!("SELECT count(*), {host} FROM {from} WHERE ({where_sql})"),
        params_from_iter(scope.params.clone()),
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let distinct: i64 = conn.query_row(
        &format!(
            "SELECT count(*) FROM \
             (SELECT 1 FROM {from} WHERE ({where_sql}) GROUP BY {key_list})"
        ),
        params_from_iter(scope.params.clone()),
        |r| r.get(0),
    )?;

    let direction = match order {
        StackOrder::Rarest => "ASC",
        StackOrder::Commonest => "DESC",
    };
    // Ordinals 1..=n are the key columns of the select list.
    let tiebreak: Vec<String> = (1..=by.len())
        .map(|i| format!("{i} ASC NULLS LAST"))
        .collect();
    let sql = format!(
        "SELECT {key_list}, count(*), {host}, {first}, {last}, \
         count(*) FILTER (WHERE {marked}) \
         FROM {from} WHERE ({where_sql}) GROUP BY {key_list} \
         ORDER BY count(*) {direction}, {tiebreak} LIMIT ? OFFSET ?",
        marked = marked_sql(),
        tiebreak = tiebreak.join(", "),
    );
    let mut params = scope.params.clone();
    params.push(SqlValue::BigInt(i64::from(limit)));
    params.push(SqlValue::UBigInt(offset));

    let n = by.len();
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;
    let mut out = Vec::new();
    let mut cells_clipped = false;
    while let Some(row) = rows.next()? {
        let mut values = Vec::with_capacity(n);
        for i in 0..n {
            let (value, clipped) = clip_text(row.get::<_, Option<String>>(i)?);
            cells_clipped |= clipped;
            values.push(value);
        }
        out.push(StackRow {
            values,
            count: as_count(row.get(n)?),
            hosts: as_count(row.get(n + 1)?),
            first: row.get(n + 2)?,
            last: row.get(n + 3)?,
            suspicious: as_count(row.get(n + 4)?),
        });
    }
    let distinct = as_count(distinct);
    let shown = offset.saturating_add(out.len() as u64);
    Ok(Stack {
        by: by.to_vec(),
        hosts_total: as_count(hosts_total),
        distinct,
        rows_counted: as_count(rows_counted),
        truncated: shown < distinct,
        rows: out,
        cells_clipped,
    })
}
