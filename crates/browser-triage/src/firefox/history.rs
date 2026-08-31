//! Firefox `places.sqlite` -> `moz_places` + `moz_historyvisits`.
//!
//! The mirror of the Chromium history parser, including the orphan pass:
//! Firefox expires visit rows while keeping the place, so a `moz_places` row
//! with a visit count and no visits is the same deletion signal it is in
//! Chromium.

use crate::profile::BrowserId;
use crate::records::HistoryRecord;
use crate::sql::{self, Notes};
use crate::timeline::{artifact_name, kind, Timeline};
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_sqlite::Database;

const PLACE_COLUMNS: &[&str] = &[
    "id",
    "url",
    "title",
    "visit_count",
    "typed",
    "last_visit_date",
    "hidden",
    "frecency",
];

pub fn parse(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    if !db.table_exists("moz_places").unwrap_or(false) {
        return Ok(0);
    }
    let has_visits = db.table_exists("moz_historyvisits").unwrap_or(false);
    let place_cols = sql::columns(db, "moz_places");
    let visit_cols = sql::columns(db, "moz_historyvisits");
    let source = path.display().to_string();
    let mut written = 0u64;

    if has_visits {
        // `source` and `triggeringPlaceId` are recent additions, so they are
        // resolved against the live schema rather than assumed present.
        let sql_text = format!(
            "SELECT v.id, v.from_visit, v.visit_date, v.visit_type, {visit_source}, {places} \
             FROM moz_historyvisits v LEFT JOIN moz_places p ON p.id = v.place_id \
             ORDER BY v.id",
            visit_source = sql::alternatives(&visit_cols, &["source"], Some("v")),
            places = sql::projection_aliased(&place_cols, PLACE_COLUMNS, "p"),
        );
        let rows = db.query(&sql_text).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: format!("moz_historyvisits: {e}"),
        })?;

        for row in &rows {
            let mut notes = Notes::new();
            if !id.note.is_empty() {
                notes.push(id.note.clone());
            }

            let url = sql::text(sql::cell(row, 6));
            let title = sql::text(sql::cell(row, 7));
            notes.note_if_lossy("URL", &url);
            notes.note_if_lossy("Title", &title);

            let place_id = sql::int(sql::cell(row, 5));
            if place_id.is_none() {
                notes.push("moz_historyvisits.place_id has no matching moz_places row".to_string());
            }

            let visit_time =
                WinTimestamp::from_unix_micros(sql::int(sql::cell(row, 2)).unwrap_or_default());

            out.write(
                "history",
                &HistoryRecord {
                    browser: id.browser.clone(),
                    channel: id.channel.clone(),
                    profile: id.profile.clone(),
                    record_type: HistoryRecord::VISIT,
                    visit_time,
                    url: url.clone(),
                    title,
                    visit_type: super::visit_type(sql::int(sql::cell(row, 3))),
                    // Firefox has no transition qualifier bitmask.
                    transition_qualifiers: String::new(),
                    transition_raw: sql::int(sql::cell(row, 3)),
                    visit_duration_secs: None,
                    visit_count: sql::int(sql::cell(row, 8)),
                    // `typed` is 0 or 1 in Firefox, not a count. Documented on
                    // the tool page; the column is shared with Chromium, where
                    // it genuinely is a count.
                    typed_count: sql::int(sql::cell(row, 9)),
                    last_visit_time: WinTimestamp::from_unix_micros(
                        sql::int(sql::cell(row, 10)).unwrap_or_default(),
                    ),
                    hidden: sql::bool_str(sql::cell(row, 11)),
                    from_visit_id: sql::int(sql::cell(row, 1)),
                    opener_visit_id: None,
                    visit_id: sql::int(sql::cell(row, 0)),
                    url_id: place_id,
                    frecency: sql::int(sql::cell(row, 12)),
                    notes: notes.into_string(),
                    source_file: source.clone(),
                },
            )?;
            written += 1;
            timeline.push(out, visit_time, kind::VISITED, artifact_name::HISTORY, &url)?;
        }
    }

    // Places with no surviving visit — the deletion signal.
    let orphan_sql = if has_visits {
        format!(
            "SELECT {} FROM moz_places p WHERE NOT EXISTS \
             (SELECT 1 FROM moz_historyvisits v WHERE v.place_id = p.id) ORDER BY p.id",
            sql::projection_aliased(&place_cols, PLACE_COLUMNS, "p")
        )
    } else {
        format!(
            "SELECT {} FROM moz_places p ORDER BY p.id",
            sql::projection_aliased(&place_cols, PLACE_COLUMNS, "p")
        )
    };
    let orphans = db.query(&orphan_sql).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: format!("moz_places: {e}"),
    })?;

    for row in &orphans {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }
        let url = sql::text(sql::cell(row, 1));
        let title = sql::text(sql::cell(row, 2));
        notes.note_if_lossy("URL", &url);
        notes.note_if_lossy("Title", &title);

        let last_visit_time =
            WinTimestamp::from_unix_micros(sql::int(sql::cell(row, 5)).unwrap_or_default());

        out.write(
            "history",
            &HistoryRecord {
                browser: id.browser.clone(),
                channel: id.channel.clone(),
                profile: id.profile.clone(),
                record_type: HistoryRecord::URL_ONLY,
                visit_time: WinTimestamp::none(),
                url: url.clone(),
                title,
                visit_type: String::new(),
                transition_qualifiers: String::new(),
                transition_raw: None,
                visit_duration_secs: None,
                visit_count: sql::int(sql::cell(row, 3)),
                typed_count: sql::int(sql::cell(row, 4)),
                last_visit_time,
                hidden: sql::bool_str(sql::cell(row, 6)),
                from_visit_id: None,
                opener_visit_id: None,
                visit_id: None,
                url_id: sql::int(sql::cell(row, 0)),
                frecency: sql::int(sql::cell(row, 7)),
                notes: notes.into_string(),
                source_file: source.clone(),
            },
        )?;
        written += 1;
        timeline.push(
            out,
            last_visit_time,
            kind::VISITED,
            artifact_name::HISTORY,
            &url,
        )?;
    }

    Ok(written)
}
