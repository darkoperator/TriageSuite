//! Turn whatever the user pointed `run` at — a directory, a `.zip`, or a
//! folder of `.zip`s — into the host list the rest of the orchestrator expects.
//!
//! Keeping this out of `main.rs` matters because the archive path has real
//! decisions in it (reuse vs re-extract, skip vs fail) that deserve tests.

use std::path::{Path, PathBuf};

use crate::archive::{self, ArchiveSkip, ExtractOptions, ExtractReport, Probe};
use crate::capture::{self, CaptureType, HostCapture};
use crate::progress_ui::ProgressUi;

/// Directory under `--out` holding extracted archives. Kept after the run so a
/// re-run doesn't pay extraction again.
pub const EXTRACTED_DIR: &str = "_extracted";

/// Name of the completion marker written beside each extraction. Written last,
/// so an interrupted extraction leaves no marker and is redone next time
/// rather than being parsed as if it were complete.
fn marker_path(extracted_root: &Path, stem: &str) -> PathBuf {
    extracted_root.join(format!("{stem}.source.json"))
}

#[derive(Debug, Clone)]
pub struct SkippedArchive {
    pub archive: PathBuf,
    pub reason: String,
}

pub struct PrepareOptions {
    /// False when `--overwrite` was given: forces re-extraction.
    pub reuse_existing: bool,
    pub extract: ExtractOptions,
}

impl Default for PrepareOptions {
    fn default() -> Self {
        Self {
            reuse_existing: true,
            extract: ExtractOptions::default(),
        }
    }
}

#[derive(Debug)]
pub struct PreparedInput {
    pub capture_type: CaptureType,
    pub hosts: Vec<HostCapture>,
    pub extractions: Vec<ExtractReport>,
    pub skipped: Vec<SkippedArchive>,
}

/// What a previous run recorded about an extraction.
#[derive(serde::Serialize, serde::Deserialize)]
struct SourceMarker {
    schema_version: u32,
    archive_name: String,
    archive_size: u64,
    archive_mtime_secs: i64,
    files_written: u64,
    bytes_written: u64,
}

fn archive_identity(path: &Path) -> Option<(String, u64, i64)> {
    let md = std::fs::metadata(path).ok()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some((name, md.len(), mtime))
}

fn read_marker(path: &Path) -> Option<SourceMarker> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_marker(path: &Path, archive: &Path, report: &ExtractReport) {
    let Some((archive_name, archive_size, archive_mtime_secs)) = archive_identity(archive) else {
        return;
    };
    let marker = SourceMarker {
        schema_version: 1,
        archive_name,
        archive_size,
        archive_mtime_secs,
        files_written: report.files_written,
        bytes_written: report.bytes_written,
    };
    if let Ok(text) = serde_json::to_string_pretty(&marker) {
        let _ = std::fs::write(path, text);
    }
}

/// Is an existing extraction still a faithful copy of this archive?
fn marker_matches(marker: &SourceMarker, archive: &Path) -> bool {
    match archive_identity(archive) {
        Some((name, size, mtime)) => {
            marker.archive_name == name
                && marker.archive_size == size
                && marker.archive_mtime_secs == mtime
        }
        None => false,
    }
}

/// Resolve the capture input, extracting any archives found.
///
/// Returns `Err` only when there is nothing to work on at all — a missing path,
/// or archives that all turned out to be unusable with no other collection
/// present. Individual bad archives are skipped, never fatal.
pub fn prepare(
    capture: &Path,
    out: &Path,
    opts: &PrepareOptions,
    ui: &ProgressUi,
) -> Result<PreparedInput, String> {
    let extracted_root = out.join(EXTRACTED_DIR);

    // Which archives are we dealing with, if any?
    let (archives, scan_root): (Vec<PathBuf>, Option<PathBuf>) = if capture.is_file() {
        if !archive::is_zip_path(capture) {
            // Preserve the historical message for a plain non-directory input.
            return Err(format!("not a directory: {}", capture.display()));
        }
        (vec![capture.to_path_buf()], None)
    } else if capture.is_dir() {
        (
            archive::find_archives(capture, &[extracted_root.clone()]),
            Some(capture.to_path_buf()),
        )
    } else {
        return Err(format!("not found: {}", capture.display()));
    };

    // Fast path: an ordinary directory with no archives behaves exactly as it
    // always has, and `<out>/_extracted` is never even created.
    if archives.is_empty() {
        if let Some(root) = &scan_root {
            let (ty, hosts) = capture::enumerate(root)?;
            return Ok(PreparedInput {
                capture_type: ty,
                hosts,
                extractions: Vec::new(),
                skipped: Vec::new(),
            });
        }
    }

    let mut extractions: Vec<ExtractReport> = Vec::new();
    let mut skipped: Vec<SkippedArchive> = Vec::new();
    // Second allocator: two archives in different folders can share a stem, and
    // they must not extract over each other.
    let mut stems = triage_core::attribution::ComponentAllocator::default();

    for path in &archives {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "archive".to_string());
        let stem = stems.allocate(&stem);
        let dest = extracted_root.join(&stem);
        let marker = marker_path(&extracted_root, &stem);

        let inspection = match archive::probe(path) {
            Probe::Usable(i) => i,
            Probe::Skip(reason) => {
                ui.archive_skipped(path, &reason.message());
                skipped.push(SkippedArchive {
                    archive: path.clone(),
                    reason: reason.message(),
                });
                continue;
            }
        };

        // Reuse decision. An existing extraction is only trusted when its
        // marker still matches the archive on disk, so swapping an archive for
        // a different one of the same name can never be parsed as stale
        // evidence.
        if dest.exists() && opts.reuse_existing {
            match read_marker(&marker) {
                Some(m) if marker_matches(&m, path) => {
                    let report = ExtractReport {
                        archive: path.clone(),
                        dest: dest.clone(),
                        reused: true,
                        re_extracted: false,
                        files_written: m.files_written,
                        bytes_written: m.bytes_written,
                        skipped_entries: 0,
                        skipped_reasons: Vec::new(),
                        error: None,
                        duration: std::time::Duration::default(),
                    };
                    ui.archive_finished(&report);
                    extractions.push(report);
                    continue;
                }
                Some(_) => {
                    let reason = ArchiveSkip::Stale(
                        "existing extraction is from a different archive".to_string(),
                    )
                    .message();
                    ui.archive_skipped(path, &reason);
                    skipped.push(SkippedArchive {
                        archive: path.clone(),
                        reason,
                    });
                    continue;
                }
                None => {
                    let reason =
                        ArchiveSkip::Stale("existing extraction is incomplete".to_string())
                            .message();
                    ui.archive_skipped(path, &reason);
                    skipped.push(SkippedArchive {
                        archive: path.clone(),
                        reason,
                    });
                    continue;
                }
            }
        }

        let re_extracting = dest.exists();
        if re_extracting {
            let _ = std::fs::remove_dir_all(&dest);
            let _ = std::fs::remove_file(&marker);
        }

        ui.archive_started(path, inspection.entries, inspection.declared_uncompressed);
        let mut report = archive::extract(&inspection, &dest, &opts.extract, |done, total| {
            ui.archive_progress(done, total);
        });
        report.re_extracted = re_extracting;
        if report.error.is_none() {
            write_marker(&marker, path, &report);
        }
        ui.archive_finished(&report);
        extractions.push(report);
    }

    // Each extraction destination is its own root: `collect_collections`
    // inspects the directory itself and its immediate children, which covers
    // both a collection at the archive root and one under a wrapper directory.
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(root) = &scan_root {
        roots.push(root.clone());
    }
    for report in &extractions {
        if report.error.is_none() {
            roots.push(report.dest.clone());
        }
    }

    // No Raw fallback once archives are in play: a folder holding only ZIPs
    // must never be mistaken for a raw capture named after the folder.
    let raw_fallback = if archives.is_empty() {
        scan_root.as_deref()
    } else {
        None
    };

    let (capture_type, mut hosts) =
        capture::enumerate_multi(&roots, raw_fallback).map_err(|_| {
            let detail = if skipped.is_empty() {
                String::new()
            } else {
                format!(" ({} archive(s) skipped)", skipped.len())
            };
            format!("no usable capture found in {}{detail}", capture.display())
        })?;

    // Stamp provenance so the manifest can record which archive a host came from.
    for host in hosts.iter_mut() {
        if let Some(report) = extractions
            .iter()
            .find(|r| host.collection_dir.starts_with(&r.dest))
        {
            host.source_archive = Some(report.archive.clone());
        }
    }

    Ok(PreparedInput {
        capture_type,
        hosts,
        extractions,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    fn ui() -> ProgressUi {
        ProgressUi::new(true)
    }

    fn write_collection_zip(path: &Path, prefix: &str, host: &str) {
        let f = fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        w.start_file(format!("{prefix}uploads.json"), opts).unwrap();
        w.write_all(b"{}").unwrap();
        w.start_file(format!("{prefix}client_info.json"), opts)
            .unwrap();
        w.write_all(
            format!(r#"{{"Hostname":"{host}","Platform":"Windows","PlatformVersion":"11"}}"#)
                .as_bytes(),
        )
        .unwrap();
        w.start_file(
            format!("{prefix}uploads/auto/C%3A/Windows/Prefetch/A.pf"),
            opts,
        )
        .unwrap();
        w.write_all(b"pf").unwrap();
        w.finish().unwrap();
    }

    #[test]
    fn single_zip_is_extracted_and_becomes_a_host() {
        let td = TempDir::new().unwrap();
        let z = td.path().join("Collection-H1.zip");
        write_collection_zip(&z, "", "H1");
        let out = td.path().join("out");
        let p = prepare(&z, &out, &PrepareOptions::default(), &ui()).unwrap();
        assert_eq!(p.hosts.len(), 1);
        assert_eq!(p.hosts[0].host, "H1");
        assert!(p.hosts[0].source_archive.is_some());
        assert!(out.join("_extracted/Collection-H1").is_dir());
    }

    #[test]
    fn wrapper_directory_layout_is_handled() {
        let td = TempDir::new().unwrap();
        let z = td.path().join("Collection-H2.zip");
        write_collection_zip(&z, "Collection-H2-2026/", "H2");
        let out = td.path().join("out");
        let p = prepare(&z, &out, &PrepareOptions::default(), &ui()).unwrap();
        assert_eq!(p.hosts.len(), 1);
        assert_eq!(p.hosts[0].host, "H2");
    }

    #[test]
    fn folder_of_zips_yields_every_host() {
        let td = TempDir::new().unwrap();
        let dir = td.path().join("zips");
        fs::create_dir_all(&dir).unwrap();
        write_collection_zip(&dir.join("A.zip"), "", "HOSTA");
        write_collection_zip(&dir.join("B.zip"), "", "HOSTB");
        let out = td.path().join("out");
        let p = prepare(&dir, &out, &PrepareOptions::default(), &ui()).unwrap();
        let mut names: Vec<_> = p.hosts.iter().map(|h| h.host.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["HOSTA", "HOSTB"]);
    }

    #[test]
    fn mixed_folder_of_zip_and_unzipped_collection() {
        let td = TempDir::new().unwrap();
        let dir = td.path().join("mixed");
        fs::create_dir_all(&dir).unwrap();
        write_collection_zip(&dir.join("A.zip"), "", "ZIPPED");
        let plain = dir.join("Collection-PLAIN");
        fs::create_dir_all(plain.join("uploads/auto")).unwrap();
        fs::write(plain.join("uploads.json"), "{}").unwrap();
        fs::write(
            plain.join("client_info.json"),
            r#"{"Hostname":"PLAIN","Platform":"Windows","PlatformVersion":"11"}"#,
        )
        .unwrap();
        let out = td.path().join("out");
        let p = prepare(&dir, &out, &PrepareOptions::default(), &ui()).unwrap();
        let mut names: Vec<_> = p.hosts.iter().map(|h| h.host.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["PLAIN", "ZIPPED"]);
    }

    #[test]
    fn invalid_archive_is_skipped_but_good_ones_still_run() {
        let td = TempDir::new().unwrap();
        let dir = td.path().join("zips");
        fs::create_dir_all(&dir).unwrap();
        write_collection_zip(&dir.join("good.zip"), "", "GOOD");
        fs::write(dir.join("garbage.zip"), b"definitely not a zip").unwrap();
        let out = td.path().join("out");
        let p = prepare(&dir, &out, &PrepareOptions::default(), &ui()).unwrap();
        assert_eq!(p.hosts.len(), 1);
        assert_eq!(p.hosts[0].host, "GOOD");
        assert_eq!(p.skipped.len(), 1);
    }

    #[test]
    fn all_archives_unusable_is_an_error_not_an_empty_success() {
        let td = TempDir::new().unwrap();
        let dir = td.path().join("zips");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.zip"), b"nope").unwrap();
        fs::write(dir.join("b.zip"), b"also nope").unwrap();
        let out = td.path().join("out");
        let err = prepare(&dir, &out, &PrepareOptions::default(), &ui()).unwrap_err();
        assert!(err.contains("no usable capture"), "got: {err}");
        assert!(err.contains("2 archive(s) skipped"), "got: {err}");
    }

    #[test]
    fn plain_directory_is_unchanged_and_creates_no_extracted_dir() {
        let td = TempDir::new().unwrap();
        let dir = td.path().join("plain");
        let coll = dir.join("Collection-P");
        fs::create_dir_all(coll.join("uploads/auto")).unwrap();
        fs::write(coll.join("uploads.json"), "{}").unwrap();
        fs::write(
            coll.join("client_info.json"),
            r#"{"Hostname":"P","Platform":"Windows","PlatformVersion":"11"}"#,
        )
        .unwrap();
        let out = td.path().join("out");
        let p = prepare(&dir, &out, &PrepareOptions::default(), &ui()).unwrap();
        assert_eq!(p.hosts.len(), 1);
        assert!(p.extractions.is_empty());
        assert!(!out.join(EXTRACTED_DIR).exists());
    }

    #[test]
    fn second_run_reuses_the_existing_extraction() {
        let td = TempDir::new().unwrap();
        let z = td.path().join("C.zip");
        write_collection_zip(&z, "", "H1");
        let out = td.path().join("out");
        let first = prepare(&z, &out, &PrepareOptions::default(), &ui()).unwrap();
        assert!(!first.extractions[0].reused);
        let second = prepare(&z, &out, &PrepareOptions::default(), &ui()).unwrap();
        assert!(second.extractions[0].reused, "should have reused");
        assert_eq!(second.hosts.len(), 1);
    }

    #[test]
    fn a_swapped_archive_is_not_reused_as_stale_evidence() {
        let td = TempDir::new().unwrap();
        let z = td.path().join("C.zip");
        write_collection_zip(&z, "", "ORIGINAL");
        let out = td.path().join("out");
        let first = prepare(&z, &out, &PrepareOptions::default(), &ui()).unwrap();
        assert_eq!(first.hosts[0].host, "ORIGINAL");

        // Replace the archive with different content under the same name.
        std::thread::sleep(std::time::Duration::from_millis(1100)); // distinct mtime
        write_collection_zip(&z, "", "REPLACED");

        // The archive changed, so the existing extraction is refused rather
        // than silently reused. It was the only archive, so there is nothing
        // left to run — an error, not an empty "success".
        let err = prepare(&z, &out, &PrepareOptions::default(), &ui()).unwrap_err();
        assert!(err.contains("no usable capture"), "got: {err}");

        // Crucially, the stale tree was NOT parsed as if it were the new one.
        let marker = out.join("_extracted/C.source.json");
        assert!(
            marker.is_file(),
            "marker should still describe the old copy"
        );
    }

    #[test]
    fn overwrite_forces_re_extraction() {
        let td = TempDir::new().unwrap();
        let z = td.path().join("C.zip");
        write_collection_zip(&z, "", "ORIGINAL");
        let out = td.path().join("out");
        prepare(&z, &out, &PrepareOptions::default(), &ui()).unwrap();

        write_collection_zip(&z, "", "REPLACED");
        let opts = PrepareOptions {
            reuse_existing: false,
            ..PrepareOptions::default()
        };
        let second = prepare(&z, &out, &opts, &ui()).unwrap();
        assert_eq!(second.hosts.len(), 1);
        assert_eq!(second.hosts[0].host, "REPLACED");
        assert!(second.extractions[0].re_extracted);
    }

    #[test]
    fn a_plain_file_that_is_not_a_zip_keeps_the_old_message() {
        let td = TempDir::new().unwrap();
        let f = td.path().join("notes.txt");
        fs::write(&f, b"x").unwrap();
        let err = prepare(
            &f,
            &td.path().join("out"),
            &PrepareOptions::default(),
            &ui(),
        )
        .unwrap_err();
        assert!(err.starts_with("not a directory:"), "got: {err}");
    }
}
