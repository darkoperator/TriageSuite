//! How many rows in the case's views carry each mark, for the counts the
//! workbench keeps in sight.

use super::{locate_rows, Standing};
use crate::case::Case;
use crate::error::Result;
use crate::query::MarkState;
use duckdb::params;

/// Marked rows that are in their relation's view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MarkCounts {
    pub suspicious: u64,
}

/// The rows marked suspicious that are in their relation's view: a mark
/// left behind on a superseded row, or on a removed source's, is not
/// counted (`marks` still lists it, with `active` false).
pub fn mark_counts(case: &Case) -> Result<MarkCounts> {
    let conn = case.conn();
    let marked: Vec<i64> = conn
        .prepare("SELECT DISTINCT row_id FROM ox.row_mark WHERE mark = ?")?
        .query_map(params![MarkState::Suspicious.as_str()], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let located = locate_rows(conn, &marked)?;
    let active = located
        .values()
        .filter(|row| row.standing == Standing::Active)
        .count();
    Ok(MarkCounts {
        suspicious: u64::try_from(active).unwrap_or(u64::MAX),
    })
}
