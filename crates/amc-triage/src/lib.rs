//! AmcacheTriage: AmcacheParser-compatible new-format Amcache.hve parser.
//! Emits 8 datasets from Root\Inventory* keys. New format only.

pub mod cli;
pub mod records;
pub mod values;

use std::path::{Path, PathBuf};

use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};
use triage_registry::hive::Hive;

/// `regf` magic at the start of every registry hive.
const REGF_MAGIC: [u8; 4] = [0x72, 0x65, 0x67, 0x66];

pub const DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        id: "unassociated_file_entries",
        default_basename: "AmcacheTriage_UnassociatedFileEntries_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: None,
    },
    DatasetSpec {
        id: "associated_file_entries",
        default_basename: "AmcacheTriage_AssociatedFileEntries_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_AssociatedFileEntries"),
    },
    DatasetSpec {
        id: "program_entries",
        default_basename: "AmcacheTriage_ProgramEntries_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_ProgramEntries"),
    },
    DatasetSpec {
        id: "shortcuts",
        default_basename: "AmcacheTriage_ShortCuts_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_ShortCuts"),
    },
    DatasetSpec {
        id: "drive_binaries",
        default_basename: "AmcacheTriage_DriveBinaries_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_DriveBinaries"),
    },
    DatasetSpec {
        id: "device_containers",
        default_basename: "AmcacheTriage_DeviceContainers_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_DeviceContainers"),
    },
    DatasetSpec {
        id: "driver_packages",
        default_basename: "AmcacheTriage_DriverPackages_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_DriverPackages"),
    },
    DatasetSpec {
        id: "device_pnps",
        default_basename: "AmcacheTriage_DevicePnps_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: Some("_DevicePnps"),
    },
];

#[derive(Default)]
pub struct AmcacheTool {
    /// Skip pairing the hive with `.LOG1`/`.LOG2` siblings.
    pub no_logs: bool,
}

/// New format iff `Root\InventoryApplicationFile` exists (AmcacheNew Helper.IsNewFormat).
pub fn is_new_format(hive: &mut Hive) -> bool {
    hive.get_key(r"Root\InventoryApplicationFile").is_some()
}

impl Tool for AmcacheTool {
    fn binary_name(&self) -> &'static str {
        "AmcacheTriage"
    }

    fn patterns(&self) -> &[&'static str] {
        &["Amcache.hve"]
    }

    fn validate_legacy(&self, path: &Path) -> bool {
        let name_upper = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_ascii_uppercase())
            .unwrap_or_default();
        if name_upper.ends_with(".LOG")
            || name_upper.ends_with(".LOG1")
            || name_upper.ends_with(".LOG2")
        {
            return false;
        }
        let Ok(mut f) = std::fs::File::open(path) else {
            return false;
        };
        use std::io::Read;
        let mut buf = [0u8; 4];
        matches!(f.read_exact(&mut buf), Ok(())) && buf == REGF_MAGIC
    }

    fn invalid_content_is_corrupt(&self) -> bool {
        true
    }

    fn datasets(&self) -> &'static [DatasetSpec] {
        DATASETS
    }

    fn scope(&self) -> Scope {
        Scope::SystemWide
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        let logs: Vec<PathBuf> = if self.no_logs {
            Vec::new()
        } else {
            find_log_siblings(path)
        };
        let mut hive = Hive::open(path, &logs, true).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        if !is_new_format(&mut hive) {
            eprintln!(
                "warning: {} is not a new-format Amcache hive (no Root\\InventoryApplicationFile); skipping",
                path.display()
            );
            return Ok(0);
        }

        let mut count = 0u64;
        let (n_prog, program_names) = records::emit_program_entries(&mut hive, out)?;
        count += n_prog;
        count += records::emit_file_entries(&mut hive, out, &program_names)?;
        count += records::emit_shortcuts(&mut hive, out)?;
        count += records::emit_drive_binaries(&mut hive, out)?;
        count += records::emit_device_containers(&mut hive, out)?;
        count += records::emit_driver_packages(&mut hive, out)?;
        count += records::emit_device_pnps(&mut hive, out)?;
        Ok(count)
    }
}

/// `.LOG1`/`.LOG2` siblings of `primary` (LOG1 before LOG2). Copied from sbe-triage.
fn find_log_siblings(primary: &Path) -> Vec<PathBuf> {
    let Some(dir) = primary.parent() else {
        return Vec::new();
    };
    let Some(stem) = primary.file_name().and_then(|n| n.to_str()) else {
        return Vec::new();
    };
    let stem_lower = stem.to_ascii_lowercase();
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    let mut logs = Vec::new();
    for ext in [".log1", ".log2"] {
        let target = format!("{stem_lower}{ext}");
        if let Some(found) = entries.iter().find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_ascii_lowercase() == target)
                .unwrap_or(false)
        }) {
            logs.push(found.clone());
        }
    }
    logs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datasets_have_one_primary() {
        let primaries = DATASETS
            .iter()
            .filter(|d| d.override_suffix.is_none())
            .count();
        assert_eq!(primaries, 1, "exactly one primary dataset");
        assert_eq!(DATASETS.len(), 8);
    }

    #[test]
    fn validate_rejects_log_siblings() {
        let tool = AmcacheTool::default();
        assert!(!tool.validate_legacy(Path::new("/x/Amcache.hve.LOG1")));
    }

    #[test]
    fn binary_name_and_patterns() {
        let tool = AmcacheTool::default();
        assert_eq!(tool.binary_name(), "AmcacheTriage");
        assert_eq!(tool.patterns(), &["Amcache.hve"]);
    }
}
