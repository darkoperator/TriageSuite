//! Port of RegistryPlugin.RADAR (RADAR.cs + ValuesOut.cs). Extracts heap-leak
//! detection application data from RADAR registry keys.
//!
//! Key paths:
//!   Microsoft\RADAR\HeapLeakDetection
//!   WOW6432Node\Microsoft\RADAR\HeapLeakDetection
//!
//! Detail-CSV column order (fixture-authoritative):
//!   KeyName, BatchKeyPath, Filename, BatchValueName, LastDetectionTime

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct Radar;

/// Parse a decimal file-time string (as stored in RADAR's LastDetectionTime value)
/// to ISO-8601 UTC.
fn filetime_str_to_iso8601(value_data: &str) -> Option<String> {
    let ft = value_data.parse::<i64>().ok()?;
    if ft <= 0 {
        return None;
    }
    let ft = ft as u64;
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    let dt: DateTime<chrono::Utc> = DateTime::from_timestamp(secs, nanos)?;
    Some(WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string())
}

/// Parse a decimal file-time string to RECmd literal `yyyy-MM-dd HH:mm:ss.fffffff`.
/// Used in BatchValueData2 (embedded, not normalized by testkit).
fn filetime_str_to_recmd_literal(value_data: &str) -> Option<String> {
    let ft = value_data.parse::<i64>().ok()?;
    if ft <= 0 {
        return None;
    }
    let ft = ft as u64;
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    let dt: DateTime<chrono::Utc> = DateTime::from_timestamp(secs, nanos)?;
    let ticks = dt.timestamp_subsec_nanos() / 100;
    Some(format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks))
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

fn make_row(
    key_name: &str,
    batch_key_path: &str,
    filename: &str,
    last_detection_time_iso: &str,
    last_detection_time_recmd: &str,
) -> PluginRow {
    PluginRow {
        batch_value_name: "Multiple".to_string(),
        // BatchValueData1: "Filename: {Filename}"
        batch_value_data1: format!("Filename: {filename}"),
        // BatchValueData2: "LastDetectionTime: {LastDetectionTime?.ToUniversalTime():...}"
        batch_value_data2: format!("LastDetectionTime: {last_detection_time_recmd}"),
        // BatchValueData3: "KeyName: {KeyName}"
        batch_value_data3: format!("KeyName: {key_name}"),
        detail_columns: vec![
            ("KeyName".to_string(), key_name.to_string()),
            ("BatchKeyPath".to_string(), batch_key_path.to_string()),
            ("Filename".to_string(), filename.to_string()),
            ("BatchValueName".to_string(), "Multiple".to_string()),
            (
                "LastDetectionTime".to_string(),
                last_detection_time_iso.to_string(),
            ),
        ],
    }
}

impl RegistryPlugin for Radar {
    fn plugin_name(&self) -> &'static str {
        "RADAR"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[
            r"Microsoft\RADAR\HeapLeakDetection",
            r"WOW6432Node\Microsoft\RADAR\HeapLeakDetection",
        ]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let mut rows = Vec::new();

        // Collect the top-level subkeys (DiagnosedApplications, ReflectionApplications)
        let top_subkeys = hive.sub_keys(key);

        for mut top in top_subkeys {
            let top_name = top.key_name.clone();

            let is_diagnosed = top_name.eq_ignore_ascii_case("DiagnosedApplications");
            let is_reflection = top_name.eq_ignore_ascii_case("ReflectionApplications");

            if !is_diagnosed && !is_reflection {
                continue;
            }

            // Iterate sub-subkeys (one per application)
            let app_subkeys = hive.sub_keys(&mut top);
            for app in app_subkeys {
                let filename = app.key_name.clone();
                let batch_key_path = app.path.trim_start_matches('\\').to_string();

                let (last_detection_time_iso, last_detection_time_recmd) = if is_diagnosed {
                    let vd = get_str_value(&app, "LastDetectionTime");
                    let iso = filetime_str_to_iso8601(&vd).unwrap_or_default();
                    let recmd = filetime_str_to_recmd_literal(&vd).unwrap_or_default();
                    (iso, recmd)
                } else {
                    // ReflectionApplications: LastDetectionTime = null → empty
                    (String::new(), String::new())
                };

                rows.push(make_row(
                    &top_name,
                    &batch_key_path,
                    &filename,
                    &last_detection_time_iso,
                    &last_detection_time_recmd,
                ));
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
        assert_eq!(Radar.plugin_name(), "RADAR");
        let paths = Radar.key_paths();
        assert!(paths.contains(&r"Microsoft\RADAR\HeapLeakDetection"));
        assert!(paths.contains(&r"WOW6432Node\Microsoft\RADAR\HeapLeakDetection"));
    }

    #[test]
    fn detail_column_order() {
        let expected = [
            "KeyName",
            "BatchKeyPath",
            "Filename",
            "BatchValueName",
            "LastDetectionTime",
        ];
        assert_eq!(expected.len(), 5);
    }

    #[test]
    fn filetime_str_parses_known_value() {
        // Round-trip test: a known positive FILETIME parses to UTC and ends with Z.
        // FILETIME 134_178_295_687_227_042 = 2026-03-12 22:52:48.7227042 UTC
        // (also used in bam.rs unit tests — same fixture value).
        let ft: i64 = 134_178_295_687_227_042;
        let out = filetime_str_to_iso8601(&ft.to_string()).unwrap();
        assert!(out.starts_with("2026-03-12T22:52:48"), "got {out}");
        assert!(out.ends_with('Z'), "got {out}");
    }
}
