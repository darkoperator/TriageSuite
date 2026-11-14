//! Timeline Explorer-style grouping: groups of one column with counts and
//! time spans, nested by further columns and expanded one level at a time.

use super::page::{clip_text, mark_sql};
use super::{
    check_offset, find_column, marked_sql, scope, GridQuery, Scope, ISO_FORMAT, ROW_ALIAS,
};
use crate::case::Case;
use crate::catalog::{ColumnInfo, TimeSemantics};
use crate::error::{EngineError, Result};
use crate::filter::{FilterNode, Leaf, Op, MARK_COLUMN, MARK_NONE};
use crate::listing::{check_limit, fetch_limit, Listing};
use duckdb::params_from_iter;
use duckdb::types::Value as SqlValue;
use serde_json::Value;
use triage_core::output::duckdb::sql::quote_ident;

/// Group-by columns allowed at once.
pub(super) const MAX_BY: usize = 4;

#[derive(Debug, Clone, serde::Serialize)]
pub struct GroupRow {
    /// The column's text, cut to `CELL_CAP` characters; `None` for an empty
    /// cell.
    pub key: Option<String>,
    /// `key` was longer than `CELL_CAP` characters and was cut. A cut key is
    /// not the group's key: as a `parent` or in `group_filter` it selects
    /// nothing. Use `full_key` for that.
    pub key_clipped: bool,
    /// The whole key, for a caller in this process to drill into the group
    /// with (`parent`, `group_filter`). Never serialised, so list output
    /// stays within the cell cap.
    #[serde(skip)]
    pub full_key: Option<String>,
    pub count: u64,
    /// The earliest and latest primary time in the group, UTC; `None` when
    /// the relation has no primary time or the group has no timed row.
    pub first: Option<String>,
    pub last: Option<String>,
    /// Rows with a mark of any state (`ox.row_state`). The name predates
    /// the derived marks; it counts Malicious and Agent: malicious rows too.
    pub suspicious: u64,
}

/// Up to `limit` groups of `by[parent.len()]` from `offset`, among rows
/// whose earlier `by` columns equal `parent`. Ordered by count descending,
/// then key. A `by` column may be the `_mark` pseudo-column, whose key is
/// the mark as `ox.row_state` spells it or `None` for an unmarked row. `Invalid` for an empty `by`, more than four columns, a
/// `parent` as long as `by`, a `limit` of 0 or over `MAX_LIMIT`, or an
/// `offset` past `i64::MAX`.
pub fn groups(
    case: &Case,
    q: &GridQuery,
    by: &[String],
    parent: &[Option<String>],
    limit: u32,
    offset: u64,
) -> Result<Listing<GroupRow>> {
    let fetched = group_spans(case, q, by, parent, limit, offset)?;
    Ok(Listing {
        items: fetched.items.into_iter().map(|g| g.row).collect(),
        truncated: fetched.truncated,
    })
}

/// A group with the earliest and latest primary time as the source wrote
/// them (the text of a row holding each), which the grid renders in its
/// display zone with the fraction as written. `None` where `GroupRow`'s
/// `first` and `last` are.
pub(crate) struct GroupSpan {
    pub row: GroupRow,
    pub first_text: Option<String>,
    pub last_text: Option<String>,
}

/// `groups`, with each group's first and last time as text.
pub(crate) fn group_spans(
    case: &Case,
    q: &GridQuery,
    by: &[String],
    parent: &[Option<String>],
    limit: u32,
    offset: u64,
) -> Result<Listing<GroupSpan>> {
    check_limit(limit)?;
    check_offset(offset)?;
    if by.is_empty() || by.len() > MAX_BY || parent.len() >= by.len() {
        return Err(EngineError::Invalid(format!(
            "group by needs 1 to {MAX_BY} columns and a parent key shorter than that list \
             (got {} columns, parent of {})",
            by.len(),
            parent.len()
        )));
    }
    let scope = scope(case, q)?;
    let all = &scope.all;
    let keys: Vec<String> = by
        .iter()
        .map(|name| group_key_sql(&scope, name))
        .collect::<Result<_>>()?;

    let mut conditions = String::new();
    let mut params = scope.params.clone();
    for (key_expr, key) in keys.iter().zip(parent) {
        match key {
            None => conditions.push_str(&format!(" AND {key_expr} IS NULL")),
            Some(text) => {
                conditions.push_str(&format!(" AND {key_expr} = ?"));
                params.push(SqlValue::Text(text.clone()));
            }
        }
    }
    let (first, last) = first_last_sql(&scope.relation.primary_time, all);
    let (first_text, last_text) = first_last_text_sql(&scope.relation.primary_time, all);
    let sql = format!(
        "SELECT {key} AS ox_key, count(*), {first}, {last}, \
         count(*) FILTER (WHERE {marked}), {first_text}, {last_text} \
         FROM {from} WHERE ({where_sql}){conditions} \
         GROUP BY ox_key ORDER BY count(*) DESC, ox_key ASC NULLS LAST LIMIT ? OFFSET ?",
        key = keys[parent.len()],
        marked = marked_sql(scope.marks),
        from = scope.from_sql,
        where_sql = scope.where_sql,
    );
    params.push(SqlValue::BigInt(fetch_limit(limit)));
    params.push(SqlValue::UBigInt(offset));

    let mut stmt = case.conn().prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let full_key: Option<String> = row.get(0)?;
        let (key, key_clipped) = clip_text(full_key.clone());
        out.push(GroupSpan {
            row: GroupRow {
                key,
                key_clipped,
                full_key,
                count: as_count(row.get(1)?),
                first: row.get(2)?,
                last: row.get(3)?,
                suspicious: as_count(row.get(4)?),
            },
            first_text: row.get(5)?,
            last_text: row.get(6)?,
        });
    }
    Ok(Listing::from_fetched(out, limit))
}

/// The filter that selects one group's rows: q's filter AND each `by[i] =
/// key[i]` (IsEmpty for a None key). Pairs beyond the shorter list are
/// ignored. The keys are whole keys (`GroupRow::full_key`); a key cut to
/// the cell cap matches no row. On `_mark` a key is the mark (`eq
/// "malicious"`) and `None` is no mark (`eq "none"`), as its filter
/// spells them.
pub fn group_filter(q: &GridQuery, by: &[String], key: &[Option<String>]) -> GridQuery {
    let leaves = by.iter().zip(key).map(|(column, key)| {
        FilterNode::Leaf(match key {
            _ if column == MARK_COLUMN => Leaf {
                column: column.clone(),
                op: Op::Eq,
                value: Value::String(key.clone().unwrap_or_else(|| MARK_NONE.to_string())),
                case_sensitive: true,
            },
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

/// The group key of `name` among the scope's catalogued columns: `key_sql`
/// of the column, or for the `_mark` pseudo-column the row's mark as
/// `ox.row_state` spells it, NULL for none. `NotFound` for a column the
/// relation lacks.
pub(crate) fn group_key_sql(scope: &Scope, name: &str) -> Result<String> {
    if name == MARK_COLUMN {
        return Ok(mark_sql(scope.marks));
    }
    Ok(key_sql(find_column(
        &scope.all,
        &scope.relation.name,
        name,
    )?))
}

/// A cell's group key: its text, with an empty cell read as NULL.
pub(super) fn key_sql(c: &ColumnInfo) -> String {
    format!(
        "NULLIF(CAST({ROW_ALIAS}.{} AS VARCHAR), '')",
        quote_ident(&c.text_column)
    )
}

/// SQL for the earliest and latest primary time of a group, rendered as UTC
/// text (ruling R1); NULL when the relation has no typed primary time.
pub(super) fn first_last_sql(primary: &Option<String>, all: &[ColumnInfo]) -> (String, String) {
    match typed_primary_time(primary, all) {
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
    }
}

/// SQL for the text of a row holding the earliest and of one holding the
/// latest primary time of a group, as the source wrote it; NULL when the
/// relation has no typed primary time.
fn first_last_text_sql(primary: &Option<String>, all: &[ColumnInfo]) -> (String, String) {
    let column = primary
        .as_deref()
        .and_then(|name| all.iter().find(|c| c.name == name));
    match column.and_then(|c| Some((c, c.typed_column.as_deref()?))) {
        Some((c, typed)) => {
            let t = format!("{ROW_ALIAS}.{}", quote_ident(typed));
            let text = format!(
                "CAST({ROW_ALIAS}.{} AS VARCHAR)",
                quote_ident(&c.text_column)
            );
            (
                format!("arg_min({text}, {t}) FILTER (WHERE {t} IS NOT NULL)"),
                format!("arg_max({text}, {t}) FILTER (WHERE {t} IS NOT NULL)"),
            )
        }
        None => (
            "CAST(NULL AS VARCHAR)".to_string(),
            "CAST(NULL AS VARCHAR)".to_string(),
        ),
    }
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

/// The time semantics of the relation's typed primary time: how the times
/// a pivot compares and returns are zoned. `None` without a typed primary
/// time. An `Unknown` time is compared as written, as if it were UTC.
pub(crate) fn primary_time_semantics(
    primary: &Option<String>,
    all: &[ColumnInfo],
) -> Option<TimeSemantics> {
    let name = primary.as_deref()?;
    all.iter()
        .find(|c| c.name == name && c.typed_column.is_some())
        .and_then(|c| c.time_semantics)
}

pub(crate) fn as_count(n: i64) -> u64 {
    u64::try_from(n).unwrap_or_default()
}
