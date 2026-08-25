//! Port of RegistryPlugin.JumplistData (JumplistData.cs + ValuesOut.cs).
//! Extracts program execution timestamps from the JumplistData key.
//! Each value is a REG_QWORD Windows FILETIME (100-ns intervals since 1601).
//!
//! Key path:
//!   `Software\Microsoft\Windows\CurrentVersion\Search\JumplistData`
//!
//! Batch row format (from NTUSER batch.csv "JumplistData" rows):
//!   ValueData  = "Name: <value_name>"
//!   ValueData2 = "Executed: <yyyy-MM-dd HH:mm:ss.fffffff>"  (UTC)
//!   ValueData3 = ""
//!
//! Detail-CSV columns: JumpListName, BatchKeyPath, ExecutedOn, BatchValueName

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct JumplistData;

/// FILETIME → RECmd's `yyyy-MM-dd HH:mm:ss.fffffff` literal (UTC).
/// Matches C# `DateTimeOffset.FromFileTime(...).ToUniversalTime()` formatted
/// with `ToString("yyyy-MM-dd HH:mm:ss.fffffff")`.
fn filetime_to_recmd_literal(ft: u64) -> Option<String> {
    // FILETIME epoch is 1601-01-01; Unix epoch is 1970-01-01.
    const FILETIME_TO_UNIX_OFFSET: i64 = 11_644_473_600;
    let secs = (ft / 10_000_000) as i64 - FILETIME_TO_UNIX_OFFSET;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    let dt: DateTime<chrono::Utc> = DateTime::from_timestamp(secs, nanos)?;
    let ticks = dt.timestamp_subsec_nanos() / 100;
    Some(format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks))
}

impl RegistryPlugin for JumplistData {
    fn plugin_name(&self) -> &'static str {
        "JumplistData"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Software\Microsoft\Windows\CurrentVersion\Search\JumplistData"]
    }

    fn value_name(&self) -> Option<&'static str> {
        None
    }

    fn process(&self, key: &CellKeyNode, values: &[PluginValue]) -> Vec<PluginRow> {
        let key_path = key.path.trim_start_matches('\\');
        let mut rows = Vec::new();
        for v in values {
            // Need at least 8 bytes for a FILETIME.
            //
            // C# divergence (deliberate, no fixture coverage):
            //   When fewer than 8 bytes are present, C# reads the available bytes
            //   (zero-padded to 8) and emits a row with DateTimeOffset.MinValue
            //   (0001-01-01 00:00:00.0000000). No fixture row exercises this path —
            //   all JumplistData values in the evidence captures are exactly 8 bytes.
            //   We skip short values instead. This divergence is acceptable and
            //   documented here to prevent accidental "fix" in the future.
            let Some(bytes) = v.raw.get(0..8) else {
                continue;
            };
            let ft = u64::from_le_bytes(bytes.try_into().unwrap());
            let ts = filetime_to_recmd_literal(ft).unwrap_or_default();
            rows.push(PluginRow {
                batch_value_name: v.name.clone(),
                batch_value_data1: format!("Name: {}", v.name),
                batch_value_data2: format!("Executed: {ts}"),
                batch_value_data3: String::new(),
                detail_columns: vec![
                    ("JumpListName".to_string(), v.name.clone()),
                    ("BatchKeyPath".to_string(), key_path.to_string()),
                    ("ExecutedOn".to_string(), ts),
                    ("BatchValueName".to_string(), v.name.clone()),
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
        let p = JumplistData;
        assert_eq!(p.plugin_name(), "JumplistData");
        assert!(p
            .key_paths()
            .contains(&r"Software\Microsoft\Windows\CurrentVersion\Search\JumplistData"));
    }

    #[test]
    fn filetime_known_value() {
        // 2024-06-28 23:16:05.8470694 UTC
        // FileTime = (seconds since 1601 epoch) * 10_000_000 + ticks
        // seconds since 1601 = 2024-06-28 23:16:05 UTC as Unix + 11644473600
        // Unix: 1719616565 => filetime seconds = 1719616565 + 11644473600 = 13364090165
        // ticks: 0.8470694s * 10_000_000 = 8_470_694
        let ft: u64 = 13364090165 * 10_000_000 + 8_470_694;
        let out = filetime_to_recmd_literal(ft).unwrap();
        assert_eq!(out, "2024-06-28 23:16:05.8470694");
    }
}
