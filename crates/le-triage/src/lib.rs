//! LETriage — LECmd-compatible Windows Shell Link parser.

pub mod macvendor;

use serde::Serialize;
use std::path::Path;
use std::time::SystemTime;
use triage_core::error::TriageError;
use triage_core::output::dataset::{DatasetSpec, JsonFraming};
use triage_core::output::router::OutputRouter;
use triage_core::timestamp::WinTimestamp;
use triage_core::tool::{Scope, Tool, Validation};
use triage_lnk::LnkFile;

/// `DRIVE_REMOVABLE` from the LinkInfo drive-type enum (used by `--removable-only`).
const DRIVE_REMOVABLE: u32 = 2;

/// One LECmd `CsvOut`-compatible record (the 27-column contract; column names
/// and order are pinned to `tests/fixtures/lecmd/DESKTOP/lecmd.csv`). Optional
/// fields use `Option`/`WinTimestamp` so JSON null-omission (M1 convention)
/// drops absent values, matching LECmd's `ToJson` nuke-nulls behavior. Fields
/// LECmd always emits (`FileSize`, `TargetIDAbsolutePath`, `ExtraBlocksPresent`)
/// are non-optional.
#[derive(Serialize)]
pub struct LnkRecord {
    #[serde(rename = "SourceFile")]
    pub source_file: String,
    #[serde(rename = "SourceCreated")]
    pub source_created: WinTimestamp,
    #[serde(rename = "SourceModified")]
    pub source_modified: WinTimestamp,
    #[serde(rename = "SourceAccessed")]
    pub source_accessed: WinTimestamp,
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
    #[serde(rename = "NetworkPath")]
    pub network_path: Option<String>,
    #[serde(rename = "CommonPath")]
    pub common_path: Option<String>,
    #[serde(rename = "Arguments")]
    pub arguments: Option<String>,
    /// `Option` so an empty path serializes as JSON null (omitted by the
    /// null-nuking sink, matching LECmd's `ToJson`) yet renders as an empty CSV
    /// cell. LECmd omits this property from JSON when no Target ID list resolves.
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
    #[serde(rename = "MACVendor")]
    pub mac_vendor: Option<String>,
    #[serde(rename = "TrackerCreatedOn")]
    pub tracker_created_on: WinTimestamp,
    /// `Option` for the same reason as `TargetIDAbsolutePath`: LECmd omits this
    /// property from JSON when no extra-data blocks are present, but the CSV
    /// cell is empty rather than absent.
    #[serde(rename = "ExtraBlocksPresent")]
    pub extra_blocks_present: Option<String>,
}

pub const DATASETS: &[DatasetSpec] = &[DatasetSpec {
    id: "main",
    default_basename: "LETriage_Output",
    framing: JsonFraming::Ndjson,
    csv_only: false,
    override_suffix: None,
}];

/// LECmd target FILETIME -> WinTimestamp with the 1601->null rule. LECmd emits
/// an empty cell when a target timestamp is unset; an unset FILETIME is either
/// literally 0 (handled by `from_filetime`) or a value that renders in year
/// 1601. `from_filetime` already maps 0 and out-of-range values to None, but a
/// small non-zero FILETIME (e.g. an offset) can still land in 1601; treat that
/// as unset too so our Target* cells match the fixture's empties.
fn target_filetime(ft: u64) -> WinTimestamp {
    let ts = WinTimestamp::from_filetime(ft);
    if ts.is_none() {
        return ts;
    }
    // `Display` renders ISO 8601 with a 4-digit year prefix; a 1601 year means
    // the value is the FILETIME epoch (unset), so drop it.
    if ts.to_string().starts_with("1601") {
        return WinTimestamp::none();
    }
    ts
}

/// fs metadata SystemTime -> WinTimestamp (full nanosecond precision). Anything
/// before the Unix epoch or otherwise unrepresentable maps to None.
fn systemtime_to_ts(t: SystemTime) -> WinTimestamp {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => WinTimestamp::from_unix_nanos(d.as_secs() as i64, d.subsec_nanos()),
        // Pre-1970 timestamps: fall back to negative-seconds path.
        Err(e) => {
            let d = e.duration();
            let secs = -(d.as_secs() as i64);
            // Borrow a second to keep nanos non-negative for from_unix_nanos.
            if d.subsec_nanos() == 0 {
                WinTimestamp::from_unix_nanos(secs, 0)
            } else {
                WinTimestamp::from_unix_nanos(secs - 1, 1_000_000_000 - d.subsec_nanos())
            }
        }
    }
}

/// Assemble the 27-column record from a parsed LNK plus its source path.
fn build_record(source_file: String, path: &Path, lnk: &LnkFile) -> LnkRecord {
    // Source* = fs metadata of the LNK file itself; on metadata error, none.
    let (source_created, source_modified, source_accessed) = match std::fs::metadata(path) {
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
    };

    let li = lnk.link_info.as_ref();

    // MFT info: LECmd reads the LAST target ID's last Beef0004 extension block
    // (Program.cs ~843: `lnk.TargetIDs.Last()`), rendering entry/sequence as
    // uppercase hex `0x{X}` strings; empty when absent.
    let (mft_entry, mft_seq) = lnk
        .target_ids
        .last()
        .and_then(|si| si.extension.as_ref())
        .map(|ext| {
            (
                ext.mft_entry.map(|e| format!("0x{e:X}")),
                ext.mft_sequence.map(|s| format!("0x{s:X}")),
            )
        })
        .unwrap_or((None, None));

    let tracker = lnk.extra_data.tracker.as_ref();
    let machine_mac_address = tracker.and_then(|t| t.mac_address.clone());
    // LECmd: MACVendor = GetVendorFromMac(tnbBlock?.MacAddress). With no
    // tracker the MAC is null and the resulting MACVendor cell is EMPTY
    // (verified: all 14 no-MAC fixture rows have empty MACVendor). So only
    // resolve a vendor when a MAC is present.
    let mac_vendor = machine_mac_address
        .as_deref()
        .map(macvendor::vendor_for_mac);
    let tracker_created_on = tracker
        .map(|t| WinTimestamp::from_filetime(t.created_on))
        .unwrap_or_else(WinTimestamp::none);

    LnkRecord {
        source_file,
        source_created,
        source_modified,
        source_accessed,
        target_created: target_filetime(lnk.header.target_creation),
        target_modified: target_filetime(lnk.header.target_write),
        target_accessed: target_filetime(lnk.header.target_access),
        file_size: lnk.header.file_size,
        relative_path: lnk.string_data.relative_path.clone(),
        working_directory: lnk.string_data.working_directory.clone(),
        file_attributes: lnk.header.file_attributes_string(),
        header_flags: lnk.header.header_flags_string(),
        drive_type: li
            .and_then(|l| l.drive_type.as_deref())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "(None)".to_string()),
        volume_serial_number: li.and_then(|l| l.volume_serial_number.clone()),
        volume_label: li.and_then(|l| l.volume_label.clone()),
        local_path: li.and_then(|l| l.local_path.clone()),
        network_path: li.and_then(|l| l.network_path.clone()),
        common_path: li.and_then(|l| l.common_path.clone()),
        arguments: lnk.string_data.arguments.clone(),
        target_id_absolute_path: non_empty(triage_shellitems::absolute_path(&lnk.target_ids)),
        target_mft_entry_number: mft_entry,
        target_mft_sequence_number: mft_seq,
        machine_id: tracker.and_then(|t| t.machine_id.clone()),
        machine_mac_address,
        mac_vendor,
        tracker_created_on,
        extra_blocks_present: non_empty(lnk.extra_data.blocks_present.join(", ")),
    }
}

/// `Some(s)` when non-empty, else `None`. Used so empty LECmd-always-present
/// fields serialize as a JSON null (omitted) but an empty CSV cell.
fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

pub struct LeTool {
    /// Only emit LNKs whose target volume is removable (LECmd `--removable`).
    pub removable_only: bool,
    /// Examine every file as an LNK candidate (content validation still gates).
    pub all: bool,
    /// Code page for legacy (non-Unicode) string decoding.
    pub codepage: u16,
}

impl Default for LeTool {
    fn default() -> Self {
        LeTool {
            removable_only: false,
            all: false,
            codepage: 1252,
        }
    }
}

impl Tool for LeTool {
    fn binary_name(&self) -> &'static str {
        "LETriage"
    }

    fn patterns(&self) -> &[&'static str] {
        if self.all {
            &["*"]
        } else {
            &["*.lnk"]
        }
    }

    /// Content validation (spec 3.2, never extension-only): the fixed 0x4C-byte
    /// header must parse. This rejects `$I` decoys and non-LNK files even under
    /// `--all`.
    fn validate_legacy(&self, path: &Path) -> bool {
        let Ok(bytes) = read_header_bytes(path) else {
            return false;
        };
        triage_lnk::header::Header::parse(&bytes).is_ok()
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
                reason: "filename matched .lnk but content is not a Shell Link".into(),
            }
        }
    }

    fn datasets(&self) -> &'static [DatasetSpec] {
        DATASETS
    }

    /// LNK files attribute to the in-capture user when derivable, else system.
    fn scope(&self) -> Scope {
        Scope::UserElseSystem
    }

    fn parse(&self, path: &Path, out: &mut OutputRouter) -> Result<u64, TriageError> {
        let raw = std::fs::read(path).map_err(|e| TriageError::Artifact {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let lnk = triage_lnk::parse_with_codepage(&raw, self.codepage).map_err(|e| {
            TriageError::Artifact {
                path: path.to_path_buf(),
                message: e.to_string(),
            }
        })?;

        // `--removable-only`: skip links whose target volume is not removable
        // (LECmd returns null for these). drive_type_raw absent => not removable.
        if self.removable_only {
            let is_removable = lnk
                .link_info
                .as_ref()
                .and_then(|l| l.drive_type_raw)
                .is_some_and(|dt| dt == DRIVE_REMOVABLE);
            if !is_removable {
                return Ok(0);
            }
        }

        let record = build_record(path.display().to_string(), path, &lnk);
        out.write("main", &record)?;
        Ok(1)
    }
}

/// Read up to the fixed-header length (0x4C) for content validation.
fn read_header_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; triage_lnk::header::Header::SIZE];
    let n = f.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}
