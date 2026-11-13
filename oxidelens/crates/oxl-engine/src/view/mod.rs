//! The grid's state as the app holds it: filter chips, the search box, the
//! host scope, the sort and the shown columns.
//!
//! The app sends a `GridState` with every grid request, so its JSON shape
//! is a contract and arrives untrusted: unknown fields are refused, chip ids
//! and labels are checked, and the chips are capped. `GridState::query`
//! compiles it to the `GridQuery` every page, count, group and histogram
//! already takes, so the grid's state reaches SQL through one path: the
//! enabled chips' filters ANDed, every value bound, every column found in
//! the catalog.
//!
//! The search box has two modes. In `filter` mode its text narrows the rows
//! like a chip. In `find` mode it does not: every row the chips select stays
//! in the grid, and `matches` gives the positions of those the search
//! selects, in the grid's order, for the app to step between.

mod export;
mod page;
mod row;
mod rules;
mod saved;
mod split;
mod time;
mod values;

pub use export::{
    check_export_target, create_export_partial, export_csv, export_csv_guarded, ExportReport,
};
pub use page::{page_view, ColumnMeta, GridCell, GridPage, PageOptions};
pub use row::{
    around_totals, cell_value, merged_view, row_view, CellValue, FieldView, MergedView, Provenance,
    RowView, WindowTotal, MAX_PIVOT_WINDOWS, MAX_WINDOW_SECONDS,
};
pub use rules::{
    default_colour_rules, ColourRule, RuleColour, RuleScope, MAX_RULES, MAX_RULE_NAME,
};
pub use saved::{
    apply_view, delete_view, export_view, import_view, save_view, views, views_matching,
    AppliedView, ColumnLayout, MatchingView, SavedView, ViewDoc, ViewKind, ViewWarning,
    MAX_COLUMN_WIDTH, MAX_VIEW_BYTES, MAX_VIEW_FILE_BYTES, MAX_VIEW_NAME, MAX_VIEW_PATTERN,
    MIN_COLUMN_WIDTH,
};
pub use split::{split_value, ValuePart, MAX_PARTS};
pub use time::{date_range_chip, groups_view, histogram_view, GroupView};
pub use values::{values, ValueCount, ValueOrder};

use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::filter::{check_caps, failed_sql, FilterNode, SqlFragment};
use crate::listing::{check_limit, fetch_limit, Listing};
use crate::query::{
    as_count, check_offset, order_sql, scope, search_fragment, GridQuery, Scope, SortKey, ROW_ALIAS,
};
use crate::search::SearchOptions;
use duckdb::params_from_iter;
use duckdb::types::Value as SqlValue;
use std::collections::BTreeSet;
use triage_core::output::duckdb::sql::quote_ident;

/// Chips one state may carry, enabled or not.
pub const MAX_CHIPS: usize = 50;
/// Characters in a chip id, which are ASCII letters, digits, `_` and `-`.
pub const MAX_CHIP_ID: usize = 64;
/// Characters in a chip label.
pub const MAX_CHIP_LABEL: usize = 200;

/// What one grid shows.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct GridState {
    pub relation: String,
    #[serde(default)]
    pub chips: Vec<Chip>,
    #[serde(default)]
    pub search: SearchState,
    /// Empty means every host.
    #[serde(default)]
    pub hosts: Vec<String>,
    /// Empty means the primary time ascending when there is one.
    #[serde(default)]
    pub sort: Vec<SortKey>,
    /// `None` means every catalogued column.
    #[serde(default)]
    pub columns: Option<Vec<String>>,
}

/// One filter chip. A disabled chip is kept, shown dimmed, and not applied.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Chip {
    /// 1 to `MAX_CHIP_ID` of `[A-Za-z0-9_-]`, unique within the state.
    pub id: String,
    /// At most `MAX_CHIP_LABEL` characters; display text, never parsed.
    pub label: String,
    pub enabled: bool,
    pub origin: ChipOrigin,
    pub filter: FilterNode,
}

/// The part of the grid a chip was made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ChipOrigin {
    FilterRow,
    Text,
    Values,
    DateRange,
    Cell,
    Histogram,
    Advanced,
    Group,
    Preset,
}

/// The search box: its text in Timeline Explorer's syntax (`search`), the
/// `Rx` and `Aa` toggles, and the filter | find switch.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(default, deny_unknown_fields)]
pub struct SearchState {
    pub text: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub mode: SearchMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    /// Hide the rows the search does not select.
    #[default]
    Filter,
    /// Keep every row; step between the ones the search selects.
    Find,
}

impl GridState {
    /// The `GridQuery` this state asks for: the enabled chips' filters
    /// ANDed (`None` when no chip is enabled), the search only in filter
    /// mode, and the hosts, sort and columns as they are. `Invalid` for more
    /// than `MAX_CHIPS` chips, a bad or repeated chip id, a label over
    /// `MAX_CHIP_LABEL` characters, any chip's filter (switched on or off)
    /// over the caps or with a value of the wrong shape, or a combined filter
    /// over the depth or condition caps. Columns and hosts are checked by the query that runs
    /// it.
    pub fn query(&self) -> Result<GridQuery> {
        self.compile(&[])
    }

    /// `query`, leaving out the chips whose ids are in `except`, as the
    /// column filter panel counts a column's values under the other chips.
    /// The whole state is checked, the excepted chips too. An id no chip has
    /// leaves out nothing.
    pub(crate) fn compile(&self, except: &[String]) -> Result<GridQuery> {
        self.check()?;
        let filters: Vec<FilterNode> = self
            .chips
            .iter()
            .filter(|c| c.enabled && !except.contains(&c.id))
            .map(|c| c.filter.clone())
            .collect();
        // Always one `and` around the chips, even one, so that the caps
        // apply to what is compiled: a chip at the depth cap is over it here.
        let filter = (!filters.is_empty()).then(|| FilterNode::And(filters));
        if let Some(node) = &filter {
            check_caps(node)?;
        }
        let search = match self.search.mode {
            SearchMode::Filter => Some(self.search.text.clone()),
            SearchMode::Find => None,
        };
        Ok(GridQuery {
            relation: self.relation.clone(),
            filter,
            search,
            search_regex: self.search.regex,
            search_case_sensitive: self.search.case_sensitive,
            hosts: self.hosts.clone(),
            sort: self.sort.clone(),
            columns: self.columns.clone(),
        })
    }

    fn check(&self) -> Result<()> {
        if self.chips.len() > MAX_CHIPS {
            return Err(EngineError::Invalid(format!(
                "a grid has at most {MAX_CHIPS} chips, not {}",
                self.chips.len()
            )));
        }
        let mut seen = BTreeSet::new();
        for chip in &self.chips {
            let id = &chip.id;
            if !valid_id(id) {
                return Err(EngineError::Invalid(format!(
                    "chip id {} is not 1 to {MAX_CHIP_ID} of letters, digits, '_' and '-'",
                    shown(id)
                )));
            }
            if chip.label.chars().count() > MAX_CHIP_LABEL {
                return Err(EngineError::Invalid(format!(
                    "chip {id}: a label is at most {MAX_CHIP_LABEL} characters"
                )));
            }
            if !seen.insert(id.as_str()) {
                return Err(EngineError::Invalid(format!(
                    "chip id {id} is used by more than one chip"
                )));
            }
            // Every chip's filter on its own, switched on or off: depth,
            // conditions and value shapes. A switched-off chip is kept and
            // may be switched on, saved or exported, so it is held to the
            // same rules as one that is applied.
            check_caps(&chip.filter)
                .map_err(|e| EngineError::Invalid(format!("chip {id}: {e}")))?;
        }
        Ok(())
    }
}

/// The id rule chips and colour rules share: 1 to `MAX_CHIP_ID` of
/// `[A-Za-z0-9_-]`.
fn valid_id(id: &str) -> bool {
    (1..=MAX_CHIP_ID).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Untrusted text for a message: quoted with its control characters
/// escaped, and cut to `MAX_CHIP_ID` characters.
fn shown(text: &str) -> String {
    let cut: String = text.chars().take(MAX_CHIP_ID).collect();
    if cut.len() < text.len() {
        format!("{cut:?}…")
    } else {
        format!("{cut:?}")
    }
}

/// The grid's status-bar counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GridCounts {
    /// Rows the state selects.
    pub rows: u64,
    /// Rows of the relation, nothing narrowing them.
    pub total: u64,
    /// Cells among `rows`, in every typed column shown or not, whose text
    /// is there but did not convert.
    pub failed_values: u64,
}

/// How many rows `state` selects, of how many, and how many typed cells
/// among them did not convert. One statement.
pub fn count_view(case: &Case, state: &GridState) -> Result<GridCounts> {
    let q = state.query()?;
    let scope = scope(case, &q)?;
    let failed: Vec<String> = scope
        .all
        .iter()
        .filter_map(|c| {
            let typed = c.typed_column.as_ref()?;
            let text = format!("{ROW_ALIAS}.{}", quote_ident(&c.text_column));
            let typed = format!("{ROW_ALIAS}.{}", quote_ident(typed));
            Some(format!("CAST({} AS BIGINT)", failed_sql(&text, &typed)))
        })
        .collect();
    let failed = if failed.is_empty() {
        "CAST(0 AS BIGINT)".to_string()
    } else {
        format!("CAST(coalesce(sum({}), 0) AS BIGINT)", failed.join(" + "))
    };
    // The total is a scalar subquery of its own over the same projection,
    // not a second FROM item beside it.
    let sql = format!(
        "SELECT count(*), {failed}, (SELECT count(*) FROM {from}) FROM {from} WHERE {where_sql}",
        from = scope.from_sql,
        where_sql = scope.where_sql,
    );
    let (rows, failed, total): (i64, i64, i64) =
        case.conn()
            .query_row(&sql, params_from_iter(&scope.params), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
    Ok(GridCounts {
        rows: as_count(rows),
        total: as_count(total),
        failed_values: as_count(failed),
    })
}

/// The catalogued columns, in display order, shown or not, with no
/// non-empty text among the rows `state` selects: what *Hide empty* hides.
/// Over no rows that is every column.
pub fn empty_columns(case: &Case, state: &GridState) -> Result<Vec<String>> {
    let q = state.query()?;
    let scope = scope(case, &q)?;
    if scope.all.is_empty() {
        return Ok(Vec::new());
    }
    let select: Vec<String> = scope
        .all
        .iter()
        .map(|c| {
            format!(
                "count(NULLIF(CAST({ROW_ALIAS}.{} AS VARCHAR), ''))",
                quote_ident(&c.text_column)
            )
        })
        .collect();
    let sql = format!(
        "SELECT {} FROM {} WHERE {}",
        select.join(", "),
        scope.from_sql,
        scope.where_sql
    );
    let filled: Vec<i64> = case
        .conn()
        .query_row(&sql, params_from_iter(&scope.params), |r| {
            (0..scope.all.len()).map(|i| r.get(i)).collect()
        })?;
    Ok(scope
        .all
        .iter()
        .zip(filled)
        .filter(|(_, n)| *n == 0)
        .map(|(c, _)| c.name.clone())
        .collect())
}

/// One row the search finds: its place in the grid's order (the offset a
/// page starting at it takes), its id and its rank among all the matches
/// (the first match is 1), so a step can say "k of n".
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MatchPos {
    pub offset: u64,
    pub row_id: i64,
    pub ordinal: u64,
}

/// Up to `limit` positions of the rows the search selects among the rows the
/// chips and hosts select, in the state's order. Forward (`backward` false):
/// the matches after `from` (all when `None`), nearest first, ascending.
/// Backward: the matches before `from` (all when `None`), nearest first,
/// descending, so `None` starts at the last match. `Invalid` unless the
/// search is in find mode, for a `limit` of 0 or over `MAX_LIMIT`, or a
/// `from` past `i64::MAX`. A blank search finds nothing.
pub fn matches(
    case: &Case,
    state: &GridState,
    from: Option<u64>,
    backward: bool,
    limit: u32,
) -> Result<Listing<MatchPos>> {
    check_limit(limit)?;
    if let Some(from) = from {
        check_offset(from)?;
    }
    let (q, scope, search) = find_scope(case, state)?;
    let Some(search) = search else {
        return Ok(Listing::from_fetched(Vec::new(), limit));
    };
    let order = order_sql(&scope, &q)?;
    // Every row in scope is numbered in the grid's order; the search is a
    // column of that numbering, not a narrowing of it, so the numbers are
    // the rows' places in the grid.
    let sql = format!(
        "SELECT ox_pos, ox_id, ox_ord FROM (\
           SELECT ox_pos, ox_id, ox_hit, \
                  sum(CASE WHEN ox_hit THEN 1 ELSE 0 END) OVER (ORDER BY ox_pos) AS ox_ord \
           FROM (\
             SELECT row_number() OVER (ORDER BY {order}) - 1 AS ox_pos, \
                    {ROW_ALIAS}._ox_row AS ox_id, ({hit}) AS ox_hit \
             FROM {from} WHERE {where_sql})) \
         WHERE ox_hit AND ox_pos {cmp} ? ORDER BY ox_pos {dir} LIMIT ?",
        cmp = if backward { "<" } else { ">" },
        dir = if backward { "DESC" } else { "ASC" },
        hit = search.sql,
        from = scope.from_sql,
        where_sql = scope.where_sql,
    );
    // The search's `?`s come first in the text, then the scope's.
    let mut params = search.params;
    params.extend(scope.params);
    // `check_offset` bounded `from`, so it fits; no `from` is before the
    // first row going forward and past the last going backward.
    let none = if backward { i64::MAX } else { -1 };
    params.push(SqlValue::BigInt(
        from.map_or(none, |a| i64::try_from(a).unwrap_or(i64::MAX)),
    ));
    params.push(SqlValue::BigInt(fetch_limit(limit)));
    let found: Vec<MatchPos> = case
        .conn()
        .prepare(&sql)?
        .query_map(params_from_iter(params), |r| {
            Ok(MatchPos {
                offset: as_count(r.get(0)?),
                row_id: r.get(1)?,
                ordinal: as_count(r.get(2)?),
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(Listing::from_fetched(found, limit))
}

/// The place of row `row_id` in what `state` shows: its offset in the state's
/// order (the numbering `page_view` and `matches` use), or `None` when the
/// chips, the search (in filter mode) or the hosts leave it out, or it is not
/// a row of the state's relation. In find mode the search does not narrow the
/// rows, so a row it does not match still has its place.
pub fn row_offset(case: &Case, state: &GridState, row_id: i64) -> Result<Option<u64>> {
    let q = state.query()?;
    let scope = scope(case, &q)?;
    let order = order_sql(&scope, &q)?;
    // Every row in scope is numbered in the grid's order, as `matches` does.
    let sql = format!(
        "SELECT ox_pos FROM (\
           SELECT row_number() OVER (ORDER BY {order}) - 1 AS ox_pos, \
                  {ROW_ALIAS}._ox_row AS ox_id \
           FROM {} WHERE {}) \
         WHERE ox_id = ?",
        scope.from_sql, scope.where_sql
    );
    let mut params = scope.params;
    params.push(SqlValue::BigInt(row_id));
    let found: Option<i64> = case
        .conn()
        .prepare(&sql)?
        .query_map(params_from_iter(params), |r| r.get(0))?
        .next()
        .transpose()?;
    Ok(found.map(as_count))
}

/// How many rows `matches` would list in all. `Invalid` unless the search
/// is in find mode; 0 for a blank search.
pub fn match_count(case: &Case, state: &GridState) -> Result<u64> {
    let (_, scope, search) = find_scope(case, state)?;
    let Some(search) = search else {
        return Ok(0);
    };
    let sql = format!(
        "SELECT count(*) FROM {} WHERE ({}) AND ({})",
        scope.from_sql, scope.where_sql, search.sql
    );
    let mut params = scope.params;
    params.extend(search.params);
    let n: i64 = case
        .conn()
        .query_row(&sql, params_from_iter(params), |r| r.get(0))?;
    Ok(as_count(n))
}

/// The rows a find runs over (the chips and hosts; in find mode `query`
/// leaves the search out) and the search as a fragment over them.
fn find_scope(case: &Case, state: &GridState) -> Result<(GridQuery, Scope, Option<SqlFragment>)> {
    if state.search.mode != SearchMode::Find {
        return Err(EngineError::Invalid(
            "matches are listed only with the search in find mode".to_string(),
        ));
    }
    let q = state.query()?;
    let scope = scope(case, &q)?;
    let opts = SearchOptions {
        regex: state.search.regex,
        case_sensitive: state.search.case_sensitive,
    };
    let search = search_fragment(case.conn(), &state.search.text, opts, &scope.all)?;
    Ok((q, scope, search))
}
