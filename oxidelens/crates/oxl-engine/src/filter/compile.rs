//! `FilterNode` -> SQL.
//!
//! The fragment is a boolean expression over one relation's columns, with no
//! leading `WHERE`, so a query can AND it with other fragments and place it
//! after `FROM main."<relation>"`. Every value in the document becomes a
//! bound parameter; the only text from the document written into the SQL is
//! a column name, and only after it was found in the catalog, through
//! `quote_ident`. The constants the engine itself writes (`''`, the `LIKE`
//! escape character, the regex `'i'` flag) are not document values.
//!
//! Every leaf is wrapped in `COALESCE(…, FALSE)`, so a condition on a NULL
//! cell is false rather than unknown. That makes `not` the exact complement
//! of what it wraps: "not contains x" keeps the rows whose cell is empty,
//! as an analyst reading the chip would expect, instead of dropping them
//! the way SQL's three-valued logic would.

use super::{check_caps, FilterNode, Leaf, Op, MARK_COLUMN};
use crate::catalog::{ColumnInfo, SqlType};
use crate::error::{EngineError, Result};
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

/// The values a `_mark` condition accepts.
const MARK_SUSPICIOUS: &str = "suspicious";
const MARK_NONE: &str = "none";

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
    Ok(format!("{not}COALESCE({expr}, FALSE)"))
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
            let needle = escape_like(&needle);
            let pattern = match op {
                Op::Contains => format!("%{needle}%"),
                Op::StartsWith => format!("{needle}%"),
                _ => format!("%{needle}"),
            };
            params.push(SqlValue::Text(pattern));
            let like = if leaf.case_sensitive { "LIKE" } else { "ILIKE" };
            Ok(format!("{text} {like} ? ESCAPE '\\'"))
        }
        Op::Regex => {
            params.push(string_value(leaf)?);
            let flags = if leaf.case_sensitive { "" } else { ", 'i'" };
            Ok(format!("regexp_matches({text}, ?{flags})"))
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
            Ok(format!(
                "({} IS NULL AND {text} IS NOT NULL AND {text} <> '')",
                quote_ident(typed)
            ))
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
        Op::Ne | Op::NotContains | Op::NotIn | Op::NotEmpty => Err(invalid(
            leaf,
            "internal: negated operator reached positive_sql",
        )),
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

/// `%`, `_` and the escape character itself, escaped so a `LIKE` pattern
/// built around the text matches it literally. Pair it with `ESCAPE '\'`.
pub(crate) fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
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
