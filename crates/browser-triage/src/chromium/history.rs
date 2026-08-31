//! Chromium `History`: the `urls` and `visits` tables.
//!
//! Two passes, and the second one is the point: a `urls` row whose `visits`
//! rows have expired or been deleted is still emitted, as `Record Type =
//! URL Only`. Chromium expires visit rows on retention while keeping the URL,
//! and sync-imported entries have URL rows with no visits from birth — a
//! `visit_count` of 12 with no visit rows is a deletion indicator, and dropping
//! it would destroy exactly the evidence an examiner is looking for.

use crate::profile::BrowserId;
use crate::records::HistoryRecord;
use crate::sql::{self, Notes};
use crate::timeline::{artifact_name, kind, Timeline};
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_sqlite::Database;

/// Columns read from `visits`. Projected rather than `SELECT *` so a column
/// added or removed across Chromium milestones cannot shift the indices.
const VISIT_COLUMNS: &[&str] = &[
    "id",
    "url",
    "visit_time",
    "from_visit",
    "transition",
    "visit_duration",
    "opener_visit",
];

const URL_COLUMNS: &[&str] = &[
    "id",
    "url",
    "title",
    "visit_count",
    "typed_count",
    "last_visit_time",
    "hidden",
];

fn artifact_error(path: &Path, message: impl std::fmt::Display) -> TriageError {
    TriageError::Artifact {
        path: path.to_path_buf(),
        message: message.to_string(),
    }
}

/// Emit every visit, then every URL with no visits. Returns rows written,
/// timeline included.
pub fn parse(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    let has_urls = db.table_exists("urls").unwrap_or(false);
    let has_visits = db.table_exists("visits").unwrap_or(false);
    if !has_urls {
        // Not a history database (Chromium reuses the `History` name for
        // nothing else, but an empty or foreign file can still reach us).
        return Ok(0);
    }

    let source = path.display().to_string();
    let url_cols = sql::columns(db, "urls");
    let visit_cols = sql::columns(db, "visits");
    let mut written = 0u64;

    if has_visits {
        // LEFT JOIN, never INNER: a visit whose URL foreign key is dangling is
        // itself evidence of history deletion and must still be emitted.
        let sql_text = format!(
            "SELECT {}, {} FROM visits v LEFT JOIN urls u ON u.id = v.url ORDER BY v.id",
            prefixed(&visit_cols, VISIT_COLUMNS, "v"),
            prefixed(&url_cols, URL_COLUMNS, "u"),
        );
        let rows = db
            .query(&sql_text)
            .map_err(|e| artifact_error(path, format!("visits: {e}")))?;

        for row in &rows {
            let mut notes = Notes::new();
            if !id.note.is_empty() {
                notes.push(id.note.clone());
            }

            let visit_id = sql::int(sql::cell(row, 0));
            let url_fk = sql::int(sql::cell(row, 1));
            let visit_time =
                WinTimestamp::from_webkit_micros(sql::int(sql::cell(row, 2)).unwrap_or_default());
            let from_visit = sql::int(sql::cell(row, 3));
            let transition_raw = sql::int(sql::cell(row, 4));
            let visit_duration_secs = sql::int(sql::cell(row, 5)).map(|us| us as f64 / 1_000_000.0);
            let opener_visit = sql::int(sql::cell(row, 6));

            let url_id = sql::int(sql::cell(row, 7));
            let url = sql::text(sql::cell(row, 8));
            let title = sql::text(sql::cell(row, 9));
            notes.note_if_lossy("URL", &url);
            notes.note_if_lossy("Title", &title);

            if url_id.is_none() {
                notes.push(format!(
                    "visits.url {} has no matching urls row",
                    url_fk.map(|v| v.to_string()).unwrap_or_else(|| "?".into())
                ));
            }

            let record = HistoryRecord {
                browser: id.browser.clone(),
                channel: id.channel.clone(),
                profile: id.profile.clone(),
                record_type: HistoryRecord::VISIT,
                visit_time,
                url: url.clone(),
                title,
                visit_type: transition_raw
                    .map(super::transition_core)
                    .unwrap_or_default(),
                transition_qualifiers: transition_raw
                    .map(super::transition_qualifiers)
                    .unwrap_or_default(),
                transition_raw,
                visit_duration_secs,
                visit_count: sql::int(sql::cell(row, 10)),
                typed_count: sql::int(sql::cell(row, 11)),
                last_visit_time: WinTimestamp::from_webkit_micros(
                    sql::int(sql::cell(row, 12)).unwrap_or_default(),
                ),
                hidden: sql::bool_str(sql::cell(row, 13)),
                from_visit_id: from_visit,
                opener_visit_id: opener_visit,
                visit_id,
                url_id,
                frecency: None,
                notes: notes.into_string(),
                source_file: source.clone(),
            };
            out.write("history", &record)?;
            written += 1;
            timeline.push(out, visit_time, kind::VISITED, artifact_name::HISTORY, &url)?;
        }
    }

    // Orphan pass: URLs with no surviving visit. Skipped when there is no
    // `visits` table at all, because then every URL row is already an orphan
    // and the query below would emit them twice.
    let orphan_sql = if has_visits {
        format!(
            "SELECT {} FROM urls u WHERE NOT EXISTS \
             (SELECT 1 FROM visits v WHERE v.url = u.id) ORDER BY u.id",
            prefixed(&url_cols, URL_COLUMNS, "u")
        )
    } else {
        format!(
            "SELECT {} FROM urls u ORDER BY u.id",
            prefixed(&url_cols, URL_COLUMNS, "u")
        )
    };
    let orphans = db
        .query(&orphan_sql)
        .map_err(|e| artifact_error(path, format!("urls: {e}")))?;

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
            WinTimestamp::from_webkit_micros(sql::int(sql::cell(row, 5)).unwrap_or_default());

        let record = HistoryRecord {
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
            frecency: None,
            notes: notes.into_string(),
            source_file: source.clone(),
        };
        out.write("history", &record)?;
        written += 1;
        // A URL with no visit row still carries the aggregate last-visit time,
        // which is the only instant we have for it.
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

/// Like [`sql::projection`] but qualified with a table alias, so a projected
/// column reads `u."title"` and a missing one still reads `NULL AS "title"`.
fn prefixed(cols: &[String], wanted: &[&str], alias: &str) -> String {
    wanted
        .iter()
        .map(|name| {
            if cols.iter().any(|c| c == &name.to_ascii_lowercase()) {
                format!("{alias}.\"{name}\"")
            } else {
                format!("NULL AS \"{name}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixed_qualifies_present_columns_and_pads_absent_ones() {
        let cols = vec!["id".to_string(), "url".to_string()];
        assert_eq!(
            prefixed(&cols, &["id", "url", "opener_visit"], "v"),
            "v.\"id\", v.\"url\", NULL AS \"opener_visit\""
        );
    }
}
