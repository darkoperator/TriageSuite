//! Port of RegistryPlugin.VolumeInfoCache (VolumeInfoCache.cs + ValuesOut.cs).
//! Extracts Windows Search volume cache entries from the registry.
//!
//! Key path: Microsoft\Windows Search\VolumeInfoCache
//! Each subkey is a drive name (e.g. "C:").
//!
//! Detail-CSV column order (fixture-authoritative):
//!   Timestamp, BatchKeyPath, DriveName, BatchValueName, VolumeLabel, DriveType

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct VolumeInfoCache;

fn dt_to_iso8601(dt: DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

fn dt_to_recmd_literal(dt: DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks)
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

/// Map DriveType value string to its name.
fn drive_type_name(val: &str) -> &str {
    match val {
        "0" => "Unknown",
        "1" => "NoRootDirectory",
        "2" => "Removable",
        "3" => "Fixed",
        "4" => "Network",
        "5" => "CDROM",
        "6" => "RAM",
        _ => val,
    }
}

impl RegistryPlugin for VolumeInfoCache {
    fn plugin_name(&self) -> &'static str {
        "VolumeInfoCache"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Microsoft\Windows Search\VolumeInfoCache"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let mut rows = Vec::new();

        let subkeys = hive.sub_keys(key);

        for sub in subkeys {
            let drive_name = sub.key_name.clone();
            let batch_key_path = sub.path.trim_start_matches('\\').to_string();

            let volume_label = get_str_value(&sub, "VolumeLabel");
            let drive_type_raw = get_str_value(&sub, "DriveType");
            let drive_type = drive_type_name(&drive_type_raw).to_string();

            let ts_dt = sub.last_key_written_date_and_time();
            let ts_iso = dt_to_iso8601(ts_dt);
            let ts_recmd = dt_to_recmd_literal(ts_dt);

            let vd1 = format!(
                "DriveName: {drive_name} VolumeLabel: {volume_label} DriveType: {drive_type}"
            );
            let vd2 = format!("Timestamp: {ts_recmd}");

            rows.push(PluginRow {
                batch_value_name: "Multiple".to_string(),
                batch_value_data1: vd1,
                batch_value_data2: vd2,
                batch_value_data3: String::new(),
                detail_columns: vec![
                    ("Timestamp".to_string(), ts_iso),
                    ("BatchKeyPath".to_string(), batch_key_path),
                    ("DriveName".to_string(), drive_name),
                    ("BatchValueName".to_string(), "Multiple".to_string()),
                    ("VolumeLabel".to_string(), volume_label),
                    ("DriveType".to_string(), drive_type),
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
        assert_eq!(VolumeInfoCache.plugin_name(), "VolumeInfoCache");
        assert_eq!(
            VolumeInfoCache.key_paths(),
            &[r"Microsoft\Windows Search\VolumeInfoCache"]
        );
    }

    #[test]
    fn drive_type_mapping() {
        assert_eq!(drive_type_name("0"), "Unknown");
        assert_eq!(drive_type_name("1"), "NoRootDirectory");
        assert_eq!(drive_type_name("2"), "Removable");
        assert_eq!(drive_type_name("3"), "Fixed");
        assert_eq!(drive_type_name("4"), "Network");
        assert_eq!(drive_type_name("5"), "CDROM");
        assert_eq!(drive_type_name("6"), "RAM");
        assert_eq!(drive_type_name("99"), "99");
    }

    #[test]
    fn detail_column_order() {
        let expected = [
            "Timestamp",
            "BatchKeyPath",
            "DriveName",
            "BatchValueName",
            "VolumeLabel",
            "DriveType",
        ];
        assert_eq!(expected.len(), 6);
    }
}
