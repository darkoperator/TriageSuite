//! The artifact table: the single source of truth behind `patterns()`,
//! `validate()` and the `parse()` dispatch.
//!
//! Keeping all three driven by one `classify()` makes it structurally
//! impossible to declare a glob without wiring a parser for it, or to validate
//! a file one way and then parse it another.

use std::path::Path;
use triage_core::winpath::eq_ci;

/// One recognized browser artifact. The browser family is implied by the
/// filename, which is why `profile::identify` can always name the family even
/// when the path is unrecognizable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// Chromium `History`: urls, visits, downloads, keyword_search_terms.
    ChromiumHistory,
    /// Chromium `Cookies` (profile root pre-96, `Network/` subdirectory after).
    ChromiumCookies,
    /// Chromium `Web Data`: autofill.
    ChromiumWebData,
    /// Chromium `Bookmarks` (JSON tree).
    ChromiumBookmarks,
    /// Chromium `Login Data` / `Login Data For Account` (metadata only).
    ChromiumLogins,
    /// Chromium `Preferences` / `Secure Preferences`: extensions.settings.
    ChromiumPreferences,
    /// Firefox `places.sqlite`: moz_places, moz_historyvisits, moz_annos,
    /// moz_bookmarks.
    FirefoxPlaces,
    /// Firefox `cookies.sqlite`: moz_cookies.
    FirefoxCookies,
    /// Firefox `formhistory.sqlite`: moz_formhistory.
    FirefoxFormHistory,
    /// Firefox `logins.json` (metadata only).
    FirefoxLogins,
    /// Firefox `extensions.json`: addons[].
    FirefoxExtensions,
}

/// How an artifact is stored, which decides how `validate()` checks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backing {
    Sqlite,
    /// `generic_name` marks filenames that unrelated software also uses
    /// (`Preferences`), so a content check alone is not enough to claim them.
    Json {
        generic_name: bool,
    },
}

/// Filename -> artifact. Every entry here appears in `patterns()` and has a
/// `parse()` arm.
const TABLE: &[(&str, ArtifactKind)] = &[
    ("History", ArtifactKind::ChromiumHistory),
    ("Cookies", ArtifactKind::ChromiumCookies),
    ("Web Data", ArtifactKind::ChromiumWebData),
    ("Bookmarks", ArtifactKind::ChromiumBookmarks),
    ("Login Data", ArtifactKind::ChromiumLogins),
    ("Login Data For Account", ArtifactKind::ChromiumLogins),
    ("Preferences", ArtifactKind::ChromiumPreferences),
    ("Secure Preferences", ArtifactKind::ChromiumPreferences),
    ("places.sqlite", ArtifactKind::FirefoxPlaces),
    ("cookies.sqlite", ArtifactKind::FirefoxCookies),
    ("formhistory.sqlite", ArtifactKind::FirefoxFormHistory),
    ("logins.json", ArtifactKind::FirefoxLogins),
    ("extensions.json", ArtifactKind::FirefoxExtensions),
];

/// Discovery globs, derived from `TABLE` so the two cannot drift.
///
/// Deliberately exact filenames, never `*.sqlite` or `*.db`: a broad glob would
/// hand us `permissions.sqlite`, `content-prefs.sqlite` and every unrelated
/// application database, all of which would then have to be rejected by
/// content — expensively, and with a `Corrupt` verdict that would be wrong.
pub const PATTERNS: &[&str] = &[
    "History",
    "Cookies",
    "Web Data",
    "Bookmarks",
    "Login Data",
    "Login Data For Account",
    "Preferences",
    "Secure Preferences",
    "places.sqlite",
    "cookies.sqlite",
    "formhistory.sqlite",
    "logins.json",
    "extensions.json",
];

/// Exact, case-insensitive filename match.
pub fn classify(path: &Path) -> Option<ArtifactKind> {
    let name = path.file_name()?.to_str()?;
    TABLE
        .iter()
        .find(|(candidate, _)| eq_ci(candidate, name))
        .map(|(_, kind)| *kind)
}

pub fn backing(kind: ArtifactKind) -> Backing {
    use ArtifactKind::*;
    match kind {
        ChromiumHistory | ChromiumCookies | ChromiumWebData | ChromiumLogins | FirefoxPlaces
        | FirefoxCookies | FirefoxFormHistory => Backing::Sqlite,
        ChromiumPreferences => Backing::Json { generic_name: true },
        ChromiumBookmarks | FirefoxLogins | FirefoxExtensions => Backing::Json {
            generic_name: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn kind_of(name: &str) -> Option<ArtifactKind> {
        classify(&PathBuf::from(format!("/some/profile/{name}")))
    }

    #[test]
    fn every_pattern_classifies_to_an_artifact() {
        for pattern in PATTERNS {
            assert!(
                kind_of(pattern).is_some(),
                "pattern {pattern:?} has no classify() arm"
            );
        }
    }

    #[test]
    fn every_table_entry_is_advertised_as_a_pattern() {
        for (name, _) in TABLE {
            assert!(
                PATTERNS.contains(name),
                "{name:?} is classified but never discovered"
            );
        }
    }

    #[test]
    fn filenames_match_case_insensitively() {
        assert_eq!(kind_of("history"), Some(ArtifactKind::ChromiumHistory));
        assert_eq!(kind_of("HISTORY"), Some(ArtifactKind::ChromiumHistory));
        assert_eq!(kind_of("Places.SQLite"), Some(ArtifactKind::FirefoxPlaces));
    }

    #[test]
    fn both_login_stores_map_to_one_kind() {
        assert_eq!(kind_of("Login Data"), Some(ArtifactKind::ChromiumLogins));
        assert_eq!(
            kind_of("Login Data For Account"),
            Some(ArtifactKind::ChromiumLogins)
        );
    }

    /// Sidecars and backups share a stem with real artifacts and must not be
    /// picked up: `-wal`/`-shm` are opened through the main database by
    /// triage-sqlite, and `Bookmarks.bak` would duplicate every bookmark.
    #[test]
    fn sidecars_and_backups_are_not_artifacts() {
        for name in [
            "History-wal",
            "History-shm",
            "History-journal",
            "Bookmarks.bak",
            "places.sqlite-wal",
            "manifest.json",
            "Favicons",
            "Top Sites",
        ] {
            assert_eq!(kind_of(name), None, "{name} must not classify");
        }
    }

    #[test]
    fn only_preferences_is_treated_as_a_generic_name() {
        assert_eq!(
            backing(ArtifactKind::ChromiumPreferences),
            Backing::Json { generic_name: true }
        );
        assert_eq!(
            backing(ArtifactKind::ChromiumBookmarks),
            Backing::Json {
                generic_name: false
            }
        );
        assert_eq!(backing(ArtifactKind::ChromiumHistory), Backing::Sqlite);
    }
}
