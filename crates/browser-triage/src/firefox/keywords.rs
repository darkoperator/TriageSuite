//! Firefox search terms.
//!
//! Two sources, because they cover different Firefox eras and carry different
//! timestamp fidelity:
//!
//! * `moz_places_metadata` + `moz_places_metadata_search_queries` (Firefox
//!   111+) records the term with a real timestamp.
//! * `moz_inputhistory` is the older address-bar history. It has a use count
//!   but no timestamp at all, so those rows are emitted with an empty
//!   `Search Time` and labelled by `Search Source` rather than dropped.

use crate::profile::BrowserId;
use crate::records::KeywordSearchRecord;
use crate::sql::{self, Notes};
use crate::timeline::{artifact_name, kind, Timeline};
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_sqlite::Database;

pub fn parse(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    let source = path.display().to_string();
    let mut written = 0u64;

    if db
        .table_exists("moz_places_metadata_search_queries")
        .unwrap_or(false)
        && db.table_exists("moz_places_metadata").unwrap_or(false)
    {
        let rows = db
            .query(
                "SELECT q.terms, m.created_at, m.updated_at, m.total_view_time, \
                        p.url, p.title, p.visit_count, p.last_visit_date, p.id \
                 FROM moz_places_metadata m \
                 JOIN moz_places_metadata_search_queries q ON q.id = m.search_query_id \
                 LEFT JOIN moz_places p ON p.id = m.place_id \
                 ORDER BY m.created_at",
            )
            .map_err(|e| TriageError::Artifact {
                path: path.to_path_buf(),
                message: format!("moz_places_metadata: {e}"),
            })?;

        for row in &rows {
            let mut notes = Notes::new();
            if !id.note.is_empty() {
                notes.push(id.note.clone());
            }
            let term = sql::text(sql::cell(row, 0));
            let url = sql::text(sql::cell(row, 4));
            notes.note_if_lossy("Search Term", &term);

            // These two are unix milliseconds, unlike the microseconds
            // everywhere else in places.sqlite.
            let search_time =
                WinTimestamp::from_unix_millis(sql::int(sql::cell(row, 1)).unwrap_or_default());
            let last_updated =
                WinTimestamp::from_unix_millis(sql::int(sql::cell(row, 2)).unwrap_or_default());

            out.write(
                "keyword_searches",
                &KeywordSearchRecord {
                    browser: id.browser.clone(),
                    channel: id.channel.clone(),
                    profile: id.profile.clone(),
                    search_source: "moz_places_metadata".to_string(),
                    search_time,
                    search_term: term.clone(),
                    search_term_lower: String::new(),
                    search_url: url.clone(),
                    search_engine_host: crate::chromium::keywords::host_of(&url),
                    page_title: sql::text(sql::cell(row, 5)),
                    last_visit_time: WinTimestamp::from_unix_micros(
                        sql::int(sql::cell(row, 7)).unwrap_or_default(),
                    ),
                    visit_count: sql::int(sql::cell(row, 6)),
                    keyword_id: None,
                    url_id: sql::int(sql::cell(row, 8)),
                    visit_id: None,
                    notes: notes.into_string(),
                    source_file: source.clone(),
                },
            )?;
            written += 1;
            timeline.push(
                out,
                search_time,
                kind::SEARCH,
                artifact_name::KEYWORD_SEARCHES,
                &term,
            )?;
            timeline.push(
                out,
                last_updated,
                kind::SEARCH_LAST_UPDATED,
                artifact_name::KEYWORD_SEARCHES,
                &term,
            )?;
        }
    }

    // The older address-bar history. No timestamp exists to invent.
    if db.table_exists("moz_inputhistory").unwrap_or(false) {
        let rows = db
            .query(
                "SELECT h.input, h.use_count, p.url, p.title, p.visit_count, \
                        p.last_visit_date, p.id \
                 FROM moz_inputhistory h LEFT JOIN moz_places p ON p.id = h.place_id \
                 ORDER BY h.place_id",
            )
            .map_err(|e| TriageError::Artifact {
                path: path.to_path_buf(),
                message: format!("moz_inputhistory: {e}"),
            })?;

        for row in &rows {
            let mut notes = Notes::new();
            if !id.note.is_empty() {
                notes.push(id.note.clone());
            }
            notes.push("moz_inputhistory records no timestamp".to_string());

            let term = sql::text(sql::cell(row, 0));
            let url = sql::text(sql::cell(row, 2));
            notes.note_if_lossy("Search Term", &term);

            out.write(
                "keyword_searches",
                &KeywordSearchRecord {
                    browser: id.browser.clone(),
                    channel: id.channel.clone(),
                    profile: id.profile.clone(),
                    search_source: "moz_inputhistory".to_string(),
                    search_time: WinTimestamp::none(),
                    search_term: term,
                    search_term_lower: String::new(),
                    search_url: url.clone(),
                    search_engine_host: crate::chromium::keywords::host_of(&url),
                    page_title: sql::text(sql::cell(row, 3)),
                    last_visit_time: WinTimestamp::from_unix_micros(
                        sql::int(sql::cell(row, 5)).unwrap_or_default(),
                    ),
                    visit_count: sql::int(sql::cell(row, 4)),
                    keyword_id: None,
                    url_id: sql::int(sql::cell(row, 6)),
                    visit_id: None,
                    notes: notes.into_string(),
                    source_file: source.clone(),
                },
            )?;
            written += 1;
        }
    }

    Ok(written)
}
