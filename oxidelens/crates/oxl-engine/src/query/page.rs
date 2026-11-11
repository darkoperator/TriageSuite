//! Pages of cells and row counts: what the grid shows.

use super::{order_sql, scope, GridQuery, ROW_ALIAS};
use crate::case::Case;
use crate::error::{EngineError, Result};
use crate::listing::check_limit;
use duckdb::params_from_iter;
use duckdb::types::Value as SqlValue;
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
/// `limit` is 0 or over `MAX_LIMIT`.
pub fn page(case: &Case, q: &GridQuery, limit: u32, offset: u64) -> Result<Page> {
    check_limit(limit)?;
    let scope = scope(case, q)?;
    let order = order_sql(case, &scope, q)?;

    // Fetching one more than asked says whether the page is the last one.
    // Each text is cut a character past the cap in SQL, so a huge cell is
    // not carried whole only to be clipped here.
    let mut select = vec![format!("{ROW_ALIAS}._ox_row"), mark_sql()];
    for c in &scope.columns {
        let text = format!("{ROW_ALIAS}.{}", quote_ident(&c.text_column));
        select.push(format!("left(CAST({text} AS VARCHAR), {})", CELL_CAP + 1));
        select.push(match &c.typed_column {
            Some(typed) => format!(
                "({ROW_ALIAS}.{} IS NULL AND coalesce(CAST({text} AS VARCHAR), '') <> '')",
                quote_ident(typed)
            ),
            None => "FALSE".into(),
        });
    }
    let sql = format!(
        "SELECT {} FROM {} WHERE {} ORDER BY {order} LIMIT ? OFFSET ?",
        select.join(", "),
        scope.from_sql,
        scope.where_sql,
    );
    let mut params = scope.params.clone();
    params.push(SqlValue::BigInt(i64::from(limit) + 1));
    params.push(SqlValue::UBigInt(offset));

    let width = scope.columns.len();
    let mut out = Page {
        columns: scope.columns.iter().map(|c| c.name.clone()).collect(),
        row_ids: Vec::new(),
        marks: Vec::new(),
        cells: Vec::new(),
        truncated: false,
    };
    let mut stmt = case.conn().prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(params))?;
    while let Some(row) = rows.next()? {
        if out.row_ids.len() == limit as usize {
            out.truncated = true;
            break;
        }
        out.row_ids.push(row.get(0)?);
        out.marks.push(read_mark(row.get(1)?)?);
        let mut cells = Vec::with_capacity(width);
        for i in 0..width {
            let text: Option<String> = row.get(2 + 2 * i)?;
            let failed: bool = row.get(3 + 2 * i)?;
            cells.push(clip(text, failed));
        }
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

/// The value `mark_sql` produced, decoded; `Invalid` for a mark the engine
/// does not know.
pub(crate) fn read_mark(text: Option<String>) -> Result<Option<MarkState>> {
    text.map(|text| {
        MarkState::parse(&text)
            .ok_or_else(|| EngineError::Invalid(format!("the case holds an unknown mark {text:?}")))
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
pub(super) fn clip_text(text: Option<String>) -> (Option<String>, bool) {
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
