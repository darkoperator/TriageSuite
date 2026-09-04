//! Firefox-family parsers and their shared decoders.
//!
//! Firefox differs from Chromium in three ways that matter throughout this
//! module: timestamps are PRTime microseconds since 1970 rather than WebKit
//! microseconds since 1601, downloads live in an annotation table rather than
//! their own, and cookie values are stored in the clear.

pub mod autofill;
pub mod bookmarks;
pub mod cookies;
pub mod downloads;
pub mod extensions;
pub mod history;
pub mod keywords;
pub mod logins;

/// `moz_historyvisits.visit_type`.
const VISIT_TYPES: &[(i64, &str)] = &[
    (1, "Link"),
    (2, "Typed"),
    (3, "Bookmark"),
    (4, "Embed"),
    (5, "Redirect Permanent"),
    (6, "Redirect Temporary"),
    (7, "Download"),
    (8, "Framed Link"),
    (9, "Reload"),
];

/// `moz_historyvisits.source` — how the visit was initiated, from
/// `nsINavHistoryService.idl`.
///
/// This records the origin of the navigation, not the origin of the *record*:
/// it does not say whether a visit was synced from another device or imported
/// from another browser. An earlier revision of this table claimed it did, and
/// the values below are transcribed from the IDL so that cannot recur.
const VISIT_SOURCES: &[(i64, &str)] = &[
    (0, "Organic"),
    (1, "Sponsored"),
    (2, "Bookmarked"),
    (3, "Searched"),
];

/// `moz_bookmarks.type`.
const BOOKMARK_TYPES: &[(i64, &str)] = &[(1, "URL"), (2, "Folder"), (3, "Separator")];

/// The `state` field of a `downloads/metaData` annotation, from
/// `toolkit/components/downloads/DownloadHistory.sys.mjs`.
///
/// Firefox and Chromium agree only on 1. Decoding these with Chromium's
/// `downloads.state` table inverts failed and cancelled, which is why this
/// table exists rather than being shared.
const DOWNLOAD_METADATA_STATES: &[(i64, &str)] = &[
    (1, "Finished"),
    (2, "Failed"),
    (3, "Canceled"),
    (4, "Paused"),
    (6, "Blocked Parental"),
    (8, "Blocked Reputation"),
    (9, "Blocked Content Analysis"),
];

pub fn download_metadata_state(value: Option<i64>) -> String {
    decode(DOWNLOAD_METADATA_STATES, value)
}

pub fn decode(table: &[(i64, &'static str)], value: Option<i64>) -> String {
    match value {
        None => String::new(),
        Some(v) => table
            .iter()
            .find(|(candidate, _)| *candidate == v)
            .map(|(_, name)| (*name).to_string())
            .unwrap_or_else(|| format!("Unknown ({v})")),
    }
}

pub fn visit_type(value: Option<i64>) -> String {
    decode(VISIT_TYPES, value)
}

pub fn visit_source(value: Option<i64>) -> String {
    decode(VISIT_SOURCES, value)
}

pub fn bookmark_type(value: Option<i64>) -> String {
    decode(BOOKMARK_TYPES, value)
}

/// The friendly name of a Firefox root, from its stable GUID.
pub fn root_name(guid: &str) -> &'static str {
    match guid {
        "root________" => "root",
        "menu________" => "menu",
        "toolbar_____" => "toolbar",
        "unfiled_____" => "unfiled",
        "mobile______" => "mobile",
        "tags________" => "tags",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visit_types_decode_and_report_the_unknown() {
        assert_eq!(visit_type(Some(1)), "Link");
        assert_eq!(visit_type(Some(7)), "Download");
        assert_eq!(visit_type(Some(99)), "Unknown (99)");
        assert_eq!(visit_type(None), "");
    }

    /// Pinned to `nsINavHistoryService.idl`. The column describes how the
    /// navigation started, and nothing above 3 is defined.
    #[test]
    fn visit_sources_match_the_upstream_idl() {
        assert_eq!(visit_source(Some(0)), "Organic");
        assert_eq!(visit_source(Some(1)), "Sponsored");
        assert_eq!(visit_source(Some(2)), "Bookmarked");
        assert_eq!(visit_source(Some(3)), "Searched");
        assert_eq!(visit_source(Some(4)), "Unknown (4)");
        assert_eq!(visit_source(None), "");
    }

    #[test]
    fn bookmark_roots_resolve_from_their_stable_guids() {
        assert_eq!(root_name("toolbar_____"), "toolbar");
        assert_eq!(root_name("unfiled_____"), "unfiled");
        assert_eq!(root_name("not-a-root"), "");
    }

    #[test]
    fn bookmark_types_decode() {
        assert_eq!(bookmark_type(Some(1)), "URL");
        assert_eq!(bookmark_type(Some(2)), "Folder");
        assert_eq!(bookmark_type(Some(3)), "Separator");
    }

    /// Pinned to `DownloadHistory.sys.mjs`. Every value except 1 differs from
    /// Chromium's `downloads.state`, and decoding these with Chromium's table
    /// reported a cancelled download as interrupted and a failed one as
    /// cancelled.
    #[test]
    fn download_metadata_states_match_the_upstream_source() {
        assert_eq!(download_metadata_state(Some(1)), "Finished");
        assert_eq!(download_metadata_state(Some(2)), "Failed");
        assert_eq!(download_metadata_state(Some(3)), "Canceled");
        assert_eq!(download_metadata_state(Some(4)), "Paused");
        assert_eq!(download_metadata_state(Some(6)), "Blocked Parental");
        assert_eq!(download_metadata_state(Some(9)), "Blocked Content Analysis");
        assert_eq!(download_metadata_state(Some(99)), "Unknown (99)");
        assert_eq!(download_metadata_state(None), "");
    }

    /// The specific confusion this table was introduced to end: Chromium's
    /// table names these three values something else entirely.
    #[test]
    fn firefox_states_do_not_match_chromiums_for_the_divergent_values() {
        for value in [2, 3, 4] {
            assert_ne!(
                download_metadata_state(Some(value)),
                crate::chromium::download_state(value),
                "state {value} must not be read with Chromium's table"
            );
        }
    }
}
