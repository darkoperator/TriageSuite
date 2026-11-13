//! The grid's page as the app shows it: the plain page's rows and cells,
//! each UTC time also rendered in the display zone, which colour rule each
//! row matches, and in find mode which rows the search selects.
//!
//! The front end does no date arithmetic, so the text it shows for a time
//! is `GridCell::display`, made here: the cell's own text, which the source
//! wrote, converted by `zone::render_utc_text` with its fraction kept as
//! written. Only a `utc` timestamp column is converted; an `unknown` or
//! `local` one is shown as written, never guessed.

use super::rules::{check_rules, compile_rule, rule_columns, ColourRule};
use super::{GridState, SearchMode};
use crate::case::Case;
use crate::catalog::{ColumnInfo, ColumnRole, SqlType, TimeSemantics, TypeOrigin};
use crate::error::{EngineError, Result};
use crate::filter::{failed_sql, null_safe};
use crate::listing::check_limit;
use crate::query::{
    check_offset, check_zone, clip_text, mark_sql, order_sql, read_mark, scope, search_fragment,
    MarkState, CELL_CAP, ISO_FORMAT, ROW_ALIAS,
};
use crate::search::SearchOptions;
use duckdb::params_from_iter;
use duckdb::types::Value as SqlValue;
use std::collections::HashMap;
use triage_core::output::duckdb::sql::quote_ident;
use triage_core::timestamp::zone;

/// One column of a page, as the catalog describes it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ColumnMeta {
    pub name: String,
    pub role: ColumnRole,
    #[cfg_attr(feature = "ts", ts(as = "Option<crate::ts::SqlTypeTs>"))]
    pub sql_type: Option<SqlType>,
    /// How a timestamp is zoned: only `utc` cells are rendered in the zone;
    /// the app badges the others `?tz`.
    #[cfg_attr(feature = "ts", ts(as = "Option<crate::ts::TimeSemanticsTs>"))]
    pub time_semantics: Option<TimeSemantics>,
    pub origin: Option<TypeOrigin>,
}

impl From<&ColumnInfo> for ColumnMeta {
    fn from(c: &ColumnInfo) -> ColumnMeta {
        ColumnMeta {
            name: c.name.clone(),
            role: c.role,
            sql_type: c.sql_type,
            time_semantics: c.time_semantics,
            origin: c.origin,
        }
    }
}

/// One cell of a page.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GridCell {
    /// The source's text, cut to `CELL_CAP` characters.
    pub text: Option<String>,
    /// Only for a `utc` timestamp column whose text converted: the instant
    /// in the page's zone, `YYYY-MM-DD HH:MM:SS` and the fraction exactly as
    /// the text wrote it, no zone suffix.
    pub display: Option<String>,
    /// The column is typed and this text did not convert.
    pub failed: bool,
    /// The text was longer than `CELL_CAP` characters and was cut.
    pub clipped: bool,
}

fn utc() -> String {
    "UTC".to_string()
}

/// How the app wants a page shown. Arrives as JSON from the app: unknown
/// fields are refused.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PageOptions {
    /// The display zone, an IANA name; `UTC` when left out.
    #[serde(default = "utc")]
    pub zone: String,
    /// The relation's colour rules, in order; the first a row matches wins.
    #[serde(default)]
    pub rules: Vec<ColourRule>,
}

/// One page of the grid.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GridPage {
    pub columns: Vec<ColumnMeta>,
    pub row_ids: Vec<i64>,
    pub marks: Vec<Option<MarkState>>,
    /// One row per row id, one cell per column.
    pub cells: Vec<Vec<GridCell>>,
    /// Per row: the index into the options' rules of the first enabled rule
    /// the row matches.
    pub row_rule: Vec<Option<u16>>,
    /// Per rule: the columns its conditions name, which a cell-scope rule
    /// tints.
    pub rule_columns: Vec<Vec<String>>,
    /// Enabled rules that were skipped, by index, and why (a column the
    /// relation lacks, a regex DuckDB refuses).
    pub rule_errors: Vec<(u16, String)>,
    /// Per row, in find mode: the search selects the row. Otherwise all
    /// false (in filter mode the search has already narrowed the rows).
    pub search_hits: Vec<bool>,
    /// More rows exist past this page.
    pub truncated: bool,
    /// The zone `display` is in.
    pub zone: String,
}

/// Up to `limit` rows of what `state` shows, from `offset`, in its order,
/// as `page` reads them, rendered for the app in `opts.zone` with
/// `opts.rules` matched.
///
/// `Invalid` for a bad state, a `limit` of 0 or over `MAX_LIMIT`, an
/// `offset` past `i64::MAX`, an unknown zone, bad rules (`check_rules`), or a
/// find-mode search DuckDB refuses. A rule that cannot be compiled against
/// the relation is reported in `rule_errors`, not refused.
///
/// Two statements, as `page`: the first keeps the page's row ids, the
/// second reads their cells and marks, which rule each matches (one `CASE`
/// over the enabled rules in order) and whether the search selects it.
pub fn page_view(
    case: &Case,
    state: &GridState,
    limit: u32,
    offset: u64,
    opts: &PageOptions,
) -> Result<GridPage> {
    check_limit(limit)?;
    check_offset(offset)?;
    check_zone(&opts.zone)?;
    check_rules(&opts.rules)?;
    let q = state.query()?;
    let scope = scope(case, &q)?;
    let order = order_sql(&scope, &q)?;
    let conn = case.conn();

    // The rules, compiled against the relation: one `WHEN` per enabled rule
    // that compiles, carrying its index into `opts.rules`.
    let mut whens = Vec::new();
    let mut rule_params: Vec<SqlValue> = Vec::new();
    let mut rule_errors = Vec::new();
    for (i, rule) in opts.rules.iter().enumerate() {
        if !rule.enabled {
            continue;
        }
        // `check_rules` capped the list well under u16::MAX.
        let index = u16::try_from(i).unwrap_or(u16::MAX);
        match compile_rule(conn, rule, &scope.all) {
            Ok(fragment) => {
                whens.push(format!("WHEN {} THEN {index}", null_safe(&fragment.sql)));
                rule_params.extend(fragment.params);
            }
            Err(e) => rule_errors.push((index, e.to_string())),
        }
    }
    let rule_sql = if whens.is_empty() {
        "CAST(NULL AS INTEGER)".to_string()
    } else {
        format!("CASE {} END", whens.join(" "))
    };
    let rule_columns = opts
        .rules
        .iter()
        .map(|r| rule_columns(&r.filter, &scope.all))
        .collect();

    // In find mode the search does not narrow the rows; it marks them.
    let search = match state.search.mode {
        SearchMode::Find => {
            let search_opts = SearchOptions {
                regex: state.search.regex,
                case_sensitive: state.search.case_sensitive,
            };
            search_fragment(conn, &state.search.text, search_opts, &scope.all)?
        }
        SearchMode::Filter => None,
    };

    // Fetching one more than asked says whether the page is the last one.
    let sql = format!(
        "SELECT {ROW_ALIAS}._ox_row FROM {} WHERE {} ORDER BY {order} LIMIT ? OFFSET ?",
        scope.from_sql, scope.where_sql,
    );
    let mut params = scope.params.clone();
    params.push(SqlValue::BigInt(i64::from(limit) + 1));
    params.push(SqlValue::UBigInt(offset));
    let mut row_ids: Vec<i64> = conn
        .prepare(&sql)?
        .query_map(params_from_iter(params), |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let truncated = row_ids.len() > limit as usize;
    row_ids.truncate(limit as usize);

    let mut out = GridPage {
        columns: scope.columns.iter().map(ColumnMeta::from).collect(),
        marks: Vec::with_capacity(row_ids.len()),
        cells: Vec::with_capacity(row_ids.len()),
        row_rule: Vec::with_capacity(row_ids.len()),
        rule_columns,
        rule_errors,
        search_hits: Vec::with_capacity(row_ids.len()),
        truncated,
        zone: opts.zone.clone(),
        row_ids,
    };
    if out.row_ids.is_empty() {
        return Ok(out);
    }

    // Per column: the text cut a character past the cap, whether it failed
    // to convert, and for a UTC time its typed value as R1 text, which says
    // it converted and stands in when the text is not in the UTC form.
    let mut select = vec![format!("{ROW_ALIAS}._ox_row"), mark_sql()];
    let mut renders = Vec::with_capacity(scope.columns.len());
    for c in &scope.columns {
        let text = format!("{ROW_ALIAS}.{}", quote_ident(&c.text_column));
        select.push(format!("left(CAST({text} AS VARCHAR), {})", CELL_CAP + 1));
        let typed = c
            .typed_column
            .as_ref()
            .map(|t| format!("{ROW_ALIAS}.{}", quote_ident(t)));
        select.push(match &typed {
            Some(typed) => failed_sql(&text, typed),
            None => "FALSE".into(),
        });
        let render =
            c.sql_type == Some(SqlType::Timestamp) && c.time_semantics == Some(TimeSemantics::Utc);
        select.push(match (&typed, render) {
            (Some(typed), true) => format!("strftime({typed}, '{ISO_FORMAT}')"),
            _ => "CAST(NULL AS VARCHAR)".into(),
        });
        renders.push(render);
    }
    let hit_sql = match &search {
        Some(fragment) => null_safe(&fragment.sql),
        None => "FALSE".into(),
    };
    // The `?`s come in the text's order: the rules', the search's, the ids.
    let sql = format!(
        "SELECT {}, {rule_sql}, {hit_sql} FROM {} WHERE {ROW_ALIAS}._ox_row IN ({})",
        select.join(", "),
        scope.from_sql,
        vec!["?"; out.row_ids.len()].join(", "),
    );
    let mut params = rule_params;
    if let Some(fragment) = search {
        params.extend(fragment.params);
    }
    params.extend(out.row_ids.iter().map(|id| SqlValue::BigInt(*id)));

    let width = scope.columns.len();
    let (rule_at, hit_at) = (2 + 3 * width, 3 + 3 * width);
    type Read = (Option<MarkState>, Vec<GridCell>, Option<u16>, bool);
    let mut read: HashMap<i64, Read> = HashMap::with_capacity(out.row_ids.len());
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;
    while let Some(row) = rows.next()? {
        let mut cells = Vec::with_capacity(width);
        for (i, render) in renders.iter().enumerate() {
            let text: Option<String> = row.get(2 + 3 * i)?;
            let failed: bool = row.get(3 + 3 * i)?;
            let typed: Option<String> = row.get(4 + 3 * i)?;
            let display = match typed {
                Some(typed) if *render => {
                    render_time(text.as_deref(), &typed, &opts.zone).map(|r| r.local)
                }
                _ => None,
            };
            let (text, clipped) = clip_text(text);
            cells.push(GridCell {
                text,
                display,
                failed,
                clipped,
            });
        }
        let rule: Option<i64> = row.get(rule_at)?;
        let rule = rule.and_then(|r| u16::try_from(r).ok());
        read.insert(
            row.get(0)?,
            (read_mark(row.get(1)?)?, cells, rule, row.get(hit_at)?),
        );
    }
    for id in &out.row_ids {
        // Both statements read the same rows of one connection's snapshot.
        let (mark, cells, rule, hit) = read.remove(id).ok_or_else(|| {
            EngineError::Internal(format!("row {id} was paged but could not be read back"))
        })?;
        out.marks.push(mark);
        out.cells.push(cells);
        out.row_rule.push(rule);
        out.search_hits.push(hit);
    }
    Ok(out)
}

/// A converted UTC cell in `zone`: its text when that is UTC text, so the
/// fraction stays as written; else its typed value (`typed`, R1), when the
/// text converted from another form (a column declared UTC whose text has
/// no `Z`). The grid shows `local`; the row detail adds `offset`.
pub(super) fn render_time(text: Option<&str>, typed: &str, zone: &str) -> Option<zone::Rendered> {
    text.and_then(|t| zone::render_utc_text(t, zone))
        .or_else(|| zone::render_utc_text(typed, zone))
}
