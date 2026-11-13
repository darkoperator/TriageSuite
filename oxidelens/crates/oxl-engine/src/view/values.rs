//! The column filter panel's Values tab: one column's values with their
//! counts under the other active filters.

use super::GridState;
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::filter::{literal_sql, null_safe, LiteralMatch, MARK_COLUMN};
use crate::listing::{check_limit, fetch_limit, Listing};
use crate::query::{
    as_count, check_offset, clip_text, find_column, group_key_sql, scope, ROW_ALIAS,
};
use crate::search::MAX_SEARCH;
use duckdb::params_from_iter;
use duckdb::types::Value as SqlValue;
use triage_core::output::duckdb::sql::quote_ident;

/// How a column's values are listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ValueOrder {
    /// Most rows first; ties by value.
    Commonest,
    /// Fewest rows first, so the rarest can be listed first; ties by value.
    Rarest,
    /// By value: a typed column by its typed value, any other by its text
    /// without regard to case, as the grid sorts them; empty last.
    Value,
}

/// One value of a column and how many rows have it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ValueCount {
    /// The cell's text, cut to `CELL_CAP` characters; `None` for the empty
    /// cells, which are one entry however they are empty.
    pub value: Option<String>,
    /// `value` was longer than `CELL_CAP` characters and was cut.
    pub clipped: bool,
    pub count: u64,
}

/// Up to `limit` values of `column` from `offset`, counted over the rows
/// `state` selects under every enabled chip except those whose ids are in
/// `except_chips` (the search in filter mode and the hosts still apply).
/// `contains` keeps the values holding it as a literal substring, compared
/// without regard to case; empty keeps every value. `column` may be the
/// `_mark` pseudo-column, whose values are the marks.
///
/// `Invalid` for a bad state, a `limit` of 0 or over `MAX_LIMIT`, an
/// `offset` past `i64::MAX`, or a `contains` over `MAX_SEARCH` characters;
/// `NotFound` for a column the relation lacks.
// Each argument is one independent input of the Values tab's request (the
// grid, the column, the chips set aside, the search box, the order and the
// page), and this signature is the interface the app's command is built on;
// a struct around them would only move the same eight names.
#[allow(clippy::too_many_arguments)]
pub fn values(
    case: &Case,
    state: &GridState,
    column: &str,
    except_chips: &[String],
    contains: Option<&str>,
    order: ValueOrder,
    limit: u32,
    offset: u64,
) -> Result<Listing<ValueCount>> {
    check_limit(limit)?;
    check_offset(offset)?;
    let q = state.compile(except_chips)?;
    let scope = scope(case, &q)?;
    let key = group_key_sql(&scope.all, &scope.relation.name, column)?;
    let typed = if column == MARK_COLUMN {
        None
    } else {
        find_column(&scope.all, &scope.relation.name, column)?
            .typed_column
            .as_deref()
    };

    let mut params = scope.params.clone();
    let mut narrow = String::new();
    if let Some(needle) = contains.filter(|n| !n.is_empty()) {
        if needle.chars().count() > MAX_SEARCH {
            return Err(EngineError::Invalid(format!(
                "a value search is at most {MAX_SEARCH} characters"
            )));
        }
        // The filter's own `contains`: a substring test, so `%` and `_`
        // are characters. An empty cell contains nothing.
        let test = literal_sql(&key, needle, LiteralMatch::Contains, false, &mut params);
        narrow = format!(" AND {}", null_safe(&test));
    }

    // Two texts of one typed value (`1`, `01`) are two values, ordered
    // by their text after the typed value.
    let typed_min = match typed {
        Some(t) => format!("min({ROW_ALIAS}.{})", quote_ident(t)),
        None => "CAST(NULL AS VARCHAR)".to_string(),
    };
    let by_value = "ox_typed ASC NULLS LAST, lower(ox_key) ASC NULLS LAST, ox_key ASC NULLS LAST";
    let order_by = match order {
        ValueOrder::Commonest => format!("ox_n DESC, {by_value}"),
        ValueOrder::Rarest => format!("ox_n ASC, {by_value}"),
        ValueOrder::Value => by_value.to_string(),
    };
    let sql = format!(
        "SELECT ox_key, ox_n FROM (\
           SELECT {key} AS ox_key, count(*) AS ox_n, {typed_min} AS ox_typed \
           FROM {from} WHERE ({where_sql}){narrow} GROUP BY ox_key) \
         ORDER BY {order_by} LIMIT ? OFFSET ?",
        from = scope.from_sql,
        where_sql = scope.where_sql,
    );
    params.push(SqlValue::BigInt(fetch_limit(limit)));
    params.push(SqlValue::UBigInt(offset));

    let mut stmt = case.conn().prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let (value, clipped) = clip_text(row.get(0)?);
        out.push(ValueCount {
            value,
            clipped,
            count: as_count(row.get(1)?),
        });
    }
    Ok(Listing::from_fetched(out, limit))
}
