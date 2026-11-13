//! `FilterNode` -> SQL.
//!
//! The fragment is a boolean expression over one relation's columns, with no
//! leading `WHERE`, so a query can AND it with other fragments and place it
//! after `FROM main."<relation>"`. Every value in the document becomes a
//! bound parameter; the only text from the document written into the SQL is
//! a column name, and only after it was found in the catalog, through
//! `quote_ident`. The constants the engine itself writes (`''`, the regex
//! `'i'` flag) are not document values.
//!
//! Every leaf is wrapped in `COALESCE(…, FALSE)`, so a condition on a NULL
//! cell is false rather than unknown. That makes `not` the exact complement
//! of what it wraps: "not contains x" keeps the rows whose cell is empty,
//! as an analyst reading the chip would expect, instead of dropping them
//! the way SQL's three-valued logic would.

use super::{check_caps, FilterNode, Leaf, Op, MARK_COLUMN};
use crate::catalog::{ColumnInfo, SqlType};
use crate::error::{EngineError, Result};
use crate::query::MARK_SUSPICIOUS;
use crate::time::parse_utc_text;
use duckdb::types::Value as SqlValue;
use serde_json::Value;
use triage_core::output::duckdb::sql::quote_ident;

/// A boolean SQL expression and the parameters its `?` placeholders bind,
/// in order.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlFragment {
    pub sql: String,
    pub params: Vec<SqlValue>,
}

/// The value a `_mark` condition takes for the absence of any mark; the
/// other value it accepts is the stored spelling, `MARK_SUSPICIOUS`.
pub(crate) const MARK_NONE: &str = "none";

/// Compile `node` against `columns`, the catalogued columns of the relation
/// it will run over. `Invalid` on an unknown column or a value of the wrong
/// shape.
pub fn compile_filter(node: &FilterNode, columns: &[ColumnInfo]) -> Result<SqlFragment> {
    // Bounds the recursion below as well as enforcing the caps.
    check_caps(node)?;
    let mut params = Vec::new();
    let sql = node_sql(node, columns, &mut params)?;
    Ok(SqlFragment { sql, params })
}

fn node_sql(
    node: &FilterNode,
    columns: &[ColumnInfo],
    params: &mut Vec<SqlValue>,
) -> Result<String> {
    let (children, joiner, identity) = match node {
        FilterNode::And(children) => (children, " AND ", "TRUE"),
        FilterNode::Or(children) => (children, " OR ", "FALSE"),
        FilterNode::Not(inner) => {
            return Ok(format!("(NOT {})", node_sql(inner, columns, params)?))
        }
        FilterNode::Leaf(leaf) => return leaf_sql(leaf, columns, params),
    };
    // An empty `and` is every row and an empty `or` none: the identities, so
    // that removing the last chip of a group does what the group said.
    if children.is_empty() {
        return Ok(identity.into());
    }
    let parts = children
        .iter()
        .map(|child| node_sql(child, columns, params))
        .collect::<Result<Vec<_>>>()?;
    Ok(format!("({})", parts.join(joiner)))
}

fn invalid(leaf: &Leaf, what: &str) -> EngineError {
    EngineError::Invalid(format!("filter on \"{}\": {what}", leaf.column))
}

fn leaf_sql(leaf: &Leaf, columns: &[ColumnInfo], params: &mut Vec<SqlValue>) -> Result<String> {
    if leaf.column == MARK_COLUMN {
        return mark_sql(leaf, params);
    }
    let column = columns
        .iter()
        .find(|c| c.name == leaf.column)
        .ok_or_else(|| EngineError::Invalid(format!("unknown column \"{}\"", leaf.column)))?;
    let (negated, positive) = match leaf.op {
        Op::Ne => (true, Op::Eq),
        Op::NotContains => (true, Op::Contains),
        Op::NotIn => (true, Op::In),
        Op::NotEmpty => (true, Op::IsEmpty),
        op => (false, op),
    };
    let expr = positive_sql(leaf, positive, column, params)?;
    let not = if negated { "NOT " } else { "" };
    Ok(format!("{not}{}", null_safe(&expr)))
}

/// `expr` with NULL read as false, so a condition on an empty cell is false
/// and `NOT` of it is the exact complement. Filter leaves and search terms
/// both go through here.
pub(crate) fn null_safe(expr: &str) -> String {
    format!("COALESCE({expr}, FALSE)")
}

/// True where a typed column's text did not convert: the typed value
/// (`typed`, SQL) is NULL while the text (`text`, SQL) is there and not
/// empty. An empty cell, NULL or `''`, has not failed. The one predicate
/// behind the grid's `failed` flag, the `conversion_failed` filter and the
/// row detail's `failed`, so the three cannot disagree. Never NULL.
pub(crate) fn failed_sql(text: &str, typed: &str) -> String {
    format!("({typed} IS NULL AND coalesce(CAST({text} AS VARCHAR), '') <> '')")
}

/// Where a literal needle must appear in the text.
#[derive(Clone, Copy)]
pub(crate) enum LiteralMatch {
    Contains,
    StartsWith,
    EndsWith,
}

/// `haystack` (SQL) tested for the literal text `needle`, which is pushed as
/// a parameter as written. Unless `case_sensitive`, both sides are lowered
/// first, which is what `ILIKE` does to them.
///
/// These are DuckDB's substring functions rather than `LIKE` with the needle
/// escaped: the two select the same rows, but `LIKE … ESCAPE` runs the
/// general pattern matcher over every row and costs about twice as much.
pub(crate) fn literal_sql(
    haystack: &str,
    needle: &str,
    shape: LiteralMatch,
    case_sensitive: bool,
    params: &mut Vec<SqlValue>,
) -> String {
    params.push(SqlValue::Text(needle.to_string()));
    let function = match shape {
        LiteralMatch::Contains => "contains",
        LiteralMatch::StartsWith => "starts_with",
        LiteralMatch::EndsWith => "ends_with",
    };
    format!(
        "{function}({}, {})",
        fold(haystack, case_sensitive),
        fold("?", case_sensitive)
    )
}

/// `haystack` (SQL) tested for being exactly the text `needle`, which is
/// pushed as a parameter; both sides lowered first unless `case_sensitive`,
/// as an `eq` filter on a column's text compares them.
pub(crate) fn literal_eq_sql(
    haystack: &str,
    needle: &str,
    case_sensitive: bool,
    params: &mut Vec<SqlValue>,
) -> String {
    params.push(SqlValue::Text(needle.to_string()));
    format!(
        "{} = {}",
        fold(haystack, case_sensitive),
        fold("?", case_sensitive)
    )
}

/// `haystack` (SQL) tested against the regex `pattern`, which is pushed as a
/// parameter; case-insensitive unless `case_sensitive`.
pub(crate) fn regex_sql(
    haystack: &str,
    pattern: &str,
    case_sensitive: bool,
    params: &mut Vec<SqlValue>,
) -> String {
    params.push(SqlValue::Text(pattern.to_string()));
    let flags = if case_sensitive { "" } else { ", 'i'" };
    format!("regexp_matches({haystack}, ?{flags})")
}

/// The SQL for a non-negated operator.
fn positive_sql(
    leaf: &Leaf,
    op: Op,
    column: &ColumnInfo,
    params: &mut Vec<SqlValue>,
) -> Result<String> {
    let text = quote_ident(&column.text_column);
    let fold = |sql: &str| fold(sql, leaf.case_sensitive);
    match op {
        Op::Eq => {
            params.push(string_value(leaf)?);
            Ok(format!("{} = {}", fold(&text), fold("?")))
        }
        Op::In => {
            let values = string_list(leaf)?;
            if values.is_empty() {
                return Ok("FALSE".into());
            }
            let slots = vec![fold("?"); values.len()].join(", ");
            params.extend(values);
            Ok(format!("{} IN ({slots})", fold(&text)))
        }
        Op::Contains | Op::StartsWith | Op::EndsWith => {
            let SqlValue::Text(needle) = string_value(leaf)? else {
                return Err(invalid(leaf, "expects a string value"));
            };
            let shape = match op {
                Op::Contains => LiteralMatch::Contains,
                Op::StartsWith => LiteralMatch::StartsWith,
                _ => LiteralMatch::EndsWith,
            };
            Ok(literal_sql(
                &text,
                &needle,
                shape,
                leaf.case_sensitive,
                params,
            ))
        }
        Op::Regex => {
            let SqlValue::Text(pattern) = string_value(leaf)? else {
                return Err(invalid(leaf, "expects a string value"));
            };
            Ok(regex_sql(&text, &pattern, leaf.case_sensitive, params))
        }
        Op::IsEmpty => {
            require_no_value(leaf)?;
            Ok(format!("({text} IS NULL OR {text} = '')"))
        }
        Op::ConversionFailed => {
            require_no_value(leaf)?;
            let typed = column
                .typed_column
                .as_deref()
                .ok_or_else(|| invalid(leaf, "conversion_failed needs a typed column"))?;
            Ok(failed_sql(&text, &quote_ident(typed)))
        }
        Op::Gt | Op::Gte | Op::Lt | Op::Lte => {
            let (lhs, rhs) = ordered_operand(leaf, &leaf.value, column, params)?;
            let cmp = match op {
                Op::Gt => ">",
                Op::Gte => ">=",
                Op::Lt => "<",
                _ => "<=",
            };
            Ok(format!("{lhs} {cmp} {rhs}"))
        }
        Op::Between => {
            let [low, high] = leaf.value.as_array().map(Vec::as_slice).unwrap_or_default() else {
                return Err(invalid(leaf, "between expects [low, high]"));
            };
            let (lhs, low) = ordered_operand(leaf, low, column, params)?;
            let (lhs_high, high) = ordered_operand(leaf, high, column, params)?;
            // The left side is chosen by the bound's kind, so a mismatch is
            // a number paired with a string on an untyped column.
            if lhs != lhs_high {
                return Err(invalid(leaf, "between bounds must be of the same kind"));
            }
            Ok(format!("{lhs} BETWEEN {low} AND {high}"))
        }
        // Negated operators were turned positive by `leaf_sql`.
        Op::Ne | Op::NotContains | Op::NotIn | Op::NotEmpty => Err(EngineError::Internal(format!(
            "filter on \"{}\": a negated operator reached positive_sql",
            leaf.column
        ))),
    }
}

/// `lower(…)` unless the comparison is case-sensitive.
fn fold(sql: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        sql.to_string()
    } else {
        format!("lower({sql})")
    }
}

fn string_value(leaf: &Leaf) -> Result<SqlValue> {
    match &leaf.value {
        Value::String(s) => Ok(SqlValue::Text(s.clone())),
        _ => Err(invalid(
            leaf,
            &format!("{} expects a string value", op_name(leaf.op)),
        )),
    }
}

fn string_list(leaf: &Leaf) -> Result<Vec<SqlValue>> {
    let shape = || {
        invalid(
            leaf,
            &format!("{} expects an array of strings", op_name(leaf.op)),
        )
    };
    leaf.value
        .as_array()
        .ok_or_else(shape)?
        .iter()
        .map(|v| {
            v.as_str()
                .map(|s| SqlValue::Text(s.into()))
                .ok_or_else(shape)
        })
        .collect()
}

fn require_no_value(leaf: &Leaf) -> Result<()> {
    if leaf.value.is_null() {
        Ok(())
    } else {
        Err(invalid(
            leaf,
            &format!("{} takes no value", op_name(leaf.op)),
        ))
    }
}

/// The left and right sides of an ordered comparison against `value`, with
/// the value pushed as a parameter.
///
/// On a typed column the value is checked here against the column's type,
/// so the `CAST` in the SQL cannot fail at run time. On an untyped column a
/// JSON number compares numerically and a string compares as text.
fn ordered_operand(
    leaf: &Leaf,
    value: &Value,
    column: &ColumnInfo,
    params: &mut Vec<SqlValue>,
) -> Result<(String, String)> {
    let text = quote_ident(&column.text_column);
    if let (Some(typed), Some(sql_type)) = (&column.typed_column, column.sql_type) {
        let bound = typed_value(value, sql_type).ok_or_else(|| {
            invalid(
                leaf,
                &format!("{} needs a {} value", op_name(leaf.op), type_hint(sql_type)),
            )
        })?;
        params.push(bound);
        return Ok((quote_ident(typed), format!("CAST(? AS {})", sql_type.sql())));
    }
    match value {
        Value::Number(number) => {
            let number = number
                .as_f64()
                .ok_or_else(|| invalid(leaf, "number out of range"))?;
            params.push(SqlValue::Double(number));
            Ok((format!("TRY_CAST({text} AS DOUBLE)"), "?".into()))
        }
        Value::String(s) => {
            params.push(SqlValue::Text(s.clone()));
            Ok((
                fold(&text, leaf.case_sensitive),
                fold("?", leaf.case_sensitive),
            ))
        }
        _ => Err(invalid(
            leaf,
            &format!("{} expects a number or a string", op_name(leaf.op)),
        )),
    }
}

/// `value` as a parameter of `sql_type`, or `None` when it is not one.
fn typed_value(value: &Value, sql_type: SqlType) -> Option<SqlValue> {
    match sql_type {
        SqlType::Timestamp => parse_utc_text(value.as_str()?).map(SqlValue::Text),
        SqlType::BigInt => value
            .as_i64()
            .or_else(|| value.as_str()?.parse().ok())
            .map(SqlValue::BigInt),
        SqlType::UBigInt => value
            .as_u64()
            .or_else(|| value.as_str()?.parse().ok())
            .map(SqlValue::UBigInt),
        SqlType::Boolean => value.as_bool().map(SqlValue::Boolean),
    }
}

fn type_hint(sql_type: SqlType) -> &'static str {
    match sql_type {
        SqlType::Timestamp => "UTC timestamp (YYYY-MM-DDTHH:MM:SS[.f]Z)",
        SqlType::BigInt => "signed integer",
        SqlType::UBigInt => "unsigned integer",
        SqlType::Boolean => "boolean",
    }
}

fn op_name(op: Op) -> String {
    serde_json::to_value(op)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// `_mark`: whether `ox.row_mark` holds a mark for the row. `"none"` is the
/// absence of any mark.
fn mark_sql(leaf: &Leaf, params: &mut Vec<SqlValue>) -> Result<String> {
    let one = |value: &Value, params: &mut Vec<SqlValue>| -> Result<String> {
        let exists = format!(
            "EXISTS (SELECT 1 FROM ox.row_mark AS ox_m WHERE ox_m.row_id = {}",
            quote_ident("_ox_row")
        );
        match value.as_str() {
            Some(MARK_NONE) => Ok(format!("NOT {exists})")),
            Some(MARK_SUSPICIOUS) => {
                params.push(SqlValue::Text(MARK_SUSPICIOUS.into()));
                Ok(format!("{exists} AND ox_m.mark = ?)"))
            }
            _ => Err(invalid(
                leaf,
                &format!("{MARK_COLUMN} is \"{MARK_SUSPICIOUS}\" or \"{MARK_NONE}\""),
            )),
        }
    };
    match leaf.op {
        Op::Eq => one(&leaf.value, params),
        Op::Ne => Ok(format!("NOT {}", one(&leaf.value, params)?)),
        Op::In => {
            let values = leaf
                .value
                .as_array()
                .ok_or_else(|| invalid(leaf, "in expects an array"))?;
            if values.is_empty() {
                return Ok("FALSE".into());
            }
            let parts = values
                .iter()
                .map(|v| one(v, params))
                .collect::<Result<Vec<_>>>()?;
            Ok(format!("({})", parts.join(" OR ")))
        }
        _ => Err(invalid(leaf, &format!("{MARK_COLUMN} takes eq, ne or in"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Found on the first real run: `ILIKE ? ESCAPE '\'` sends every row
    /// through DuckDB's general pattern matcher, which no rewrite reaches
    /// once there is an `ESCAPE`. Searching 75,000 events for a word took
    /// 640 ms that way and 300 ms as a substring test, with the same rows.
    #[test]
    fn a_literal_match_is_a_substring_test_not_a_pattern() {
        let compiled = |shape, case_sensitive| {
            let mut params = Vec::new();
            let sql = literal_sql("\"c\"", "50%_a\\B", shape, case_sensitive, &mut params);
            // The needle is bound as written: nothing in it is a wildcard.
            assert_eq!(params, [SqlValue::Text("50%_a\\B".into())]);
            sql
        };
        assert_eq!(
            compiled(LiteralMatch::Contains, false),
            "contains(lower(\"c\"), lower(?))"
        );
        assert_eq!(
            compiled(LiteralMatch::StartsWith, false),
            "starts_with(lower(\"c\"), lower(?))"
        );
        assert_eq!(
            compiled(LiteralMatch::EndsWith, false),
            "ends_with(lower(\"c\"), lower(?))"
        );
        assert_eq!(compiled(LiteralMatch::Contains, true), "contains(\"c\", ?)");
        assert_eq!(
            compiled(LiteralMatch::StartsWith, true),
            "starts_with(\"c\", ?)"
        );
        assert_eq!(
            compiled(LiteralMatch::EndsWith, true),
            "ends_with(\"c\", ?)"
        );
    }
}
