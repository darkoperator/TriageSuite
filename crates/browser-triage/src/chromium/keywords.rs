//! Chromium `keyword_search_terms`.
//!
//! What someone typed into the omnibox, which frequently says more than the URL
//! it produced. Worth its own dataset because the term survives independently
//! of the history rows: clearing history can leave `keyword_search_terms`
//! populated, and a term with no surviving visit is a strong signal.

use crate::profile::BrowserId;
use crate::records::KeywordSearchRecord;
use crate::sql::{self, Notes};
use crate::timeline::{artifact_name, kind, Timeline};
use std::path::Path;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_sqlite::Database;

const SOURCE_TABLE: &str = "keyword_search_terms";

/// The host of a URL, without pulling in a URL parser: everything between the
/// scheme separator and the next `/`, minus any userinfo or port.
pub fn host_of(url: &str) -> String {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or(authority);
    // Leave an IPv6 literal intact; only strip a trailing :port.
    match host.rsplit_once(':') {
        Some((left, right))
            if !left.ends_with(']') && right.chars().all(|c| c.is_ascii_digit()) =>
        {
            left.to_string()
        }
        _ => host.to_string(),
    }
}

pub fn parse(
    db: &Database,
    path: &Path,
    id: &BrowserId,
    out: &mut OutputRouter,
    timeline: &mut Timeline<'_>,
) -> Result<u64, TriageError> {
    if !db.table_exists(SOURCE_TABLE).unwrap_or(false) {
        return Ok(0);
    }
    let has_visits = db.table_exists("visits").unwrap_or(false);
    let source = path.display().to_string();
    let mut written = 0u64;

    // Chromium renamed `lower_term` to `normalized_term`; both are in the wild,
    // so the column is chosen from the live schema rather than hardcoded.
    // Hardcoding it made the whole query fail on a current Chrome profile and
    // silently produced zero rows.
    let keyword_cols = sql::columns(db, SOURCE_TABLE);
    let normalized = if keyword_cols.iter().any(|c| c == "normalized_term") {
        "k.\"normalized_term\""
    } else if keyword_cols.iter().any(|c| c == "lower_term") {
        "k.\"lower_term\""
    } else {
        "NULL"
    };

    // LEFT JOIN both ways down the chain: a term whose URL row is gone, or
    // whose visits are gone, is still emitted. Each surviving visit is a
    // separate search execution and therefore its own row.
    let sql_text = if has_visits {
        format!(
            "SELECT k.keyword_id, k.url_id, k.term, {normalized}, \
                    u.url, u.title, u.visit_count, u.last_visit_time, \
                    v.id, v.visit_time \
             FROM keyword_search_terms k \
             LEFT JOIN urls u ON u.id = k.url_id \
             LEFT JOIN visits v ON v.url = k.url_id \
             ORDER BY k.url_id, v.id"
        )
    } else {
        format!(
            "SELECT k.keyword_id, k.url_id, k.term, {normalized}, \
                    u.url, u.title, u.visit_count, u.last_visit_time, \
                    NULL, NULL \
             FROM keyword_search_terms k \
             LEFT JOIN urls u ON u.id = k.url_id \
             ORDER BY k.url_id"
        )
    };

    let rows = db.query(&sql_text).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: format!("{SOURCE_TABLE}: {e}"),
    })?;

    for row in &rows {
        let mut notes = Notes::new();
        if !id.note.is_empty() {
            notes.push(id.note.clone());
        }

        let url_id = sql::int(sql::cell(row, 1));
        let term = sql::text(sql::cell(row, 2));
        let url = sql::text(sql::cell(row, 4));
        notes.note_if_lossy("Search Term", &term);

        if url.is_empty() && url_id.is_some() {
            notes.push(format!(
                "keyword_search_terms.url_id {} has no matching urls row",
                url_id.unwrap_or_default()
            ));
        }

        let search_time =
            WinTimestamp::from_webkit_micros(sql::int(sql::cell(row, 9)).unwrap_or_default());
        let visit_id = sql::int(sql::cell(row, 8));
        if visit_id.is_none() {
            notes.push("search term outlived its visit rows".to_string());
        }

        let record = KeywordSearchRecord {
            browser: id.browser.clone(),
            channel: id.channel.clone(),
            profile: id.profile.clone(),
            search_source: SOURCE_TABLE.to_string(),
            search_time,
            search_term: term.clone(),
            search_term_lower: sql::text(sql::cell(row, 3)),
            search_url: url.clone(),
            search_engine_host: host_of(&url),
            page_title: sql::text(sql::cell(row, 5)),
            last_visit_time: WinTimestamp::from_webkit_micros(
                sql::int(sql::cell(row, 7)).unwrap_or_default(),
            ),
            visit_count: sql::int(sql::cell(row, 6)),
            keyword_id: sql::int(sql::cell(row, 0)),
            url_id,
            visit_id,
            notes: notes.into_string(),
            source_file: source.clone(),
        };
        out.write("keyword_searches", &record)?;
        written += 1;

        // The bare term is what an analyst scans a timeline for, so the search
        // is indexed separately from the History row for the same visit.
        timeline.push(
            out,
            search_time,
            kind::SEARCH,
            artifact_name::KEYWORD_SEARCHES,
            &term,
        )?;
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_extracts_the_provider() {
        assert_eq!(
            host_of("https://www.google.com/search?q=a"),
            "www.google.com"
        );
        assert_eq!(host_of("https://duckduckgo.com/?q=b"), "duckduckgo.com");
        assert_eq!(host_of("http://user:pw@host.test:8080/x"), "host.test");
        assert_eq!(host_of("https://[2001:db8::1]/x"), "[2001:db8::1]");
        assert_eq!(host_of(""), "");
        assert_eq!(host_of("not a url"), "not a url");
    }
}
