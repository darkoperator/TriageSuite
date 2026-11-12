//! Filter documents: the JSON that an analyst's filter chips, the agent's
//! `--filter` argument and saved views all compile through.
//!
//! A document is untrusted input. `parse_filter` turns any text into a
//! `FilterNode` or an `Invalid` error, enforcing the size caps on the way;
//! `compile` turns a node into SQL in which every value is a bound
//! parameter.
//!
//! ```json
//! {"and": [{"column": "Channel", "op": "eq", "value": "Security"},
//!          {"not": {"column": "EventId", "op": "in", "value": ["4624", "4625"]}}]}
//! ```

mod compile;

pub use compile::{compile_filter, SqlFragment};
pub(crate) use compile::{literal_eq_sql, literal_sql, null_safe, regex_sql, LiteralMatch};

use crate::error::{EngineError, Result};
use serde_json::Value;

/// One node of a filter document.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterNode {
    And(Vec<FilterNode>),
    Or(Vec<FilterNode>),
    Not(Box<FilterNode>),
    #[serde(untagged)]
    Leaf(Leaf),
}

/// One condition on one column.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Leaf {
    pub column: String,
    pub op: Op,
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub case_sensitive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Eq,
    Ne,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    Regex,
    In,
    NotIn,
    Between,
    Gt,
    Gte,
    Lt,
    Lte,
    IsEmpty,
    NotEmpty,
    ConversionFailed,
}

/// The deepest document accepted, counting a lone leaf as depth 1 and each
/// `and`, `or` or `not` around it as one more.
pub const MAX_DEPTH: usize = 8;
pub const MAX_LEAVES: usize = 200;
/// Values in one `in` or `not_in` list.
pub const MAX_IN: usize = 10_000;
/// Characters in one `regex` pattern.
pub const MAX_REGEX: usize = 1_000;

/// The pseudo-column that filters on a row's mark: `eq`, `ne` or `in` with
/// `"suspicious"` or `"none"`.
pub const MARK_COLUMN: &str = "_mark";

/// Parse a filter document. `Invalid` on any malformed or over-cap document.
pub fn parse_filter(json: &str) -> Result<FilterNode> {
    // serde_json refuses nesting past its own recursion limit, so text that
    // is all brackets is an error here rather than a stack overflow.
    let value: Value = serde_json::from_str(json)
        .map_err(|e| EngineError::Invalid(format!("filter is not JSON: {e}")))?;
    check_shape(&value)?;
    // `check_shape` bounded the depth, so this recursion is at most
    // MAX_DEPTH frames deep.
    let node = build(&value)?;
    check_caps(&node)?;
    Ok(node)
}

/// A node object, if `value` is one: `{"and": …}`, `{"or": …}` or
/// `{"not": …}` and nothing else. Anything else is read as a leaf.
fn combinator(value: &Value) -> Option<(&str, &Value)> {
    let map = value.as_object()?;
    if map.len() != 1 {
        return None;
    }
    let (key, inner) = map.iter().next()?;
    matches!(key.as_str(), "and" | "or" | "not").then_some((key.as_str(), inner))
}

/// Depth and leaf count, walked with an explicit stack so that the check
/// itself cannot recurse.
fn check_shape(root: &Value) -> Result<()> {
    let mut stack = vec![(root, 1usize)];
    let mut leaves = 0usize;
    while let Some((value, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err(EngineError::Invalid(format!(
                "filter is nested deeper than {MAX_DEPTH} levels"
            )));
        }
        match combinator(value) {
            Some(("not", inner)) => stack.push((inner, depth + 1)),
            Some((_, Value::Array(children))) => {
                stack.extend(children.iter().map(|child| (child, depth + 1)));
            }
            // A malformed `and`/`or` body; `build` reports it.
            Some(_) => {}
            None => {
                leaves += 1;
                if leaves > MAX_LEAVES {
                    return Err(EngineError::Invalid(format!(
                        "filter has more than {MAX_LEAVES} conditions"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Build the node by hand rather than through `FilterNode`'s derived
/// `Deserialize`: an untagged variant reports every mistake as "did not match
/// any variant", where reading the leaf directly names the bad field.
fn build(value: &Value) -> Result<FilterNode> {
    match combinator(value) {
        Some(("not", inner)) => Ok(FilterNode::Not(Box::new(build(inner)?))),
        Some((key, Value::Array(children))) => {
            let children = children.iter().map(build).collect::<Result<Vec<_>>>()?;
            Ok(if key == "and" {
                FilterNode::And(children)
            } else {
                FilterNode::Or(children)
            })
        }
        Some((key, _)) => Err(EngineError::Invalid(format!(
            "filter \"{key}\" takes an array of conditions"
        ))),
        None => serde_json::from_value::<Leaf>(value.clone())
            .map(FilterNode::Leaf)
            .map_err(|e| EngineError::Invalid(format!("filter condition: {e}"))),
    }
}

/// Every cap, over a built node. `parse_filter` has already bounded depth
/// and leaves on the raw JSON; this repeats them because `compile_filter`
/// also runs it, and a node can arrive by other routes (a saved view read
/// back with the derived `Deserialize`, or code). Iterative, like
/// `check_shape`, so an over-deep node is refused rather than overflowing.
pub(crate) fn check_caps(root: &FilterNode) -> Result<()> {
    let mut stack = vec![(root, 1usize)];
    let mut leaves = 0usize;
    while let Some((node, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err(EngineError::Invalid(format!(
                "filter is nested deeper than {MAX_DEPTH} levels"
            )));
        }
        let leaf = match node {
            FilterNode::And(children) | FilterNode::Or(children) => {
                stack.extend(children.iter().map(|child| (child, depth + 1)));
                continue;
            }
            FilterNode::Not(inner) => {
                stack.push((inner, depth + 1));
                continue;
            }
            FilterNode::Leaf(leaf) => leaf,
        };
        leaves += 1;
        if leaves > MAX_LEAVES {
            return Err(EngineError::Invalid(format!(
                "filter has more than {MAX_LEAVES} conditions"
            )));
        }
        match (leaf.op, &leaf.value) {
            (Op::In | Op::NotIn, Value::Array(values)) if values.len() > MAX_IN => {
                return Err(EngineError::Invalid(format!(
                    "filter on {}: more than {MAX_IN} values in one list",
                    leaf.column
                )));
            }
            (Op::Regex, Value::String(pattern)) if pattern.chars().count() > MAX_REGEX => {
                return Err(EngineError::Invalid(format!(
                    "filter on {}: regex longer than {MAX_REGEX} characters",
                    leaf.column
                )));
            }
            _ => {}
        }
    }
    Ok(())
}
