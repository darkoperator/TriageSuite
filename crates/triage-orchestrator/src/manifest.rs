use crate::capture::CaptureType;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub run_id: String,
    pub orchestrator_version: String,
    pub started_utc: String,
    pub finished_utc: String,
    pub capture_type: String,
    pub final_exit_status: i32,
    /// Input archives seen this run. Omitted entirely when the capture was
    /// already an unzipped directory, so those manifests are unchanged.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub archives: Vec<ArchiveEntry>,
    pub hosts: Vec<HostEntry>,
}

/// Chain-of-custody record for one input `.zip`.
#[derive(Serialize)]
pub struct ArchiveEntry {
    pub archive: String,
    pub archive_path: String,
    pub size_bytes: u64,
    /// "extracted" | "re-extracted" | "reused" | "skipped" | "failed"
    pub status: String,
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

#[allow(clippy::too_many_arguments)]
pub fn build(
    capture_type: CaptureType,
    run_id: &str,
    started: &str,
    finished: &str,
    final_exit_status: i32,
    archives: Vec<ArchiveEntry>,
    hosts: Vec<HostEntry>,
) -> Manifest {
    let ty = match capture_type {
        CaptureType::Velociraptor => "velociraptor",
        CaptureType::Raw => "raw",
    };
    Manifest {
        // 2: added `archives[]` and `hosts[].source_archive` for zip input.
        schema_version: 2,
        run_id: run_id.to_string(),
        orchestrator_version: env!("CARGO_PKG_VERSION").to_string(),
        started_utc: started.to_string(),
        finished_utc: finished.to_string(),
        capture_type: ty.to_string(),
        final_exit_status,
        archives,
        hosts,
    }
}

/// Build the manifest's archive records from what `input::prepare` did.
pub fn archive_entries(
    extractions: &[crate::archive::ExtractReport],
    skipped: &[crate::input::SkippedArchive],
    out_root: &Path,
) -> Vec<ArchiveEntry> {
    let name_of = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let mut entries: Vec<ArchiveEntry> = Vec::new();
    for r in extractions {
        let status = if r.error.is_some() {
            "failed"
        } else if r.reused {
            "reused"
        } else if r.re_extracted {
            "re-extracted"
        } else {
            "extracted"
        };
        entries.push(ArchiveEntry {
            archive: name_of(&r.archive),
            archive_path: r.archive.display().to_string(),
            size_bytes: std::fs::metadata(&r.archive).map(|m| m.len()).unwrap_or(0),
            status: status.to_string(),
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
    }
    for s in skipped {
        entries.push(ArchiveEntry {
            archive: name_of(&s.archive),
            archive_path: s.archive.display().to_string(),
            size_bytes: std::fs::metadata(&s.archive).map(|m| m.len()).unwrap_or(0),
            status: "skipped".to_string(),
            extracted_to: None,
            files_written: 0,
            bytes_written: 0,
            skipped_entries: 0,
            skipped_reasons: Vec::new(),
            error: Some(s.reason.clone()),
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
        let m = build(
            crate::capture::CaptureType::Velociraptor,
            "20260710120000000",
            "T0",
            "T1",
            0,
            Vec::new(),
            vec![host],
        );
        write(&m, td.path()).unwrap();
        let text = std::fs::read_to_string(td.path().join("run_manifest.json")).unwrap();
        assert!(text.contains("\"capture_type\": \"velociraptor\""));
        assert!(text.contains("\"records\": 99"));
        assert!(text.contains("\"tool\": \"hayabusa-csv\""));
    }
}
