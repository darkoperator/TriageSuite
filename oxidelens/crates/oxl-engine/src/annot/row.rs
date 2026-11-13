//! One row in full: every catalogued column's whole text, and what the
//! case holds about it.
//!
//! The row is read from its own source table, not the relation's view, so
//! a superseded row still opens (reported `active: false`). A row of a
//! removed source opens too, with no fields: its data is gone, but the
//! mark, tags and notes left on it are what an analyst needs to see.

use super::marks::marks_on;
use super::notes::{notes_on, Note};
use super::{find_row, no_row, LocatedRow, Standing};
use crate::case::Case;
use crate::catalog::{self, physical_columns, ColumnRole};
use crate::error::{EngineError, Result};
use crate::filter::failed_sql;
use crate::load::stage::{host_column, source_schema};
use crate::query::{read_mark, Cell, MarkState};
use duckdb::{params, Connection};
use triage_core::output::duckdb::sql::quote_ident;

/// Where a loaded row stands in the case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RowStanding {
    /// In its relation's view.
    Active,
    /// A later run of its host replaced its dataset; the row is kept but is
    /// out of the view.
    Superseded,
    /// Its source was removed; its data is gone.
    Removed,
}

/// A row's standing, and for a superseded row the row in its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RowPlace {
    pub standing: RowStanding,
    /// For a superseded row, the row now in the view with the same content
    /// (hash and occurrence) for the same host; `None` for any other row,
    /// and for a superseded row no run in the view has. Supersession usually
    /// moved the row's annotations there, but not always: each supersession
    /// moves them only between its two runs, so a run in between that
    /// lacked the row left them on the copy before it (this row or a later
    /// superseded one), and a mark or tag the next copy already carried
    /// stays too. Each supersession's history entry lists what it moved (`from_row`,
    /// `to_row`) and what it left behind (`from_row`).
    pub current_row: Option<i64>,
}

/// Where row `row_id` stands; `None` for an id no source ever loaded.
pub fn row_place(case: &Case, row_id: i64) -> Result<Option<RowPlace>> {
    let conn = case.conn();
    let Some(row) = find_row(conn, row_id)? else {
        return Ok(None);
    };
    Ok(Some(place_of(conn, &row)?))
}

fn place_of(conn: &Connection, row: &LocatedRow) -> Result<RowPlace> {
    let standing = match row.standing {
        Standing::Active => RowStanding::Active,
        Standing::Superseded => RowStanding::Superseded,
        Standing::Removed => RowStanding::Removed,
    };
    let current_row = match standing {
        RowStanding::Superseded => replacement(conn, row)?,
        _ => None,
    };
    Ok(RowPlace {
        standing,
        current_row,
    })
}

/// The row in the view with superseded `row`'s content, matched as
/// supersession matches rows: same hash, occurrence and host.
fn replacement(conn: &Connection, row: &LocatedRow) -> Result<Option<i64>> {
    let (Some(host_column), Some(host)) = (host_column(conn, &row.relation)?, &row.host) else {
        return Ok(None);
    };
    let has_view: i64 = conn.query_row(
        "SELECT count(*) FROM duckdb_views() \
         WHERE database_name = current_database() AND schema_name = 'main' AND view_name = ?",
        params![row.relation],
        |r| r.get(0),
    )?;
    if has_view == 0 {
        return Ok(None);
    }
    let host_column = quote_ident(&host_column);
    Ok(conn
        .prepare(&format!(
            "SELECT v._ox_row FROM main.{view} v \
             JOIN {table} o ON o._ox_hash = v._ox_hash AND o._ox_occ = v._ox_occ \
             WHERE o._ox_row = ? AND CAST(v.{host_column} AS VARCHAR) = ? \
             ORDER BY v._ox_row LIMIT 1",
            view = quote_ident(&row.relation),
            table = row.table,
        ))?
        .query_map(params![row.row_id, host], |r| r.get(0))?
        .next()
        .transpose()?)
}

#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RowDetail {
    pub row_id: i64,
    /// Where the row stands; `None` only for an annotated row id no source
    /// range covers (see `relation`).
    pub standing: Option<RowStanding>,
    /// For a superseded row, the row in its place (`RowPlace`).
    pub current_row: Option<i64>,
    /// The relation the row fed. Empty only for an annotated row id that no
    /// `ox.source_table` range covers, which a case the engine wrote never
    /// holds (a removed source keeps its ranges).
    pub relation: String,
    /// The source the row came from; 0 (never a source id) in the same
    /// case as an empty `relation`.
    pub source_id: i64,
    /// `None` too for a removed source's row: its data is gone.
    pub host: Option<String>,
    /// `_ox_hash`, what supersession matches rows on; `None` for a removed
    /// source's row, whose hash is no longer known.
    pub hash: Option<String>,
    /// The row is in its relation's view.
    pub active: bool,
    /// Every catalogued column, in display order, its text never clipped;
    /// empty for a removed source's row.
    pub fields: Vec<(String, Cell)>,
    pub mark: Option<MarkState>,
    pub marked_by: Option<String>,
    pub tags: Vec<String>,
    /// The notes not deleted, oldest first.
    pub notes: Vec<Note>,
    /// The text of the relation's evidence path column.
    pub evidence_path: Option<String>,
    /// The text of the output-file metadata column.
    pub output_file: Option<String>,
}

/// Row `row_id` in full. A row of a removed source is reported inactive,
/// with no fields, hash, host or paths, and with its annotations. `NotFound`
/// for an id no source ever loaded and no annotation names.
pub fn row_detail(case: &Case, row_id: i64) -> Result<RowDetail> {
    let conn = case.conn();
    let row = find_row(conn, row_id)?;
    let data = match &row {
        Some(row) if row.standing != Standing::Removed => Some(read_data(case, row)?),
        _ => None,
    };
    let annotated: i64 = conn.query_row(
        "SELECT (SELECT count(*) FROM ox.row_mark WHERE row_id = ?) \
              + (SELECT count(*) FROM ox.row_tag WHERE row_id = ?) \
              + (SELECT count(*) FROM ox.note WHERE row_id = ?)",
        params![row_id, row_id, row_id],
        |r| r.get(0),
    )?;
    if row.is_none() && annotated == 0 {
        return Err(EngineError::NotFound(no_row(row_id)));
    }

    // The mark exactly as a mark or clear call reads it.
    let (mark, marked_by) = marks_on(conn, &[row_id])?
        .remove(&row_id)
        .map_or((None, None), |m| (Some(m.mark), Some(m.marked_by)));
    let tags: Vec<String> = conn
        .prepare("SELECT tag FROM ox.row_tag WHERE row_id = ? ORDER BY tag")?
        .query_map(params![row_id], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let data = data.unwrap_or_default();
    let place = row.as_ref().map(|r| place_of(conn, r)).transpose()?;
    let (relation, source_id, host, active) = match row {
        Some(row) => (
            row.relation,
            row.source_id,
            row.host,
            row.standing == Standing::Active,
        ),
        None => (String::new(), 0, None, false),
    };

    Ok(RowDetail {
        row_id,
        standing: place.map(|p| p.standing),
        current_row: place.and_then(|p| p.current_row),
        relation,
        source_id,
        host,
        hash: data.hash,
        active,
        fields: data.fields,
        mark: read_mark(mark)?,
        marked_by,
        tags,
        notes: notes_on(conn, row_id)?,
        evidence_path: data.evidence_path,
        output_file: data.output_file,
    })
}

/// What a row's own table says about it; all absent once its source is
/// removed.
#[derive(Default)]
struct RowData {
    hash: Option<String>,
    fields: Vec<(String, Cell)>,
    evidence_path: Option<String>,
    output_file: Option<String>,
}

/// Read `row` (of a source not removed) from its own table.
fn read_data(case: &Case, row: &LocatedRow) -> Result<RowData> {
    let conn = case.conn();
    let row_id = row.row_id;
    let relation = catalog::relation(case, &row.relation)?;
    let columns = catalog::columns(case, &row.relation)?;
    // A column only another source brought is absent from this table.
    let present = physical_columns(conn, &source_schema(row.source_id), &row.relation)?;
    let has = |name: &str| present.contains(&name.to_lowercase());

    let mut select = vec!["_ox_hash".to_string()];
    for c in &columns {
        let text = if has(&c.text_column) {
            format!("CAST({} AS VARCHAR)", quote_ident(&c.text_column))
        } else {
            "CAST(NULL AS VARCHAR)".into()
        };
        select.push(match c.typed_column.as_deref().filter(|t| has(t)) {
            Some(typed) => failed_sql(&text, &quote_ident(typed)),
            None => "FALSE".into(),
        });
        select.push(text);
    }
    let (hash, fields) = conn.query_row(
        &format!(
            "SELECT {} FROM {} WHERE _ox_row = ?",
            select.join(", "),
            row.table
        ),
        params![row_id],
        |r| {
            let mut fields = Vec::with_capacity(columns.len());
            for (i, c) in columns.iter().enumerate() {
                let failed: bool = r.get(1 + 2 * i)?;
                let text: Option<String> = r.get(2 + 2 * i)?;
                fields.push((
                    c.name.clone(),
                    Cell {
                        text,
                        failed,
                        clipped: false,
                    },
                ));
            }
            Ok((r.get::<_, Option<String>>(0)?, fields))
        },
    )?;

    let text_of = |name: Option<&str>| -> Option<String> {
        let name = name?;
        fields
            .iter()
            .find(|(k, _)| k == name)
            .and_then(|(_, cell)| cell.text.clone())
    };
    let evidence_path = text_of(relation.evidence_path_column.as_deref());
    let output_file = text_of(
        columns
            .iter()
            .find(|c| c.role == ColumnRole::OutputFile)
            .map(|c| c.name.as_str()),
    );
    Ok(RowData {
        hash,
        fields,
        evidence_path,
        output_file,
    })
}
