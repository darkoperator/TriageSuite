//! Timeline Explorer-style grouping: groups of one column with counts and
//! time spans, nested by further columns and expanded one level at a time.

use super::{find_column, marked_sql, scope, GridQuery, ISO_FORMAT, ROW_ALIAS};
use crate::case::Case;
use crate::catalog::{self, ColumnInfo};
use crate::error::{EngineError, Result};
use crate::filter::{FilterNode, Leaf, Op};
use duckdb::params_from_iter;
use duckdb::types::Value as SqlValue;
use serde_json::Value;
use triage_core::output::duckdb::sql::quote_ident;

/// Group-by columns allowed at once.
const MAX_BY: usize = 4;

#[derive(Debug, Clone, serde::Serialize)]
pub struct GroupRow {
    /// The column's text; `None` for an empty cell.
    pub key: Option<String>,
    pub count: u64,
    /// The earliest and latest primary time in the group, UTC; `None` when
    /// the relation has no primary time or the group has no timed row.
    pub first: Option<String>,
    pub last: Option<String>,
    pub suspicious: u64,
}

/// Groups of `by[parent.len()]` among rows whose earlier `by` columns equal
/// `parent`. Ordered by count descending, then key. `Invalid` for an empty
/// `by`, more than four columns, or a `parent` as long as `by`.
pub fn groups(
    case: &Case,
    q: &GridQuery,
    by: &[String],
    parent: &[Option<String>],
    limit: u32,
    offset: u64,
) -> Result<Vec<GroupRow>> {
    if by.is_empty() || by.len() > MAX_BY || parent.len() >= by.len() {
        return Err(EngineError::Invalid(format!(
            "group by needs 1 to {MAX_BY} columns and a parent key shorter than that list \
             (got {} columns, parent of {})",
            by.len(),
            parent.len()
        )));
    }
    let scope = scope(case, q)?;
    let all = catalog::columns(case, &q.relation)?;
    let by_cols: Vec<&ColumnInfo> = by
        .iter()
        .map(|name| find_column(&all, &scope.relation.name, name))
        .collect::<Result<_>>()?;

    let mut conditions = String::new();
    let mut params = scope.params.clone();
    for (col, key) in by_cols.iter().zip(parent) {
        match key {
            None => conditions.push_str(&format!(" AND {} IS NULL", key_sql(col))),
            Some(text) => {
                conditions.push_str(&format!(" AND {} = ?", key_sql(col)));
                params.push(SqlValue::Text(text.clone()));
            }
        }
    }
    let (first, last) = match typed_primary_time(&scope.relation.primary_time, &all) {
        Some(t) => {
            let t = format!("{ROW_ALIAS}.{}", quote_ident(t));
            (
                format!("strftime(min({t}), '{ISO_FORMAT}')"),
                format!("strftime(max({t}), '{ISO_FORMAT}')"),
            )
        }
        None => (
            "CAST(NULL AS VARCHAR)".to_string(),
            "CAST(NULL AS VARCHAR)".to_string(),
        ),
    };
    let sql = format!(
        "SELECT {key} AS ox_key, count(*), {first}, {last}, \
         count(*) FILTER (WHERE {marked}) \
         FROM {from} WHERE ({where_sql}){conditions} \
         GROUP BY ox_key ORDER BY count(*) DESC, ox_key ASC NULLS LAST LIMIT ? OFFSET ?",
        key = key_sql(by_cols[parent.len()]),
        marked = marked_sql(),
        from = scope.from_sql,
        where_sql = scope.where_sql,
    );
    params.push(SqlValue::BigInt(i64::from(limit)));
    params.push(SqlValue::UBigInt(offset));

    let mut stmt = case.conn().prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(GroupRow {
            key: row.get(0)?,
            count: as_count(row.get(1)?),
            first: row.get(2)?,
            last: row.get(3)?,
            suspicious: as_count(row.get(4)?),
        });
    }
    Ok(out)
}

/// The filter that selects one group's rows: q's filter AND each `by[i] =
/// key[i]` (IsEmpty for a None key). Pairs beyond the shorter list are
/// ignored.
pub fn group_filter(q: &GridQuery, by: &[String], key: &[Option<String>]) -> GridQuery {
    let leaves = by.iter().zip(key).map(|(column, key)| {
        FilterNode::Leaf(match key {
            Some(text) => Leaf {
                column: column.clone(),
                op: Op::Eq,
                value: Value::String(text.clone()),
                // A group is one exact key, not a case-folded family of them.
                case_sensitive: true,
            },
            None => Leaf {
                column: column.clone(),
                op: Op::IsEmpty,
                value: Value::Null,
                case_sensitive: false,
            },
        })
    });
    // Joining an existing top-level `and` keeps the document no deeper than
    // it was, so the depth cap does not bite on a long group path.
    let mut all = match q.filter.clone() {
        None => Vec::new(),
        Some(FilterNode::And(children)) => children,
        Some(other) => vec![other],
    };
    all.extend(leaves);
    let mut out = q.clone();
    out.filter = Some(FilterNode::And(all));
    out
}

/// A cell's group key: its text, with an empty cell read as NULL.
fn key_sql(c: &ColumnInfo) -> String {
    format!(
        "NULLIF(CAST({ROW_ALIAS}.{} AS VARCHAR), '')",
        quote_ident(&c.text_column)
    )
}

/// The typed physical column of the relation's primary time, if it has one.
pub(super) fn typed_primary_time<'a>(
    primary: &Option<String>,
    all: &'a [ColumnInfo],
) -> Option<&'a str> {
    let name = primary.as_deref()?;
    all.iter()
        .find(|c| c.name == name)
        .and_then(|c| c.typed_column.as_deref())
}

pub(super) fn as_count(n: i64) -> u64 {
    u64::try_from(n).unwrap_or_default()
}
