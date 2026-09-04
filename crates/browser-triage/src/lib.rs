//! BrowserTriage — Chromium and Firefox browser artifact parser.
//!
//! # Why this exists alongside SQLETriage
//!
//! `SQLETriage` already carries ~24 browser `.smap` maps and can dump the same
//! tables. This crate exists for the four things a generic SQL-map engine
//! cannot do: typed [`WinTimestamp`] columns instead of whole-second
//! `datetime()` strings, browser and profile attribution columns, the JSON
//! artifacts (`Bookmarks`, `logins.json`, `extensions.json`) that the map
//! engine cannot read at all, and a derived cross-artifact timeline.
//!
//! # The completeness contract
//!
//! Every parser in this crate emits one output row per source row. There is no
//! `WHERE <timestamp> > 0`, no `INNER JOIN`, and no `filter_map(Result::ok)`
//! anywhere; a row with a null timestamp, a dangling foreign key or an
//! undecodable cell is still emitted, with the reason recorded in its `Notes`
//! column. This is deliberate and load-bearing: the tool this crate replaced
//! discarded 41% of what it extracted in a single run while reporting success.

pub mod artifact;
pub mod chromium;
pub mod firefox;
pub mod json;
pub mod profile;
pub mod records;
pub mod sql;
pub mod timeline;

use artifact::{ArtifactKind, Backing};
use std::io::Read;
use std::path::Path;
use timeline::Timeline;
use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{ResourceClass, Scope, Tool, Validation};

/// Nine datasets: eight typed artifact tables plus a derived timeline.
///
/// Exactly one carries `override_suffix: None` — `OutputRouter::new` rejects
/// `--csvf`/`--jsonf` when there is more than one primary, because the forced
/// name would be ambiguous. History is the primary as the most-used artifact,
/// so `--csvf case.csv` yields `case.csv` plus `case_Downloads.csv` and so on.
pub const DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        id: "history",
        default_basename: "BrowserTriage_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: None,
    },
    DatasetSpec {
        id: "downloads",
        default_basename: "BrowserTriage_Output_Downloads",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_Downloads"),
    },
    DatasetSpec {
        id: "cookies",
        default_basename: "BrowserTriage_Output_Cookies",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_Cookies"),
    },
    DatasetSpec {
        id: "autofill",
        default_basename: "BrowserTriage_Output_Autofill",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_Autofill"),
    },
    DatasetSpec {
        id: "bookmarks",
        default_basename: "BrowserTriage_Output_Bookmarks",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_Bookmarks"),
    },
    DatasetSpec {
        id: "logins",
        default_basename: "BrowserTriage_Output_Logins",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_Logins"),
    },
    DatasetSpec {
        id: "keyword_searches",
        default_basename: "BrowserTriage_Output_KeywordSearches",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_KeywordSearches"),
    },
    DatasetSpec {
        id: "extensions",
        default_basename: "BrowserTriage_Output_Extensions",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_Extensions"),
    },
    DatasetSpec {
        id: "timeline",
        // framing is unused while csv_only is set; kept valid for symmetry.
        framing: JsonFraming::Ndjson,
        default_basename: "BrowserTriage_Output_Timeline",
        csv_only: true,
        override_suffix: Some("_Timeline"),
    },
];

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

#[derive(Default)]
pub struct BrowserTool {
    /// When set, the derived `_Timeline` dataset is not emitted. The router
    /// opens dataset files lazily on first write, so simply never writing to
    /// the dataset leaves no empty file behind.
    pub no_timeline: bool,
}

impl BrowserTool {
    pub fn new(no_timeline: bool) -> Self {
        BrowserTool { no_timeline }
    }
}

/// First 16 bytes, or fewer if the file is shorter.
fn read_magic(path: &Path) -> Option<[u8; 16]> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 16];
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => return None,
            Ok(n) => filled += n,
            Err(_) => return None,
        }
    }
    Some(buf)
}

/// First non-whitespace byte, skipping a UTF-8 BOM. Reads a small prefix only:
/// `Preferences` can be several megabytes and validation must not parse it.
fn first_meaningful_byte(path: &Path) -> Option<u8> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 64];
    let n = file.read(&mut buf).ok()?;
    let mut bytes = &buf[..n];
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bytes = &bytes[3..];
    }
    bytes.iter().copied().find(|b| !b.is_ascii_whitespace())
}

impl Tool for BrowserTool {
    fn binary_name(&self) -> &'static str {
        "BrowserTriage"
    }

    fn patterns(&self) -> &[&'static str] {
        artifact::PATTERNS
    }

    fn validate_legacy(&self, path: &Path) -> bool {
        matches!(self.validate(path), Validation::Supported)
    }

    /// Overridden rather than relying on the default, because the right verdict
    /// depends on how strongly the filename identifies the artifact — the same
    /// distinction `SqleTool` makes. `History` without a SQLite header is
    /// damaged evidence and should be audited as `Corrupt`; a `Preferences`
    /// belonging to some unrelated application is merely `Unsupported` and
    /// should pass quietly.
    fn validate(&self, path: &Path) -> Validation {
        if let Err(error) = std::fs::File::open(path) {
            return Validation::Unreadable {
                error: error.to_string(),
            };
        }
        let Some(kind) = artifact::classify(path) else {
            // Only reachable through an explicit `-f` on a non-artifact file;
            // discovery would never offer it.
            return Validation::Unsupported {
                reason: "not a known browser artifact filename".into(),
            };
        };
        match artifact::backing(kind) {
            Backing::Sqlite => {
                if read_magic(path).as_ref() == Some(SQLITE_MAGIC) {
                    Validation::Supported
                } else {
                    Validation::Corrupt {
                        reason: "browser SQLite artifact has an invalid SQLite header".into(),
                    }
                }
            }
            Backing::Json {
                generic_name: false,
            } => match first_meaningful_byte(path) {
                Some(b'{') => Validation::Supported,
                _ => Validation::Corrupt {
                    reason: "browser JSON artifact does not begin with an object".into(),
                },
            },
            Backing::Json { generic_name: true } => {
                // `Preferences` is a name macOS bundles, Java and others use
                // too. Require a JSON object AND a resolvable Chromium profile
                // anchor in the path before claiming it, so a false positive is
                // a quiet skip rather than a reported corruption.
                match first_meaningful_byte(path) {
                    Some(b'{') => Validation::Supported,
                    _ => Validation::Unsupported {
                        reason: "not a Chromium profile Preferences file".into(),
                    },
                }
            }
        }
    }

    fn invalid_content_is_corrupt(&self) -> bool {
        true
    }

    /// Every row carries a `Profile`, so two byte-identical artifacts at two
    /// paths are two findings, not one.
    ///
    /// A browser update leaves `Snapshots/<version>/Default` copies that are
    /// byte-identical to each other and to the live profile. Content dedupe
    /// would keep the rows but attribute them all to whichever profile was
    /// discovered first, which is a silent loss of exactly the attribution
    /// this tool exists to provide. Duplicate content is legible in the output
    /// — the `Profile` and `Source File` columns say which copy a row came
    /// from — whereas missing attribution is not recoverable.
    fn dedupe_by_content(&self) -> bool {
        false
    }

    fn datasets(&self) -> &'static [DatasetSpec] {
        DATASETS
    }

    /// Browser artifacts live under a user profile, so an artifact found
    /// outside one belongs in `users/unknown` rather than being attributed to
    /// the system.
    fn scope(&self) -> Scope {
        Scope::UserSpecific
    }

    /// Opens SQLite databases and may copy a WAL set into a temporary
    /// directory, so it is paced against the other heavy parsers.
    fn resource_class(&self) -> ResourceClass {
        ResourceClass::Heavy
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        let Some(kind) = artifact::classify(path) else {
            return Ok(0);
        };
        let id = profile::identify(path, kind);
        let source = path.display().to_string();
        let mut timeline = Timeline::new(&id, &source, !self.no_timeline);
        let mut subs = SubParsers::new();

        match kind {
            ArtifactKind::ChromiumHistory => {
                let db = open_db(path)?;
                // One damaged table must not cost us the others: `History`
                // carries history, downloads and searches, and a failure in
                // any one of them is a per-table problem, not a per-file one.
                subs.run(
                    chromium::history::parse(&db, path, &id, out, &mut timeline),
                    path,
                    "urls/visits",
                )?;
                subs.run(
                    chromium::downloads::parse(&db, path, &id, out, &mut timeline),
                    path,
                    "downloads",
                )?;
                subs.run(
                    chromium::keywords::parse(&db, path, &id, out, &mut timeline),
                    path,
                    "keyword_search_terms",
                )?;
            }
            ArtifactKind::ChromiumCookies => {
                let db = open_db(path)?;
                subs.run(
                    chromium::cookies::parse(&db, path, &id, out, &mut timeline),
                    path,
                    "cookies",
                )?;
            }
            ArtifactKind::ChromiumWebData => {
                let db = open_db(path)?;
                subs.run(
                    chromium::autofill::parse(&db, path, &id, out, &mut timeline),
                    path,
                    "autofill",
                )?;
            }
            ArtifactKind::ChromiumLogins => {
                let db = open_db(path)?;
                subs.run(
                    chromium::logins::parse(&db, path, &id, out, &mut timeline),
                    path,
                    "logins",
                )?;
            }
            ArtifactKind::ChromiumBookmarks => {
                subs.run(
                    chromium::bookmarks::parse(path, &id, out, &mut timeline),
                    path,
                    "Bookmarks",
                )?;
            }
            ArtifactKind::ChromiumPreferences => {
                subs.run(
                    chromium::extensions::parse(path, &id, out, &mut timeline),
                    path,
                    "Preferences extensions",
                )?;
            }
            ArtifactKind::FirefoxPlaces => {
                let db = open_db(path)?;
                // places.sqlite holds four of the eight artifacts, so a
                // failure in one table must not cost the other three.
                subs.run(
                    firefox::history::parse(&db, path, &id, out, &mut timeline),
                    path,
                    "moz_places/moz_historyvisits",
                )?;
                subs.run(
                    firefox::downloads::parse(&db, path, &id, out, &mut timeline),
                    path,
                    "moz_annos downloads",
                )?;
                subs.run(
                    firefox::bookmarks::parse(&db, path, &id, out, &mut timeline),
                    path,
                    "moz_bookmarks",
                )?;
                subs.run(
                    firefox::keywords::parse(&db, path, &id, out, &mut timeline),
                    path,
                    "firefox search terms",
                )?;
            }
            ArtifactKind::FirefoxCookies => {
                let db = open_db(path)?;
                subs.run(
                    firefox::cookies::parse(&db, path, &id, out, &mut timeline),
                    path,
                    "moz_cookies",
                )?;
            }
            ArtifactKind::FirefoxFormHistory => {
                let db = open_db(path)?;
                subs.run(
                    firefox::autofill::parse(&db, path, &id, out, &mut timeline),
                    path,
                    "moz_formhistory",
                )?;
            }
            ArtifactKind::FirefoxLogins => {
                subs.run(
                    firefox::logins::parse(path, &id, out, &mut timeline),
                    path,
                    "logins.json",
                )?;
            }
            ArtifactKind::FirefoxExtensions => {
                subs.run(
                    firefox::extensions::parse(path, &id, out, &mut timeline),
                    path,
                    "extensions.json",
                )?;
            }
        }

        Ok(subs.finish()? + timeline.emitted)
    }
}

/// Run one sub-parser, adding its rows to the running count.
///
/// A source-data failure is warned about and skipped so its siblings still
/// produce output — one broken `downloads` table must not cost us the history
/// rows in the same file. An *output* failure propagates, because the shared
/// runner treats `TriageError::Output` as terminal and continuing would risk a
/// partially written file.
pub(crate) fn soft(
    result: Result<u64, TriageError>,
    path: &Path,
    what: &str,
    total: &mut u64,
) -> Result<(), TriageError> {
    let mut subs = SubParsers::new();
    subs.written = *total;
    subs.run(result, path, what)?;
    *total = subs.written;
    Ok(())
}

/// Accumulates the sub-parsers of one artifact.
///
/// A single file can hold several independent tables — `History` carries three
/// and `places.sqlite` four — and one damaged table must not cost the others.
/// But a file whose tables *all* failed must be recorded as a failure rather
/// than as a successfully parsed file that happened to contain nothing, because
/// an empty dataset is otherwise indistinguishable from an empty profile.
pub(crate) struct SubParsers {
    pub written: u64,
    first_error: Option<TriageError>,
}

impl SubParsers {
    pub(crate) fn new() -> Self {
        SubParsers {
            written: 0,
            first_error: None,
        }
    }

    /// Run one sub-parser, adding its rows to the running count.
    ///
    /// A source-data failure is warned about and remembered so its siblings
    /// still produce output. An *output* failure propagates immediately,
    /// because the shared runner treats `TriageError::Output` as terminal and
    /// continuing would risk a partially written file.
    pub(crate) fn run(
        &mut self,
        result: Result<u64, TriageError>,
        path: &Path,
        what: &str,
    ) -> Result<(), TriageError> {
        match result {
            Ok(rows) => {
                self.written += rows;
                Ok(())
            }
            Err(error @ TriageError::Output { .. }) => Err(error),
            Err(error) => {
                tracing::warn!("{}: {what}: {error}", path.display());
                if self.first_error.is_none() {
                    self.first_error = Some(error);
                }
                Ok(())
            }
        }
    }

    /// The row count, or the first failure when nothing at all was written.
    pub(crate) fn finish(self) -> Result<u64, TriageError> {
        match self.first_error {
            Some(error) if self.written == 0 => Err(error),
            _ => Ok(self.written),
        }
    }
}

/// Open an evidence database through `triage-sqlite`, which opens read-only and
/// `immutable=1` when there is no WAL, and otherwise copies the `{db,-wal,-shm}`
/// set to a temporary directory before checkpointing the copy. A plain
/// read-write open would checkpoint an attached WAL on close and rewrite the
/// original — the one thing a forensic tool must never do.
fn open_db(path: &Path) -> Result<triage_sqlite::Database, TriageError> {
    triage_sqlite::Database::open(path).map_err(|e| TriageError::Artifact {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write(dir: &TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    fn sqlite_bytes() -> Vec<u8> {
        let mut bytes = SQLITE_MAGIC.to_vec();
        bytes.extend_from_slice(&[0u8; 64]);
        bytes
    }

    /// `--csvf` is rejected when more than one dataset is primary, so the
    /// count is a contract, not an accident.
    #[test]
    fn exactly_one_dataset_is_primary() {
        let primaries = DATASETS
            .iter()
            .filter(|d| d.override_suffix.is_none())
            .count();
        assert_eq!(primaries, 1, "exactly one dataset may omit override_suffix");
        assert_eq!(DATASETS.len(), 9);
    }

    #[test]
    fn dataset_ids_and_basenames_are_unique() {
        let mut ids: Vec<&str> = DATASETS.iter().map(|d| d.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), DATASETS.len(), "dataset ids must be unique");

        let mut names: Vec<&str> = DATASETS.iter().map(|d| d.default_basename).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), DATASETS.len(), "basenames must be unique");
    }

    /// The timeline duplicates timestamps already present in the typed
    /// datasets, so a JSON copy of it would be pure bloat.
    #[test]
    fn only_the_timeline_is_csv_only() {
        for spec in DATASETS {
            assert_eq!(spec.csv_only, spec.id == "timeline", "{}", spec.id);
        }
    }

    #[test]
    fn a_sqlite_artifact_with_a_good_header_is_supported() {
        let dir = TempDir::new().unwrap();
        let tool = BrowserTool::default();
        for name in ["History", "Cookies", "Web Data", "places.sqlite"] {
            let path = write(&dir, name, &sqlite_bytes());
            assert_eq!(tool.validate(&path), Validation::Supported, "{name}");
        }
    }

    /// A file *named* `History` that is not SQLite is damaged evidence, not an
    /// unrelated file that happens to share the name.
    #[test]
    fn a_sqlite_artifact_with_a_bad_header_is_corrupt() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "History", b"this is not a database at all!!!");
        assert!(matches!(
            BrowserTool::default().validate(&path),
            Validation::Corrupt { .. }
        ));
    }

    #[test]
    fn a_truncated_file_is_corrupt_rather_than_panicking() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "History", b"SQLi");
        assert!(matches!(
            BrowserTool::default().validate(&path),
            Validation::Corrupt { .. }
        ));
    }

    #[test]
    fn json_artifacts_are_recognized_by_their_opening_brace() {
        let dir = TempDir::new().unwrap();
        let tool = BrowserTool::default();
        for name in ["Bookmarks", "logins.json", "extensions.json"] {
            let path = write(&dir, name, b"\n  {\"roots\": {}}");
            assert_eq!(tool.validate(&path), Validation::Supported, "{name}");
        }
    }

    #[test]
    fn a_utf8_bom_does_not_defeat_json_detection() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "Bookmarks", b"\xEF\xBB\xBF{\"roots\":{}}");
        assert_eq!(
            BrowserTool::default().validate(&path),
            Validation::Supported
        );
    }

    /// `Preferences` is shared with unrelated software, so a non-JSON one is a
    /// quiet skip; `Bookmarks` is unambiguous, so a non-JSON one is corruption.
    #[test]
    fn a_generic_name_degrades_to_unsupported_not_corrupt() {
        let dir = TempDir::new().unwrap();
        let tool = BrowserTool::default();

        let prefs = write(&dir, "Preferences", b"# an ini file, not JSON\n");
        assert!(matches!(
            tool.validate(&prefs),
            Validation::Unsupported { .. }
        ));

        let bookmarks = write(&dir, "Bookmarks", b"# an ini file, not JSON\n");
        assert!(matches!(
            tool.validate(&bookmarks),
            Validation::Corrupt { .. }
        ));
    }

    #[test]
    fn an_unknown_filename_is_unsupported() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "notes.txt", b"hello");
        assert!(matches!(
            BrowserTool::default().validate(&path),
            Validation::Unsupported { .. }
        ));
    }

    #[test]
    fn a_missing_file_is_unreadable() {
        assert!(matches!(
            BrowserTool::default().validate(Path::new("/definitely/not/here/History")),
            Validation::Unreadable { .. }
        ));
    }

    /// This tool must opt out of the orchestrator's content dedupe. A browser
    /// update leaves `Snapshots/<version>` copies byte-identical to the live
    /// profile, and every row here carries a `Profile` derived from the path,
    /// so collapsing them would keep the content and silently drop the second
    /// profile's attribution. Flipping this back to the default would look
    /// harmless and quietly reintroduce that loss.
    #[test]
    fn content_dedupe_is_off_because_profile_comes_from_the_path() {
        assert!(!BrowserTool::default().dedupe_by_content());
        assert!(
            !BrowserTool::new(true).dedupe_by_content(),
            "--no-timeline must not change the dedupe policy"
        );
    }
}
