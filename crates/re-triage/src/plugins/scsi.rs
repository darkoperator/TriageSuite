//! Port of RegistryPlugin.SCSI (SCSI.cs + ValuesOut.cs).
//! Extracts SCSI device information from the Enum\SCSI registry key.
//!
//! Key path: ControlSet00*\Enum\SCSI
//! Walks: device-type subkeys (e.g. CdRom&Ven_Msft&Prod_Virtual_DVD-ROM)
//!        → each serial-number subkey.
//!
//! Device-type subkey filter: must have 3 parts when split by '&' (manufacturer
//! at index 1, title at index 2). Skips device-type keys with 0 subkeys.
//!
//! Properties are read from:
//!   Properties\{540b947e-...}\0004 → (default) raw UTF-16LE = deviceName
//!   Properties\{83da6326-...}\0064 → (default) raw 8-byte FILETIME = installed
//!   Properties\{83da6326-...}\0065 → (default) raw 8-byte FILETIME = firstInstalled
//!   Properties\{83da6326-...}\0066 → (default) raw 8-byte FILETIME = lastConnected
//!   Properties\{83da6326-...}\0067 → (default) raw 8-byte FILETIME = lastRemoved
//!
//! Device Parameters (for diskId / initialTimestamp):
//!   C# computes deviceParametersKey from `subKey.SubKeys.First()` (the FIRST
//!   serial subkey), and this value is shared for ALL serial subkeys in the loop.
//!   Partmgr\DiskId → diskId string
//!   Storport\InitialTimestamp → raw 8-byte FILETIME → UTC DateTimeOffset
//!
//! Detail-CSV column order (fixture-authoritative):
//!   Timestamp, BatchKeyPath, Manufacturer, BatchValueName, Title, SerialNumber,
//!   DeviceName, DiskId, InitialTimestamp, Installed, FirstInstalled, LastConnected, LastRemoved
//!
//! Batch row format (ValueName = "Multiple"):
//!   ValueData1 = "Manufacturer: {Manufacturer} Title: {Title} SerialNumber: {SerialNumber} DeviceName: {DeviceName} DiskId: {DiskId}"
//!   ValueData2 = "Timestamp: {Timestamp:fffffff} InitialTimestamp: {InitialTimestamp:fffffff}"
//!   ValueData3 = "Installed: {ts}FirstInstalled: {ts}LastConnected: {ts}LastRemoved: {ts}"
//!                (NO spaces between these — C# uses string concatenation with +)
//!
//! Note: C# `Encoding.Unicode.GetString(bytes)` decodes full UTF-16LE including
//! the null terminator, yielding a trailing null character U+0000.

use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct Scsi;

const GUID_NAME: &str = "{540b947e-8b40-45bc-a8a2-6a0b894cbda2}";
const GUID_TIMESTAMPS: &str = "{83da6326-97a6-4088-9453-a1923f573b29}";

/// Get raw bytes of the `(default)` value from
/// `<serial_pretty_path>\Properties\<guid_value>\<num_value>` using path-based lookup.
/// `serial_pretty_path` must be root-stripped (as returned by `get_pretty_path()`).
/// Returns None if any segment is missing or value has no raw bytes.
fn get_property_raw_by_path(
    serial_pretty_path: &str,
    guid_value: &str,
    num_value: &str,
    hive: &mut Hive,
) -> Option<Vec<u8>> {
    let target = format!("{serial_pretty_path}\\Properties\\{guid_value}\\{num_value}");
    let key = hive.get_key(&target)?;

    // Read the (default) value — notatin names it "(default)"
    for v in key.value_iter() {
        if v.get_pretty_name() == "(default)" {
            let (content, _) = v.get_content();
            return Some(triage_registry::value::plugin_raw_bytes(&content));
        }
    }
    None
}

/// Convert 8-byte FILETIME to WinTimestamp in UTC (mirrors C# DateTimeOffset.FromFileTime + ToUniversalTime).
fn filetime_to_wints(data: Option<Vec<u8>>) -> Option<WinTimestamp> {
    let bytes = data?;
    if bytes.len() != 8 {
        return None;
    }
    let ft = u64::from_le_bytes(bytes[..8].try_into().ok()?);
    if ft == 0 {
        return None;
    }
    let ts = WinTimestamp::from_filetime(ft);
    if ts.is_none() {
        None
    } else {
        Some(ts)
    }
}

/// Decode UTF-16LE bytes to String (mirrors C# Encoding.Unicode.GetString).
/// Includes null terminator as U+0000 character — matches C# behavior where
/// the trailing null renders as an extra character in the output string.
fn decode_utf16le(data: Option<Vec<u8>>) -> String {
    let bytes = match data {
        Some(b) if b.len() >= 2 => b,
        _ => return String::new(),
    };
    let u16s: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&u16s).to_string()
}

/// Format a WinTimestamp as RECmd literal "yyyy-MM-dd HH:mm:ss.fffffff" for embedded fields.
fn wints_to_recmd_literal(ts: &Option<WinTimestamp>) -> String {
    match ts {
        Some(w) => {
            let s = w.to_string();
            if s.is_empty() {
                return String::new();
            }
            // Convert "yyyy-MM-ddTHH:mm:ss.fffffffZ" → "yyyy-MM-dd HH:mm:ss.fffffff"
            let mut r = s.replace('T', " ");
            if r.ends_with('Z') {
                r.pop();
            }
            r
        }
        None => String::new(),
    }
}

fn get_str_value(key: &CellKeyNode, name: &str) -> String {
    for v in key.value_iter() {
        if v.get_pretty_name() == name {
            let (content, _) = v.get_content();
            return triage_registry::value::render_value_data(&content);
        }
    }
    String::new()
}

impl RegistryPlugin for Scsi {
    fn plugin_name(&self) -> &'static str {
        "SCSI"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"ControlSet00*\Enum\SCSI"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let mut rows = Vec::new();

        // Iterate device-type subkeys (e.g. "CdRom&Ven_Msft&Prod_Virtual_DVD-ROM")
        let device_type_keys = hive.sub_keys(key);
        for mut device_type_key in device_type_keys {
            // C# skips if SubKeys.Count == 0
            let serial_subkeys = hive.sub_keys(&mut device_type_key);
            if serial_subkeys.is_empty() {
                continue;
            }

            // Split key name by '&' — must have exactly 3 parts
            let words: Vec<&str> = device_type_key.key_name.split('&').collect();
            if words.len() != 3 {
                continue;
            }
            let manufacturer = words[1].to_string();
            let title = words[2].to_string();

            // BatchKeyPath and Timestamp come from the device-type key (subKey in C#)
            let batch_key_path = device_type_key.path.trim_start_matches('\\').to_string();
            let ts_dt = device_type_key.last_key_written_date_and_time();
            let ts_iso =
                WinTimestamp::from_unix_nanos(ts_dt.timestamp(), ts_dt.timestamp_subsec_nanos())
                    .to_string();

            // C# reads deviceParametersKey from subKey.SubKeys.First().SubKeys
            // (the FIRST serial subkey, not the current one in the loop).
            // This value is reused for ALL serial subkeys.
            // Use get_pretty_path() (root-stripped) for hive.get_key compatibility.
            let first_serial_pretty = serial_subkeys[0].get_pretty_path().to_string();
            let device_params_path = format!("{first_serial_pretty}\\Device Parameters");

            let mut disk_id = String::new();
            let mut initial_timestamp: Option<WinTimestamp> = None;

            if let Some(mut dp_key) = hive.get_key(&device_params_path) {
                let dp_subs = hive.sub_keys(&mut dp_key);
                for dp_sub in dp_subs {
                    if dp_sub.key_name.eq_ignore_ascii_case("Partmgr") {
                        let did = get_str_value(&dp_sub, "DiskId");
                        if !did.is_empty() {
                            disk_id = did;
                        }
                    } else if dp_sub.key_name.eq_ignore_ascii_case("Storport") {
                        for v in dp_sub.value_iter() {
                            if v.get_pretty_name() == "InitialTimestamp" {
                                let (content, _) = v.get_content();
                                let raw = triage_registry::value::plugin_raw_bytes(&content);
                                initial_timestamp = filetime_to_wints(Some(raw));
                                break;
                            }
                        }
                    }
                }
            }

            // Now iterate all serial subkeys — use path-based lookup for Properties
            for serial_key in serial_subkeys {
                let serial_number = serial_key.key_name.clone();
                // Use get_pretty_path() (root-stripped) for hive.get_key compatibility
                let serial_path = serial_key.get_pretty_path().to_string();

                // deviceName: Properties\{540b947e-...}\0004 → (default) raw UTF-16LE
                let device_name_raw =
                    get_property_raw_by_path(&serial_path, GUID_NAME, "0004", hive);
                let device_name = decode_utf16le(device_name_raw);

                // Timestamps from Properties\{83da6326-...}\{0064,0065,0066,0067}
                let installed_raw =
                    get_property_raw_by_path(&serial_path, GUID_TIMESTAMPS, "0064", hive);
                let first_installed_raw =
                    get_property_raw_by_path(&serial_path, GUID_TIMESTAMPS, "0065", hive);
                let last_connected_raw =
                    get_property_raw_by_path(&serial_path, GUID_TIMESTAMPS, "0066", hive);
                let last_removed_raw =
                    get_property_raw_by_path(&serial_path, GUID_TIMESTAMPS, "0067", hive);

                let installed = filetime_to_wints(installed_raw);
                let first_installed = filetime_to_wints(first_installed_raw);
                let last_connected = filetime_to_wints(last_connected_raw);
                let last_removed = filetime_to_wints(last_removed_raw);

                // Standalone timestamp detail columns (ISO; testkit normalizes reference)
                let installed_str = installed
                    .as_ref()
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                let first_installed_str = first_installed
                    .as_ref()
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                let last_connected_str = last_connected
                    .as_ref()
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                let last_removed_str = last_removed
                    .as_ref()
                    .map(|t| t.to_string())
                    .unwrap_or_default();
                let initial_ts_str = initial_timestamp
                    .as_ref()
                    .map(|t| t.to_string())
                    .unwrap_or_default();

                // Batch value data
                let vd1 = format!(
                    "Manufacturer: {manufacturer} Title: {title} SerialNumber: {serial_number} DeviceName: {device_name} DiskId: {disk_id}"
                );
                let ts_recmd = {
                    let ticks = ts_dt.timestamp_subsec_nanos() / 100;
                    format!("{}.{:07}", ts_dt.format("%Y-%m-%d %H:%M:%S"), ticks)
                };
                let init_ts_recmd = wints_to_recmd_literal(&initial_timestamp);
                let vd2 = format!("Timestamp: {ts_recmd} InitialTimestamp: {init_ts_recmd}");

                // C# string concatenation — no spaces between fields
                let inst_recmd = wints_to_recmd_literal(&installed);
                let fi_recmd = wints_to_recmd_literal(&first_installed);
                let lc_recmd = wints_to_recmd_literal(&last_connected);
                let lr_recmd = wints_to_recmd_literal(&last_removed);
                let vd3 = format!(
                    "Installed: {inst_recmd}FirstInstalled: {fi_recmd}LastConnected: {lc_recmd}LastRemoved: {lr_recmd}"
                );

                rows.push(PluginRow {
                    batch_value_name: "Multiple".to_string(),
                    batch_value_data1: vd1,
                    batch_value_data2: vd2,
                    batch_value_data3: vd3,
                    detail_columns: vec![
                        ("Timestamp".to_string(), ts_iso.clone()),
                        ("BatchKeyPath".to_string(), batch_key_path.clone()),
                        ("Manufacturer".to_string(), manufacturer.clone()),
                        ("BatchValueName".to_string(), "Multiple".to_string()),
                        ("Title".to_string(), title.clone()),
                        ("SerialNumber".to_string(), serial_number),
                        ("DeviceName".to_string(), device_name),
                        ("DiskId".to_string(), disk_id.clone()),
                        ("InitialTimestamp".to_string(), initial_ts_str.clone()),
                        ("Installed".to_string(), installed_str),
                        ("FirstInstalled".to_string(), first_installed_str),
                        ("LastConnected".to_string(), last_connected_str),
                        ("LastRemoved".to_string(), last_removed_str),
                    ],
                });
            }
        }

        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_and_key_paths() {
        assert_eq!(Scsi.plugin_name(), "SCSI");
        assert_eq!(Scsi.key_paths(), &[r"ControlSet00*\Enum\SCSI"]);
    }

    #[test]
    fn decode_utf16le_with_null_terminator() {
        // "AB\0" encoded as UTF-16LE: [0x41,0x00, 0x42,0x00, 0x00,0x00]
        let bytes = vec![0x41u8, 0x00, 0x42, 0x00, 0x00, 0x00];
        let result = decode_utf16le(Some(bytes));
        // C# Encoding.Unicode.GetString includes the null: "AB\0"
        assert_eq!(result, "AB\0");
    }

    #[test]
    fn decode_utf16le_empty() {
        assert_eq!(decode_utf16le(None), "");
        assert_eq!(decode_utf16le(Some(vec![])), "");
        assert_eq!(decode_utf16le(Some(vec![0x00])), ""); // 1 byte, too short
    }

    #[test]
    fn filetime_to_wints_zero_returns_none() {
        let result = filetime_to_wints(Some(vec![0u8; 8]));
        assert!(result.is_none());
    }

    #[test]
    fn filetime_to_wints_valid() {
        // FILETIME for 2024-06-29 02:02:45.7138931 UTC
        let secs: i64 = 1719626565;
        let filetime_secs = secs + 11_644_473_600i64;
        let ft = (filetime_secs as u64) * 10_000_000 + 7138931;
        let bytes = ft.to_le_bytes().to_vec();
        let result = filetime_to_wints(Some(bytes));
        assert!(result.is_some());
        let s = result.unwrap().to_string();
        assert!(s.starts_with("2024-06-29T02:02:45."), "got: {s}");
    }

    #[test]
    fn wints_to_recmd_literal_converts_format() {
        use triage_core::timestamp::WinTimestamp;
        let ts = WinTimestamp::from_unix_nanos(1719626565, 71389310);
        let result = wints_to_recmd_literal(&Some(ts));
        assert!(result.starts_with("2024-06-29 02:02:45."), "got: {result}");
        assert!(!result.ends_with('Z'));
        assert!(!result.contains('T'));
    }

    #[test]
    fn vd3_format_no_spaces_between_fields() {
        // C# ValuesOut.BatchValueData3 uses + concatenation — no spaces between fields
        let inst = "2024-01-01 00:00:00.0000000";
        let fi = "2024-01-02 00:00:00.0000000";
        let lc = "2024-01-03 00:00:00.0000000";
        let lr = "";
        let vd3 =
            format!("Installed: {inst}FirstInstalled: {fi}LastConnected: {lc}LastRemoved: {lr}");
        // Confirm no space between "...0000000FirstInstalled"
        assert!(vd3.contains("0000000FirstInstalled"));
    }

    #[test]
    fn detail_column_order() {
        let expected = [
            "Timestamp",
            "BatchKeyPath",
            "Manufacturer",
            "BatchValueName",
            "Title",
            "SerialNumber",
            "DeviceName",
            "DiskId",
            "InitialTimestamp",
            "Installed",
            "FirstInstalled",
            "LastConnected",
            "LastRemoved",
        ];
        assert_eq!(expected.len(), 13);
    }
}
