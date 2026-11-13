//! Pages of cells and row counts: what the grid shows.

use super::{check_offset, order_sql, scope, GridQuery, ROW_ALIAS};
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::filter::failed_sql;
use crate::listing::check_limit;
use duckdb::params_from_iter;
use duckdb::types::Value as SqlValue;
use std::collections::HashMap;
use triage_core::output::duckdb::sql::quote_ident;

pub const DEFAULT_LIMIT: u32 = 50;
pub const MAX_LIMIT: u32 = 1000;
/// Characters of one cell's text a page carries.
pub const CELL_CAP: usize = 2_000;

/// The suspicious mark as `ox.row_mark.mark` stores it, and as the `_mark`
/// filter spells it. Every reader and writer of a mark uses this one
/// constant, so the spellings cannot drift apart.
pub(crate) const MARK_SUSPICIOUS: &str = "suspicious";

/// A row's mark, as `ox.row_mark` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkState {
    Suspicious,
}

impl MarkState {
    pub(crate) fn parse(text: &str) -> Option<MarkState> {
        match text {
            MARK_SUSPICIOUS => Some(MarkState::Suspicious),
            _ => None,
        }
    }

    /// The spelling stored in `ox.row_mark.mark`.
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            MarkState::Suspicious => MARK_SUSPICIOUS,
        }
    }
}

/// One cell: the column's text as the source wrote it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Cell {
    pub text: Option<String>,
    /// The column is typed and this text did not convert.
    pub failed: bool,
    /// The text was longer than `CELL_CAP` characters and was cut.
    pub clipped: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Page {
    pub columns: Vec<String>,
    pub row_ids: Vec<i64>,
    pub marks: Vec<Option<MarkState>>,
    /// One row per row id, one cell per column.
    pub cells: Vec<Vec<Cell>>,
    /// More rows exist past this page.
    pub truncated: bool,
}

/// Up to `limit` rows of `q` from `offset`, in `q`'s order. `Invalid` when
/// `limit` is 0 or over `MAX_LIMIT`, or `offset` past `i64::MAX`.
///
/// Two statements: the first orders the rows in scope and keeps the page's
/// row ids, the second reads the cells and marks of those rows alone. Done
/// in one, every cell of every row in scope was cast, cut and looked up
/// before the page was kept, which on a real case was most of the time a
/// page took.
pub fn page(case: &Case, q: &GridQuery, limit: u32, offset: u64) -> Result<Page> {
    check_limit(limit)?;
    check_offset(offset)?;
    let scope = scope(case, q)?;
    let order = order_sql(&scope, q)?;
    let conn = case.conn();

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

    let mut out = Page {
        columns: scope.columns.iter().map(|c| c.name.clone()).collect(),
        marks: Vec::with_capacity(row_ids.len()),
        cells: Vec::with_capacity(row_ids.len()),
        row_ids,
        truncated,
    };
    if out.row_ids.is_empty() {
        return Ok(out);
    }

    // Each text is cut a character past the cap in SQL, so a huge cell is
    // not carried whole only to be clipped here.
    let mut select = vec![format!("{ROW_ALIAS}._ox_row"), mark_sql()];
    for c in &scope.columns {
        let text = format!("{ROW_ALIAS}.{}", quote_ident(&c.text_column));
        select.push(format!("left(CAST({text} AS VARCHAR), {})", CELL_CAP + 1));
        select.push(match &c.typed_column {
            Some(typed) => failed_sql(&text, &format!("{ROW_ALIAS}.{}", quote_ident(typed))),
            None => "FALSE".into(),
        });
    }
    let sql = format!(
        "SELECT {} FROM {} WHERE {ROW_ALIAS}._ox_row IN ({})",
        select.join(", "),
        scope.from_sql,
        vec!["?"; out.row_ids.len()].join(", "),
    );
    let width = scope.columns.len();
    let mut read: HashMap<i64, (Option<MarkState>, Vec<Cell>)> =
        HashMap::with_capacity(out.row_ids.len());
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(&out.row_ids))?;
    while let Some(row) = rows.next()? {
        let mut cells = Vec::with_capacity(width);
        for i in 0..width {
            let text: Option<String> = row.get(2 + 2 * i)?;
            let failed: bool = row.get(3 + 2 * i)?;
            cells.push(clip(text, failed));
        }
        read.insert(row.get(0)?, (read_mark(row.get(1)?)?, cells));
    }
    for id in &out.row_ids {
        // Both statements read the same rows of one connection's snapshot.
        let (mark, cells) = read.remove(id).ok_or_else(|| {
            EngineError::Internal(format!("row {id} was paged but could not be read back"))
        })?;
        out.marks.push(mark);
        out.cells.push(cells);
    }
    Ok(out)
}

/// How many rows `q` selects.
pub fn count(case: &Case, q: &GridQuery) -> Result<u64> {
    let scope = scope(case, q)?;
    let sql = format!(
        "SELECT count(*) FROM {} WHERE {}",
        scope.from_sql, scope.where_sql
    );
    let n: i64 = case
        .conn()
        .query_row(&sql, params_from_iter(&scope.params), |r| r.get(0))?;
    Ok(u64::try_from(n).unwrap_or_default())
}

/// SQL for the mark of the row in scope, as `ox.row_mark` spells it; NULL
/// for an unmarked row. Read it back with `read_mark`.
pub(super) fn mark_sql() -> String {
    format!(
        "(SELECT max(ox_pm.mark) FROM ox.row_mark AS ox_pm \
         WHERE ox_pm.row_id = {ROW_ALIAS}._ox_row)"
    )
}

/// The value `mark_sql` produced, decoded; `Internal` for a mark the engine
/// does not know, which only a damaged case holds.
pub(crate) fn read_mark(text: Option<String>) -> Result<Option<MarkState>> {
    text.map(|text| {
        MarkState::parse(&text).ok_or_else(|| {
            EngineError::Internal(format!("the case holds an unknown mark {text:?}"))
        })
    })
    .transpose()
}

fn clip(text: Option<String>, failed: bool) -> Cell {
    let (text, clipped) = clip_text(text);
    Cell {
        text,
        failed,
        clipped,
    }
}

/// `text` cut to its first `CELL_CAP` characters, and whether it was cut.
pub(crate) fn clip_text(text: Option<String>) -> (Option<String>, bool) {
    let Some(mut text) = text else {
        return (None, false);
    };
    match text.char_indices().nth(CELL_CAP) {
        Some((cut, _)) => {
            text.truncate(cut);
            (Some(text), true)
        }
        None => (Some(text), false),
    }
}
