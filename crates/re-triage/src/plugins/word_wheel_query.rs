//! Port of RegistryPlugin.WordWheelQuery (WordWheelQuery.cs + ValuesOut.cs).
//! Extracts recent Windows Explorer search terms from the WordWheelQuery key.
//!
//! Fires on NTUSER.DAT at:
//!   `Software\Microsoft\Windows\CurrentVersion\Explorer\WordWheelQuery`
//!
//! Detail-CSV column order (fixture-authoritative, from
//! `STCL1__cperez_NTUSER__plugin_WordWheelQuery_NTUSER.DAT.csv`):
//!   SearchTerm, BatchKeyPath, MruPosition, BatchValueName, KeyName, LastWriteTimestamp
//!
//! Batch row format (C# ValuesOut):
//!   ValueData1 = "Term: {SearchTerm}"
//!   ValueData2 = "MRU: {MruPosition}"
//!   ValueData3 = ""
//!
//! Design: the C# plugin processes both the matched key itself AND its subkeys
//! (see `ProcessKey` which calls `ProcessLocalKey` on the key + all sub-keys).
//! We use `process_with_hive` to iterate subkeys. Each key contributing rows
//! must have a `MRUListEx` value; keys without it are silently skipped.
//!
//! MRU ordering: `MRUListEx` is an array of uint32 values ending with 0xFFFFFFFF.
//! Position 0 in the list was most-recently used; the value at MRU position 0
//! gets `LastWriteTimestamp` = the key's last-write time, all others get null.
//!
//! The `LastWriteTimestamp` column is a STANDALONE timestamp column. The testkit
//! normalizes the reference from RECmd's `yyyy-MM-dd HH:mm:ss.fffffff` to
//! ISO-8601 UTC. We emit ISO-8601 UTC directly — no AcceptedDelta needed.

use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};
use triage_registry::value::plugin_raw_bytes;

pub struct WordWheelQuery;

/// Parse a `MRUListEx` binary value into a map from entry-index → MRU-position.
/// The list is a sequence of little-endian uint32 values terminated by 0xFFFFFFFF.
/// Position 0 in the returned list means "most recently used".
fn parse_mru_list_ex(raw: &[u8]) -> std::collections::HashMap<u32, usize> {
    let mut map = std::collections::HashMap::new();
    let mut pos = 0usize;
    let mut i = 0usize;
    while pos + 4 <= raw.len() {
        let entry = u32::from_le_bytes(raw[pos..pos + 4].try_into().unwrap());
        if entry == 0xFFFF_FFFF {
            break;
        }
        map.insert(entry, i);
        pos += 4;
        i += 1;
    }
    map
}

/// Decode a REG_SZ/REG_BINARY value as UTF-16LE, strip trailing nulls.
fn decode_utf16le(raw: &[u8]) -> String {
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

/// Format a `DateTime<Utc>` as ISO-8601 UTC with 7 fractional digits.
/// Used for the standalone `LastWriteTimestamp` detail column.
fn dt_to_iso8601(dt: chrono::DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

/// Extract raw bytes from a CellKeyValue — handles both Binary (MRUListEx)
/// and String (search terms encoded as UTF-16LE in the registry).
/// Delegates to `plugin_raw_bytes` which centralises the string re-encoding logic.
fn raw_bytes_for_mru(v: &notatin::cell_key_value::CellKeyValue) -> Vec<u8> {
    let (content, _) = v.get_content();
    plugin_raw_bytes(&content)
}

/// Process one key (the matched key or one of its subkeys) that has a MRUListEx.
/// Returns rows sorted by MRU position (ascending = most-recently-used first).
fn process_local_key(node: &CellKeyNode) -> Vec<PluginRow> {
    // Collect all values first.
    let all_values: Vec<(String, Vec<u8>)> = node
        .value_iter()
        .map(|v| (v.get_pretty_name(), raw_bytes_for_mru(&v)))
        .collect();

    // Find MRUListEx.
    let mru_raw = match all_values.iter().find(|(n, _)| n == "MRUListEx") {
        Some((_, raw)) => raw.clone(),
        None => return Vec::new(),
    };

    let mru_positions = parse_mru_list_ex(&mru_raw);
    if mru_positions.is_empty() {
        return Vec::new();
    }

    let key_name = node.key_name.clone();
    let key_path = node.path.trim_start_matches('\\').to_string();
    let last_write = node.last_key_written_date_and_time();

    let mut rows: Vec<(usize, PluginRow)> = Vec::new();

    for (name, raw) in &all_values {
        if name == "MRUListEx" {
            continue;
        }
        let idx = match name.parse::<u32>() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let mru_pos = match mru_positions.get(&idx) {
            Some(&p) => p,
            None => continue,
        };

        let search_term = decode_utf16le(raw);

        // Only the MRU-position-0 entry gets the timestamp.
        let ts_iso = if mru_pos == 0 {
            dt_to_iso8601(last_write)
        } else {
            String::new()
        };

        let row = PluginRow {
            batch_value_name: name.clone(),
            batch_value_data1: format!("Term: {search_term}"),
            batch_value_data2: format!("MRU: {mru_pos}"),
            batch_value_data3: String::new(),
            // Column order from fixture:
            //   SearchTerm, BatchKeyPath, MruPosition, BatchValueName, KeyName, LastWriteTimestamp
            detail_columns: vec![
                ("SearchTerm".to_string(), search_term),
                ("BatchKeyPath".to_string(), key_path.clone()),
                ("MruPosition".to_string(), mru_pos.to_string()),
                ("BatchValueName".to_string(), name.clone()),
                ("KeyName".to_string(), key_name.clone()),
                ("LastWriteTimestamp".to_string(), ts_iso),
            ],
        };
        rows.push((mru_pos, row));
    }

    // Sort by MRU position ascending (0 = most recent first).
    rows.sort_by_key(|(pos, _)| *pos);
    rows.into_iter().map(|(_, r)| r).collect()
}

impl WordWheelQuery {
    /// Unit-testable row builder: takes pre-parsed values as (name, raw) pairs.
    ///
    /// `key_name` — the key's name (e.g., "WordWheelQuery")
    /// `key_path` — root-stripped full path
    /// `last_write_iso` — ISO-8601 UTC string for the most-recently-used entry
    /// `values` — all values including MRUListEx
    pub fn rows_from_kv(
        &self,
        key_name: &str,
        key_path: &str,
        last_write_iso: &str,
        values: &[(String, Vec<u8>)],
    ) -> Vec<PluginRow> {
        let mru_raw = match values.iter().find(|(n, _)| n == "MRUListEx") {
            Some((_, raw)) => raw.clone(),
            None => return Vec::new(),
        };
        let mru_positions = parse_mru_list_ex(&mru_raw);

        let mut rows: Vec<(usize, PluginRow)> = Vec::new();

        for (name, raw) in values {
            if name == "MRUListEx" {
                continue;
            }
            let idx = match name.parse::<u32>() {
                Ok(n) => n,
                Err(_) => continue,
            };
            let mru_pos = match mru_positions.get(&idx) {
                Some(&p) => p,
                None => continue,
            };

            let search_term = decode_utf16le(raw);
            let ts_iso = if mru_pos == 0 {
                last_write_iso.to_string()
            } else {
                String::new()
            };

            let row = PluginRow {
                batch_value_name: name.clone(),
                batch_value_data1: format!("Term: {search_term}"),
                batch_value_data2: format!("MRU: {mru_pos}"),
                batch_value_data3: String::new(),
                detail_columns: vec![
                    ("SearchTerm".to_string(), search_term),
                    ("BatchKeyPath".to_string(), key_path.to_string()),
                    ("MruPosition".to_string(), mru_pos.to_string()),
                    ("BatchValueName".to_string(), name.clone()),
                    ("KeyName".to_string(), key_name.to_string()),
                    ("LastWriteTimestamp".to_string(), ts_iso),
                ],
            };
            rows.push((mru_pos, row));
        }

        rows.sort_by_key(|(pos, _)| *pos);
        rows.into_iter().map(|(_, r)| r).collect()
    }
}

impl RegistryPlugin for WordWheelQuery {
    fn plugin_name(&self) -> &'static str {
        "WordWheelQuery"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Software\Microsoft\Windows\CurrentVersion\Explorer\WordWheelQuery"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        // Process the matched key itself, then all its subkeys.
        let mut all_rows = process_local_key(key);

        for sub in hive.sub_keys(key) {
            let sub_rows = process_local_key(&sub);
            all_rows.extend(sub_rows);
        }

        // Final sort by MRU position (matching C#'s `l.OrderBy(t => t.MruPosition)`).
        all_rows.sort_by_key(|r| {
            r.detail_columns
                .iter()
                .find(|(k, _)| k == "MruPosition")
                .and_then(|(_, v)| v.parse::<usize>().ok())
                .unwrap_or(usize::MAX)
        });

        all_rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mru_list_ex(order: &[u32]) -> Vec<u8> {
        let mut raw: Vec<u8> = order.iter().flat_map(|n| n.to_le_bytes()).collect();
        raw.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        raw
    }

    fn sz_val(name: &str, text: &str) -> (String, Vec<u8>) {
        let mut raw: Vec<u8> = text.encode_utf16().flat_map(|w| w.to_le_bytes()).collect();
        raw.extend_from_slice(&[0u8, 0u8]);
        (name.to_string(), raw)
    }

    #[test]
    fn mru_order_two_terms() {
        // MRUListEx: [1, 0] → entry "1" is most recent (pos 0), entry "0" is pos 1.
        let p = WordWheelQuery;
        let values = vec![
            ("MRUListEx".to_string(), mru_list_ex(&[1, 0])),
            sz_val("0", "startwars"),
            sz_val("1", "*.wav"),
        ];
        let rows = p.rows_from_kv(
            "WordWheelQuery",
            r"ROOT\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\WordWheelQuery",
            "2023-03-15T17:54:04.3162408Z",
            &values,
        );
        assert_eq!(rows.len(), 2);
        // First row (MRU pos 0) = "*.wav" with timestamp
        assert_eq!(rows[0].batch_value_data1, "Term: *.wav");
        assert_eq!(rows[0].batch_value_data2, "MRU: 0");
        let ts_col = &rows[0].detail_columns[5];
        assert_eq!(ts_col.0, "LastWriteTimestamp");
        assert_eq!(ts_col.1, "2023-03-15T17:54:04.3162408Z");
        // Second row (MRU pos 1) = "startwars" with no timestamp
        assert_eq!(rows[1].batch_value_data1, "Term: startwars");
        assert_eq!(rows[1].batch_value_data2, "MRU: 1");
        assert_eq!(rows[1].detail_columns[5].1, "");
    }

    #[test]
    fn no_mru_list_returns_empty() {
        let p = WordWheelQuery;
        let values = vec![sz_val("0", "test")];
        let rows = p.rows_from_kv("WordWheelQuery", "SomePath", "", &values);
        assert!(rows.is_empty(), "no MRUListEx → no rows");
    }

    #[test]
    fn detail_columns_order_matches_fixture() {
        let p = WordWheelQuery;
        let values = vec![
            ("MRUListEx".to_string(), mru_list_ex(&[0])),
            sz_val("0", "query"),
        ];
        let rows = p.rows_from_kv(
            "WordWheelQuery",
            "SomePath",
            "2023-01-01T00:00:00.0000000Z",
            &values,
        );
        assert_eq!(rows.len(), 1);
        let col_names: Vec<&str> = rows[0]
            .detail_columns
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(
            col_names,
            &[
                "SearchTerm",
                "BatchKeyPath",
                "MruPosition",
                "BatchValueName",
                "KeyName",
                "LastWriteTimestamp"
            ]
        );
    }

    #[test]
    fn plugin_name_and_key_paths() {
        let p = WordWheelQuery;
        assert_eq!(p.plugin_name(), "WordWheelQuery");
        assert!(p
            .key_paths()
            .contains(&r"Software\Microsoft\Windows\CurrentVersion\Explorer\WordWheelQuery"));
    }
}
