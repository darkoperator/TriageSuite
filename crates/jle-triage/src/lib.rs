//! JLETriage — JLECmd-compatible Windows Jump List parser.
//!
//! Assembles two record shapes from parsed jump lists:
//!  - [`AutoRecord`] (44 columns) from an `*.automaticDestinations-ms` OLE
//!    compound file: per `DestList` entry, the MRU metadata plus the embedded
//!    LNK columns (or empties when the entry has no embedded LNK).
//!  - [`CustomRecord`] (28 columns) from a flat `*.customDestinations-ms` file:
//!    one row per embedded LNK, tagged with its category name.
//!
//! Column names/order are pinned to the JLECmd reference fixtures in
//! `tests/fixtures/jlecmd/DESKTOP/{auto,custom}.csv`. Optional embedded-LNK and
//! Source* fields use `Option`/`WinTimestamp` so JSON null-omission drops absent
//! values; fields the CSV always emits (`AppId`, `HasSps`, `DestListVersion`,
//! etc.) are plain `String`. `.NET` bools (`HasSps`, `PinStatus`) render as the
//! capitalized strings `"True"`/`"False"`.

pub mod lnkfields;

use std::path::Path;
use std::time::SystemTime;

use serde::Serialize;
use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_core::tool::{Scope, Tool, Validation};

use lnkfields::{AbsPathFallback, LnkFields};
use triage_jumplist::appid::AppIdTable;

/// OLE compound-file magic at offset 0 (`D0 CF 11 E0 A1 B1 1A E1`). Identifies
/// an Automatic Destinations file.
const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// Custom Destinations footer signature `0xBABFFBAB` (little-endian bytes).
const CUSTOM_FOOTER: [u8; 4] = [0xAB, 0xFB, 0xBF, 0xBA];

/// One JLECmd `AutomaticDestinations` CSV record (44 columns, order pinned to
/// the fixture).
#[derive(Serialize)]
pub struct AutoRecord {
    #[serde(rename = "SourceFile")]
    pub source_file: String,
    #[serde(rename = "SourceCreated")]
    pub source_created: WinTimestamp,
    #[serde(rename = "SourceModified")]
    pub source_modified: WinTimestamp,
    #[serde(rename = "SourceAccessed")]
    pub source_accessed: WinTimestamp,
    #[serde(rename = "AppId")]
    pub app_id: String,
    #[serde(rename = "AppIdDescription")]
    pub app_id_description: String,
    /// `.NET` bool: capitalized `"True"`/`"False"`.
    #[serde(rename = "HasSps")]
    pub has_sps: String,
    #[serde(rename = "DestListVersion")]
    pub dest_list_version: String,
    #[serde(rename = "LastUsedEntryNumber")]
    pub last_used_entry_number: String,
    #[serde(rename = "MRU")]
    pub mru: String,
    #[serde(rename = "EntryNumber")]
    pub entry_number: String,
    #[serde(rename = "CreationTime")]
    pub creation_time: WinTimestamp,
    #[serde(rename = "LastModified")]
    pub last_modified: WinTimestamp,
    #[serde(rename = "Hostname")]
    pub hostname: Option<String>,
    #[serde(rename = "MacAddress")]
    pub mac_address: Option<String>,
    #[serde(rename = "Path")]
    pub path: Option<String>,
    #[serde(rename = "InteractionCount")]
    pub interaction_count: String,
    /// `.NET` bool: capitalized `"True"`/`"False"`.
    #[serde(rename = "PinStatus")]
    pub pin_status: String,
    #[serde(rename = "FileBirthDroid")]
    pub file_birth_droid: Option<String>,
    #[serde(rename = "FileDroid")]
    pub file_droid: Option<String>,
    #[serde(rename = "VolumeBirthDroid")]
    pub volume_birth_droid: Option<String>,
    #[serde(rename = "VolumeDroid")]
    pub volume_droid: Option<String>,
    #[serde(rename = "TargetCreated")]
    pub target_created: WinTimestamp,
    #[serde(rename = "TargetModified")]
    pub target_modified: WinTimestamp,
    #[serde(rename = "TargetAccessed")]
    pub target_accessed: WinTimestamp,
    #[serde(rename = "FileSize")]
    pub file_size: u32,
    #[serde(rename = "RelativePath")]
    pub relative_path: Option<String>,
    #[serde(rename = "WorkingDirectory")]
    pub working_directory: Option<String>,
    #[serde(rename = "FileAttributes")]
    pub file_attributes: String,
    #[serde(rename = "HeaderFlags")]
    pub header_flags: String,
    #[serde(rename = "DriveType")]
    pub drive_type: String,
    #[serde(rename = "VolumeSerialNumber")]
    pub volume_serial_number: Option<String>,
    #[serde(rename = "VolumeLabel")]
    pub volume_label: Option<String>,
    #[serde(rename = "LocalPath")]
    pub local_path: Option<String>,
    #[serde(rename = "CommonPath")]
    pub common_path: Option<String>,
    #[serde(rename = "TargetIDAbsolutePath")]
    pub target_id_absolute_path: Option<String>,
    #[serde(rename = "TargetMFTEntryNumber")]
    pub target_mft_entry_number: Option<String>,
    #[serde(rename = "TargetMFTSequenceNumber")]
    pub target_mft_sequence_number: Option<String>,
    #[serde(rename = "MachineID")]
    pub machine_id: Option<String>,
    #[serde(rename = "MachineMACAddress")]
    pub machine_mac_address: Option<String>,
    #[serde(rename = "TrackerCreatedOn")]
    pub tracker_created_on: WinTimestamp,
    #[serde(rename = "ExtraBlocksPresent")]
    pub extra_blocks_present: Option<String>,
    #[serde(rename = "Arguments")]
    pub arguments: Option<String>,
    #[serde(rename = "Notes")]
    pub notes: Option<String>,
}

/// One JLECmd `CustomDestinations` CSV record (28 columns, order pinned to the
/// fixture).
#[derive(Serialize)]
pub struct CustomRecord {
    #[serde(rename = "SourceFile")]
    pub source_file: String,
    #[serde(rename = "SourceCreated")]
    pub source_created: WinTimestamp,
    #[serde(rename = "SourceModified")]
    pub source_modified: WinTimestamp,
    #[serde(rename = "SourceAccessed")]
    pub source_accessed: WinTimestamp,
    #[serde(rename = "AppId")]
    pub app_id: String,
    #[serde(rename = "AppIdDescription")]
    pub app_id_description: String,
    #[serde(rename = "EntryName")]
    pub entry_name: Option<String>,
    #[serde(rename = "TargetCreated")]
    pub target_created: WinTimestamp,
    #[serde(rename = "TargetModified")]
    pub target_modified: WinTimestamp,
    #[serde(rename = "TargetAccessed")]
    pub target_accessed: WinTimestamp,
    #[serde(rename = "FileSize")]
    pub file_size: u32,
    #[serde(rename = "RelativePath")]
    pub relative_path: Option<String>,
    #[serde(rename = "WorkingDirectory")]
    pub working_directory: Option<String>,
    #[serde(rename = "FileAttributes")]
    pub file_attributes: String,
    #[serde(rename = "HeaderFlags")]
    pub header_flags: String,
    #[serde(rename = "DriveType")]
    pub drive_type: String,
    #[serde(rename = "VolumeSerialNumber")]
    pub volume_serial_number: Option<String>,
    #[serde(rename = "VolumeLabel")]
    pub volume_label: Option<String>,
    #[serde(rename = "LocalPath")]
    pub local_path: Option<String>,
    #[serde(rename = "CommonPath")]
    pub common_path: Option<String>,
    #[serde(rename = "TargetIDAbsolutePath")]
    pub target_id_absolute_path: Option<String>,
    #[serde(rename = "TargetMFTEntryNumber")]
    pub target_mft_entry_number: Option<String>,
    #[serde(rename = "TargetMFTSequenceNumber")]
    pub target_mft_sequence_number: Option<String>,
    #[serde(rename = "MachineID")]
    pub machine_id: Option<String>,
    #[serde(rename = "MachineMACAddress")]
    pub machine_mac_address: Option<String>,
    #[serde(rename = "TrackerCreatedOn")]
    pub tracker_created_on: WinTimestamp,
    #[serde(rename = "ExtraBlocksPresent")]
    pub extra_blocks_present: Option<String>,
    #[serde(rename = "Arguments")]
    pub arguments: Option<String>,
}

pub const DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        id: "auto",
        default_basename: "JLETriage_AutomaticDestinations_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: None,
    },
    DatasetSpec {
        id: "custom",
        default_basename: "JLETriage_CustomDestinations_Output",
        framing: JsonFraming::Ndjson,
        csv_only: false,
        override_suffix: None,
    },
];

/// Render a `.NET` bool the way JLECmd's CSV writer does: capitalized.
fn dotnet_bool(b: bool) -> String {
    if b {
        "True".to_string()
    } else {
        "False".to_string()
    }
}

/// `Some(s)` when non-empty, else `None` (empty CSV cell / omitted JSON).
fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// The all-zero GUID JLECmd renders as an empty droid cell.
const ZERO_GUID: &str = "00000000-0000-0000-0000-000000000000";

/// A droid GUID column: `None` when empty OR the all-zero GUID. JLECmd's CSV
/// writer collapses `00000000-0000-0000-0000-000000000000` to an empty string
/// (`Program.cs` FileBirthDroid/FileDroid/VolumeBirthDroid/VolumeDroid), so a
/// DestList entry whose droids are unset renders as blank cells, not zeros.
fn droid(s: String) -> Option<String> {
    if s.is_empty() || s == ZERO_GUID {
        None
    } else {
        Some(s)
    }
}

/// The all-zero MAC JLECmd renders as an empty cell (`Program.cs` MacAddress).
fn mac_address(m: Option<String>) -> Option<String> {
    match m {
        Some(s) if s == "00:00:00:00:00:00" => None,
        other => other,
    }
}

/// fs metadata `SystemTime` -> `WinTimestamp` (full nanosecond precision),
/// mirroring LETriage's Source* derivation.
fn systemtime_to_ts(t: SystemTime) -> WinTimestamp {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => WinTimestamp::from_unix_nanos(d.as_secs() as i64, d.subsec_nanos()),
        Err(e) => {
            let d = e.duration();
            let secs = -(d.as_secs() as i64);
            if d.subsec_nanos() == 0 {
                WinTimestamp::from_unix_nanos(secs, 0)
            } else {
                WinTimestamp::from_unix_nanos(secs - 1, 1_000_000_000 - d.subsec_nanos())
            }
        }
    }
}

/// Source* timestamps from the jump list file's fs metadata; none on error.
fn source_times(path: &Path) -> (WinTimestamp, WinTimestamp, WinTimestamp) {
    match std::fs::metadata(path) {
        Ok(md) => (
            md.created()
                .map(systemtime_to_ts)
                .unwrap_or_else(|_| WinTimestamp::none()),
            md.modified()
                .map(systemtime_to_ts)
                .unwrap_or_else(|_| WinTimestamp::none()),
            md.accessed()
                .map(systemtime_to_ts)
                .unwrap_or_else(|_| WinTimestamp::none()),
        ),
        Err(_) => (
            WinTimestamp::none(),
            WinTimestamp::none(),
            WinTimestamp::none(),
        ),
    }
}

/// FILETIME -> WinTimestamp, mapping the FILETIME epoch (0) to none.
fn dest_filetime(ft: u64) -> WinTimestamp {
    if ft == 0 {
        WinTimestamp::none()
    } else {
        WinTimestamp::from_filetime(ft)
    }
}

/// DestList `LastModified` FILETIME -> WinTimestamp, rendering the FILETIME
/// epoch (0) as the literal `1601-01-01T00:00:00.0000000Z` rather than empty.
/// JLECmd writes `LastModified` UNCONDITIONALLY
/// (`destListEntry.LastModified.ToString(dt)` with no 1601 guard), so a
/// DestList entry whose `LastModified` field is 0 renders as the 1601 epoch —
/// unlike `CreationTime`, which JLECmd collapses when its year is the Gregorian
/// 1582 epoch (droid not a v1 UUID).
fn dest_last_modified(ft: u64) -> WinTimestamp {
    if ft == 0 {
        // FILETIME 0 == the 1601-01-01 epoch; express it as the equivalent
        // Unix-epoch second so it renders literally instead of collapsing.
        WinTimestamp::from_unix(-11_644_473_600)
    } else {
        WinTimestamp::from_filetime(ft)
    }
}

/// True when `bytes` opens with the OLE compound-file magic (Automatic).
fn is_automatic(bytes: &[u8]) -> bool {
    bytes.len() >= OLE_MAGIC.len() && bytes[..OLE_MAGIC.len()] == OLE_MAGIC
}

/// True when `bytes` contain the Custom Destinations footer `0xBABFFBAB`
/// (`AB FB BF BA`). Real Custom Destinations files always carry this footer,
/// even empty stubs that have a header but zero entries. Standalone `.lnk`
/// files carry the LNK-header signature (`4C 00 00 00`) but NOT this footer,
/// so gating on the footer eliminates the false-positive that occurred under
/// `--all` (pattern `"*"`): bare `.lnk` files previously passed `is_custom`
/// via the LNK-signature branch, were dispatched to `custom::parse`, and
/// immediately failed ("footer missing"), inflating `summary.failed`.
fn is_custom(bytes: &[u8]) -> bool {
    contains(bytes, &CUSTOM_FOOTER)
}

/// Naive substring search.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// AppId = the first dot-separated segment of the file name (the rest is the
/// `.automaticDestinations-ms` / `.customDestinations-ms` extension).
fn app_id_from_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.split('.').next())
        .unwrap_or("")
        .to_string()
}

pub struct JleTool {
    /// Examine every file as a jump-list candidate (content validation gates).
    pub all: bool,
    /// Surface numbered streams not referenced by the `DestList` (`--withDir`).
    pub with_dir: bool,
    /// Code page for legacy (non-Unicode) string decoding.
    pub codepage: u16,
    /// AppId description table (built-in plus any `--appIds` overrides).
    pub app_ids: triage_jumplist::appid::AppIdTable,
}

impl Default for JleTool {
    fn default() -> Self {
        JleTool {
            all: false,
            with_dir: false,
            codepage: 1252,
            app_ids: AppIdTable::with_builtin(),
        }
    }
}

impl Tool for JleTool {
    fn binary_name(&self) -> &'static str {
        "JLETriage"
    }

    fn patterns(&self) -> &[&'static str] {
        if self.all {
            &["*"]
        } else {
            &["*.automaticDestinations-ms", "*.customDestinations-ms"]
        }
    }

    /// Content validation (never extension-only): a candidate is an Automatic
    /// Destinations file (OLE compound magic) or a Custom Destinations file
    /// (footer / embedded-LNK signature). This gates `--all` against decoys.
    fn validate_legacy(&self, path: &Path) -> bool {
        let Ok(bytes) = std::fs::read(path) else {
            return false;
        };
        is_automatic(&bytes) || is_custom(&bytes)
    }

    fn validate(&self, path: &Path) -> Validation {
        if let Err(error) = std::fs::File::open(path) {
            return Validation::Unreadable {
                error: error.to_string(),
            };
        }
        if self.validate_legacy(path) {
            Validation::Supported
        } else {
            Validation::Unsupported {
                reason: "unrecognized or unsupported Jump List container variant".into(),
            }
        }
    }

    fn datasets(&self) -> &'static [DatasetSpec] {
        DATASETS
    }

    fn scope(&self) -> Scope {
        Scope::UserElseSystem
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        let raw = std::fs::read(path).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

        let source_file = path.display().to_string();
        let (src_created, src_modified, src_accessed) = source_times(path);
        let app_id = app_id_from_name(path);
        let app_id_description = self.app_ids.describe(&app_id);

        if is_automatic(&raw) {
            let parsed = triage_jumplist::automatic::parse(&raw, self.codepage, self.with_dir)
                .map_err(|e| TriageError::Artifact {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                })?;

            let mut count = 0u64;
            for entry in &parsed.entries {
                let dest = &entry.dest;
                let lf = match &entry.lnk {
                    Some(lnk) => LnkFields::from_lnk(lnk, AbsPathFallback::Automatic),
                    None => LnkFields::empty(),
                };
                let record = AutoRecord {
                    source_file: source_file.clone(),
                    source_created: src_created,
                    source_modified: src_modified,
                    source_accessed: src_accessed,
                    app_id: app_id.clone(),
                    app_id_description: app_id_description.clone(),
                    has_sps: dotnet_bool(dest.has_sps),
                    dest_list_version: parsed.dest_list_version.to_string(),
                    last_used_entry_number: parsed.last_used_entry_number.to_string(),
                    mru: dest.mru_position.to_string(),
                    // JLECmd renders EntryNumber as uppercase hex with no `0x`
                    // prefix (`destListEntry.EntryNumber.ToString("X")`).
                    entry_number: format!("{:X}", dest.entry_number),
                    creation_time: dest_filetime(dest.creation_time),
                    last_modified: dest_last_modified(dest.last_modified),
                    hostname: non_empty(dest.hostname.clone()),
                    mac_address: mac_address(dest.mac_address.clone()),
                    path: non_empty(dest.path.clone()),
                    interaction_count: dest.interaction_count.to_string(),
                    pin_status: dotnet_bool(dest.pin_status != -1),
                    file_birth_droid: droid(dest.file_birth_droid.clone()),
                    file_droid: droid(dest.file_droid.clone()),
                    volume_birth_droid: droid(dest.volume_birth_droid.clone()),
                    volume_droid: droid(dest.volume_droid.clone()),
                    target_created: lf.target_created,
                    target_modified: lf.target_modified,
                    target_accessed: lf.target_accessed,
                    file_size: lf.file_size,
                    relative_path: lf.relative_path,
                    working_directory: lf.working_directory,
                    file_attributes: lf.file_attributes,
                    header_flags: lf.header_flags,
                    drive_type: lf.drive_type,
                    volume_serial_number: lf.volume_serial_number,
                    volume_label: lf.volume_label,
                    local_path: lf.local_path,
                    common_path: lf.common_path,
                    target_id_absolute_path: lf.target_id_absolute_path,
                    target_mft_entry_number: lf.target_mft_entry_number,
                    target_mft_sequence_number: lf.target_mft_sequence_number,
                    machine_id: lf.machine_id,
                    machine_mac_address: lf.machine_mac_address,
                    tracker_created_on: lf.tracker_created_on,
                    extra_blocks_present: lf.extra_blocks_present,
                    arguments: lf.arguments,
                    notes: None,
                };
                out.write("auto", &record)?;
                count += 1;
            }
            Ok(count)
        } else {
            let parsed = triage_jumplist::custom::parse(&raw, self.codepage).map_err(|e| {
                TriageError::Artifact {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                }
            })?;

            let mut count = 0u64;
            for entry in &parsed.entries {
                let lf = LnkFields::from_lnk(&entry.lnk, AbsPathFallback::Custom);
                let record = CustomRecord {
                    source_file: source_file.clone(),
                    source_created: src_created,
                    source_modified: src_modified,
                    source_accessed: src_accessed,
                    app_id: app_id.clone(),
                    app_id_description: app_id_description.clone(),
                    entry_name: non_empty(entry.entry_name.clone()),
                    target_created: lf.target_created,
                    target_modified: lf.target_modified,
                    target_accessed: lf.target_accessed,
                    file_size: lf.file_size,
                    relative_path: lf.relative_path,
                    working_directory: lf.working_directory,
                    file_attributes: lf.file_attributes,
                    header_flags: lf.header_flags,
                    drive_type: lf.drive_type,
                    volume_serial_number: lf.volume_serial_number,
                    volume_label: lf.volume_label,
                    local_path: lf.local_path,
                    common_path: lf.common_path,
                    target_id_absolute_path: lf.target_id_absolute_path,
                    target_mft_entry_number: lf.target_mft_entry_number,
                    target_mft_sequence_number: lf.target_mft_sequence_number,
                    machine_id: lf.machine_id,
                    machine_mac_address: lf.machine_mac_address,
                    tracker_created_on: lf.tracker_created_on,
                    extra_blocks_present: lf.extra_blocks_present,
                    arguments: lf.arguments,
                };
                out.write("custom", &record)?;
                count += 1;
            }
            Ok(count)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn droid_nulls_all_zero_guid() {
        assert_eq!(droid(String::new()), None);
        assert_eq!(droid(ZERO_GUID.to_string()), None);
        assert_eq!(
            droid("11223344-5566-1788-99aa-000c293e1f2c".to_string()).as_deref(),
            Some("11223344-5566-1788-99aa-000c293e1f2c")
        );
    }

    #[test]
    fn mac_address_nulls_all_zero() {
        assert_eq!(mac_address(Some("00:00:00:00:00:00".to_string())), None);
        assert_eq!(mac_address(None), None);
        assert_eq!(
            mac_address(Some("00:15:5d:7c:01:17".to_string())).as_deref(),
            Some("00:15:5d:7c:01:17")
        );
    }

    #[test]
    fn dest_last_modified_renders_1601_epoch_for_zero() {
        // JLECmd writes LastModified unconditionally; a 0 FILETIME is the 1601
        // epoch, NOT an empty cell.
        assert_eq!(
            dest_last_modified(0).to_string(),
            "1601-01-01T00:00:00.0000000Z"
        );
        // dest_filetime (used for CreationTime) still collapses 0 to empty.
        assert!(dest_filetime(0).is_none());
    }

    /// A bare LNK-header-only buffer: `4C 00 00 00` followed by zeros.
    /// This must NOT be treated as a Custom Destinations file.
    fn bare_lnk_bytes() -> Vec<u8> {
        let mut b = vec![0x4C, 0x00, 0x00, 0x00];
        b.extend_from_slice(&[0u8; 72]); // minimal LNK header filler
        b
    }

    /// A minimal Custom Destinations buffer: some payload followed by the
    /// required footer `AB FB BF BA`.
    fn custom_with_footer() -> Vec<u8> {
        let mut b = vec![0x4C, 0x00, 0x00, 0x00]; // has LNK sig too
        b.extend_from_slice(&[0u8; 20]);
        b.extend_from_slice(&CUSTOM_FOOTER);
        b
    }

    /// OLE compound-file magic at offset 0.
    fn ole_bytes() -> Vec<u8> {
        let mut b = OLE_MAGIC.to_vec();
        b.extend_from_slice(&[0u8; 512]);
        b
    }

    #[test]
    fn is_custom_requires_footer_not_bare_lnk() {
        // A bare LNK signature alone must NOT pass is_custom.
        assert!(
            !is_custom(&bare_lnk_bytes()),
            "bare LNK should not pass is_custom"
        );
    }

    #[test]
    fn is_custom_true_when_footer_present() {
        assert!(
            is_custom(&custom_with_footer()),
            "custom file with footer should pass is_custom"
        );
    }

    #[test]
    fn is_custom_false_for_empty_and_short_buffers() {
        assert!(!is_custom(&[]), "empty buffer should not pass is_custom");
        assert!(
            !is_custom(&[0xAB, 0xFB, 0xBF]),
            "3-byte partial footer should not pass is_custom"
        );
    }

    #[test]
    fn is_automatic_true_for_ole_magic() {
        assert!(
            is_automatic(&ole_bytes()),
            "OLE magic should pass is_automatic"
        );
    }

    #[test]
    fn is_automatic_false_for_custom_and_lnk() {
        assert!(
            !is_automatic(&custom_with_footer()),
            "custom file should not pass is_automatic"
        );
        assert!(
            !is_automatic(&bare_lnk_bytes()),
            "bare LNK should not pass is_automatic"
        );
    }
}
