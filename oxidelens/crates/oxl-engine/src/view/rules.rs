//! Colour rules: an ordered list of filters, each with a colour from a fixed
//! set, of which the first a row matches tints it.
//!
//! Rules arrive from the app with every page request and are untrusted like
//! chips: unknown fields are refused, ids and names are checked, the list
//! and each filter are capped. A rule is matched in SQL through the same
//! filter compiler as a chip, so its values are bound parameters and its
//! columns are found in the catalog. A rule naming a column the relation
//! lacks is not an error for the page: it is skipped and reported, and the
//! other rules still apply.

use super::{shown, valid_id, MAX_CHIP_ID};
use crate::case::Case;
use crate::catalog::{self, ColumnInfo};
use crate::error::{EngineError, Result};
use crate::filter::{check_caps, compile_filter, FilterNode, Leaf, Op, SqlFragment, MARK_COLUMN};
use crate::query::{probe_regex, regex_leaves};
use duckdb::Connection;
use serde_json::Value;
use std::collections::BTreeSet;

/// Rules one page may carry, enabled or not.
pub const MAX_RULES: usize = 50;
/// Characters in a rule's name.
pub const MAX_RULE_NAME: usize = 80;

/// The colours a rule may take: a fixed token set the app maps to fixed
/// classes, never a colour value from the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuleColour {
    Red,
    Orange,
    Amber,
    Green,
    Blue,
    Teal,
    Grey,
}

/// What a matching rule tints: the whole row, or only the cells of the
/// columns its conditions name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RuleScope {
    Row,
    Cell,
}

/// One colour rule, as the app sends it with a page request.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ColourRule {
    /// 1 to `MAX_CHIP_ID` of `[A-Za-z0-9_-]`, unique among the rules.
    pub id: String,
    /// At most `MAX_RULE_NAME` characters; display text, never parsed.
    pub name: String,
    /// A disabled rule is kept, and never matches.
    pub enabled: bool,
    pub filter: FilterNode,
    pub colour: RuleColour,
    pub scope: RuleScope,
}

/// Hayabusa's alert levels, as its CSV timeline writes them in the `Level`
/// column: abbreviated by default (`crit`, `high`, `med`, `low`, `info`) and
/// in full under some output profiles (`critical`, `high`, `medium`, `low`,
/// `informational`). Each entry is a default rule: its id suffix, the
/// spellings it matches without regard to case, and its colour, in the
/// spec's order (crit red, high orange, med amber, low green, info none).
const HAYABUSA_LEVELS: [(&str, &[&str], &str, RuleColour); 4] = [
    ("crit", &["crit", "critical"], "critical", RuleColour::Red),
    ("high", &["high"], "high", RuleColour::Orange),
    ("med", &["med", "medium"], "medium", RuleColour::Amber),
    ("low", &["low"], "low", RuleColour::Green),
];

/// The rules a relation starts with when the analyst has saved none. A
/// relation with catalogued `Level` and `RuleTitle` columns (Hayabusa's
/// timeline, whatever it was loaded as) gets one row rule per alert level,
/// most severe first, ids `hayabusa-<level>`; any other gets none.
/// `NotFound` for a relation the case lacks.
pub fn default_colour_rules(case: &Case, relation: &str) -> Result<Vec<ColourRule>> {
    let columns = catalog::columns(case, relation)?;
    let has = |name: &str| columns.iter().any(|c| c.name == name);
    if !(has("Level") && has("RuleTitle")) {
        return Ok(Vec::new());
    }
    Ok(HAYABUSA_LEVELS
        .iter()
        .map(|(id, spellings, name, colour)| ColourRule {
            id: format!("hayabusa-{id}"),
            name: format!("Hayabusa level: {name}"),
            enabled: true,
            filter: FilterNode::Leaf(Leaf {
                column: "Level".to_string(),
                op: Op::In,
                value: Value::Array(
                    spellings
                        .iter()
                        .map(|s| Value::String((*s).to_string()))
                        .collect(),
                ),
                case_sensitive: false,
            }),
            colour: *colour,
            scope: RuleScope::Row,
        })
        .collect())
}

/// `Invalid` for more than `MAX_RULES` rules, a bad or repeated id, a name
/// over `MAX_RULE_NAME` characters, or a filter over the depth or condition
/// caps. Columns are not checked here: a rule naming one the relation lacks
/// is skipped by the page, not refused.
pub(super) fn check_rules(rules: &[ColourRule]) -> Result<()> {
    if rules.len() > MAX_RULES {
        return Err(EngineError::Invalid(format!(
            "a page takes at most {MAX_RULES} colour rules, not {}",
            rules.len()
        )));
    }
    let mut seen = BTreeSet::new();
    for rule in rules {
        let id = &rule.id;
        if !valid_id(id) {
            return Err(EngineError::Invalid(format!(
                "colour rule id {} is not 1 to {MAX_CHIP_ID} of letters, digits, '_' and '-'",
                shown(id)
            )));
        }
        if !seen.insert(id.as_str()) {
            return Err(EngineError::Invalid(format!(
                "colour rule id {id} is used by more than one rule"
            )));
        }
        if rule.name.chars().count() > MAX_RULE_NAME {
            return Err(EngineError::Invalid(format!(
                "colour rule {id}: a name is at most {MAX_RULE_NAME} characters"
            )));
        }
        check_caps(&rule.filter)
            .map_err(|e| EngineError::Invalid(format!("colour rule {id}: {e}")))?;
    }
    Ok(())
}

/// `rule`'s filter over `all`, the relation's catalogued columns, as SQL to
/// place where the scope's projection is the only FROM item; or why it
/// cannot be: a column the relation lacks, a value of the wrong shape, a
/// regex DuckDB will not compile.
pub(super) fn compile_rule(
    conn: &Connection,
    rule: &ColourRule,
    all: &[ColumnInfo],
) -> Result<SqlFragment> {
    let fragment = compile_filter(&rule.filter, all)?;
    let mut patterns = Vec::new();
    regex_leaves(&rule.filter, &mut patterns);
    for (pattern, case_sensitive) in &patterns {
        probe_regex(conn, pattern, *case_sensitive)?;
    }
    Ok(fragment)
}

/// The catalogued columns `filter`'s conditions name, in the order they are
/// first named: the cells a cell-scope rule tints. The `_mark`
/// pseudo-column and columns the relation lacks are left out. The filter
/// has passed `check_caps`, which bounds this recursion.
pub(super) fn rule_columns(filter: &FilterNode, all: &[ColumnInfo]) -> Vec<String> {
    fn walk(node: &FilterNode, all: &[ColumnInfo], out: &mut Vec<String>) {
        match node {
            FilterNode::And(children) | FilterNode::Or(children) => {
                children.iter().for_each(|c| walk(c, all, out));
            }
            FilterNode::Not(inner) => walk(inner, all, out),
            FilterNode::Leaf(leaf) => {
                if leaf.column != MARK_COLUMN
                    && all.iter().any(|c| c.name == leaf.column)
                    && !out.contains(&leaf.column)
                {
                    out.push(leaf.column.clone());
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(filter, all, &mut out);
    out
}
