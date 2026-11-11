//! One row in full: every catalogued column's whole text, and what the
//! case holds about it.
//!
//! The row is read from its own source table, not the relation's view, so
//! a superseded row still opens (reported `active: false`).

use super::notes::{notes_on, Note};
use super::{find_row, no_row, Standing};
use crate::case::Case;
use crate::catalog::{self, physical_columns, ColumnRole};
use crate::error::{EngineError, Result};
use crate::load::stage::source_schema;
use crate::query::{read_mark, Cell, MarkState};
use duckdb::params;
use triage_core::output::duckdb::sql::quote_ident;

#[derive(Debug, Clone, serde::Serialize)]
pub struct RowDetail {
    pub row_id: i64,
    pub relation: String,
    pub source_id: i64,
    pub host: Option<String>,
    /// `_ox_hash`: what supersession matches rows on.
    pub hash: String,
    /// The row is in its relation's view.
    pub active: bool,
    /// Every catalogued column, in display order, its text never clipped.
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

/// Row `row_id` in full. `NotFound` for an unknown id, and for a row of a
/// removed source, whose data has left the case.
pub fn row_detail(case: &Case, row_id: i64) -> Result<RowDetail> {
    let conn = case.conn();
    let row = find_row(conn, row_id)?.ok_or_else(|| EngineError::NotFound(no_row(row_id)))?;
    if row.standing == Standing::Removed {
        return Err(EngineError::NotFound(format!(
            "row {row_id} was in source {}, which was removed; its data is no longer in the case",
            row.source_id
        )));
    }
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
            Some(typed) => format!(
                "({} IS NULL AND coalesce({text}, '') <> '')",
                quote_ident(typed)
            ),
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
            Ok((r.get::<_, String>(0)?, fields))
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

    let (mark, marked_by) = conn
        .prepare(
            "SELECT mark, marked_by FROM ox.row_mark WHERE row_id = ? ORDER BY marked_at LIMIT 1",
        )?
        .query_map(params![row_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .next()
        .transpose()?
        .map_or((None, None), |(mark, by)| (Some(mark), Some(by)));
    let tags: Vec<String> = conn
        .prepare("SELECT tag FROM ox.row_tag WHERE row_id = ? ORDER BY tag")?
        .query_map(params![row_id], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;

    Ok(RowDetail {
        row_id,
        relation: row.relation,
        source_id: row.source_id,
        host: row.host,
        hash,
        active: row.standing == Standing::Active,
        fields,
        mark: read_mark(mark)?,
        marked_by,
        tags,
        notes: notes_on(conn, row_id)?,
        evidence_path,
        output_file,
    })
}
