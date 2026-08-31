//! The derived `_Timeline` dataset.
//!
//! A cross-artifact index over every non-null instant the typed datasets carry,
//! so an analyst can sort one file by time instead of joining eight. Rows are
//! written as they are produced, in artifact-discovery order — unsorted, like
//! PETriage's existing `_Timeline`, because the `Tool` trait has no end-of-run
//! hook and these CSVs are opened in tools that sort for you.
//!
//! Multi-timestamp artifacts fan out: a completed download contributes both a
//! `Download Started` and a `Download Completed` row, following the same
//! pattern as PETriage emitting one row per run time.

use crate::profile::BrowserId;
use serde::Serialize;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;

/// The `Timestamp Type` vocabulary, as constants so that twenty-odd emit sites
/// cannot drift into `Cookie Created` versus `Cookie created`.
pub mod kind {
    pub const VISITED: &str = "Visited";
    pub const DOWNLOAD_STARTED: &str = "Download Started";
    pub const DOWNLOAD_COMPLETED: &str = "Download Completed";
    pub const DOWNLOAD_LAST_ACCESSED: &str = "Download Last Accessed";
    pub const COOKIE_CREATED: &str = "Cookie Created";
    pub const COOKIE_LAST_ACCESSED: &str = "Cookie Last Accessed";
    pub const COOKIE_LAST_UPDATED: &str = "Cookie Last Updated";
    pub const COOKIE_EXPIRES: &str = "Cookie Expires";
    pub const AUTOFILL_FIRST_USED: &str = "Autofill First Used";
    pub const AUTOFILL_LAST_USED: &str = "Autofill Last Used";
    pub const BOOKMARK_ADDED: &str = "Bookmark Added";
    pub const BOOKMARK_MODIFIED: &str = "Bookmark Modified";
    pub const BOOKMARK_LAST_USED: &str = "Bookmark Last Used";
    pub const LOGIN_CREATED: &str = "Login Created";
    pub const LOGIN_LAST_USED: &str = "Login Last Used";
    pub const PASSWORD_CHANGED: &str = "Password Changed";
    pub const LOGIN_RECEIVED: &str = "Login Received";
    pub const SEARCH: &str = "Search";
    pub const SEARCH_LAST_UPDATED: &str = "Search Last Updated";
    pub const EXTENSION_INSTALLED: &str = "Extension Installed";
    pub const EXTENSION_UPDATED: &str = "Extension Updated";
}

/// The `Artifact` column: the display name of the dataset a row came from.
pub mod artifact_name {
    pub const HISTORY: &str = "History";
    pub const DOWNLOADS: &str = "Downloads";
    pub const COOKIES: &str = "Cookies";
    pub const AUTOFILL: &str = "Autofill";
    pub const BOOKMARKS: &str = "Bookmarks";
    pub const LOGINS: &str = "Logins";
    pub const KEYWORD_SEARCHES: &str = "Keyword Searches";
    pub const EXTENSIONS: &str = "Extensions";
}

/// `Browser Channel` is deliberately absent: the timeline is meant to stay
/// narrow, and the channel is recoverable by joining `Source File` back to the
/// typed dataset.
#[derive(Debug, Default, Serialize)]
pub struct TimelineRecord {
    #[serde(rename = "Timestamp")]
    pub timestamp: WinTimestamp,
    #[serde(rename = "Timestamp Type")]
    pub timestamp_type: String,
    #[serde(rename = "Browser")]
    pub browser: String,
    #[serde(rename = "Profile")]
    pub profile: String,
    #[serde(rename = "Artifact")]
    pub artifact: String,
    #[serde(rename = "Value")]
    pub value: String,
    #[serde(rename = "Source File")]
    pub source_file: String,
}

/// Writes timeline rows for one artifact file, carrying the attribution so the
/// call sites stay to one line each.
pub struct Timeline<'a> {
    browser: &'a str,
    profile: &'a str,
    source_file: &'a str,
    enabled: bool,
    /// Rows written, which the caller folds into `parse()`'s return value.
    pub emitted: u64,
}

impl<'a> Timeline<'a> {
    pub fn new(id: &'a BrowserId, source_file: &'a str, enabled: bool) -> Self {
        Timeline {
            browser: &id.browser,
            profile: &id.profile,
            source_file,
            enabled,
            emitted: 0,
        }
    }

    /// Emit one row, unless the timestamp is unset.
    ///
    /// A null timestamp writes nothing, which does not violate the completeness
    /// contract: the underlying record is still fully emitted in its typed CSV
    /// with an empty timestamp cell. The timeline is an index over instants, and
    /// a row with no instant would be noise in every tool that consumes it.
    pub fn push(
        &mut self,
        out: &mut OutputRouter,
        timestamp: WinTimestamp,
        timestamp_type: &str,
        artifact: &str,
        value: &str,
    ) -> Result<(), TriageError> {
        if !self.enabled || timestamp.is_none() {
            return Ok(());
        }
        out.write(
            "timeline",
            &TimelineRecord {
                timestamp,
                timestamp_type: timestamp_type.to_string(),
                browser: self.browser.to_string(),
                profile: self.profile.to_string(),
                artifact: artifact.to_string(),
                value: value.to_string(),
                source_file: self.source_file.to_string(),
            },
        )?;
        self.emitted += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::header_test_support::headers;

    #[test]
    fn the_timeline_has_exactly_the_documented_columns() {
        assert_eq!(
            headers::<TimelineRecord>(),
            vec![
                "Timestamp",
                "Timestamp Type",
                "Browser",
                "Profile",
                "Artifact",
                "Value",
                "Source File",
            ]
        );
    }

    #[test]
    fn timestamp_type_values_are_distinct() {
        let all = [
            kind::VISITED,
            kind::DOWNLOAD_STARTED,
            kind::DOWNLOAD_COMPLETED,
            kind::DOWNLOAD_LAST_ACCESSED,
            kind::COOKIE_CREATED,
            kind::COOKIE_LAST_ACCESSED,
            kind::COOKIE_LAST_UPDATED,
            kind::COOKIE_EXPIRES,
            kind::AUTOFILL_FIRST_USED,
            kind::AUTOFILL_LAST_USED,
            kind::BOOKMARK_ADDED,
            kind::BOOKMARK_MODIFIED,
            kind::BOOKMARK_LAST_USED,
            kind::LOGIN_CREATED,
            kind::LOGIN_LAST_USED,
            kind::PASSWORD_CHANGED,
            kind::LOGIN_RECEIVED,
            kind::SEARCH,
            kind::SEARCH_LAST_UPDATED,
            kind::EXTENSION_INSTALLED,
            kind::EXTENSION_UPDATED,
        ];
        let mut sorted = all.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            all.len(),
            "Timestamp Type values must be unique"
        );
    }
}
