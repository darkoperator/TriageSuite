//! Pre-flight capture validation, mirroring VeloProcessor's Test-VeloCollection.
//!
//! Checking a capture actually contains the artifacts the parsers need,
//! before processing it, catches two real failure shapes early: a capture
//! that is missing whole artifact classes (wrong collector profile), and a
//! "double-zipped" collection -- an archive whose only entry is another
//! archive -- which otherwise silently produces a run with zero matches for
//! every tool.
//!
//! Everything here checks *one* capture. A folder of collector ZIPs, or of
//! collections, is a container rather than a capture: its own file list is
//! archive names, not artifacts, so checking it as one capture fails every
//! criterion at once and reads a lone archive inside it as a double-zip.
//! Callers must therefore expand a container first -- `run` by validating
//! each collection enumeration found, `validate` via [`validate_input`].

use std::path::{Path, PathBuf};

use crate::archive::is_zip_path;

/// Registry hives every parser that reads them needs. Checked as errors:
/// their absence means whole tool categories cannot run at all.
const REQUIRED_HIVES: [&str; 4] = ["SYSTEM", "SOFTWARE", "SAM", "SECURITY"];

/// What both paths say about a collection that was zipped twice.
///
/// `validate` reaches this conclusion here, from a capture's entry list;
/// `run` reaches it in `archive::probe`, which rejects the archive before it
/// is ever extracted and so never gets as far as a capture to validate. The
/// two describe the same packaging mistake to the same analyst, so they say
/// it with the same words rather than drifting apart.
pub const DOUBLE_ZIPPED: &str =
    "double-zipped collection: the archive contains one .zip and nothing else";

pub struct ValidationReport {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationReport {
    fn error(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
        self.valid = false;
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
}

/// Check a capture for the artifacts the parsers need.
///
/// Never returns `Err`: an absent, unreadable or malformed input is a finding
/// to report, not an error to propagate -- which is also what makes this
/// total over hostile paths. Errors block processing for this input;
/// warnings do not.
pub fn validate_capture(path: &Path) -> ValidationReport {
    let mut report = ValidationReport {
        valid: true,
        errors: Vec::new(),
        warnings: Vec::new(),
    };

    if !path.exists() {
        report.error(format!("input does not exist: {}", path.display()));
        return report;
    }

    // Inventory what the capture holds. For a .zip this reads the central
    // directory; for a directory it walks. Either way a read failure is a
    // finding, not a panic.
    let names = match inventory(path) {
        Ok(names) => names,
        Err(message) => {
            report.error(message);
            return report;
        }
    };

    // A collector zip containing exactly one .zip and nothing else was
    // zipped twice. Processing it finds no artifacts at all, which presents
    // as an empty capture rather than as the packaging mistake it is.
    let zips = names.iter().filter(|n| n.ends_with(".zip")).count();
    if zips == 1 && names.len() == 1 {
        report.error(DOUBLE_ZIPPED);
        return report;
    }

    // Entry/path separators are always `/`, both in a zip's central
    // directory (per the zip spec) and in the relative paths `inventory`
    // builds for a directory walk -- so this substring check works the same
    // way regardless of host OS or of whether Velociraptor URL-encoded the
    // segments above `config` (`uploads/auto/C%3A/...`). Those encoded
    // segments never touch `config` or the hive filename itself, so a raw
    // mounted tree and a URL-encoded collector zip both match here
    // unmodified. Verified against a real capture in `test captures/`.
    let has = |needle: &str| {
        let needle = needle.to_ascii_lowercase();
        names
            .iter()
            .any(|n| n.to_ascii_lowercase().contains(&needle))
    };

    if !names
        .iter()
        .any(|n| n.to_ascii_lowercase().ends_with(".evtx"))
    {
        report.error("no event log (.evtx) files found");
    }
    for hive in REQUIRED_HIVES {
        if !has(&format!("config/{hive}")) {
            report.error(format!("registry hive not found: {hive}"));
        }
    }
    if !has("$mft") {
        report.warn("no $MFT found: file system timeline output will be absent");
    }
    if !has("ntuser.dat") {
        report.warn("no NTUSER.DAT found: per-user registry output will be absent");
    }
    if !has("amcache.hve") {
        report.warn("no Amcache.hve found: Amcache execution output will be absent");
    }
    report
}

/// Check a user-supplied input, expanding a container into the individual
/// captures it holds first.
///
/// The captures are the collector ZIPs and collection directories sitting
/// directly inside a folder. Everything else -- a `.zip`, a collection
/// directory, a raw mounted tree, a path that does not exist -- is one
/// capture and expands to itself, so the single-capture case is exactly
/// [`validate_capture`]. Results come back in path order, each paired with
/// the input it describes, because a folder's findings are only actionable
/// when they name the archive they came from.
pub fn validate_input(path: &Path) -> Vec<(PathBuf, ValidationReport)> {
    let mut inputs: Vec<PathBuf> = Vec::new();
    // A collection directory is a capture, not a container, even though it
    // is a directory: expanding it would validate any stray `.zip` sitting
    // inside the collection instead of the collection itself.
    if path.is_dir() && !crate::capture::is_collection(path) {
        inputs.extend(crate::archive::find_archives(path, &[]));
        inputs.extend(
            crate::capture::collect_collections(path)
                .into_iter()
                .map(|host| host.collection_dir),
        );
    }
    if inputs.is_empty() {
        inputs.push(path.to_path_buf());
    }
    inputs.sort();
    inputs
        .into_iter()
        .map(|input| {
            let report = validate_capture(&input);
            (input, report)
        })
        .collect()
}

/// Entry names inside a `.zip` (read from the central directory only -- no
/// entry is decompressed), or relative paths under a directory, joined with
/// `/` regardless of host OS so they compare the same way as zip entry
/// names.
fn inventory(path: &Path) -> Result<Vec<String>, String> {
    if is_zip_path(path) {
        return inventory_zip(path);
    }
    if path.is_dir() {
        return inventory_dir(path);
    }
    Err(format!(
        "not a .zip archive or a capture directory: {}",
        path.display()
    ))
}

fn inventory_zip(path: &Path) -> Result<Vec<String>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let archive = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    Ok(archive.file_names().map(str::to_string).collect())
}

fn inventory_dir(root: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    // Relative subpaths still to visit, root first (empty relative path).
    let mut stack: Vec<PathBuf> = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let abs = root.join(&rel);
        let entries = std::fs::read_dir(&abs).map_err(|e| format!("{}: {e}", abs.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let file_type = entry.file_type().map_err(|e| e.to_string())?;
            // A capture is untrusted input: following a symlink here could
            // walk outside the capture root. Links are skipped, not
            // resolved, matching `veloresults::copy_tree`'s posture.
            if file_type.is_symlink() {
                continue;
            }
            let rel_child = if rel.as_os_str().is_empty() {
                PathBuf::from(entry.file_name())
            } else {
                rel.join(entry.file_name())
            };
            if file_type.is_dir() {
                stack.push(rel_child.clone());
                continue;
            }
            let joined = rel_child
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push(joined);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A capture directory holding valid registry hives (URL-encoded, as
    /// Velociraptor writes them) but no event logs at all.
    #[test]
    fn a_capture_missing_event_logs_is_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        let capture = tmp.path().join("cap");
        std::fs::create_dir_all(capture.join("uploads/auto/C%3A/Windows/System32/config")).unwrap();
        for hive in ["SYSTEM", "SOFTWARE", "SAM", "SECURITY"] {
            std::fs::write(
                capture.join(format!("uploads/auto/C%3A/Windows/System32/config/{hive}")),
                b"regf",
            )
            .unwrap();
        }
        let report = validate_capture(&capture);
        assert!(!report.valid);
        assert!(
            report.errors.iter().any(|e| e.contains("event log")),
            "got {:?}",
            report.errors
        );
    }

    /// Builds a capture with everything `validate_capture` checks for,
    /// except `$MFT`. Returns the owning `TempDir` too, so the caller can
    /// keep it alive for as long as the fixture is in use.
    fn fixture_with_everything_but_mft() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let capture = tmp.path().join("cap").join("uploads/auto/C%3A/Windows");
        std::fs::create_dir_all(capture.join("System32/config")).unwrap();
        std::fs::create_dir_all(capture.join("System32/winevt/Logs")).unwrap();
        for hive in ["SYSTEM", "SOFTWARE", "SAM", "SECURITY"] {
            std::fs::write(capture.join(format!("System32/config/{hive}")), b"regf").unwrap();
        }
        std::fs::write(capture.join("System32/winevt/Logs/Security.evtx"), b"evtx").unwrap();
        let root = tmp.path().join("cap");
        (tmp, root)
    }

    #[test]
    fn a_missing_mft_is_a_warning_not_an_error() {
        let (_tmp, capture) = fixture_with_everything_but_mft();
        let report = validate_capture(&capture);
        assert!(report.valid, "got {:?}", report.errors);
        assert!(
            report.warnings.iter().any(|w| w.contains("MFT")),
            "got {:?}",
            report.warnings
        );
    }

    /// A collector ZIP that contains a single ZIP and nothing else was
    /// zipped twice; processing it finds no artifacts and reports nothing
    /// useful.
    fn double_zipped_fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let inner = tmp.path().join("inner.zip");
        std::fs::write(&inner, b"not a real zip, contents are irrelevant here").unwrap();
        let outer = tmp.path().join("outer.zip");
        let f = std::fs::File::create(&outer).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("Collection-HOST.zip", opts).unwrap();
        std::io::Write::write_all(&mut w, &std::fs::read(&inner).unwrap()).unwrap();
        w.finish().unwrap();
        (tmp, outer)
    }

    #[test]
    fn a_double_zipped_collection_is_detected() {
        let (_tmp, zip) = double_zipped_fixture();
        let report = validate_capture(&zip);
        assert!(!report.valid);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.to_lowercase().contains("double")),
            "got {:?}",
            report.errors
        );
    }

    /// A folder of collector ZIPs is a container: checked as one capture it
    /// fails every criterion, and a folder holding exactly one archive is
    /// read as a double-zip. Expanding it first is what makes the findings
    /// about the archives themselves.
    #[test]
    fn a_folder_of_zips_expands_to_one_report_per_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("engagement-zips");
        std::fs::create_dir_all(&folder).unwrap();
        for host in ["HOSTA", "HOSTB"] {
            triage_testkit::synthetic::write_gate_passing_collection_zip(
                &folder.join(format!("Collection-{host}.zip")),
                "",
                host,
            );
        }

        let checked = validate_input(&folder);
        let names: Vec<String> = checked
            .iter()
            .map(|(path, _)| crate::file_name_lossy(path))
            .collect();
        assert_eq!(
            names,
            vec!["Collection-HOSTA.zip", "Collection-HOSTB.zip"],
            "each archive must be reported under its own name"
        );
        for (path, report) in &checked {
            assert!(report.valid, "{}: {:?}", path.display(), report.errors);
        }
    }

    /// The container rule must not swallow the single-capture case: a lone
    /// archive in a folder is one archive, not a double-zip.
    #[test]
    fn a_folder_holding_one_archive_is_not_a_double_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("one");
        std::fs::create_dir_all(&folder).unwrap();
        triage_testkit::synthetic::write_gate_passing_collection_zip(
            &folder.join("Collection-ONLY.zip"),
            "",
            "ONLY",
        );

        let checked = validate_input(&folder);
        assert_eq!(checked.len(), 1);
        assert!(checked[0].1.valid, "got {:?}", checked[0].1.errors);
    }

    /// A collection directory is a capture even though it is a directory:
    /// expanding it would check a `.zip` that happens to sit inside the
    /// collection instead of the collection itself.
    #[test]
    fn a_collection_directory_is_checked_as_one_capture() {
        let tmp = tempfile::tempdir().unwrap();
        let collection = tmp.path().join("Collection-HOST");
        triage_testkit::synthetic::write_gate_passing_collection(&collection, "HOST");
        // A stray archive sitting in the collection root, which an expansion
        // would mistake for the thing to check.
        triage_testkit::synthetic::write_collection_zip(
            &collection.join("some-other-capture.zip"),
            "",
            "OTHER",
        );

        let checked = validate_input(&collection);
        assert_eq!(checked.len(), 1);
        assert_eq!(checked[0].0, collection);
        assert!(checked[0].1.valid, "got {:?}", checked[0].1.errors);
    }

    /// An input that is not a container at all still behaves exactly as
    /// `validate_capture` does, including for a path that does not exist.
    #[test]
    fn a_single_capture_expands_to_itself() {
        let (_tmp, zip) = double_zipped_fixture();
        let checked = validate_input(&zip);
        assert_eq!(checked.len(), 1);
        assert_eq!(checked[0].0, zip);
        assert!(!checked[0].1.valid);

        let missing = std::path::Path::new("/nonexistent/capture");
        let checked = validate_input(missing);
        assert_eq!(checked.len(), 1);
        assert!(!checked[0].1.valid);
    }

    #[test]
    fn validation_never_panics_on_a_hostile_path() {
        for path in ["", "/nonexistent", "/dev/null", "\u{0}"] {
            let _ = validate_capture(std::path::Path::new(path));
        }
    }

    /// The hive check must also work against a raw mounted tree, where
    /// Velociraptor's `%3A`/`%5C` URL-encoding of the parent segments is
    /// absent (e.g. a capture already extracted or mounted by hand).
    #[test]
    fn hive_check_matches_a_raw_unencoded_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let capture = tmp.path().join("cap");
        std::fs::create_dir_all(capture.join("C/Windows/System32/config")).unwrap();
        std::fs::create_dir_all(capture.join("C/Windows/System32/winevt/Logs")).unwrap();
        for hive in ["SYSTEM", "SOFTWARE", "SAM", "SECURITY"] {
            std::fs::write(
                capture.join(format!("C/Windows/System32/config/{hive}")),
                b"regf",
            )
            .unwrap();
        }
        std::fs::write(
            capture.join("C/Windows/System32/winevt/Logs/Security.evtx"),
            b"evtx",
        )
        .unwrap();
        let report = validate_capture(&capture);
        assert!(report.valid, "got {:?}", report.errors);
    }
}
