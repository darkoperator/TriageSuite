//! Firefox downloads, from the `moz_annos` annotation table.
//!
//! Firefox has no downloads table. A download is a place annotated with
//! `downloads/destinationFileURI` (where the file went) and usually a second
//! `downloads/metaData` annotation holding a small JSON blob with the end time,
//! size and state.
//!
//! Consequently Firefox rows fill perhaps a third of the shared download
//! schema — there is no MIME type, referrer, tab URL or danger type to read.
//! Those columns are empty rather than absent, so one dataset still serves both
//! families.

use crate::json;
use crate::profile::BrowserId;
use crate::records::DownloadRecord;
use crate::sql::{self, Notes};
use crate::timeline::{artifact_name, kind, Timeline};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_sqlite::Database;

const DESTINATION: &str = "downloads/destinationFileURI";
const METADATA: &str = "downloads/metaData";

/// One place's download annotations.
#[derive(Default)]
struct Annotations {
    destination: Option<String>,
    metadata: Option<String>,
    url: String,
    title: String,
    date_added: Option<i64>,
}

pub fn parse(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    if !db.table_exists("moz_annos").unwrap_or(false)
        || !db.table_exists("moz_anno_attributes").unwrap_or(false)
    {
        return Ok(0);
    }

    let source = path.display().to_string();
    let rows = db
        .query(
            "SELECT a.place_id, n.name, a.content, a.dateAdded, p.url, p.title \
             FROM moz_annos a \
             JOIN moz_anno_attributes n ON n.id = a.anno_attribute_id \
             LEFT JOIN moz_places p ON p.id = a.place_id \
             WHERE n.name IN ('downloads/destinationFileURI', 'downloads/metaData') \
             ORDER BY a.place_id",
        )
        .map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: format!("moz_annos: {e}"),
        })?;

    // Group the two annotation kinds by place, so one download is one row even
    // though it is two source rows.
    let mut by_place: BTreeMap<i64, Annotations> = BTreeMap::new();
    for row in &rows {
        let Some(place_id) = sql::int(sql::cell(row, 0)) else {
            continue;
        };
        let entry = by_place.entry(place_id).or_default();
        let content = sql::text(sql::cell(row, 2));
        match sql::text(sql::cell(row, 1)).as_str() {
            DESTINATION => entry.destination = Some(content),
            METADATA => entry.metadata = Some(content),
            _ => {}
        }
        if entry.date_added.is_none() {
            entry.date_added = sql::int(sql::cell(row, 3));
        }
        if entry.url.is_empty() {
            entry.url = sql::text(sql::cell(row, 4));
        }
        if entry.title.is_empty() {
            entry.title = sql::text(sql::cell(row, 5));
        }
    }

    let mut written = 0u64;
    for (place_id, anno) in by_place {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }

        // A metaData annotation with no destination sibling still records that
        // a download happened, so it is emitted rather than skipped.
        let record_type = if anno.destination.is_some() {
            DownloadRecord::DOWNLOAD
        } else {
            notes.push(
                "download metaData annotation with no destinationFileURI sibling".to_string(),
            );
            DownloadRecord::ORPHAN_CHAIN
        };

        let mut end_time = WinTimestamp::none();
        let mut total_bytes = None;
        let mut state = String::new();
        if let Some(raw) = &anno.metadata {
            match serde_json::from_str::<Value>(raw) {
                Ok(meta) => {
                    end_time = WinTimestamp::from_unix_millis(
                        json::int(&meta, "endTime").unwrap_or_default(),
                    );
                    total_bytes = json::int(&meta, "fileSize");
                    // Firefox's own state enum, not Chromium's: the two agree
                    // only on 1, and sharing a table swapped failed with
                    // cancelled.
                    state = super::download_metadata_state(json::int(&meta, "state"));
                    if json::bool_str(&meta, "deleted") == "True" {
                        notes.push("metaData records the file as deleted".to_string());
                    }
                }
                Err(error) => notes.push(format!("metaData is not valid JSON: {error}")),
            }
        }

        let start_time = WinTimestamp::from_unix_micros(anno.date_added.unwrap_or_default());
        let target_path = anno.destination.clone().unwrap_or_default();
        let label = if target_path.is_empty() {
            anno.url.clone()
        } else {
            target_path.clone()
        };

        out.write(
            "downloads",
            &DownloadRecord {
                browser: id.browser.clone(),
                channel: id.channel.clone(),
                profile: id.profile.clone(),
                record_type,
                start_time,
                end_time,
                target_path,
                download_url: anno.url.clone(),
                url_chain: anno.url.clone(),
                total_bytes,
                state,
                download_id: Some(place_id),
                notes: notes.into_string(),
                source_file: source.clone(),
                ..Default::default()
            },
        )?;
        written += 1;

        timeline.push(
            out,
            start_time,
            kind::DOWNLOAD_STARTED,
            artifact_name::DOWNLOADS,
            &label,
        )?;
        timeline.push(
            out,
            end_time,
            kind::DOWNLOAD_COMPLETED,
            artifact_name::DOWNLOADS,
            &label,
        )?;
    }

    Ok(written)
}
