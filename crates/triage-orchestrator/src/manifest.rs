use crate::capture::CaptureType;
use crate::file_name_lossy;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Manifest schema version. 2 added `archives[]` and `hosts[].source_archive`
/// for zip input.
pub const SCHEMA_VERSION: u32 = 2;

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
    pub size_bytes: u64,
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
    pub tools: Vec<ToolEntryReport>,
    pub external_tools: Vec<crate::external::ExternalToolReport>,
}

#[derive(Serialize)]
pub struct ToolEntryReport {
    pub tool: String,
    pub key: String,
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
        ToolEntryReport {
            tool: r.binary_name,
            key: r.key,
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
            output_paths: r.output_paths,
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

/// Size on disk, or 0 if the file cannot be stat'ed.
fn size_of(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Build the manifest's archive records from what `input::prepare` did.
pub fn archive_entries(
    extractions: &[crate::archive::ExtractReport],
    skipped: &[crate::input::SkippedArchive],
    out_root: &Path,
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
    });
    let skipped = skipped.iter().map(|s| ArchiveEntry {
        archive: file_name_lossy(&s.archive),
        archive_path: s.archive.display().to_string(),
        size_bytes: size_of(&s.archive),
        status: ArchiveStatus::Skipped,
        extracted_to: None,
        files_written: 0,
        bytes_written: 0,
        skipped_entries: 0,
        skipped_reasons: Vec::new(),
        error: Some(s.reason.clone()),
    });
    let mut entries: Vec<ArchiveEntry> = extracted.chain(skipped).collect();
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
            tools: vec![ToolEntryReport {
                tool: "PETriage".into(),
                key: "pe".into(),
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
        assert!(text.contains("\"schema_version\": 2"));
        assert!(text.contains("\"records\": 99"));
        assert!(text.contains("\"tool\": \"hayabusa-csv\""));
    }
}
