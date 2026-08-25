//! Port of RegistryPlugin.DeviceClasses (DeviceClasses.cs + ValuesOut.cs).
//! Extracts device interface information from DeviceClasses registry keys.
//!
//! Key path: ControlSet00*\Control\DeviceClasses
//! Walks: only GUID subkeys matching the allowlist → each device subkey.
//!
//! The C# plugin filters to 5 specific GUIDs (case-insensitive match):
//!   {10497B1B-BA51-44E5-8318-A65C837B6661}
//!   {53F5630D-B6BF-11D0-94F2-00A0C91EFB8B}
//!   {53F56307-B6BF-11D0-94F2-00A0C91EFB8B}
//!   {6AC27878-A6FA-4155-BA85-F98F491D4F33}
//!   {A5DCBF10-6530-11D2-901F-00C04FB951ED}
//!
//! ParseData logic:
//!   filterName = subKey.KeyName.Replace("##?#", "")
//!   keyName    = filterName.Split('#')
//!   Type       = keyName[0], Name = keyName[1], serialNumber = keyName[2]
//!   If filterName contains "##": split on "##", take result[1..], Name=result[1], serialNumber=result[2]
//!   If filterName contains "??": split on "??", take result[1..], Name=result[1], serialNumber=result[2]
//!
//! Detail-CSV column order (fixture-authoritative):
//!   Timestamp, BatchKeyPath, GuidFolder, BatchValueName, Type, Name, SerialNumber
//!
//! Batch row format (ValueName = "Multiple"):
//!   ValueData1 = "Type: {Type} Name: {Name} SerialNumber: {SerialNumber}"
//!   ValueData2 = "Timestamp: {Timestamp:yyyy-MM-dd HH:mm:ss.fffffff}"
//!   ValueData3 = ""

use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct DeviceClasses;

/// The 5 GUID allowlist from C# DeviceClasses.cs (compared case-insensitively).
const ALLOWED_GUIDS: &[&str] = &[
    "{10497b1b-ba51-44e5-8318-a65c837b6661}",
    "{53f5630d-b6bf-11d0-94f2-00a0c91efb8b}",
    "{53f56307-b6bf-11d0-94f2-00a0c91efb8b}",
    "{6ac27878-a6fa-4155-ba85-f98f491d4f33}",
    "{a5dcbf10-6530-11d2-901f-00c04fb951ed}",
];

fn is_allowed_guid(name: &str) -> bool {
    let lower = name.to_lowercase();
    ALLOWED_GUIDS.iter().any(|g| *g == lower)
}

/// Convert a chrono::DateTime<Utc> to RECmd literal "yyyy-MM-dd HH:mm:ss.fffffff"
/// for embedded ValueData2 text (not normalized by testkit).
fn dt_to_recmd_literal(dt: chrono::DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks)
}

/// Convert chrono::DateTime<Utc> to ISO-8601 UTC (standalone Timestamp column,
/// normalized by testkit from RECmd's space-separated format).
fn dt_to_iso8601(dt: chrono::DateTime<chrono::Utc>) -> String {
    let ts = WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos());
    ts.to_string()
}

/// Parse device key name to (Type, Name, SerialNumber), replicating C# ParseData exactly.
///
/// Steps:
///   1. filterName = key_name.replace("##?#", "")
///   2. Split by '#' for initial Type/Name/serialNumber
///   3. If filterName contains "##": re-parse from the "##" portion
///   4. If filterName contains "??": re-parse from the "??" portion
fn parse_data(key_name: &str) -> (String, String, String) {
    let filter_name = key_name.replace("##?#", "");

    let parts: Vec<&str> = filter_name.split('#').collect();
    let type_ = parts.first().copied().unwrap_or("").to_string();
    let mut name = parts.get(1).copied().unwrap_or("").to_string();
    let mut serial_number = parts.get(2).copied().unwrap_or("").to_string();

    // C# regex check: if filterName contains "##"
    let regex_data = if filter_name.contains("##") {
        let result: Vec<&str> = filter_name.splitn(2, "##").collect();
        // result[1] is the part after "##"
        result.get(1).copied()
    } else if filter_name.contains("??") {
        let result: Vec<&str> = filter_name.splitn(2, "??").collect();
        result.get(1).copied()
    } else {
        None
    };

    if let Some(rd) = regex_data {
        let result: Vec<&str> = rd.split('#').collect();
        name = result.get(1).copied().unwrap_or("").to_string();
        serial_number = result.get(2).copied().unwrap_or("").to_string();
    }

    (type_, name, serial_number)
}

impl RegistryPlugin for DeviceClasses {
    fn plugin_name(&self) -> &'static str {
        "DeviceClasses"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"ControlSet00*\Control\DeviceClasses"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let mut rows = Vec::new();

        // Iterate GUID subkeys; only process those in the allowlist
        let guid_subkeys = hive.sub_keys(key);
        for mut guid_key in guid_subkeys {
            if !is_allowed_guid(&guid_key.key_name) {
                continue;
            }

            let guid_folder = guid_key.key_name.clone();

            // Each device subkey under the GUID folder
            let device_subkeys = hive.sub_keys(&mut guid_key);
            for device_subkey in device_subkeys {
                let batch_key_path = device_subkey.path.trim_start_matches('\\').to_string();
                let ts_dt = device_subkey.last_key_written_date_and_time();

                let (type_, name, serial_number) = parse_data(&device_subkey.key_name);

                let ts_iso = dt_to_iso8601(ts_dt);
                let ts_recmd = dt_to_recmd_literal(ts_dt);

                let vd1 = format!("Type: {type_} Name: {name} SerialNumber: {serial_number}");
                let vd2 = format!("Timestamp: {ts_recmd}");
                let vd3 = String::new();

                rows.push(PluginRow {
                    batch_value_name: "Multiple".to_string(),
                    batch_value_data1: vd1,
                    batch_value_data2: vd2,
                    batch_value_data3: vd3,
                    detail_columns: vec![
                        ("Timestamp".to_string(), ts_iso),
                        ("BatchKeyPath".to_string(), batch_key_path),
                        ("GuidFolder".to_string(), guid_folder.clone()),
                        ("BatchValueName".to_string(), "Multiple".to_string()),
                        ("Type".to_string(), type_),
                        ("Name".to_string(), name),
                        ("SerialNumber".to_string(), serial_number),
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
        assert_eq!(DeviceClasses.plugin_name(), "DeviceClasses");
        assert_eq!(
            DeviceClasses.key_paths(),
            &[r"ControlSet00*\Control\DeviceClasses"]
        );
    }

    #[test]
    fn guid_allowlist_case_insensitive() {
        // Exact lowercase match
        assert!(is_allowed_guid("{53f56307-b6bf-11d0-94f2-00a0c91efb8b}"));
        assert!(is_allowed_guid("{53f5630d-b6bf-11d0-94f2-00a0c91efb8b}"));
        // Mixed case (as stored in registry, key names may be upper/lower)
        assert!(is_allowed_guid("{53F56307-B6BF-11D0-94F2-00A0C91EFB8B}"));
        assert!(is_allowed_guid("{53F5630D-B6BF-11D0-94F2-00A0C91EFB8B}"));
        assert!(is_allowed_guid("{10497B1B-BA51-44E5-8318-A65C837B6661}"));
        // Not in list
        assert!(!is_allowed_guid("{00000000-0000-0000-0000-000000000000}"));
        assert!(!is_allowed_guid("{4d36e972-e325-11ce-bfc1-08002be10318}"));
    }

    #[test]
    fn parse_data_scsi_disk() {
        // ##?#SCSI#Disk&Ven_Msft&Prod_Virtual_Disk#5&3638f170&0&000000#{53f56307-b6bf-11d0-94f2-00a0c91efb8b}
        // After Replace("##?#","") → "SCSI#Disk&Ven_Msft&Prod_Virtual_Disk#5&3638f170&0&000000#{53f56307-...}"
        // No "##" or "??" in result, so: Type="SCSI", Name="Disk&Ven_Msft&Prod_Virtual_Disk", serial="5&3638f170&0&000000"
        let key_name = r"##?#SCSI#Disk&Ven_Msft&Prod_Virtual_Disk#5&3638f170&0&000000#{53f56307-b6bf-11d0-94f2-00a0c91efb8b}";
        let (typ, name, serial) = parse_data(key_name);
        assert_eq!(typ, "SCSI");
        assert_eq!(name, "Disk&Ven_Msft&Prod_Virtual_Disk");
        assert_eq!(serial, "5&3638f170&0&000000");
    }

    #[test]
    fn parse_data_storage_volume() {
        // ##?#STORAGE#Volume#{0f1aa3d2-35c4-11ef-a275-806e6f6e6963}#0000000000100000#{53f5630d-...}
        // After Replace → "STORAGE#Volume#{0f1aa3d2-...}#0000000000100000#{53f5630d-...}"
        // Split('#') → ["STORAGE", "Volume", "{0f1aa3d2-...}", "0000000000100000", "{53f5630d-...}"]
        // Type="STORAGE", Name="Volume", serial="{0f1aa3d2-35c4-11ef-a275-806e6f6e6963}"
        let key_name = r"##?#STORAGE#Volume#{0f1aa3d2-35c4-11ef-a275-806e6f6e6963}#0000000000100000#{53f5630d-b6bf-11d0-94f2-00a0c91efb8b}";
        let (typ, name, serial) = parse_data(key_name);
        assert_eq!(typ, "STORAGE");
        assert_eq!(name, "Volume");
        assert_eq!(serial, "{0f1aa3d2-35c4-11ef-a275-806e6f6e6963}");
    }

    #[test]
    fn detail_column_order() {
        let expected = [
            "Timestamp",
            "BatchKeyPath",
            "GuidFolder",
            "BatchValueName",
            "Type",
            "Name",
            "SerialNumber",
        ];
        assert_eq!(expected.len(), 7);
    }
}
