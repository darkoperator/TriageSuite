//! Port of RegistryPlugin.ETW (ETW.cs + ValuesOut.cs).
//! Cross-references ETW GUIDs with provider names from the Windows Event Log.
//!
//! Key paths:
//!   ControlSet001\Control\WMI\Autologger\EventLog-Application
//!   ControlSet001\Control\WMI\Autologger\EventLog-System
//!
//! Each subkey is an ETW provider GUID (e.g. "{01090065-b467-4503-9b28-533766761087}").
//! For each subkey, reads Enabled and EnableProperty values (skips if either is absent).
//!
//! Detail-CSV column order (fixture-authoritative):
//!   LastWriteTimestamp, BatchKeyPath, Guid, BatchValueName, Provider, Enabled, EnabledProperty
//!
//! Batch row format (ValueName = "Provider"):
//!   ValueData  = "Provider: {Provider}"
//!   ValueData2 = "Enabled: {Enabled}"
//!   ValueData3 = "EnabledProperty: {EnabledProperty}"
//!
//! IMPORTANT: C# `subkey.LastWriteTime.ToString()` on .NET 6+ en-US culture produces
//! "M/d/yyyy h:mm:ss NARROW-NO-BREAK-SPACE AM/PM +00:00" (U+202F before AM/PM).
//! We reproduce this exactly so the output matches the RECmd reference fixture.

use notatin::cell_key_node::CellKeyNode;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct Etw;

/// Format a chrono::DateTime<Utc> as C# DateTimeOffset.ToString() en-US .NET 6+:
/// "M/d/yyyy h:mm:ss\u{202F}AM/PM +00:00"
/// (NARROW NO-BREAK SPACE U+202F between time and AM/PM, no zero-padding on month/day/hour)
fn dt_to_csharp_datetimeoffset(dt: chrono::DateTime<chrono::Utc>) -> String {
    use chrono::Datelike;
    use chrono::Timelike;
    let m = dt.month();
    let d = dt.day();
    let y = dt.year();
    let h24 = dt.hour();
    let min = dt.minute();
    let sec = dt.second();
    let am_pm = if h24 < 12 { "AM" } else { "PM" };
    let h12 = match h24 % 12 {
        0 => 12,
        h => h,
    };
    // U+202F = NARROW NO-BREAK SPACE (used by .NET 6+ en-US culture for AM/PM separator)
    format!("{m}/{d}/{y} {h12}:{min:02}:{sec:02}\u{202F}{am_pm} +00:00")
}

/// Look up a GUID (with braces, e.g. "{01090065-b467-4503-9b28-533766761087}")
/// in the ETW provider map. Returns the provider name or "Unmapped GUID: {guid}".
fn get_description_from_guid(guid_with_braces: &str) -> String {
    match triage_guidmap::description_for(guid_with_braces) {
        Some(desc) => desc.to_string(),
        None => format!("Unmapped GUID: {guid_with_braces}"),
    }
}

/// Read a string value from a CellKeyNode by value name (case-insensitive).
/// Returns None if the value is absent.
fn get_str_value_opt(key: &CellKeyNode, name: &str) -> Option<String> {
    for v in key.value_iter() {
        if v.get_pretty_name().eq_ignore_ascii_case(name) {
            let (content, _) = v.get_content();
            return Some(triage_registry::value::render_value_data(&content));
        }
    }
    None
}

impl RegistryPlugin for Etw {
    fn plugin_name(&self) -> &'static str {
        "ETW"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[
            r"ControlSet001\Control\WMI\Autologger\EventLog-Application",
            r"ControlSet001\Control\WMI\Autologger\EventLog-System",
        ]
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
            // C# catches any exception here and adds to Errors, then continues.
            // If Enabled or EnableProperty are absent, C# NullReferenceException → skip.
            let enabled = match get_str_value_opt(&subkey, "Enabled") {
                Some(v) => v,
                None => continue,
            };
            let enable_property = match get_str_value_opt(&subkey, "EnableProperty") {
                Some(v) => v,
                None => continue,
            };

            let guid = subkey.key_name.clone();
            let batch_key_path = subkey.path.trim_start_matches('\\').to_string();

            let lw_dt = subkey.last_key_written_date_and_time();
            let last_write_ts = dt_to_csharp_datetimeoffset(lw_dt);

            let provider = get_description_from_guid(&guid);

            let vd1 = format!("Provider: {provider}");
            let vd2 = format!("Enabled: {enabled}");
            let vd3 = format!("EnabledProperty: {enable_property}");

            rows.push(PluginRow {
                batch_value_name: "Provider".to_string(),
                batch_value_data1: vd1,
                batch_value_data2: vd2,
                batch_value_data3: vd3,
                // Column order: LastWriteTimestamp,BatchKeyPath,Guid,BatchValueName,
                //   Provider,Enabled,EnabledProperty
                detail_columns: vec![
                    ("LastWriteTimestamp".to_string(), last_write_ts),
                    ("BatchKeyPath".to_string(), batch_key_path),
                    ("Guid".to_string(), guid),
                    ("BatchValueName".to_string(), "Provider".to_string()),
                    ("Provider".to_string(), provider),
                    ("Enabled".to_string(), enabled),
                    ("EnabledProperty".to_string(), enable_property),
                ],
            });
        }

        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn plugin_name_and_key_paths() {
        assert_eq!(Etw.plugin_name(), "ETW");
        assert_eq!(Etw.key_paths().len(), 2);
        assert!(Etw.key_paths()[0].contains("EventLog-Application"));
        assert!(Etw.key_paths()[1].contains("EventLog-System"));
    }

    #[test]
    fn datetimeoffset_format_am() {
        // 2024-06-29 02:02:45 UTC → "6/29/2024 2:02:45 AM +00:00" (with NNBSP)
        let dt = chrono::Utc.with_ymd_and_hms(2024, 6, 29, 2, 2, 45).unwrap();
        let s = dt_to_csharp_datetimeoffset(dt);
        // U+202F is the narrow no-break space
        assert_eq!(s, "6/29/2024 2:02:45\u{202F}AM +00:00");
    }

    #[test]
    fn datetimeoffset_format_pm() {
        // 2024-06-29 14:30:00 UTC → "6/29/2024 2:30:00 PM +00:00"
        let dt = chrono::Utc
            .with_ymd_and_hms(2024, 6, 29, 14, 30, 0)
            .unwrap();
        let s = dt_to_csharp_datetimeoffset(dt);
        assert_eq!(s, "6/29/2024 2:30:00\u{202F}PM +00:00");
    }

    #[test]
    fn datetimeoffset_format_midnight() {
        // 2024-01-01 00:00:00 UTC → "1/1/2024 12:00:00 AM +00:00"
        let dt = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let s = dt_to_csharp_datetimeoffset(dt);
        assert_eq!(s, "1/1/2024 12:00:00\u{202F}AM +00:00");
    }

    #[test]
    fn datetimeoffset_format_noon() {
        // Noon: 12:00:00 PM
        let dt = chrono::Utc
            .with_ymd_and_hms(2024, 12, 31, 12, 0, 0)
            .unwrap();
        let s = dt_to_csharp_datetimeoffset(dt);
        assert_eq!(s, "12/31/2024 12:00:00\u{202F}PM +00:00");
    }

    #[test]
    fn guid_lookup_known_guid() {
        // {01090065-b467-4503-9b28-533766761087} → Microsoft-Windows-ParentalControls
        let result = get_description_from_guid("{01090065-b467-4503-9b28-533766761087}");
        assert_eq!(result, "Microsoft-Windows-ParentalControls");
    }

    #[test]
    fn guid_lookup_unknown_guid() {
        let result = get_description_from_guid("{ffffffff-ffff-ffff-ffff-ffffffffffff}");
        assert_eq!(
            result,
            "Unmapped GUID: {ffffffff-ffff-ffff-ffff-ffffffffffff}"
        );
    }

    #[test]
    fn detail_column_order() {
        let expected = [
            "LastWriteTimestamp",
            "BatchKeyPath",
            "Guid",
            "BatchValueName",
            "Provider",
            "Enabled",
            "EnabledProperty",
        ];
        assert_eq!(expected.len(), 7);
    }
}
