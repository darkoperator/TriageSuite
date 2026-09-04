//! ZIP capture-archive support: locate archives, decide cheaply whether one
//! holds a Velociraptor collection, and extract it safely.
//!
//! Velociraptor's offline collector ships captures as ZIPs, so requiring an
//! already-unzipped tree meant every run started with a manual unzip. This
//! module lets `TriageSuite run` take a `.zip`, or a folder of them.
//!
//! Two properties matter more here than in ordinary unzip code:
//!
//! * **Filenames are load-bearing.** `triage_core::discovery` matches on the
//!   filename component only, and Velociraptor URL-encodes path segments
//!   (`uploads/auto/C%3A/...`, `$UsnJrnl%3A$J`). Entry names are therefore
//!   written verbatim — never percent-decoded — or those artifacts become
//!   invisible to every parser.
//! * **The input is untrusted.** Archives come from incident hosts, so
//!   extraction rejects path traversal and symlinks, and caps expansion.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};

use zip::result::ZipError;
use zip::ZipArchive;

use crate::capture::COLLECTION_MARKERS;
use crate::MAX_REASON_SAMPLES;

/// Reason an archive did not become a capture. Every variant is a skip, not a
/// hard failure: one bad file in a drop folder must not abort the whole run.
#[derive(Debug, Clone)]
pub enum ArchiveSkip {
    /// Not a zip at all, or a corrupt central directory.
    NotAnArchive(String),
    /// At least one entry is encrypted. Unencrypted archives only, for now.
    Encrypted,
    /// A valid zip that simply isn't a capture.
    NotACollection,
    /// Entries share byte ranges — ambiguous, and a known attack shape.
    Overlapping,
    /// An existing extraction blocks reuse and `--overwrite` was not given.
    Stale(String),
}

impl std::fmt::Display for ArchiveSkip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArchiveSkip::NotAnArchive(e) => write!(f, "not a valid zip archive ({e})"),
            ArchiveSkip::Encrypted => f.write_str("encrypted archives are not supported"),
            ArchiveSkip::NotACollection => f.write_str(
                "no Velociraptor collection inside (uploads.json + client_info.json not found)",
            ),
            ArchiveSkip::Overlapping => f.write_str("archive has overlapping entries"),
            ArchiveSkip::Stale(why) => write!(f, "{why}; rerun with --overwrite"),
        }
    }
}

/// What a cheap central-directory read learned about an archive.
#[derive(Debug, Clone)]
pub struct ArchiveInspection {
    pub path: PathBuf,
    pub entries: usize,
    /// Total uncompressed size if the archive declares it. `None` when data
    /// descriptors are used, which also disables the percentage heartbeat.
    pub declared_uncompressed: Option<u128>,
}

pub enum Probe {
    Usable(Box<ArchiveInspection>),
    Skip(ArchiveSkip),
}

/// Limits that keep a malicious archive from filling the output volume.
/// Generous by design: a real `$MFT` or `pagefile.sys` is legitimately huge,
/// so these stop bombs, not forensics.
#[derive(Debug, Clone)]
pub struct ExtractOptions {
    pub max_entry_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            max_entry_bytes: 64 * 1024 * 1024 * 1024,       // 64 GiB
            max_total_bytes: 2 * 1024 * 1024 * 1024 * 1024, // 2 TiB
        }
    }
}

/// Outcome of extracting (or reusing) one archive.
#[derive(Debug, Clone)]
pub struct ExtractReport {
    pub archive: PathBuf,
    pub dest: PathBuf,
    pub reused: bool,
    pub re_extracted: bool,
    pub files_written: u64,
    pub bytes_written: u64,
    pub skipped_entries: u64,
    /// Capped at `MAX_REASON_SAMPLES`, matching `ToolRunResult::reason_samples`.
    pub skipped_reasons: Vec<String>,
    pub error: Option<String>,
    pub duration: std::time::Duration,
}

impl ExtractReport {
    /// A report for an archive that was not extracted this run because a
    /// previous, still-valid extraction at `dest` was reused. The counts are
    /// the ones that run recorded.
    pub fn reused(archive: &Path, dest: &Path, files_written: u64, bytes_written: u64) -> Self {
        ExtractReport {
            archive: archive.to_path_buf(),
            dest: dest.to_path_buf(),
            reused: true,
            re_extracted: false,
            files_written,
            bytes_written,
            skipped_entries: 0,
            skipped_reasons: Vec::new(),
            error: None,
            duration: std::time::Duration::default(),
        }
    }
}

pub fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

/// Archives directly inside `dir`, sorted for deterministic ordering. One
/// level only, matching `capture`'s one-level scan for collections.
pub fn find_archives(dir: &Path, exclude: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_file() && is_zip_path(&p) && !exclude.iter().any(|x| p.starts_with(x)) {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Split a zip entry name into meaningful path segments, dropping the noise
/// archivers add. Returns `None` for entries that should be ignored entirely.
fn segments(name: &str) -> Option<Vec<&str>> {
    let parts: Vec<&str> = name
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    if parts.is_empty() || parts[0] == "__MACOSX" {
        return None;
    }
    Some(parts)
}

/// Does this archive contain a collection at depth 0 or 1?
///
/// Both depths are accepted because the collector writes the collection at the
/// archive root, while a re-zipped capture usually carries a wrapper directory
/// named after the collection. Extraction preserves whichever it is, and
/// `capture::collect_collections` then handles both, since it checks the
/// directory itself *and* its immediate children.
fn holds_collection(archive: &ZipArchive<BufReader<File>>) -> bool {
    // prefix ("" for the archive root) -> which markers were seen under it
    let mut seen: std::collections::HashMap<String, [bool; 2]> = std::collections::HashMap::new();
    for name in archive.file_names() {
        let Some(parts) = segments(name) else {
            continue;
        };
        let (prefix, file) = match parts.len() {
            1 => (String::new(), parts[0]),
            2 => (parts[0].to_string(), parts[1]),
            _ => continue, // deeper than one wrapper directory
        };
        for (i, marker) in COLLECTION_MARKERS.iter().enumerate() {
            if file.eq_ignore_ascii_case(marker) {
                seen.entry(prefix.clone()).or_default()[i] = true;
            }
        }
    }
    seen.values().any(|found| found.iter().all(|f| *f))
}

/// Decide an archive's fate by reading only its central directory — no entry
/// is decompressed, so a folder of unrelated ZIPs costs almost nothing.
pub fn probe(path: &Path) -> Probe {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => return Probe::Skip(ArchiveSkip::NotAnArchive(e.to_string())),
    };
    let mut archive = match ZipArchive::new(BufReader::new(file)) {
        Ok(a) => a,
        Err(e) => return Probe::Skip(ArchiveSkip::NotAnArchive(e.to_string())),
    };
    if archive.is_empty() {
        return Probe::Skip(ArchiveSkip::NotACollection);
    }
    if matches!(archive.has_overlapping_files(), Ok(true)) {
        return Probe::Skip(ArchiveSkip::Overlapping);
    }
    if !holds_collection(&archive) {
        return Probe::Skip(ArchiveSkip::NotACollection);
    }
    // Encryption is rejected up front rather than mid-extraction. The crate
    // checks the encrypted bit before locating content, so this stays cheap.
    for i in 0..archive.len() {
        if let Err(ZipError::UnsupportedArchive(msg)) = archive.by_index(i) {
            if msg == ZipError::PASSWORD_REQUIRED {
                return Probe::Skip(ArchiveSkip::Encrypted);
            }
        }
    }
    Probe::Usable(Box::new(ArchiveInspection {
        path: path.to_path_buf(),
        entries: archive.len(),
        declared_uncompressed: archive.decompressed_size(),
    }))
}

/// Extract `archive` into `dest`.
///
/// Deliberately hand-rolled rather than `ZipArchive::extract`, which aborts the
/// whole archive on the first unsupported entry and applies archive-supplied
/// unix permissions. Read-only directories from an archive would later break
/// re-extraction, and one odd entry should never cost the whole capture.
pub fn extract(
    insp: &ArchiveInspection,
    dest: &Path,
    opts: &ExtractOptions,
    mut on_progress: impl FnMut(u64, Option<u128>),
) -> ExtractReport {
    let started = std::time::Instant::now();
    let mut report = ExtractReport {
        archive: insp.path.clone(),
        dest: dest.to_path_buf(),
        reused: false,
        re_extracted: false,
        files_written: 0,
        bytes_written: 0,
        skipped_entries: 0,
        skipped_reasons: Vec::new(),
        error: None,
        duration: std::time::Duration::default(),
    };

    let note = |report: &mut ExtractReport, reason: String| {
        report.skipped_entries += 1;
        if report.skipped_reasons.len() < MAX_REASON_SAMPLES {
            report.skipped_reasons.push(reason);
        }
    };

    let opened = File::open(&insp.path)
        .map_err(|e| e.to_string())
        .and_then(|f| ZipArchive::new(BufReader::new(f)).map_err(|e| e.to_string()))
        .and_then(|a| {
            std::fs::create_dir_all(dest)
                .map(|_| a)
                .map_err(|e| e.to_string())
        });
    let mut archive = match opened {
        Ok(a) => a,
        Err(e) => {
            report.error = Some(e);
            report.duration = started.elapsed();
            return report;
        }
    };

    let mut next_tick: u128 = 0;
    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(f) => f,
            Err(ZipError::UnsupportedArchive(msg)) => {
                note(&mut report, format!("unsupported compression: {msg}"));
                continue;
            }
            Err(e) => {
                note(&mut report, e.to_string());
                continue;
            }
        };

        // enclosed_name() is the traversal guard: it rejects `..`, absolute
        // paths, NUL bytes and Windows drive prefixes in one step.
        let Some(rel) = entry.enclosed_name() else {
            let raw = String::from_utf8_lossy(entry.name_raw()).into_owned();
            note(&mut report, format!("unsafe entry path: {raw}"));
            continue;
        };
        let outpath = dest.join(&rel);
        if !outpath.starts_with(dest) {
            note(&mut report, format!("unsafe entry path: {}", rel.display()));
            continue;
        }

        if entry.is_symlink() {
            note(&mut report, format!("symlink skipped: {}", rel.display()));
            continue;
        }
        if entry.is_dir() {
            if let Err(e) = std::fs::create_dir_all(&outpath) {
                note(&mut report, format!("{}: {e}", rel.display()));
            }
            continue;
        }
        if entry.size() > opts.max_entry_bytes {
            note(
                &mut report,
                format!("{} exceeds per-entry size limit", rel.display()),
            );
            continue;
        }
        if let Some(parent) = outpath.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                note(&mut report, format!("{}: {e}", rel.display()));
                continue;
            }
        }

        let mut out = match File::create(&outpath) {
            Ok(f) => f,
            Err(e) => {
                note(&mut report, format!("{}: {e}", rel.display()));
                continue;
            }
        };
        // The declared size is attacker-controlled, so cap the actual copy too.
        let mut limited = (&mut entry).take(opts.max_entry_bytes + 1);
        match io::copy(&mut limited, &mut out) {
            Ok(n) if n > opts.max_entry_bytes => {
                drop(out);
                let _ = std::fs::remove_file(&outpath);
                note(
                    &mut report,
                    format!("{} exceeds per-entry size limit", rel.display()),
                );
            }
            Ok(n) => {
                report.files_written += 1;
                report.bytes_written += n;
            }
            Err(e) => {
                // Includes CRC mismatch, which the reader validates at EOF.
                drop(out);
                let _ = std::fs::remove_file(&outpath);
                note(&mut report, format!("{}: {e}", rel.display()));
            }
        }

        if report.bytes_written > opts.max_total_bytes {
            report.error = Some("expansion limit exceeded".to_string());
            break;
        }

        if let Some(total) = insp.declared_uncompressed {
            // Append-only heartbeat every 5%, matching triage-cli's convention.
            let step = (total / 20).max(1);
            if report.bytes_written as u128 >= next_tick {
                on_progress(report.bytes_written, Some(total));
                next_tick = (report.bytes_written as u128 / step + 1) * step;
            }
        }
    }

    report.duration = started.elapsed();
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use triage_testkit::synthetic::write_collection_zip;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    /// Build a zip from (name, contents) pairs, for the tests that need
    /// entries the standard synthetic collection does not have. Stored, so
    /// these tests don't depend on the deflate backend.
    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let f = File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, body) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn probes_collection_at_archive_root() {
        let td = tempfile::tempdir().unwrap();
        let z = td.path().join("c.zip");
        write_collection_zip(&z, "", "H1");
        assert!(matches!(probe(&z), Probe::Usable(_)));
    }

    #[test]
    fn probes_collection_under_a_wrapper_directory() {
        let td = tempfile::tempdir().unwrap();
        let z = td.path().join("c.zip");
        write_collection_zip(&z, "Collection-H1-2026/", "H1");
        assert!(matches!(probe(&z), Probe::Usable(_)));
    }

    #[test]
    fn valid_zip_without_markers_is_not_a_collection() {
        let td = tempfile::tempdir().unwrap();
        let z = td.path().join("notes.zip");
        make_zip(&z, &[("notes.txt", b"hello")]);
        assert!(matches!(
            probe(&z),
            Probe::Skip(ArchiveSkip::NotACollection)
        ));
    }

    #[test]
    fn garbage_bytes_are_not_an_archive() {
        let td = tempfile::tempdir().unwrap();
        let z = td.path().join("broken.zip");
        std::fs::write(&z, b"this is not a zip file at all").unwrap();
        assert!(matches!(
            probe(&z),
            Probe::Skip(ArchiveSkip::NotAnArchive(_))
        ));
    }

    #[test]
    fn markers_deeper_than_one_wrapper_do_not_count() {
        let td = tempfile::tempdir().unwrap();
        let z = td.path().join("deep.zip");
        write_collection_zip(&z, "a/b/c/", "H1");
        assert!(matches!(
            probe(&z),
            Probe::Skip(ArchiveSkip::NotACollection)
        ));
    }

    #[test]
    fn extract_preserves_url_encoded_names_verbatim() {
        let td = tempfile::tempdir().unwrap();
        let z = td.path().join("c.zip");
        make_zip(
            &z,
            &[
                ("uploads.json", b"{}"),
                ("client_info.json", br#"{"Hostname":"H1"}"#),
                ("uploads/auto/C%3A/Windows/Prefetch/A.pf", b"pf"),
                ("uploads/ntfs/%5C%5C.%5CC%3A/$Extend/$UsnJrnl%3A$J", b"j"),
            ],
        );
        let Probe::Usable(insp) = probe(&z) else {
            panic!("expected usable")
        };
        let dest = td.path().join("out");
        let r = extract(&insp, &dest, &ExtractOptions::default(), |_, _| {});
        assert!(r.error.is_none(), "unexpected error: {:?}", r.error);
        // Percent-encoding must survive: discovery matches on filename only.
        assert!(dest
            .join("uploads/auto/C%3A/Windows/Prefetch/A.pf")
            .is_file());
        assert!(dest
            .join("uploads/ntfs/%5C%5C.%5CC%3A/$Extend/$UsnJrnl%3A$J")
            .is_file());
    }

    #[test]
    fn extract_rejects_path_traversal() {
        let td = tempfile::tempdir().unwrap();
        let z = td.path().join("evil.zip");
        make_zip(
            &z,
            &[
                ("uploads.json", b"{}"),
                ("client_info.json", br#"{"Hostname":"H1"}"#),
                ("../escaped.txt", b"pwned"),
            ],
        );
        let Probe::Usable(insp) = probe(&z) else {
            panic!("expected usable")
        };
        let dest = td.path().join("out");
        let r = extract(&insp, &dest, &ExtractOptions::default(), |_, _| {});
        assert!(
            !td.path().join("escaped.txt").exists(),
            "traversal entry escaped the destination"
        );
        assert_eq!(r.skipped_entries, 1);
    }

    #[test]
    fn per_entry_size_limit_skips_without_aborting() {
        let td = tempfile::tempdir().unwrap();
        let z = td.path().join("c.zip");
        make_zip(
            &z,
            &[
                ("uploads.json", b"{}"),
                ("client_info.json", br#"{"Hostname":"H1"}"#),
                ("big.bin", &[0u8; 4096]),
            ],
        );
        let Probe::Usable(insp) = probe(&z) else {
            panic!("expected usable")
        };
        let dest = td.path().join("out");
        let opts = ExtractOptions {
            max_entry_bytes: 100,
            ..ExtractOptions::default()
        };
        let r = extract(&insp, &dest, &opts, |_, _| {});
        assert_eq!(r.skipped_entries, 1);
        assert!(!dest.join("big.bin").exists());
        // the small marker files still landed
        assert!(dest.join("uploads.json").is_file());
    }

    #[test]
    fn find_archives_is_sorted_and_one_level_only() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("b.zip"), b"x").unwrap();
        std::fs::write(td.path().join("a.zip"), b"x").unwrap();
        std::fs::write(td.path().join("notes.txt"), b"x").unwrap();
        std::fs::create_dir_all(td.path().join("nested")).unwrap();
        std::fs::write(td.path().join("nested/c.zip"), b"x").unwrap();
        let found = find_archives(td.path(), &[]);
        let names: Vec<_> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.zip", "b.zip"]);
    }

    #[test]
    fn find_archives_honors_exclude() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("a.zip"), b"x").unwrap();
        let excluded = td.path().join("skipme");
        std::fs::create_dir_all(&excluded).unwrap();
        std::fs::write(excluded.join("b.zip"), b"x").unwrap();
        assert_eq!(find_archives(td.path(), &[excluded]).len(), 1);
    }
}
