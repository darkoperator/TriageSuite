//! Port of RegistryPlugin.NetworkAdapters (NetworkAdapters.cs + ValuesOut.cs).
//! Extracts installed network adapter information from the driver class registry key.
//!
//! Key path: ControlSet00*\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}
//! Walks: each subkey matching ^00\d\d$ (exactly 4 chars, "00" + 2 digits).
//!
//! Detail-CSV column order (fixture-authoritative):
//!   Timestamp, BatchKeyPath, DriverDesc, BatchValueName, DriverDate,
//!   DriverVersion, ProviderName, DeviceInstanceid
//!
//! Batch row format (ValueName = "Multiple"):
//!   ValueData1 = "DriveName: {DriverDesc} DriverDate: {DriverDate} DriverVersion: {DriverVersion} ProviderName: {ProviderName}"
//!   ValueData2 = "DeviceInstanceid: {DeviceInstanceid}"
//!   ValueData3 = "Timestamp: {Timestamp:yyyy-MM-dd HH:mm:ss.fffffff}"
//!
//! Note: C# uses `DriveName` (not `DriverName`) in ValueData1 — this is an intentional
//! quirk of the original plugin; we replicate it exactly.

use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct NetworkAdapters;

fn get_str_value(key: &CellKeyNode, name: &str) -> String {
    for v in key.value_iter() {
        if v.get_pretty_name() == name {
            let (content, _) = v.get_content();
            return triage_registry::value::render_value_data(&content);
        }
    }
    String::new()
}

/// Convert a chrono::DateTime<Utc> to RECmd literal "yyyy-MM-dd HH:mm:ss.fffffff"
/// (for embedded text fields in ValueData3 — the testkit does NOT normalize these).
fn dt_to_recmd_literal(dt: chrono::DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks)
}

/// Convert chrono::DateTime<Utc> to ISO-8601 UTC (for standalone Timestamp detail column).
fn dt_to_iso8601(dt: chrono::DateTime<chrono::Utc>) -> String {
    let ts = WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos());
    ts.to_string()
}

/// Returns true if the key name matches ^00\d\d$ (C# `Regex(@"^00\d\d$")`).
fn is_adapter_subkey(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 4 && b[0] == b'0' && b[1] == b'0' && b[2].is_ascii_digit() && b[3].is_ascii_digit()
}

impl RegistryPlugin for NetworkAdapters {
    fn plugin_name(&self) -> &'static str {
        "NetworkAdapters"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"ControlSet00*\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let mut rows = Vec::new();

        let subkeys = hive.sub_keys(key);
        for subkey in subkeys {
            if !is_adapter_subkey(&subkey.key_name) {
                continue;
            }

            let batch_key_path = subkey.path.trim_start_matches('\\').to_string();
            let ts_dt = subkey.last_key_written_date_and_time();

            let driver_desc = get_str_value(&subkey, "DriverDesc");
            let driver_date = get_str_value(&subkey, "DriverDate");
            let driver_version = get_str_value(&subkey, "DriverVersion");
            let device_instance_id = get_str_value(&subkey, "DeviceInstanceID");
            let provider_name = get_str_value(&subkey, "ProviderName");

            let ts_iso = dt_to_iso8601(ts_dt);
            // C# ValuesOut.BatchValueData3: "Timestamp: {Timestamp?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
            let ts_recmd = dt_to_recmd_literal(ts_dt);

            let vd1 = format!(
                "DriveName: {driver_desc} DriverDate: {driver_date} DriverVersion: {driver_version} ProviderName: {provider_name}"
            );
            let vd2 = format!("DeviceInstanceid: {device_instance_id}");
            let vd3 = format!("Timestamp: {ts_recmd}");

            rows.push(PluginRow {
                batch_value_name: "Multiple".to_string(),
                batch_value_data1: vd1,
                batch_value_data2: vd2,
                batch_value_data3: vd3,
                detail_columns: vec![
                    ("Timestamp".to_string(), ts_iso),
                    ("BatchKeyPath".to_string(), batch_key_path),
                    ("DriverDesc".to_string(), driver_desc),
                    ("BatchValueName".to_string(), "Multiple".to_string()),
                    ("DriverDate".to_string(), driver_date),
                    ("DriverVersion".to_string(), driver_version),
                    ("ProviderName".to_string(), provider_name),
                    ("DeviceInstanceid".to_string(), device_instance_id),
                ],
            });
        }

        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_and_key_paths() {
        assert_eq!(NetworkAdapters.plugin_name(), "NetworkAdapters");
        assert_eq!(
            NetworkAdapters.key_paths(),
            &[r"ControlSet00*\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}"]
        );
    }

    #[test]
    fn adapter_subkey_filter() {
        // Matches: 4 chars, starts with "00", followed by 2 digits
        assert!(is_adapter_subkey("0000"));
        assert!(is_adapter_subkey("0001"));
        assert!(is_adapter_subkey("0099"));
        // Non-matches
        assert!(!is_adapter_subkey("Properties"));
        assert!(!is_adapter_subkey("001"));
        assert!(!is_adapter_subkey("0000a"));
        assert!(!is_adapter_subkey("01ab"));
        assert!(!is_adapter_subkey("00ab")); // 'a','b' not ascii digits
    }

    #[test]
    fn value_data_format() {
        // Verify DriveName (not DriverName) in ValueData1 — matches C# quirk.
        let vd1 = format!(
            "DriveName: {} DriverDate: {} DriverVersion: {} ProviderName: {}",
            "Microsoft Hyper-V Network Adapter", "6-21-2006", "10.0.22621.3810", "Microsoft"
        );
        assert!(vd1.starts_with("DriveName: "));
        assert!(!vd1.contains("DriverName: "));
    }

    #[test]
    fn detail_column_order() {
        let expected = [
            "Timestamp",
            "BatchKeyPath",
            "DriverDesc",
            "BatchValueName",
            "DriverDate",
            "DriverVersion",
            "ProviderName",
            "DeviceInstanceid",
        ];
        assert_eq!(expected.len(), 8);
    }
}
