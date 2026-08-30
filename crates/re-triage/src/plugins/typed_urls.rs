//! Port of RegistryPlugin.TypedURLs (TypedURLs.cs + TypedURL.cs).
//! Extracts URLs typed in Internet Explorer's address bar, optionally with
//! timestamps from the sibling `TypedURLsTime` key.
//!
//! Fires on NTUSER.DAT at:
//!   `Software\Microsoft\Internet Explorer\TypedURLs`
//!
//! Detail-CSV column order (fixture-authoritative, from
//! `<host>__<user>_NTUSER__plugin_TypedURLs_NTUSER.DAT.csv`):
//!   Timestamp, BatchKeyPath, Url, BatchValueName, Slack
//!
//! Batch row format (C# TypedURL ValuesOut):
//!   ValueData1 = "{Url}"
//!   ValueData2 = "Timestamp: {yyyy-MM-dd HH:mm:ss.fffffff}"  (UTC RECmd literal, or "Timestamp: ")
//!   ValueData3 = "Slack: {slack}"
//!
//! Design: the C# plugin looks up `TypedURLsTime` as a sibling of the matched
//! key (`key.Parent.SubKeys.SingleOrDefault(...)`) and cross-references each
//! value's timestamp. We achieve the same by computing the sibling key path from
//! the matched key's path and calling `hive.get_key()`.
//!
//! The `Timestamp` column is a STANDALONE timestamp column (listed in
//! `detail_columns`). The testkit normalizes it from the RECmd
//! `yyyy-MM-dd HH:mm:ss.fffffff` form to ISO-8601 UTC. We emit ISO-8601 UTC
//! directly, so both sides match without an AcceptedDelta.
//!
//! The embedded `Timestamp:` inside ValueData2 is a FREE-TEXT field and is
//! NOT normalized by the testkit. We emit RECmd's literal format there.

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};
use triage_registry::value::plugin_raw_bytes;

pub struct TypedURLs;

/// Format a `DateTime<Utc>` in RECmd's `yyyy-MM-dd HH:mm:ss.fffffff` literal.
/// Used for the embedded free-text `Timestamp:` field in ValueData2.
fn dt_to_recmd_literal(dt: DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks)
}

/// Format a `DateTime<Utc>` as ISO-8601 UTC with 7 fractional digits.
/// Used for the standalone `Timestamp` detail column (testkit-normalized).
fn dt_to_iso8601(dt: DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

/// Convert a Windows FILETIME (100-ns intervals since 1601-01-01 UTC) to UTC DateTime.
fn filetime_to_datetime(ft: i64) -> Option<DateTime<chrono::Utc>> {
    // Difference between FILETIME epoch (1601-01-01) and Unix epoch (1970-01-01)
    const FILETIME_TO_UNIX_SECS: i64 = 11_644_473_600;
    let secs = (ft / 10_000_000) - FILETIME_TO_UNIX_SECS;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    DateTime::from_timestamp(secs, nanos)
}

impl TypedURLs {
    /// Build rows from the matched key's values, with an optional map of
    /// value-name → filetime from the TypedURLsTime sibling key.
    ///
    /// `key_path` — root-stripped path of the matched key (for detail column).
    /// `values` — values from the TypedURLs key.
    /// `time_map` — map from value-name to raw FILETIME bytes (8 bytes LE),
    ///              built by the caller from TypedURLsTime values.
    pub fn rows_from_values(
        &self,
        key_path: &str,
        values: &[PluginValue],
        time_map: &std::collections::HashMap<String, Vec<u8>>,
    ) -> Vec<PluginRow> {
        let mut rows = Vec::new();
        for v in values {
            let url = render_utf16_value(&v.raw);

            // Look up timestamp from TypedURLsTime sibling key.
            let ts_opt: Option<DateTime<chrono::Utc>> = time_map.get(&v.name).and_then(|raw| {
                if raw.len() < 8 {
                    return None;
                }
                let ft = i64::from_le_bytes(raw[..8].try_into().ok()?);
                let dt = filetime_to_datetime(ft)?;
                // RECmd skips 1601-01-01 (year == 1601) timestamps (they mean "null").
                if dt.format("%Y").to_string() == "1601" {
                    return None;
                }
                Some(dt)
            });

            // Slack: decode slack bytes as UTF-16LE, replacing nulls with spaces, trimmed.
            let slack = decode_slack(&v.raw);

            let ts_recmd = ts_opt.map(dt_to_recmd_literal).unwrap_or_default();
            let ts_iso = ts_opt.map(dt_to_iso8601).unwrap_or_default();

            rows.push(PluginRow {
                batch_value_name: v.name.clone(),
                batch_value_data1: url.clone(),
                batch_value_data2: format!("Timestamp: {ts_recmd}"),
                batch_value_data3: format!("Slack: {slack}"),
                // Column order from fixture:
                //   Timestamp, BatchKeyPath, Url, BatchValueName, Slack
                detail_columns: vec![
                    ("Timestamp".to_string(), ts_iso),
                    ("BatchKeyPath".to_string(), key_path.to_string()),
                    ("Url".to_string(), url),
                    ("BatchValueName".to_string(), v.name.clone()),
                    ("Slack".to_string(), slack),
                ],
            });
        }
        rows
    }
}

/// Render a registry REG_SZ or REG_EXPAND_SZ value stored as UTF-16LE bytes.
/// Returns an empty string if decoding fails.
fn render_utf16_value(raw: &[u8]) -> String {
    if raw.len() < 2 {
        return String::new();
    }
    let words: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&words)
        .trim_end_matches('\0')
        .to_string()
}

/// Decode the value's "slack" (bytes beyond the null terminator of the string
/// value). The C# code does:
///   `Encoding.Unicode.GetString(keyValue.ValueSlackRaw).Replace('\0', ' ').Trim()`
///
/// notatin does not expose ValueSlackRaw separately; the `raw` field contains
/// the full raw bytes including the trailing null and any slack. We decode all
/// bytes as UTF-16LE, replace nulls with spaces, and trim — matching C# behavior.
fn decode_slack(raw: &[u8]) -> String {
    if raw.len() < 2 {
        return String::new();
    }
    // Decode the entire buffer as UTF-16LE (notatin's raw includes everything).
    let words: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let decoded = String::from_utf16_lossy(&words);

    // Find the first null (end of the URL string). Everything after is slack.
    let slack_part = match decoded.find('\0') {
        Some(idx) => &decoded[idx..],
        None => "",
    };

    // Replace remaining nulls with spaces, trim.
    slack_part.replace('\0', " ").trim().to_string()
}

impl RegistryPlugin for TypedURLs {
    fn plugin_name(&self) -> &'static str {
        "TypedURLs"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Software\Microsoft\Internet Explorer\TypedURLs"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let key_path = key.path.trim_start_matches('\\').to_string();

        // Build a map from value-name → raw FILETIME bytes from TypedURLsTime
        // sibling key. The sibling is at the same level as TypedURLs:
        //   `Software\Microsoft\Internet Explorer\TypedURLsTime`
        //
        // NOTE: The sibling path is computed by replacing "TypedURLs" with
        // "TypedURLsTime" in the matched key's path. The matched key path
        // includes the "ROOT\" prefix from notatin (already stripped above);
        // but `hive.get_key()` expects a path WITHOUT the root-key name.
        // We strip "ROOT\" from key_path to get the hive-relative path.
        let hive_rel_path = key_path.trim_start_matches("ROOT\\");
        let sibling_path = hive_rel_path.replace("TypedURLs", "TypedURLsTime");

        let mut time_map = std::collections::HashMap::new();
        if let Some(sibling) = hive.get_key(&sibling_path) {
            for v in sibling.value_iter() {
                // TypedURLsTime values are REG_BINARY containing 8-byte FILETIMEs.
                let (content, _) = v.get_content();
                let raw = plugin_raw_bytes(&content);
                if !raw.is_empty() {
                    time_map.insert(v.get_pretty_name(), raw);
                }
            }
        }

        // The pre-built PluginValue slice now carries UTF-16LE raw bytes for REG_SZ
        // values (populated in batch.rs via plugin_raw_bytes). Use them directly —
        // no need to re-read from the hive node.
        self.rows_from_values(&key_path, values, &time_map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notatin::cell_key_value::CellKeyValueDataTypes;

    fn sz_value(name: &str, url: &str) -> PluginValue {
        // Encode as UTF-16LE with null terminator.
        let mut raw: Vec<u8> = url.encode_utf16().flat_map(|w| w.to_le_bytes()).collect();
        raw.extend_from_slice(&[0u8, 0u8]); // null terminator
        PluginValue {
            name: name.into(),
            raw,
            value_data: url.to_string(),
            data_type: CellKeyValueDataTypes::REG_SZ,
        }
    }

    fn sz_value_with_slack(name: &str, url: &str, slack: &str) -> PluginValue {
        let mut raw: Vec<u8> = url.encode_utf16().flat_map(|w| w.to_le_bytes()).collect();
        raw.extend_from_slice(&[0u8, 0u8]); // null terminator
                                            // Append slack bytes (UTF-16LE)
        let slack_bytes: Vec<u8> = slack.encode_utf16().flat_map(|w| w.to_le_bytes()).collect();
        raw.extend(slack_bytes);
        PluginValue {
            name: name.into(),
            raw,
            value_data: url.to_string(),
            data_type: CellKeyValueDataTypes::REG_SZ,
        }
    }

    #[test]
    fn rows_no_timestamps() {
        let p = TypedURLs;
        let vals = vec![
            sz_value("url1", "https://example.com"),
            sz_value("url2", "https://rust-lang.org"),
        ];
        let time_map = std::collections::HashMap::new();
        let rows = p.rows_from_values(
            r"ROOT\SOFTWARE\Microsoft\Internet Explorer\TypedURLs",
            &vals,
            &time_map,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].batch_value_name, "url1");
        assert_eq!(rows[0].batch_value_data1, "https://example.com");
        // No timestamp → empty
        assert_eq!(rows[0].batch_value_data2, "Timestamp: ");
        assert_eq!(rows[0].batch_value_data3, "Slack: ");
    }

    #[test]
    fn rows_with_timestamp() {
        let p = TypedURLs;
        let vals = vec![sz_value("url1", "https://example.com")];

        // FILETIME for 2022-09-23 19:58:01.0917015 UTC
        // = (unix_secs + 11644473600) * 10_000_000 + fractional_ticks
        // = (1663963081 + 11644473600) * 10_000_000 + 917015
        // = 133084366810917015
        let ft: i64 = 133_084_366_810_917_015;
        let mut time_map = std::collections::HashMap::new();
        time_map.insert("url1".to_string(), ft.to_le_bytes().to_vec());

        let rows = p.rows_from_values(
            r"ROOT\SOFTWARE\Microsoft\Internet Explorer\TypedURLs",
            &vals,
            &time_map,
        );
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0]
                .batch_value_data2
                .starts_with("Timestamp: 2022-09-23"),
            "got {:?}",
            rows[0].batch_value_data2
        );
        // Standalone timestamp column should be ISO-8601 UTC
        let ts_col = &rows[0].detail_columns[0];
        assert_eq!(ts_col.0, "Timestamp");
        assert!(
            ts_col.1.contains('T') && ts_col.1.ends_with('Z'),
            "ISO-8601 UTC expected, got {:?}",
            ts_col.1
        );
    }

    #[test]
    fn skips_1601_timestamp() {
        let p = TypedURLs;
        let vals = vec![sz_value("url1", "https://example.com")];

        // FILETIME for 1601-01-01 00:00:00 (the "null" sentinel in C#)
        let ft: i64 = 0i64;
        let mut time_map = std::collections::HashMap::new();
        time_map.insert("url1".to_string(), ft.to_le_bytes().to_vec());

        let rows = p.rows_from_values(
            r"ROOT\SOFTWARE\Microsoft\Internet Explorer\TypedURLs",
            &vals,
            &time_map,
        );
        assert_eq!(rows.len(), 1);
        // 1601 timestamp → treated as null → no timestamp
        assert_eq!(rows[0].batch_value_data2, "Timestamp: ");
    }

    #[test]
    fn slack_extracted_after_null_terminator() {
        let p = TypedURLs;
        let vals = vec![sz_value_with_slack(
            "url1",
            "https://example.com",
            "leftover",
        )];
        let time_map = std::collections::HashMap::new();
        let rows = p.rows_from_values("SomePath", &vals, &time_map);
        assert_eq!(rows.len(), 1);
        let slack_detail = &rows[0].detail_columns[4];
        assert_eq!(slack_detail.0, "Slack");
        assert!(
            slack_detail.1.contains("leftover"),
            "slack should contain the extra bytes, got {:?}",
            slack_detail.1
        );
    }

    #[test]
    fn detail_columns_order_matches_fixture() {
        let p = TypedURLs;
        let vals = vec![sz_value("url1", "https://example.com")];
        let time_map = std::collections::HashMap::new();
        let rows = p.rows_from_values("SomePath", &vals, &time_map);
        let col_names: Vec<&str> = rows[0]
            .detail_columns
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(
            col_names,
            &[
                "Timestamp",
                "BatchKeyPath",
                "Url",
                "BatchValueName",
                "Slack"
            ]
        );
    }

    #[test]
    fn plugin_name_and_key_paths() {
        let p = TypedURLs;
        assert_eq!(p.plugin_name(), "TypedURLs");
        assert!(p
            .key_paths()
            .contains(&r"Software\Microsoft\Internet Explorer\TypedURLs"));
    }
}
