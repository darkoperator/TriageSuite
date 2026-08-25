//! Port of RegistryPlugin.OfficeMRU (OfficeMRU.cs + ValuesOut.cs).
//! Extracts recent Microsoft Office document names and last opened/closed times.
//!
//! Fires on NTUSER.DAT at (all 4 patterns are wildcard paths):
//!   `Software\Microsoft\Office\*\*\User MRU\*\File MRU`
//!   `Software\Microsoft\Office\*\*\User MRU\*\Place MRU`
//!   `Software\Microsoft\Office\*\*\File MRU`
//!   `Software\Microsoft\Office\*\*\Place MRU`
//!
//! NOTE on duplicate-row divergence (AcceptedDelta):
//! RECmd's `GetPluginsToActivate` (`Program.cs` ~line 2103) iterates every
//! plugin's KeyPaths, converts `*` → `.+?` (crosses `\`), tail-anchors with
//! `\z`, and appends the plugin to the activation list ONCE PER MATCHING
//! KeyPath with NO deduplication.  OfficeMRU's overlapping key_paths
//! (`…\User MRU\*\File MRU` and `…\*\*\File MRU`) BOTH tail-match a nested
//! key such as `…\User MRU\AD_…\File MRU`, so RECmd activates the plugin
//! twice for that key and emits every row twice (identical duplicates).
//! RETriage's `plugins_to_activate` (crates/re-triage/src/plugins/mod.rs)
//! deduplicates — each plugin is activated at most once per key — so each
//! row is emitted exactly once.  RETriage's behaviour is strictly more
//! correct; the divergence is a deliberate, bounded improvement documented
//! as the OfficeMRU AcceptedDelta.
//!
//! Detail-CSV column order (fixture-authoritative, from
//! `STCL1__cperez_NTUSER__plugin_OfficeMRU_NTUSER.DAT.csv`):
//!   ValueName, BatchKeyPath, LastOpened, BatchValueName, LastClosed, FileName
//!
//! Batch row format (C# ValuesOut):
//!   ValueData1 = "File name: {FileName}"
//!   ValueData2 = "Last opened: {LastOpened?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
//!   ValueData3 = "Last closed: {LastClosed?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
//!
//! MRU value format: `[F00000000][T<hex_filetime>][O00000000]*<path>`
//! The T-token is a hex-encoded Windows FILETIME (64-bit little-endian, big-endian hex).
//!
//! "FOLDER*" values: treated specially — BatchValueData1=ValueName, no parsed time.
//!
//! "Reading Locations" lookup:
//!   For `User MRU\*\File MRU` keys: grandparent-of-grandparent is the app folder
//!   (e.g. Word). We look for "Reading Locations" as a sibling of the app folder.
//!   For `File MRU` keys at the app-level: parent.parent.parent is Office, no
//!   "Reading Locations" there → lastOpen remains null.
//!
//! LastOpened/LastClosed are standalone timestamp columns. The testkit normalizes
//! the reference from RECmd's "yyyy-MM-dd HH:mm:ss.fffffff" to ISO-8601 UTC.
//! We emit ISO-8601 UTC directly — no AcceptedDelta needed.
//!
//! The embedded "Last opened: ..." in ValueData2 is a free-text field and uses
//! RECmd's literal format (testkit does NOT normalize embedded fields).

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct OfficeMru;

/// Convert a Windows FILETIME (100-ns intervals since 1601-01-01 UTC) to a
/// chrono::DateTime<Utc>.
fn filetime_to_datetime(ft: i64) -> Option<DateTime<chrono::Utc>> {
    const FILETIME_TO_UNIX_SECS: i64 = 11_644_473_600;
    let secs = (ft / 10_000_000) - FILETIME_TO_UNIX_SECS;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    DateTime::from_timestamp(secs, nanos)
}

/// Format DateTime<Utc> as ISO-8601 UTC with 7 fractional digits.
/// Used for standalone timestamp columns (testkit normalizes from RECmd format).
fn dt_to_iso8601(dt: DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

/// Format DateTime<Utc> as RECmd literal "yyyy-MM-dd HH:mm:ss.fffffff".
/// Used for embedded free-text ValueData fields.
fn dt_to_recmd_literal(dt: DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks)
}

/// Parse the OfficeMRU value format: `[F00000000][T<hex_filetime>][O00000000]*<path>`
/// Returns (filename, filetime_as_i64) or None if the format doesn't match.
fn parse_mru_value(data: &str) -> Option<(String, i64)> {
    // Split on '*' — everything after the last '*' is the filename.
    let segs: Vec<&str> = data.splitn(2, '*').collect();
    if segs.len() < 2 {
        return None;
    }
    let fname = segs[1].to_string();
    let header = segs[0]; // "[F00000000][T01D005C5B44B6300][O00000000]"

    // Split on '[' to get tokens: ["", "F00000000]", "T01D...300]", "O00000000]"]
    let tokens: Vec<&str> = header.split('[').collect();
    // Token at index 2 is the T-token: "T01D005C5B44B6300]"
    if tokens.len() < 3 {
        return None;
    }
    let t_token = tokens[2]; // "T01D005C5B44B6300]"
    if !t_token.starts_with('T') {
        return None;
    }
    let raw_time = &t_token[1..t_token.len().saturating_sub(1)]; // strip leading T and trailing ]
    let ft = i64::from_str_radix(raw_time, 16).ok()?;
    Some((fname, ft))
}

/// Build a PluginRow for an OfficeMRU value.
pub fn build_row(
    value_name: &str,
    value_data: &str,
    key_path: &str,
    reading_loc_last_write: Option<DateTime<chrono::Utc>>,
) -> PluginRow {
    // FOLDER* values: special handling
    if value_name.starts_with("FOLDER") {
        return PluginRow {
            batch_value_name: value_name.to_string(),
            batch_value_data1: format!("File name: {value_data}"),
            batch_value_data2: "Last opened: ".to_string(),
            batch_value_data3: "Last closed: ".to_string(),
            detail_columns: vec![
                ("ValueName".to_string(), value_name.to_string()),
                ("BatchKeyPath".to_string(), key_path.to_string()),
                ("LastOpened".to_string(), String::new()),
                ("BatchValueName".to_string(), value_name.to_string()),
                ("LastClosed".to_string(), String::new()),
                ("FileName".to_string(), value_data.to_string()),
            ],
        };
    }

    // Parse the MRU value format.
    let (fname, ft) = match parse_mru_value(value_data) {
        Some(r) => r,
        None => {
            // Malformed: emit as-is with no timestamps.
            return PluginRow {
                batch_value_name: value_name.to_string(),
                batch_value_data1: format!("File name: {value_data}"),
                batch_value_data2: "Last opened: ".to_string(),
                batch_value_data3: "Last closed: ".to_string(),
                detail_columns: vec![
                    ("ValueName".to_string(), value_name.to_string()),
                    ("BatchKeyPath".to_string(), key_path.to_string()),
                    ("LastOpened".to_string(), String::new()),
                    ("BatchValueName".to_string(), value_name.to_string()),
                    ("LastClosed".to_string(), String::new()),
                    ("FileName".to_string(), value_data.to_string()),
                ],
            };
        }
    };

    let first_open = filetime_to_datetime(ft);

    // last_closed = lastWriteTime of the Reading Locations subkey for this file.
    let last_closed = reading_loc_last_write;

    let last_opened_iso = first_open.map(dt_to_iso8601).unwrap_or_default();
    let last_closed_iso = last_closed.map(dt_to_iso8601).unwrap_or_default();

    let last_opened_recmd = first_open.map(dt_to_recmd_literal).unwrap_or_default();
    let last_closed_recmd = last_closed.map(dt_to_recmd_literal).unwrap_or_default();

    PluginRow {
        batch_value_name: value_name.to_string(),
        batch_value_data1: format!("File name: {fname}"),
        batch_value_data2: format!("Last opened: {last_opened_recmd}"),
        batch_value_data3: format!("Last closed: {last_closed_recmd}"),
        detail_columns: vec![
            ("ValueName".to_string(), value_name.to_string()),
            ("BatchKeyPath".to_string(), key_path.to_string()),
            ("LastOpened".to_string(), last_opened_iso),
            ("BatchValueName".to_string(), value_name.to_string()),
            ("LastClosed".to_string(), last_closed_iso),
            ("FileName".to_string(), fname),
        ],
    }
}

/// Compute the path of the "Reading Locations" sibling key given the current
/// key path and depth from the C# navigation.
///
/// For "Software\Microsoft\Office\16.0\Word\User MRU\AD_...\File MRU":
///   key.Parent.Parent.Parent = "Software\Microsoft\Office\16.0\Word"
///   → "Reading Locations" = "Software\Microsoft\Office\16.0\Word\Reading Locations"
///
/// For "Software\Microsoft\Office\16.0\Word\File MRU":
///   key.Parent.Parent.Parent = "Software\Microsoft\Office\16.0"
///   → would be under "16.0" — unlikely to exist, returns None from hive.
fn reading_locations_path(key_path: &str) -> Option<String> {
    // Strip "ROOT\" prefix if present.
    let rel = key_path.trim_start_matches("ROOT\\");
    // Split path segments.
    let segs: Vec<&str> = rel.split('\\').collect();
    // Go 3 levels up: drop last 3 segments.
    if segs.len() < 4 {
        return None;
    }
    let parent3_segs = &segs[..segs.len() - 3];
    Some(format!("{}\\Reading Locations", parent3_segs.join("\\")))
}

impl RegistryPlugin for OfficeMru {
    fn plugin_name(&self) -> &'static str {
        "Office MRU"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[
            r"Software\Microsoft\Office\*\*\User MRU\*\File MRU",
            r"Software\Microsoft\Office\*\*\User MRU\*\Place MRU",
            r"Software\Microsoft\Office\*\*\File MRU",
            r"Software\Microsoft\Office\*\*\Place MRU",
        ]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let key_path = key.path.trim_start_matches('\\').to_string();

        // Build a map from filename → last_write for Reading Locations subkeys.
        // Only applies for File MRU keys (not Place MRU), but we look it up
        // regardless — if the key doesn't exist, the map is empty.
        let mut reading_loc_map: std::collections::HashMap<String, DateTime<chrono::Utc>> =
            std::collections::HashMap::new();

        if let Some(rl_path) = reading_locations_path(&key_path) {
            if let Some(mut rl_key) = hive.get_key(&rl_path) {
                for sub in hive.sub_keys(&mut rl_key) {
                    // Read the "File Path" value from each Reading Location subkey.
                    for v in sub.value_iter() {
                        if v.get_pretty_name().eq_ignore_ascii_case("File Path") {
                            let (content, _) = v.get_content();
                            let file_path = triage_registry::value::render_value_data(&content);
                            let lw = sub.last_key_written_date_and_time();
                            reading_loc_map.insert(file_path, lw);
                            break;
                        }
                    }
                }
            }
        }

        let mut rows = Vec::new();
        for v in values {
            // The filename is the last segment after '*' in the MRU value.
            let fname = if v.value_data.starts_with("FOLDER") {
                v.value_data.clone()
            } else {
                // Extract filename from MRU format to look up reading location.
                v.value_data
                    .split_once('*')
                    .map(|(_, after)| after)
                    .unwrap_or("")
                    .to_string()
            };

            let reading_loc_lw = reading_loc_map.get(&fname).copied();

            let row = build_row(&v.name, &v.value_data, &key_path, reading_loc_lw);
            rows.push(row);
        }

        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_standard_mru_value() {
        let data = "[F00000000][T01D005C5B44B6300][O00000000]*C:\\Users\\eric\\Desktop\\file.csv";
        let (fname, ft) = parse_mru_value(data).unwrap();
        assert_eq!(fname, "C:\\Users\\eric\\Desktop\\file.csv");
        // ft should be a positive filetime
        assert!(ft > 0);
        // Convert to datetime and check it's in a reasonable range (2000-2030)
        let dt = filetime_to_datetime(ft).unwrap();
        assert!(dt.format("%Y").to_string().parse::<i32>().unwrap() > 2000);
    }

    #[test]
    fn folder_value_no_parse() {
        let row = build_row(
            "FOLDERID_Desktop",
            "C:\\Users\\test\\Desktop\\",
            r"ROOT\SOFTWARE\Microsoft\Office\16.0\Excel\File MRU",
            None,
        );
        assert_eq!(row.batch_value_name, "FOLDERID_Desktop");
        assert_eq!(
            row.batch_value_data1,
            "File name: C:\\Users\\test\\Desktop\\"
        );
        assert_eq!(row.batch_value_data2, "Last opened: ");
        assert_eq!(row.detail_columns[2].1, ""); // LastOpened empty
        assert_eq!(row.detail_columns[4].1, ""); // LastClosed empty
    }

    #[test]
    fn mru_value_with_timestamp() {
        // FILETIME: 01D97D03F053ADD0 = some 2023 timestamp
        let data =
            "[F00000000][T01D97D03F053ADD0][O00000000]*C:\\Users\\cperez\\Desktop\\risks.csv";
        let row = build_row(
            "Item 1",
            data,
            r"ROOT\SOFTWARE\Microsoft\Office\16.0\Excel\User MRU\AD_...\File MRU",
            None,
        );
        assert_eq!(
            row.detail_columns[5].1,
            "C:\\Users\\cperez\\Desktop\\risks.csv"
        );
        assert!(!row.detail_columns[2].1.is_empty()); // LastOpened is set
        assert_eq!(row.detail_columns[4].1, ""); // LastClosed empty (no reading location)
    }

    #[test]
    fn detail_columns_order() {
        let row = build_row("FOLDERID_Desktop", "C:\\", "path", None);
        let col_names: Vec<&str> = row.detail_columns.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            col_names,
            &[
                "ValueName",
                "BatchKeyPath",
                "LastOpened",
                "BatchValueName",
                "LastClosed",
                "FileName"
            ]
        );
    }

    #[test]
    fn reading_locations_path_user_mru() {
        // 7 segments (after ROOT\): Office\16.0\Word\User MRU\AD_...\File MRU
        // 3 levels up: Office\16.0\Word
        let path = r"ROOT\Software\Microsoft\Office\16.0\Word\User MRU\AD_ABC\File MRU";
        let rl = reading_locations_path(path).unwrap();
        assert!(rl.ends_with("\\Reading Locations"), "got: {rl}");
        assert!(rl.contains("Word"), "should end at Word level: {rl}");
    }

    #[test]
    fn plugin_name_and_key_paths() {
        let p = OfficeMru;
        assert_eq!(p.plugin_name(), "Office MRU");
        let kps = p.key_paths();
        assert!(kps.contains(&r"Software\Microsoft\Office\*\*\File MRU"));
        assert!(kps.contains(&r"Software\Microsoft\Office\*\*\User MRU\*\File MRU"));
    }
}
