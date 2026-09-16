use crate::capture::CaptureType;
use crate::file_name_lossy;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Manifest schema version. 2 added `archives[]` and `hosts[].source_archive`
/// for zip input. 3 added `archives[].sha256` (the source archive's hash;
/// `null` when the input was not a regular file -- a directory, or a FIFO
/// or device node a rejected run recorded -- or when `--skip-hashes`
/// suppressed hashing), `archives[].sha256_skipped` (true only when
/// `--skip-hashes`,
/// not a hash failure, is why `sha256` is `null`), widened
/// `archives[].size_bytes` to `Option<u64>` so a stat failure is `null`
/// rather than a false `0`, and `hosts[].tools[].time_filter` (`applied` /
/// `not_applicable` / `null`) recording whether a run's `--start`/`--end`
/// actually reached each tool. 3 also added two things a run that *fails*
/// now has to say, because neither a rejected input nor a failed
/// output-compat write aborts before the manifest any more: the
/// `capture_type` value `"unidentified"`, for a rejection that identified no
/// capture to name (empty `hosts[]`, `final_exit_status: 3`), and
/// `hosts[].output_errors`, naming which collection's output is incomplete
/// (omitted when empty). All of it lands together in this one unreleased
/// branch, so the version goes 2 -> 3 once.
pub const SCHEMA_VERSION: u32 = 3;

pub const ORCHESTRATOR_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub run_id: String,
    pub orchestrator_version: String,
    pub started_utc: String,
    pub finished_utc: String,
    pub capture_type: CaptureType,
    pub final_exit_status: i32,
    /// Input archives seen this run. Omitted entirely when the capture was
    /// already an unzipped directory, so those manifests are unchanged.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub archives: Vec<ArchiveEntry>,
    pub hosts: Vec<HostEntry>,
}

/// What happened to one input archive this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveStatus {
    Extracted,
    ReExtracted,
    Reused,
    /// Nothing from this input was processed, and `error` says why. It can
    /// still carry `extracted_to`/`files_written`: an archive that unpacked
    /// cleanly and was then rejected by the pre-flight gate was extracted
    /// *and* skipped, and the manifest records both facts in the one entry
    /// that input gets.
    Skipped,
    Failed,
}

impl From<&crate::archive::ExtractReport> for ArchiveStatus {
    fn from(r: &crate::archive::ExtractReport) -> Self {
        if r.error.is_some() {
            ArchiveStatus::Failed
        } else if r.reused {
            ArchiveStatus::Reused
        } else if r.re_extracted {
            ArchiveStatus::ReExtracted
        } else {
            ArchiveStatus::Extracted
        }
    }
}

/// Chain-of-custody record for one input `.zip`.
#[derive(Serialize)]
pub struct ArchiveEntry {
    pub archive: String,
    pub archive_path: String,
    /// `None` (serialized as `null`) when the archive could not be stat'ed.
    /// A silent `0` here would be a false claim that the archive really is
    /// empty, indistinguishable from a genuine zero-byte file — the same
    /// evidentiary hole `write_output_hashes` was already fixed to avoid for
    /// per-file sizes.
    pub size_bytes: Option<u64>,
    pub status: ArchiveStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extracted_to: Option<String>,
    pub files_written: u64,
    pub bytes_written: u64,
    pub skipped_entries: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped_reasons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// SHA256 of the source archive file. `None` (serialized as `null`, kept
    /// out of `skip_serializing_if` deliberately) when `--skip-hashes` was
    /// set or when the hash could not be computed — an absent hash and an
    /// empty hash are different claims in a chain-of-custody record.
    pub sha256: Option<String>,
    /// True only when `--skip-hashes` is why `sha256` is `None`. Without
    /// this, a `null` hash caused by an operator's flag and a `null` hash
    /// caused by the archive being unreadable look identical, and those are
    /// different investigative facts: one is a policy choice, the other may
    /// mean the evidence itself is damaged.
    pub sha256_skipped: bool,
}

#[derive(Serialize)]
pub struct HostEntry {
    pub host: String,
    pub output_id: String,
    pub os: String,
    pub collection: String,
    /// Set when this host was extracted from an archive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_archive: Option<String>,
    pub inaccessible_entries: u64,
    /// Failures in this collection's output-compat writes -- the VeloResults
    /// copy, the source hash log, the SysInfo report, the Timeline Explorer
    /// sessions, the output hash walk. Empty (and omitted) on a healthy run.
    ///
    /// Those five used to abort the process, so the only record of them was a
    /// line on a terminal. They no longer do, which means the manifest has to
    /// say *which* collection's output is incomplete: a `final_exit_status`
    /// of 4 above two normal-looking host entries names the run but not the
    /// host. Recorded per host rather than run-wide precisely for that
    /// attribution.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub output_errors: Vec<String>,
    pub tools: Vec<ToolEntryReport>,
    pub external_tools: Vec<crate::external::ExternalToolReport>,
}

/// Whether the run's --start/--end reached this tool.
///
/// `None` when no range was given. `Applied` when the tool accepted it.
/// `NotApplicable` when the tool has no time filter — recorded explicitly,
/// because a scoped-looking run whose other tools emitted everything is how
/// a wrong conclusion reaches a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeFilter {
    Applied,
    NotApplicable,
}

/// Fill in one tool's `time_filter`: `None` when the run had no range at
/// all, otherwise `Applied`/`NotApplicable` per
/// `registry::tool_applies_time_filter`. A free function rather than a
/// method on `ToolRunResult`/`ToolEntryReport`, because neither carries the
/// run's `ToolOptions` -- only the caller assembling `HostEntry` does.
pub fn time_filter_for(key: &str, opts: &crate::registry::ToolOptions) -> Option<TimeFilter> {
    if opts.start.is_none() && opts.end.is_none() {
        return None;
    }
    Some(if crate::registry::tool_applies_time_filter(key) {
        TimeFilter::Applied
    } else {
        TimeFilter::NotApplicable
    })
}

#[derive(Serialize)]
pub struct ToolEntryReport {
    pub tool: String,
    pub key: String,
    /// `None` (serialized as `null`, kept out of `skip_serializing_if`
    /// deliberately) when the run had no `--start`/`--end` at all. A reader
    /// must be able to tell "no range this run" apart from "this tool has no
    /// filter" -- collapsing them would let a filtered run look uniformly
    /// scoped when only one tool actually filtered.
    pub time_filter: Option<TimeFilter>,
    pub files_matched: u64,
    pub discovered_candidates: u64,
    pub supported: u64,
    pub unsupported: u64,
    pub corrupt: u64,
    pub unreadable: u64,
    pub parsed: u64,
    pub failed: u64,
    pub deduplicated: u64,
    pub records: u64,
    pub output_paths: Vec<PathBuf>,
    pub reason_samples: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<crate::execute::ToolRunResult> for ToolEntryReport {
    fn from(r: crate::execute::ToolRunResult) -> Self {
        // Merged category-level files are reported alongside the per-user
        // ones they were derived from, rather than in a separate manifest
        // field: one array an analyst can read straight down beats two that
        // have to be reconciled.
        //
        // It is not an inventory of the directory, and must not be described
        // as one. `r.output_paths` is what the router *published*
        // (`OutputRouter::finish`), so two things on disk are absent from it.
        // The reclaimed system-scope slice appears under the category-root
        // path it was published to, never under the `PerUser/` name the merge
        // then renamed it to (`velo::merge`). And a per-user slice a previous
        // run left behind was never published by this one, so nothing here
        // names it -- correctly, since the merge did not read it either.
        // A tool whose router aborted publishes nothing at all, and its merge
        // then has no sources, so this array is empty while a previous run's
        // files may be sitting at every one of those paths.
        let mut output_paths = r.output_paths;
        output_paths.extend(r.merged);
        ToolEntryReport {
            tool: r.binary_name,
            key: r.key,
            // Filled in afterward by `time_filter_for`, once the caller has
            // both this report's key and the run's `ToolOptions` in hand.
            time_filter: None,
            files_matched: r.files_matched,
            // Kept equal to `files_matched` for manifest-schema compatibility.
            discovered_candidates: r.files_matched,
            supported: r.supported,
            unsupported: r.unsupported,
            corrupt: r.corrupt,
            unreadable: r.unreadable,
            parsed: r.parsed,
            failed: r.failed,
            deduplicated: r.deduplicated,
            records: r.records,
            output_paths,
            reason_samples: r.reason_samples,
            error: r.error,
        }
    }
}

pub fn now_iso() -> String {
    // chrono's `%.7f` specifier panics (unsupported in chrono 0.4 -- it only
    // recognizes .3f/.6f/.9f); instead extract subsecond nanoseconds, convert
    // to 100ns ticks, and zero-pad to 7 digits (see the same workaround in
    // re-triage's bam.rs/app_paths.rs).
    let now = chrono::Utc::now();
    // `% 10_000_000` caps the tick count to exactly 7 digits: a UTC leap
    // second can push `timestamp_subsec_nanos()` up to 1_999_999_999, which
    // divided by 100 is an 8-digit tick count that would overflow the
    // `{:07}` field width without this cap.
    let ticks = (now.timestamp_subsec_nanos() / 100) % 10_000_000;
    format!("{}.{:07}Z", now.format("%Y-%m-%dT%H:%M:%S"), ticks)
}

pub fn run_id() -> String {
    chrono::Utc::now().format("%Y%m%d%H%M%S%3f").to_string()
}

/// Size on disk, or `None` if the path cannot be stat'ed or is not a file.
///
/// A skipped input is not always an archive -- the pre-flight gate can
/// reject a collection *directory* -- and a directory's `len()` is its inode
/// size, a number with no evidentiary meaning at all. `None` says "no size"
/// honestly; `160` would read as a 160-byte archive.
fn size_of(path: &Path) -> Option<u64> {
    let md = std::fs::metadata(path).ok()?;
    md.is_file().then_some(md.len())
}

/// SHA256 of the archive file, or `None` when hashing is disabled, the path
/// is not a regular file, or the file cannot be read. Best-effort like
/// `size_of` above: by the time the manifest is assembled the archive has
/// already been extracted (or the extraction has already failed and been
/// recorded), so a hash failure here must not take down an otherwise-
/// successful run.
///
/// The regular-file test is not an optimisation. A *refused* input reaches
/// this function as whatever path the user named, and hashing opens it:
/// opening a FIFO with no writer blocks until one appears, and a character
/// device such as `/dev/zero` never reaches EOF. Either way the run would
/// never get as far as writing `run_manifest.json`, which over a reused
/// `--out` leaves the previous run's successful manifest standing. `None`
/// beside `size_of`'s `None` is the same explicit absence a directory-origin
/// skip already records: this path has neither a size nor a digest worth
/// putting in an evidence record.
fn sha256_of(path: &Path, skip_hashes: bool) -> Option<String> {
    if skip_hashes {
        return None;
    }
    if !std::fs::metadata(path).is_ok_and(|md| md.is_file()) {
        return None;
    }
    crate::velo::hashes::sha256_file(path).ok()
}

/// Build the manifest's archive records from what `input::prepare` did.
pub fn archive_entries(
    extractions: &[crate::archive::ExtractReport],
    skipped: &[crate::input::SkippedArchive],
    out_root: &Path,
    skip_hashes: bool,
) -> Vec<ArchiveEntry> {
    let extracted = extractions.iter().map(|r| ArchiveEntry {
        archive: file_name_lossy(&r.archive),
        archive_path: r.archive.display().to_string(),
        size_bytes: size_of(&r.archive),
        status: r.into(),
        extracted_to: r
            .dest
            .strip_prefix(out_root)
            .ok()
            .map(|p| p.display().to_string()),
        files_written: r.files_written,
        bytes_written: r.bytes_written,
        skipped_entries: r.skipped_entries,
        skipped_reasons: r.skipped_reasons.clone(),
        error: r.error.clone(),
        sha256: sha256_of(&r.archive, skip_hashes),
        sha256_skipped: skip_hashes,
    });
    let mut entries: Vec<ArchiveEntry> = extracted.collect();
    for s in skipped {
        let path = s.archive.display().to_string();
        // One entry per input path. A skip naming an archive this run
        // already extracted -- the pre-flight gate rejecting the collection
        // that came out of it -- is that same input reaching a later verdict,
        // not a second input: fold the reason into its entry, which keeps the
        // archive's real size and SHA256 instead of replacing them with a
        // second, emptier record of the same file.
        if let Some(existing) = entries.iter_mut().find(|e| e.archive_path == path) {
            existing.status = ArchiveStatus::Skipped;
            existing.error = Some(s.reason.clone());
            continue;
        }
        entries.push(ArchiveEntry {
            archive: file_name_lossy(&s.archive),
            archive_path: path,
            size_bytes: size_of(&s.archive),
            status: ArchiveStatus::Skipped,
            extracted_to: None,
            files_written: 0,
            bytes_written: 0,
            skipped_entries: 0,
            skipped_reasons: Vec::new(),
            error: Some(s.reason.clone()),
            sha256: sha256_of(&s.archive, skip_hashes),
            sha256_skipped: skip_hashes,
        });
    }
    entries.sort_by(|a, b| a.archive.cmp(&b.archive));
    entries
}

pub fn write(manifest: &Manifest, out_root: &Path) -> Result<(), String> {
    std::fs::create_dir_all(out_root).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(manifest).map_err(|e| e.to_string())?;
    let immutable = out_root.join(format!("run_manifest_{}.json", manifest.run_id));
    atomic_write(&immutable, json.as_bytes(), false)?;
    atomic_write(&out_root.join("run_manifest.json"), json.as_bytes(), true)
}

fn atomic_write(path: &Path, bytes: &[u8], replace: bool) -> Result<(), String> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| "manifest path has no parent".to_string())?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("manifest");
    let temporary = parent.join(format!(".{name}.tmp-{}", std::process::id()));
    let mut file = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = std::fs::remove_file(&temporary);
        return Err(e.to_string());
    }
    drop(file);
    if !replace && path.exists() {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "immutable manifest already exists: {}",
            path.display()
        ));
    }
    if replace && path.exists() && cfg!(windows) {
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        e.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn writes_manifest_json() {
        let td = TempDir::new().unwrap();
        let host = HostEntry {
            host: "H".into(),
            output_id: "H".into(),
            os: "Windows 11".into(),
            collection: "Collection-H".into(),
            source_archive: None,
            inaccessible_entries: 0,
            output_errors: Vec::new(),
            tools: vec![ToolEntryReport {
                tool: "PETriage".into(),
                key: "pe".into(),
                time_filter: None,
                files_matched: 3,
                discovered_candidates: 3,
                supported: 3,
                unsupported: 0,
                corrupt: 0,
                unreadable: 0,
                parsed: 3,
                failed: 0,
                deduplicated: 0,
                records: 99,
                output_paths: vec!["H/PETriage".into()],
                reason_samples: vec![],
                error: None,
            }],
            external_tools: vec![crate::external::ExternalToolReport {
                tool: "hayabusa-csv".into(),
                found: true,
                invoked: true,
                exit_code: Some(0),
                output_paths: vec!["H/Hayabusa/timeline.csv".into()],
                error: None,
            }],
        };
        let m = Manifest {
            schema_version: SCHEMA_VERSION,
            run_id: "20260710120000000".into(),
            orchestrator_version: ORCHESTRATOR_VERSION.into(),
            started_utc: "T0".into(),
            finished_utc: "T1".into(),
            capture_type: CaptureType::Velociraptor,
            final_exit_status: 0,
            archives: Vec::new(),
            hosts: vec![host],
        };
        write(&m, td.path()).unwrap();
        let text = std::fs::read_to_string(td.path().join("run_manifest.json")).unwrap();
        assert!(text.contains("\"capture_type\": \"velociraptor\""));
        assert!(text.contains("\"schema_version\": 3"));
        assert!(text.contains("\"records\": 99"));
        assert!(text.contains("\"tool\": \"hayabusa-csv\""));
    }

    fn extract_report(archive: &Path) -> crate::archive::ExtractReport {
        crate::archive::ExtractReport {
            archive: archive.to_path_buf(),
            dest: archive.with_extension(""),
            reused: false,
            re_extracted: false,
            files_written: 0,
            bytes_written: 0,
            skipped_entries: 0,
            skipped_reasons: Vec::new(),
            error: None,
            duration: std::time::Duration::default(),
        }
    }

    /// `sha256_skipped` must record *why* `sha256` is `null`: an operator's
    /// `--skip-hashes` flag is a policy choice, distinct from a hash attempt
    /// that actually failed. Collapsing the two into the same `null` was the
    /// exact defect this field exists to fix.
    #[test]
    fn sha256_skipped_is_true_only_for_the_skip_hashes_flag_not_a_hash_failure() {
        let td = TempDir::new().unwrap();
        let archive = td.path().join("Collection-H.zip");
        std::fs::write(&archive, b"abc").unwrap();
        let report = extract_report(&archive);

        // Hashing enabled and the archive is readable: a real digest, and
        // the flag is recorded as not the reason for anything.
        let entries = archive_entries(std::slice::from_ref(&report), &[], td.path(), false);
        assert!(entries[0].sha256.is_some(), "expected a real digest");
        assert!(!entries[0].sha256_skipped);

        // `--skip-hashes`: sha256 is null, and sha256_skipped says the flag
        // is why.
        let entries = archive_entries(std::slice::from_ref(&report), &[], td.path(), true);
        assert!(entries[0].sha256.is_none());
        assert!(entries[0].sha256_skipped);
    }

    /// A hash that does not happen (archive vanished after extraction was
    /// recorded, e.g.) must leave `sha256` `null` *and* `sha256_skipped`
    /// `false` -- otherwise a reader cannot tell "operator chose not to
    /// hash" from "we tried and the evidence was unreadable".
    ///
    /// Which of `sha256_of`'s two refusals this exercises changed when the
    /// regular-file guard went in: a nonexistent path now fails at
    /// `metadata`, so `sha256_file` is never called. The *output* asserted
    /// here is the contract and is unchanged. The remaining branch -- a
    /// regular file whose open then fails, from permissions or a mid-run
    /// unlink -- is deliberately left uncovered: provoking it needs a
    /// mode-0 file, which a root test runner would defeat, and both branches
    /// produce this same pair of values by construction.
    #[test]
    fn sha256_is_null_but_not_skipped_when_hashing_fails() {
        let td = TempDir::new().unwrap();
        let archive = td.path().join("Collection-Missing.zip");
        // Never created: it is not a regular file, so `sha256_of` refuses it
        // before opening anything.
        let report = extract_report(&archive);

        let entries = archive_entries(std::slice::from_ref(&report), &[], td.path(), false);
        assert!(entries[0].sha256.is_none(), "unreadable archive: no digest");
        assert!(
            !entries[0].sha256_skipped,
            "a failed hash attempt is not the same fact as --skip-hashes"
        );
    }

    /// `size_bytes` must be `null`, not a false `0`, when the archive cannot
    /// be stat'ed -- the same evidentiary-ambiguity class `write_output_hashes`
    /// was already fixed to avoid for per-file sizes.
    #[test]
    fn size_bytes_is_null_when_the_archive_cannot_be_stat_ed() {
        let td = TempDir::new().unwrap();
        let archive = td.path().join("Collection-Missing.zip");
        let report = extract_report(&archive);

        let entries = archive_entries(std::slice::from_ref(&report), &[], td.path(), true);
        assert_eq!(
            entries[0].size_bytes, None,
            "a nonexistent file has no size"
        );
    }
}
