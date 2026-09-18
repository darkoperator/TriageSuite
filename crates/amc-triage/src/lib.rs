//! AmcacheTriage: AmcacheParser-compatible new-format Amcache.hve parser.
//! Emits 8 datasets from Root\Inventory* keys. New format only.

pub mod cli;
pub mod records;
pub mod values;

use std::path::{Path, PathBuf};

use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::duckdb::types::{ColumnType, DatasetColumnTypes, SqlType, TimeSemantics};
use triage_core::output::router::OutputRouter;
use triage_core::tool::{Scope, Tool};
use triage_registry::hive::Hive;

/// `regf` magic at the start of every registry hive.
const REGF_MAGIC: [u8; 4] = [0x72, 0x65, 0x67, 0x66];

/// Declared SQL types for the DuckDB view layer.
///
/// Every field on every record in `records.rs` is a Rust `String`: the whole
/// point of this crate is byte-for-byte AmcacheParser CSV parity, so nothing
/// here is a native `WinTimestamp`/`bool`/integer the way the other six
/// seeded crates are. Two of `values.rs`'s renderers are nonetheless total
/// and unambiguous regardless of the field's Rust type:
///
/// - `values::ts_string` / `values::ts_opt` call `WinTimestamp::to_string()`
///   directly (including `FileEntryRecord::link_date`'s parse-failure
///   sentinel, which is itself a literal in that same format), so a column
///   built from either is exactly as safe to declare `Timestamp` as a native
///   `WinTimestamp` field would be.
/// - `ValueMap::dotnet_bool` always renders the literal `"True"` or `"False"`,
///   so a column built from it is exactly as safe to declare `Boolean` as a
///   native `bool` field would be.
///
/// Every other column is either free text (`ValueMap::str`) or an integer
/// rendered by a helper this task does not model (`parse_int`, `parse_size`,
/// `parse_usn`, `strip_sha1`, `strip_prefix4`) and stays undeclared: the
/// OMIT rule for a custom serializer applies to a rendering function exactly
/// as it would to a Rust wrapper type -- the CSV shape is chosen by the
/// function, not proven by this task's `WinTimestamp`/`bool`-only stance.
pub const COLUMN_TYPES: &[DatasetColumnTypes] = &[
    DatasetColumnTypes {
        dataset_id: "program_entries",
        columns: &[
            ColumnType {
                column: "KeyLastWriteTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "InstallDate",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "InstallDateMsi",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "HiddenArp",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "InboxModernApp",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "associated_file_entries",
        columns: &[
            ColumnType {
                column: "FileKeyLastWriteTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "LinkDate",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "IsOsComponent",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "IsPeFile",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "unassociated_file_entries",
        columns: &[
            ColumnType {
                column: "FileKeyLastWriteTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "LinkDate",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "IsOsComponent",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "IsPeFile",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "shortcuts",
        columns: &[ColumnType {
            column: "KeyLastWriteTimestamp",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
    DatasetColumnTypes {
        dataset_id: "drive_binaries",
        columns: &[
            ColumnType {
                column: "KeyLastWriteTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "DriverTimeStamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "DriverLastWriteTime",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "DriverInBox",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "DriverIsKernelMode",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "DriverSigned",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "device_containers",
        columns: &[
            ColumnType {
                column: "KeyLastWriteTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "IsActive",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "IsConnected",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "IsMachineContainer",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "IsNetworked",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
            ColumnType {
                column: "IsPaired",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "driver_packages",
        columns: &[
            ColumnType {
                column: "KeyLastWriteTimestamp",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "Date",
                sql_type: SqlType::Timestamp,
                time_semantics: Some(TimeSemantics::Utc),
            },
            ColumnType {
                column: "DriverInBox",
                sql_type: SqlType::Boolean,
                time_semantics: None,
            },
        ],
    },
    DatasetColumnTypes {
        dataset_id: "device_pnps",
        columns: &[ColumnType {
            column: "KeyLastWriteTimestamp",
            sql_type: SqlType::Timestamp,
            time_semantics: Some(TimeSemantics::Utc),
        }],
    },
];

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

    fn column_types(&self) -> &'static [DatasetColumnTypes] {
        COLUMN_TYPES
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
