//! Embedded-LNK-derived columns shared by Automatic and Custom Destination
//! records.
//!
//! These columns are derived IDENTICALLY to LETriage (M3): the same target
//! FILETIME -> WinTimestamp rule (with the 1601->none collapse), the same
//! MFT-from-last-target-id hex rendering, the same FileAttributes/HeaderFlags
//! strings, the same `(None)` DriveType sentinel, and the same JSON
//! null-omission via `Option<String>`. Unlike LETriage, Auto/Custom CSVs OMIT
//! the `NetworkPath` and `MACVendor` columns, so neither is carried here.

use triage_core::timestamp::WinTimestamp;
use triage_lnk::LnkFile;

/// JLECmd's `TargetIDAbsolutePath` resolution (`Program.cs`
/// `GetAbsolutePathFromTargetIDs` + the empty-result fallback). When the LNK
/// has no target IDs (so the shell-item join is empty), JLECmd reconstructs the
/// path from the `LinkInfo`. The Automatic and Custom writers use *different*
/// fallbacks, so this takes the resolved-or-empty base path plus an explicit
/// [`AbsPathFallback`] selector.
///
/// Automatic (Program.cs ~1330): network share present → `{share}\{common}`,
/// else `{local}\{common}`.
/// Custom (Program.cs ~1518): always `{share}\{common}` (the share name is
/// empty when there is no network info, yielding `\{common}`).
pub enum AbsPathFallback {
    Automatic,
    Custom,
}

fn resolve_abs_path(lnk: &LnkFile, base: String, kind: AbsPathFallback) -> String {
    if !base.is_empty() {
        return base;
    }
    let li = lnk.link_info.as_ref();
    let network = li.and_then(|l| l.network_path.clone());
    let local = li.and_then(|l| l.local_path.clone());
    let common = li.and_then(|l| l.common_path.clone()).unwrap_or_default();
    match kind {
        AbsPathFallback::Automatic => match network {
            Some(share) => format!("{share}\\{common}"),
            None => format!("{}\\{common}", local.unwrap_or_default()),
        },
        AbsPathFallback::Custom => {
            format!("{}\\{common}", network.unwrap_or_default())
        }
    }
}

/// LECmd/JLECmd target FILETIME -> WinTimestamp with the 1601->none rule. An
/// unset target timestamp is either literally 0 (mapped to None by
/// `from_filetime`) or a small non-zero offset that renders in year 1601;
/// collapse the latter to None so our Target* cells match JLECmd's empties.
fn target_filetime(ft: u64) -> WinTimestamp {
    let ts = WinTimestamp::from_filetime(ft);
    if ts.is_none() {
        return ts;
    }
    if ts.to_string().starts_with("1601") {
        return WinTimestamp::none();
    }
    ts
}

/// `Some(s)` when non-empty, else `None`. Empty JLECmd-always-present fields
/// serialize as a JSON null (omitted by the null-nuking sink) yet render as an
/// empty CSV cell.
fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// The embedded-LNK-derived column set shared by Auto and Custom records.
pub struct LnkFields {
    pub target_created: WinTimestamp,
    pub target_modified: WinTimestamp,
    pub target_accessed: WinTimestamp,
    pub file_size: u32,
    pub relative_path: Option<String>,
    pub working_directory: Option<String>,
    pub file_attributes: String,
    pub header_flags: String,
    pub drive_type: String,
    pub volume_serial_number: Option<String>,
    pub volume_label: Option<String>,
    pub local_path: Option<String>,
    pub common_path: Option<String>,
    pub target_id_absolute_path: Option<String>,
    pub target_mft_entry_number: Option<String>,
    pub target_mft_sequence_number: Option<String>,
    pub machine_id: Option<String>,
    pub machine_mac_address: Option<String>,
    pub tracker_created_on: WinTimestamp,
    pub extra_blocks_present: Option<String>,
    pub arguments: Option<String>,
}

impl LnkFields {
    /// Derive the column set from a parsed embedded LNK, mirroring LETriage's
    /// `build_record` exactly. `abs_fallback` selects the dataset-specific
    /// `TargetIDAbsolutePath` reconstruction JLECmd applies when the LNK carries
    /// no target IDs (Automatic vs Custom differ — see [`AbsPathFallback`]).
    pub fn from_lnk(lnk: &LnkFile, abs_fallback: AbsPathFallback) -> LnkFields {
        let li = lnk.link_info.as_ref();

        // MFT info: the LAST target ID's last Beef0004 extension block,
        // rendered as uppercase hex `0x{X}`; empty when absent.
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
        let tracker_created_on = tracker
            .map(|t| WinTimestamp::from_filetime(t.created_on))
            .unwrap_or_else(WinTimestamp::none);

        LnkFields {
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
            common_path: li.and_then(|l| l.common_path.clone()),
            target_id_absolute_path: non_empty(resolve_abs_path(
                lnk,
                triage_shellitems::absolute_path(&lnk.target_ids),
                abs_fallback,
            )),
            target_mft_entry_number: mft_entry,
            target_mft_sequence_number: mft_seq,
            machine_id: tracker.and_then(|t| t.machine_id.clone()),
            machine_mac_address,
            tracker_created_on,
            extra_blocks_present: non_empty(lnk.extra_data.blocks_present.join(", ")),
            arguments: lnk.string_data.arguments.clone(),
        }
    }

    /// The empty column set for an Automatic entry with no embedded LNK: every
    /// optional column is None, the timestamps are none, file size 0, and the
    /// DriveType carries JLECmd's `(None)` sentinel.
    pub fn empty() -> LnkFields {
        LnkFields {
            target_created: WinTimestamp::none(),
            target_modified: WinTimestamp::none(),
            target_accessed: WinTimestamp::none(),
            file_size: 0,
            relative_path: None,
            working_directory: None,
            file_attributes: String::new(),
            header_flags: String::new(),
            drive_type: "(None)".to_string(),
            volume_serial_number: None,
            volume_label: None,
            local_path: None,
            common_path: None,
            target_id_absolute_path: None,
            target_mft_entry_number: None,
            target_mft_sequence_number: None,
            machine_id: None,
            machine_mac_address: None,
            tracker_created_on: WinTimestamp::none(),
            extra_blocks_present: None,
            arguments: None,
        }
    }
}
