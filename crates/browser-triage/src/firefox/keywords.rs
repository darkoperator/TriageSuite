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

/// Run both sources. A failure in one must not cost the other: they are
/// unrelated tables covering different Firefox eras, and an early return here
/// used to discard every address-bar row because the metadata query failed.
pub fn parse(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    let mut written = 0u64;
    crate::soft(
        parse_metadata(db, path, id, out, timeline),
        path,
        "moz_places_metadata",
        &mut written,
    )?;
    crate::soft(
        parse_input_history(db, path, id, out),
        path,
        "moz_inputhistory",
        &mut written,
    )?;
    Ok(written)
}

/// Seed a row's notes with any degraded-attribution note.
fn notes_for(id: &BrowserId) -> Notes {
    let mut notes = Notes::new();
    if !id.note.is_empty() {
        notes.push(id.note.clone());
    }
    notes
}

/// Firefox 111+ search terms, from `moz_places_metadata_search_queries`.
///
/// Driven from the terms table with a `LEFT JOIN` outward, never an `INNER
/// JOIN` inward. Firefox clears `moz_places_metadata` while the terms table
/// lags behind, so a term whose metadata row is gone is precisely the
/// survived-a-deletion evidence this dataset exists for. It is emitted with an
/// empty `Search Time` and a note, rather than dropped.
fn parse_metadata(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    if !db
        .table_exists("moz_places_metadata_search_queries")
        .unwrap_or(false)
    {
        return Ok(0);
    }
    let has_metadata = db.table_exists("moz_places_metadata").unwrap_or(false);
    let source = path.display().to_string();
    let mut written = 0u64;

    let sql_text = if has_metadata {
        "SELECT q.terms, m.created_at, m.updated_at, \
                p.url, p.title, p.visit_count, p.last_visit_date, p.id \
         FROM moz_places_metadata_search_queries q \
         LEFT JOIN moz_places_metadata m ON m.search_query_id = q.id \
         LEFT JOIN moz_places p ON p.id = m.place_id \
         ORDER BY q.id"
    } else {
        // The terms survived without the metadata table at all.
        "SELECT q.terms, NULL, NULL, NULL, NULL, NULL, NULL, NULL \
         FROM moz_places_metadata_search_queries q ORDER BY q.id"
    };

    let rows = db.query(sql_text).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: format!("moz_places_metadata_search_queries: {e}"),
    })?;

    for row in &rows {
        let mut notes = notes_for(id);
        let term = sql::text(sql::cell(row, 0));
        let url = sql::text(sql::cell(row, 3));
        notes.note_if_lossy("Search Term", &term);

        // These two are unix milliseconds, unlike the microseconds everywhere
        // else in places.sqlite.
        let created = sql::int(sql::cell(row, 1));
        let search_time = WinTimestamp::from_unix_millis(created.unwrap_or_default());
        let last_updated =
            WinTimestamp::from_unix_millis(sql::int(sql::cell(row, 2)).unwrap_or_default());
        if created.is_none() {
            notes.push(
                "search term outlived its moz_places_metadata row; no search time recorded"
                    .to_string(),
            );
        }

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
                page_title: sql::text(sql::cell(row, 4)),
                last_visit_time: WinTimestamp::from_unix_micros(
                    sql::int(sql::cell(row, 6)).unwrap_or_default(),
                ),
                visit_count: sql::int(sql::cell(row, 5)),
                keyword_id: None,
                url_id: sql::int(sql::cell(row, 7)),
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

    Ok(written)
}

/// The older address-bar history. No timestamp exists to invent.
fn parse_input_history(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
) -> Result<u64, TriageError> {
    if !db.table_exists("moz_inputhistory").unwrap_or(false) {
        return Ok(0);
    }
    let source = path.display().to_string();
    let mut written = 0u64;

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
        let mut notes = notes_for(id);
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

    Ok(written)
}
