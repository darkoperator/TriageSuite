//! One row as the app's detail pane shows it, one cell's whole value, the
//! pivot totals around a row, and merged pages with their time in the
//! display zone.
//!
//! The row view is `row_detail` (every field unclipped, the mark, tags and
//! notes, where the row stands) with each field's catalog meta, each UTC
//! time rendered in the display zone with its offset by the grid's own
//! renderer (`page::render_time`), and the row's provenance: host, run,
//! evidence file and the record's place in it, output file, and whether the
//! run's manifest names that file.

use super::page::{render_time, ColumnMeta};
use super::split::{split_value, ValuePart};
use crate::annot::{find_row, locate, row_detail, RowDetail, RowStanding, Standing};
use crate::case::Case;
use crate::catalog::appmap::locator_for;
use crate::catalog::{self, physical_columns, ColumnInfo, SqlType, TimeSemantics};
use crate::error::{EngineError, Result};
use crate::listing::Listing;
use crate::load::stage::source_schema;
use crate::query::{
    anchor_of, around, check_zone, merged_rows, row_anchor, row_summary, MergedRow, MergedSource,
    RowAnchor, ISO_FORMAT,
};
use duckdb::{params, OptionalExt};
use std::collections::HashMap;
use triage_core::output::duckdb::sql::quote_ident;

/// The widest window `around_totals` counts: a day either side.
pub const MAX_WINDOW_SECONDS: u64 = 86_400;
/// Windows one `around_totals` call counts at most.
pub const MAX_PIVOT_WINDOWS: usize = 8;

/// One field of the row view.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FieldView {
    pub meta: ColumnMeta,
    /// The source's text, never clipped.
    pub text: Option<String>,
    /// Only for a `utc` timestamp column whose text converted: the instant
    /// in the view's zone, `YYYY-MM-DD HH:MM:SS`, the fraction as written,
    /// then a space and the offset (`-05:00`, or `Z` in UTC).
    pub display: Option<String>,
    /// The column is typed and this text did not convert.
    pub failed: bool,
}

/// Where a row came from.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Provenance {
    pub host: Option<String>,
    pub source_id: i64,
    /// The run's id; `None` for a loose file.
    pub run_id: Option<String>,
    /// The text of the relation's evidence path column.
    pub evidence_path: Option<String>,
    /// The `LOCATOR` column of the relation and its text in this row, when
    /// the relation has one and the row's cell is not empty: where the
    /// record is inside the evidence file.
    pub locator: Option<(String, String)>,
    /// The output file the row was read from.
    pub output_file: Option<String>,
    /// Whether the run's `run_manifest.json` names the output file under
    /// the row's host's tools (`output_paths`, internal and external). The
    /// file is matched by its path below the run root, separators
    /// normalised and letters compared without regard to case, as Windows
    /// paths are, so a run moved after it was written still matches. `None`
    /// for a loose file, a run without a manifest or with one that does not
    /// parse, and a row without an output file. The suite's manifest records
    /// paths, not hashes, so this says the run published the file, not that
    /// its content is unchanged.
    pub output_file_listed: Option<bool>,
}

/// The detail pane's row.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RowView {
    pub detail: RowDetail,
    /// The detail's fields in its order, each with its column's meta.
    pub fields: Vec<FieldView>,
    pub provenance: Provenance,
    /// The row's one-line summary, by the rule merged rows follow (the
    /// relation's `SUMMARY` columns, else its first plain values); `None`
    /// when it has none or the row's source was removed.
    pub summary: Option<String>,
    /// Where and when the row happened, as `MergedSource::Around` takes it
    /// (host and R1 UTC time); `None` for a row with no host or no usable
    /// primary time.
    pub anchor: Option<RowAnchor>,
    /// The zone `FieldView::display` is in.
    pub zone: String,
}

/// Row `row_id` for the detail pane, its UTC times in `zone`. `Invalid` for
/// an unknown zone; `NotFound` as `row_detail`. A row of a removed source
/// has no fields, as in `row_detail`.
pub fn row_view(case: &Case, row_id: i64, zone: &str) -> Result<RowView> {
    check_zone(zone)?;
    let detail = row_detail(case, row_id)?;
    let has_data = !detail.fields.is_empty() && detail.standing != Some(RowStanding::Removed);
    let columns = if has_data {
        catalog::columns(case, &detail.relation)?
    } else {
        Vec::new()
    };
    let typed = if has_data {
        typed_times(case, row_id, &columns)?
    } else {
        HashMap::new()
    };

    let mut fields = Vec::with_capacity(detail.fields.len());
    for (name, cell) in &detail.fields {
        let column = columns.iter().find(|c| c.name == *name).ok_or_else(|| {
            EngineError::Internal(format!(
                "row {row_id}: {} has no catalogued column {name}",
                detail.relation
            ))
        })?;
        let display = typed
            .get(name)
            .and_then(|t| render_time(cell.text.as_deref(), t, zone))
            .map(|r| format!("{} {}", r.local, r.offset));
        fields.push(FieldView {
            meta: ColumnMeta::from(column),
            text: cell.text.clone(),
            display,
            failed: cell.failed,
        });
    }

    let provenance = provenance(case, &detail)?;
    let summary = row_summary(case, row_id)?;
    let anchor = row_anchor(case, row_id)?;
    Ok(RowView {
        detail,
        fields,
        provenance,
        summary,
        anchor,
        zone: zone.to_string(),
    })
}

/// A UTC timestamp, which the zone converts.
fn is_utc_time(c: &ColumnInfo) -> bool {
    c.sql_type == Some(SqlType::Timestamp) && c.time_semantics == Some(TimeSemantics::Utc)
}

/// The typed value, as R1 text, of each UTC timestamp column of row
/// `row_id` that converted, by column name: what says a time converted, and
/// what is rendered when its text is not UTC text. One statement, against
/// the row's own table, as `row_detail` reads it.
fn typed_times(
    case: &Case,
    row_id: i64,
    columns: &[ColumnInfo],
) -> Result<HashMap<String, String>> {
    let conn = case.conn();
    let Some((source, relation, table)) = locate(conn, row_id)? else {
        return Ok(HashMap::new());
    };
    let present = physical_columns(conn, &source_schema(source), &relation)?;
    let wanted: Vec<(&str, &str)> = columns
        .iter()
        .filter(|c| is_utc_time(c))
        .filter_map(|c| {
            let typed = c.typed_column.as_deref()?;
            present
                .contains(&typed.to_lowercase())
                .then_some((c.name.as_str(), typed))
        })
        .collect();
    if wanted.is_empty() {
        return Ok(HashMap::new());
    }
    let select: Vec<String> = wanted
        .iter()
        .map(|(_, typed)| format!("strftime({}, '{ISO_FORMAT}')", quote_ident(typed)))
        .collect();
    let values: Option<Vec<Option<String>>> = conn
        .query_row(
            &format!(
                "SELECT {} FROM {table} WHERE _ox_row = ?",
                select.join(", ")
            ),
            params![row_id],
            |r| (0..wanted.len()).map(|i| r.get(i)).collect(),
        )
        .optional()?;
    Ok(wanted
        .iter()
        .zip(values.unwrap_or_default())
        .filter_map(|((name, _), value)| Some((name.to_string(), value?)))
        .collect())
}

fn provenance(case: &Case, detail: &RowDetail) -> Result<Provenance> {
    let source: Option<(Option<String>, String, String, Option<String>)> = case
        .conn()
        .query_row(
            "SELECT run_id, kind, path, manifest_json FROM ox.source WHERE id = ?",
            params![detail.source_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let locator = locator_for(&detail.relation).and_then(|column| {
        detail
            .fields
            .iter()
            .find(|(name, _)| name == column)
            .and_then(|(_, cell)| cell.text.clone())
            .filter(|text| !text.is_empty())
            .map(|text| (column.to_string(), text))
    });
    let (run_id, output_file_listed) = match &source {
        Some((run_id, kind, root, manifest)) if kind == "run" => {
            let listed = match (manifest, &detail.output_file) {
                (Some(manifest), Some(file)) => {
                    listed_in_manifest(manifest, root, detail.host.as_deref(), file)
                }
                _ => None,
            };
            (run_id.clone(), listed)
        }
        _ => (None, None),
    };
    Ok(Provenance {
        host: detail.host.clone(),
        source_id: detail.source_id,
        run_id,
        evidence_path: detail.evidence_path.clone(),
        locator,
        output_file: detail.output_file.clone(),
        output_file_listed,
    })
}

/// The part of a run manifest this reads: each host's tools' output paths.
/// Every other field is ignored, so the manifest's own additions do not
/// matter.
#[derive(serde::Deserialize)]
struct RunManifest {
    #[serde(default)]
    hosts: Vec<ManifestHost>,
}

#[derive(serde::Deserialize)]
struct ManifestHost {
    #[serde(default)]
    host: String,
    #[serde(default)]
    tools: Vec<ManifestTool>,
    #[serde(default)]
    external_tools: Vec<ManifestTool>,
}

#[derive(serde::Deserialize)]
struct ManifestTool {
    #[serde(default)]
    output_paths: Vec<String>,
}

/// Whether `manifest` (a run manifest's text) names `output_file` under
/// `host`'s tools, or any host's when the row has none. The file is taken
/// below `root`, the run root it was loaded from, and a manifest path names
/// it when it ends with those components. `None` when the manifest does not
/// parse.
fn listed_in_manifest(
    manifest: &str,
    root: &str,
    host: Option<&str>,
    output_file: &str,
) -> Option<bool> {
    let manifest: RunManifest = serde_json::from_str(manifest).ok()?;
    let file = components(output_file);
    let root = components(root);
    let wanted = match file.strip_prefix(root.as_slice()) {
        Some(below) if !below.is_empty() => below,
        _ => file.as_slice(),
    };
    if wanted.is_empty() {
        return Some(false);
    }
    let listed = manifest
        .hosts
        .iter()
        .filter(|h| match host {
            Some(host) => h.host.eq_ignore_ascii_case(host),
            None => true,
        })
        .flat_map(|h| h.tools.iter().chain(&h.external_tools))
        .flat_map(|t| &t.output_paths)
        .any(|path| components(path).ends_with(wanted));
    Some(listed)
}

/// A path's components, split at either separator, lower-cased; empty,
/// `.` and the `\\?\` prefix's `?` components dropped.
fn components(path: &str) -> Vec<String> {
    path.split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != "." && *c != "?")
        .map(str::to_ascii_lowercase)
        .collect()
}

/// One cell's whole value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CellValue {
    /// The source's text, never clipped.
    pub text: Option<String>,
    /// The text split into its fields (`split_value`); empty without text.
    pub parts: Vec<ValuePart>,
}

/// The whole text of `column` in row `row_id`, split. The row is read from
/// its own source table, so a superseded row's cell opens too. `NotFound`
/// for a row no source loaded, one whose source was removed, or a column
/// its relation does not catalogue.
pub fn cell_value(case: &Case, row_id: i64, column: &str) -> Result<CellValue> {
    let conn = case.conn();
    let row = find_row(conn, row_id)?
        .ok_or_else(|| EngineError::NotFound(format!("no row {row_id} in the case")))?;
    if row.standing == Standing::Removed {
        return Err(EngineError::NotFound(format!(
            "row {row_id}'s source {} was removed; its values are gone",
            row.source_id
        )));
    }
    let columns = catalog::columns(case, &row.relation)?;
    let info = columns.iter().find(|c| c.name == column).ok_or_else(|| {
        EngineError::NotFound(format!("{} has no column {column:?}", row.relation))
    })?;
    // A column only another source brought is absent from this table.
    let present = physical_columns(conn, &source_schema(row.source_id), &row.relation)?;
    let text: Option<String> = if present.contains(&info.text_column.to_lowercase()) {
        conn.query_row(
            &format!(
                "SELECT CAST({} AS VARCHAR) FROM {} WHERE _ox_row = ?",
                quote_ident(&info.text_column),
                row.table
            ),
            params![row_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten()
    } else {
        None
    };
    let parts = text.as_deref().map(split_value).unwrap_or_default();
    Ok(CellValue { text, parts })
}

/// Rows within one window around a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WindowTotal {
    /// The window, either side of the row's time.
    pub seconds: u64,
    /// `around`'s counts for the window, summed over every relation.
    pub rows: u64,
}

/// For each of `windows`, the rows on row `row_id`'s host within that many
/// seconds of its primary time, over every relation with a host and a time:
/// the live counts on the detail pane's ± pivot buttons. `Invalid` for more
/// than `MAX_PIVOT_WINDOWS` windows, one over `MAX_WINDOW_SECONDS`, and as
/// `anchor_of` for a row with no host or no usable primary time; `NotFound`
/// as `anchor_of`.
pub fn around_totals(case: &Case, row_id: i64, windows: &[u64]) -> Result<Vec<WindowTotal>> {
    if windows.len() > MAX_PIVOT_WINDOWS {
        return Err(EngineError::Invalid(format!(
            "at most {MAX_PIVOT_WINDOWS} windows are counted at once, not {}",
            windows.len()
        )));
    }
    if let Some(w) = windows.iter().find(|w| **w > MAX_WINDOW_SECONDS) {
        return Err(EngineError::Invalid(format!(
            "a pivot window is at most {MAX_WINDOW_SECONDS} seconds, not {w}"
        )));
    }
    let (host, at) = anchor_of(case, row_id)?;
    windows
        .iter()
        .map(|&seconds| {
            let rows = around(case, &host, &at, seconds)?
                .iter()
                .map(|c| c.count)
                .sum();
            Ok(WindowTotal { seconds, rows })
        })
        .collect()
}

/// One merged row with its time as the app shows it.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MergedView {
    pub row: MergedRow,
    /// For a row whose relation's primary time is `utc`: the time in the
    /// zone, as a grid cell shows it (`YYYY-MM-DD HH:MM:SS.ffffff`, no
    /// offset). `None` for an untimed row, and for an `unknown` or `local`
    /// time, which is shown as written.
    pub display_time: Option<String>,
}

/// `merged_rows`, each row's time also rendered in `zone`. `Invalid` for an
/// unknown zone and as `merged_rows`.
pub fn merged_view(
    case: &Case,
    source: &MergedSource,
    limit: u32,
    offset: u64,
    zone: &str,
) -> Result<Listing<MergedView>> {
    check_zone(zone)?;
    let page = merged_rows(case, source, limit, offset)?;
    let items = page
        .items
        .into_iter()
        .map(|row| {
            let display_time = match (&row.time, row.time_semantics) {
                (Some(time), Some(TimeSemantics::Utc)) => {
                    render_time(None, time, zone).map(|r| r.local)
                }
                _ => None,
            };
            MergedView { row, display_time }
        })
        .collect();
    Ok(Listing {
        items,
        truncated: page.truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{
        "schema_version": 3,
        "hosts": [
            {"host": "WS01", "tools": [{"tool": "EvtxTriage", "output_paths":
                ["D:\\old\\run\\Processed-WS01\\EventLogs\\evtx.csv"]}],
             "external_tools": [{"tool": "hayabusa-csv", "output_paths":
                ["Processed-WS01/Hayabusa/timeline.csv"]}]},
            {"host": "SRV01", "tools": [{"tool": "PETriage", "output_paths":
                ["D:\\old\\run\\Processed-SRV01\\Execution\\pf.csv"]}],
             "external_tools": []}
        ]
    }"#;

    #[test]
    fn a_manifest_names_a_file_by_its_place_below_the_run_root() {
        let root = r"\\?\E:\cases\run";
        let evtx = r"\\?\E:\cases\run\Processed-WS01/EventLogs/evtx.csv";
        assert_eq!(
            listed_in_manifest(MANIFEST, root, Some("WS01"), evtx),
            Some(true)
        );
        // Host names compare as Windows compares them.
        assert_eq!(
            listed_in_manifest(MANIFEST, root, Some("ws01"), evtx),
            Some(true)
        );
        // A relative path in the manifest, and an external tool's output.
        let hayabusa = r"\\?\E:\cases\run\Processed-WS01\Hayabusa\TIMELINE.csv";
        assert_eq!(
            listed_in_manifest(MANIFEST, root, Some("WS01"), hayabusa),
            Some(true)
        );
        // Under another host only.
        let pf = r"\\?\E:\cases\run\Processed-SRV01\Execution\pf.csv";
        assert_eq!(
            listed_in_manifest(MANIFEST, root, Some("WS01"), pf),
            Some(false)
        );
        assert_eq!(listed_in_manifest(MANIFEST, root, None, pf), Some(true));
        // A suffix only at a component boundary.
        let other = r"\\?\E:\cases\run\Processed-WS01\EventLogs\old_evtx.csv";
        assert_eq!(
            listed_in_manifest(MANIFEST, root, Some("WS01"), other),
            Some(false)
        );
        // The run root itself names no file.
        assert_eq!(
            listed_in_manifest(MANIFEST, root, Some("WS01"), root),
            Some(false)
        );
        // A manifest that does not parse says nothing.
        assert_eq!(listed_in_manifest("{", root, Some("WS01"), evtx), None);
        assert_eq!(
            listed_in_manifest(r#"{"hosts": 5}"#, root, Some("WS01"), evtx),
            None
        );
        // One without hosts names nothing.
        assert_eq!(
            listed_in_manifest(r#"{"schema_version": 3}"#, root, Some("WS01"), evtx),
            Some(false)
        );
    }
}
