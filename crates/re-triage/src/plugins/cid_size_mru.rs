//! Port of RegistryPlugin.CIDSizeMRU (CIDSizeMRU.cs + CIDSizeInfo.cs).
//! Extracts program executables from the ComDlg32 CIDSizeMRU key.
//!
//! Fires on NTUSER.DAT at:
//!   `Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\CIDSizeMRU`
//!
//! Detail-CSV column order (fixture-authoritative, from
//! `<host>__<user>_NTUSER__plugin_CIDSizeMRU_NTUSER.DAT.csv`):
//!   Executable, BatchKeyPath, MRUPosition, BatchValueName, OpenedOn
//!
//! Batch row format (C# CIDSizeInfo ValuesOut):
//!   ValueData1 = "{Executable}"
//!   ValueData2 = "Opened: {OpenedOn?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
//!   ValueData3 = "Mru: {MRUPosition}"
//!
//! Design: the matched key has a `MRUListEx` plus numeric value names (0, 1, 2…).
//! Each value is a null-delimited UTF-16LE string; the first chunk is the exe name.
//! Only the most-recently-used (MRU position 0) entry gets the key's
//! `LastWriteTime` as `OpenedOn`; all others are null.
//!
//! The `OpenedOn` column is a STANDALONE timestamp column. The testkit normalizes
//! the reference from RECmd's `yyyy-MM-dd HH:mm:ss.fffffff` to ISO-8601 UTC.
//! We emit ISO-8601 UTC directly — no AcceptedDelta needed.
//!
//! The embedded `Opened:` inside ValueData2 is a FREE-TEXT field and is NOT
//! normalized by the testkit — we use RECmd's literal format there.

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct CIDSizeMRU;

/// Parse a `MRUListEx` binary value into a map: entry-index → MRU-position (0=most recent).
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

/// Decode a buffer as UTF-16LE; split on NUL, return the first chunk (exe name).
fn decode_first_chunk(raw: &[u8]) -> String {
    if raw.len() < 2 {
        return String::new();
    }
    let words: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let full = String::from_utf16_lossy(&words);
    // Split on NUL; first chunk is the exe name.
    full.split('\0').next().unwrap_or("").to_string()
}

/// Format a `DateTime<Utc>` in RECmd's `yyyy-MM-dd HH:mm:ss.fffffff` literal.
/// Used for the embedded free-text `Opened:` in ValueData2.
fn dt_to_recmd_literal(dt: DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks)
}

/// Format a `DateTime<Utc>` as ISO-8601 UTC with 7 fractional digits.
/// Used for the standalone `OpenedOn` detail column.
fn dt_to_iso8601(dt: DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

impl CIDSizeMRU {
    /// Build rows from raw values, given the key path and last-write time.
    ///
    /// `key_path` — root-stripped path for the detail column.
    /// `last_write` — the key's last-write timestamp (used for MRU pos 0).
    /// `values` — all values from the key (including MRUListEx).
    pub fn rows_from_values(
        &self,
        key_path: &str,
        last_write: DateTime<chrono::Utc>,
        values: &[PluginValue],
    ) -> Vec<PluginRow> {
        // Find MRUListEx.
        let mru_raw = match values.iter().find(|v| v.name == "MRUListEx") {
            Some(v) => v.raw.clone(),
            None => return Vec::new(),
        };
        let mru_positions = parse_mru_list_ex(&mru_raw);

        let mut rows: Vec<(usize, PluginRow)> = Vec::new();

        for v in values {
            if v.name == "MRUListEx" {
                continue;
            }
            let idx = match v.name.parse::<u32>() {
                Ok(n) => n,
                Err(_) => continue,
            };
            let mru_pos = match mru_positions.get(&idx) {
                Some(&p) => p,
                None => continue,
            };

            let exe_name = decode_first_chunk(&v.raw);

            // Only MRU position 0 gets the timestamp.
            let (recmd_ts, iso_ts) = if mru_pos == 0 {
                (dt_to_recmd_literal(last_write), dt_to_iso8601(last_write))
            } else {
                (String::new(), String::new())
            };

            let row = PluginRow {
                batch_value_name: v.name.clone(),
                batch_value_data1: exe_name.clone(),
                batch_value_data2: format!("Opened: {recmd_ts}"),
                batch_value_data3: format!("Mru: {mru_pos}"),
                // Column order from fixture:
                //   Executable, BatchKeyPath, MRUPosition, BatchValueName, OpenedOn
                detail_columns: vec![
                    ("Executable".to_string(), exe_name),
                    ("BatchKeyPath".to_string(), key_path.to_string()),
                    ("MRUPosition".to_string(), mru_pos.to_string()),
                    ("BatchValueName".to_string(), v.name.clone()),
                    ("OpenedOn".to_string(), iso_ts),
                ],
            };
            rows.push((mru_pos, row));
        }

        // Sort by MRU position ascending.
        rows.sort_by_key(|(pos, _)| *pos);
        rows.into_iter().map(|(_, r)| r).collect()
    }
}

impl RegistryPlugin for CIDSizeMRU {
    fn plugin_name(&self) -> &'static str {
        "ComDlg32 CIDSizeMRU"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\CIDSizeMRU"]
    }

    fn process(&self, key: &CellKeyNode, values: &[PluginValue]) -> Vec<PluginRow> {
        let key_path = key.path.trim_start_matches('\\').to_string();
        let last_write = key.last_key_written_date_and_time();
        self.rows_from_values(&key_path, last_write, values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use notatin::cell_key_value::CellKeyValueDataTypes;

    fn mru_list_ex_val(order: &[u32]) -> PluginValue {
        let mut raw: Vec<u8> = order.iter().flat_map(|n| n.to_le_bytes()).collect();
        raw.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        PluginValue {
            name: "MRUListEx".into(),
            raw,
            value_data: String::new(),
            data_type: CellKeyValueDataTypes::REG_BIN,
        }
    }

    fn entry_val(name: &str, exe: &str) -> PluginValue {
        // UTF-16LE with null terminator
        let mut raw: Vec<u8> = exe.encode_utf16().flat_map(|w| w.to_le_bytes()).collect();
        raw.extend_from_slice(&[0u8, 0u8]);
        PluginValue {
            name: name.into(),
            raw,
            value_data: String::new(),
            data_type: CellKeyValueDataTypes::REG_BIN,
        }
    }

    fn test_lw() -> DateTime<chrono::Utc> {
        // 2023-11-25 17:06:37.7946186 UTC — from a reference fixture
        // Build from known components: 2023-11-25 17:06:37 + 794618600 ns
        chrono::Utc
            .with_ymd_and_hms(2023, 11, 25, 17, 6, 37)
            .unwrap()
            + chrono::Duration::nanoseconds(794_618_600)
    }

    #[test]
    fn basic_two_entries() {
        let p = CIDSizeMRU;
        // MRUListEx order: [0, 6] → entry "0" is MRU pos 0, entry "6" is MRU pos 1
        let values = vec![
            mru_list_ex_val(&[0, 6]),
            entry_val("0", "Code.exe"),
            entry_val("6", "ADExplorer.exe"),
        ];
        let lw = test_lw();
        let rows = p.rows_from_values(
            r"ROOT\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\CIDSizeMRU",
            lw,
            &values,
        );
        assert_eq!(rows.len(), 2);
        // MRU pos 0 = Code.exe with timestamp
        assert_eq!(rows[0].batch_value_data1, "Code.exe");
        assert_eq!(rows[0].batch_value_data3, "Mru: 0");
        assert!(
            rows[0].batch_value_data2.starts_with("Opened: 2023-11-25"),
            "got {:?}",
            rows[0].batch_value_data2
        );
        // MRU pos 1 = ADExplorer.exe with no timestamp
        assert_eq!(rows[1].batch_value_data1, "ADExplorer.exe");
        assert_eq!(rows[1].batch_value_data3, "Mru: 1");
        assert_eq!(rows[1].batch_value_data2, "Opened: ");
    }

    #[test]
    fn no_mru_list_returns_empty() {
        let p = CIDSizeMRU;
        let values = vec![entry_val("0", "Code.exe")];
        let lw = test_lw();
        let rows = p.rows_from_values("SomePath", lw, &values);
        assert!(rows.is_empty(), "no MRUListEx → no rows");
    }

    #[test]
    fn detail_columns_order_matches_fixture() {
        let p = CIDSizeMRU;
        let values = vec![mru_list_ex_val(&[0]), entry_val("0", "Code.exe")];
        let lw = test_lw();
        let rows = p.rows_from_values("SomePath", lw, &values);
        assert_eq!(rows.len(), 1);
        let col_names: Vec<&str> = rows[0]
            .detail_columns
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(
            col_names,
            &[
                "Executable",
                "BatchKeyPath",
                "MRUPosition",
                "BatchValueName",
                "OpenedOn"
            ]
        );
    }

    #[test]
    fn opened_on_iso8601_for_mru0() {
        let p = CIDSizeMRU;
        let values = vec![mru_list_ex_val(&[0]), entry_val("0", "Code.exe")];
        let lw = test_lw();
        let rows = p.rows_from_values("SomePath", lw, &values);
        assert_eq!(rows.len(), 1);
        let opened_col = &rows[0].detail_columns[4];
        assert_eq!(opened_col.0, "OpenedOn");
        // Should be ISO-8601 UTC
        assert!(
            opened_col.1.contains('T') && opened_col.1.ends_with('Z'),
            "expected ISO-8601 UTC, got {:?}",
            opened_col.1
        );
        assert!(
            opened_col.1.starts_with("2023-11-25T"),
            "got {:?}",
            opened_col.1
        );
    }

    #[test]
    fn embedded_opened_recmd_format() {
        // The embedded "Opened:" in ValueData2 should use RECmd literal format
        // (space-separated, not ISO-8601).
        let p = CIDSizeMRU;
        let values = vec![mru_list_ex_val(&[0]), entry_val("0", "Code.exe")];
        let lw = test_lw();
        let rows = p.rows_from_values("SomePath", lw, &values);
        assert_eq!(rows.len(), 1);
        // RECmd format: "Opened: yyyy-MM-dd HH:mm:ss.fffffff" (space between date/time)
        let vd2 = &rows[0].batch_value_data2;
        assert!(
            vd2.starts_with("Opened: 2023-11-25 17:06:37"),
            "expected RECmd literal format, got {vd2:?}"
        );
    }

    #[test]
    fn plugin_name_and_key_paths() {
        let p = CIDSizeMRU;
        assert_eq!(p.plugin_name(), "ComDlg32 CIDSizeMRU");
        assert!(p
            .key_paths()
            .contains(&r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\CIDSizeMRU"));
    }
}
