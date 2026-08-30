//! Port of RegistryPlugin.FirstFolder (FirstFolder.cs + FolderInfo.cs).
//! Extracts program executables and their first-folder selection from the
//! ComDlg32 FirstFolder key.
//!
//! Fires on NTUSER.DAT at:
//!   `Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\FirstFolder`
//!
//! Detail-CSV column order (fixture-authoritative, from
//! `<host>__<user>_NTUSER__plugin_FirstFolder_NTUSER.DAT.csv`):
//!   Executable, BatchKeyPath, FolderName, BatchValueName, MRUPosition, OpenedOn
//!
//! Batch row format (C# FolderInfo ValuesOut):
//!   ValueData1 = "Exe: {Executable} Folder: {FolderName}"
//!   ValueData2 = "Opened: {OpenedOn?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
//!   ValueData3 = "Mru: {MRUPosition}"
//!
//! Design: the matched key has a `MRUListEx` plus numeric value names (0, 1, …).
//! Each value is a null-delimited UTF-16LE string; the first chunk is the
//! executable path, and the optional second chunk (after the first NUL) is the
//! folder path. Only MRU position 0 gets the key's `LastWriteTime` as `OpenedOn`.
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

pub struct FirstFolder;

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

/// Decode a buffer as UTF-16LE; return (first_chunk, second_chunk) split on NUL.
/// The first chunk is the executable path; the optional second chunk is the folder.
fn decode_exe_and_folder(raw: &[u8]) -> (String, String) {
    if raw.len() < 2 {
        return (String::new(), String::new());
    }
    let words: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let full = String::from_utf16_lossy(&words);
    let mut parts = full.split('\0');
    let exe = parts.next().unwrap_or("").to_string();
    // The folder is the next non-empty chunk (if any); may be empty.
    let folder = parts
        .next()
        .unwrap_or("")
        .trim_end_matches('\0')
        .to_string();
    (exe, folder)
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

impl FirstFolder {
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

            let (exe_name, folder_name) = decode_exe_and_folder(&v.raw);

            // Only MRU position 0 gets the timestamp.
            let (recmd_ts, iso_ts) = if mru_pos == 0 {
                (dt_to_recmd_literal(last_write), dt_to_iso8601(last_write))
            } else {
                (String::new(), String::new())
            };

            let row = PluginRow {
                batch_value_name: v.name.clone(),
                batch_value_data1: format!("Exe: {exe_name} Folder: {folder_name}"),
                batch_value_data2: format!("Opened: {recmd_ts}"),
                batch_value_data3: format!("Mru: {mru_pos}"),
                // Column order from fixture:
                //   Executable, BatchKeyPath, FolderName, BatchValueName, MRUPosition, OpenedOn
                detail_columns: vec![
                    ("Executable".to_string(), exe_name),
                    ("BatchKeyPath".to_string(), key_path.to_string()),
                    ("FolderName".to_string(), folder_name),
                    ("BatchValueName".to_string(), v.name.clone()),
                    ("MRUPosition".to_string(), mru_pos.to_string()),
                    ("OpenedOn".to_string(), iso_ts),
                ],
            };
            rows.push((mru_pos, row));
        }

        // Sort by MRU position ascending (C#: `l.OrderBy(t => t.MRUPosition)`).
        rows.sort_by_key(|(pos, _)| *pos);
        rows.into_iter().map(|(_, r)| r).collect()
    }
}

impl RegistryPlugin for FirstFolder {
    fn plugin_name(&self) -> &'static str {
        "First folder"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\FirstFolder"]
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

    /// Create an entry value with exe and optional folder (null-separated UTF-16LE).
    fn entry_val(name: &str, exe: &str, folder: &str) -> PluginValue {
        let mut raw: Vec<u8> = exe.encode_utf16().flat_map(|w| w.to_le_bytes()).collect();
        raw.extend_from_slice(&[0u8, 0u8]); // null separator between exe and folder
        if !folder.is_empty() {
            let folder_bytes: Vec<u8> = folder
                .encode_utf16()
                .flat_map(|w| w.to_le_bytes())
                .collect();
            raw.extend(folder_bytes);
            raw.extend_from_slice(&[0u8, 0u8]); // terminating null
        }
        PluginValue {
            name: name.into(),
            raw,
            value_data: String::new(),
            data_type: CellKeyValueDataTypes::REG_BIN,
        }
    }

    fn test_lw() -> DateTime<chrono::Utc> {
        // 2023-03-15 17:35:07.6830174 UTC — from a reference fixture
        chrono::Utc
            .with_ymd_and_hms(2023, 3, 15, 17, 35, 7)
            .unwrap()
            + chrono::Duration::nanoseconds(683_017_400)
    }

    #[test]
    fn basic_entry_with_folder() {
        let p = FirstFolder;
        let values = vec![
            mru_list_ex_val(&[0]),
            entry_val(
                "0",
                r"C:\Program Files\Wireshark\Wireshark.exe",
                r"C:\Users\user1\Documents\",
            ),
        ];
        let lw = test_lw();
        let rows = p.rows_from_values(
            r"ROOT\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\FirstFolder",
            lw,
            &values,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].batch_value_data1,
            r"Exe: C:\Program Files\Wireshark\Wireshark.exe Folder: C:\Users\user1\Documents\"
        );
        assert_eq!(rows[0].batch_value_data3, "Mru: 0");
        assert!(
            rows[0].batch_value_data2.starts_with("Opened: 2023-03-15"),
            "got {:?}",
            rows[0].batch_value_data2
        );
    }

    #[test]
    fn entry_without_folder() {
        let p = FirstFolder;
        let values = vec![
            mru_list_ex_val(&[0]),
            entry_val("0", r"C:\Windows\system32\rundll32.exe", ""),
        ];
        let lw = test_lw();
        let rows = p.rows_from_values("SomePath", lw, &values);
        assert_eq!(rows.len(), 1);
        // Folder is empty
        let folder_col = &rows[0].detail_columns[2];
        assert_eq!(folder_col.0, "FolderName");
        assert_eq!(folder_col.1, "");
        assert_eq!(
            rows[0].batch_value_data1,
            r"Exe: C:\Windows\system32\rundll32.exe Folder: "
        );
    }

    #[test]
    fn no_mru_list_returns_empty() {
        let p = FirstFolder;
        let values = vec![entry_val("0", "test.exe", "")];
        let lw = test_lw();
        let rows = p.rows_from_values("SomePath", lw, &values);
        assert!(rows.is_empty(), "no MRUListEx → no rows");
    }

    #[test]
    fn detail_columns_order_matches_fixture() {
        let p = FirstFolder;
        let values = vec![
            mru_list_ex_val(&[0]),
            entry_val("0", "prog.exe", r"C:\folder\"),
        ];
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
                "FolderName",
                "BatchValueName",
                "MRUPosition",
                "OpenedOn"
            ]
        );
    }

    #[test]
    fn mru_ordering_two_entries() {
        let p = FirstFolder;
        // MRUListEx: [1, 0] → value "1" is MRU pos 0, value "0" is MRU pos 1
        let values = vec![
            mru_list_ex_val(&[1, 0]),
            entry_val("0", "second.exe", ""),
            entry_val("1", "first.exe", r"C:\folder\"),
        ];
        let lw = test_lw();
        let rows = p.rows_from_values("SomePath", lw, &values);
        assert_eq!(rows.len(), 2);
        // Most recent = "first.exe" (pos 0)
        assert_eq!(rows[0].detail_columns[0].1, "first.exe");
        assert_eq!(rows[0].detail_columns[4].1, "0"); // MRUPosition
        assert!(
            !rows[0].detail_columns[5].1.is_empty(),
            "pos 0 should have timestamp"
        );
        // Second = "second.exe" (pos 1)
        assert_eq!(rows[1].detail_columns[0].1, "second.exe");
        assert_eq!(rows[1].detail_columns[4].1, "1"); // MRUPosition
        assert!(
            rows[1].detail_columns[5].1.is_empty(),
            "pos 1 should have no timestamp"
        );
    }

    #[test]
    fn opened_on_iso8601_for_mru0() {
        let p = FirstFolder;
        let values = vec![mru_list_ex_val(&[0]), entry_val("0", "prog.exe", "")];
        let lw = test_lw();
        let rows = p.rows_from_values("SomePath", lw, &values);
        let opened_col = &rows[0].detail_columns[5];
        assert_eq!(opened_col.0, "OpenedOn");
        assert!(
            opened_col.1.contains('T') && opened_col.1.ends_with('Z'),
            "expected ISO-8601 UTC, got {:?}",
            opened_col.1
        );
    }

    #[test]
    fn plugin_name_and_key_paths() {
        let p = FirstFolder;
        assert_eq!(p.plugin_name(), "First folder");
        assert!(p
            .key_paths()
            .contains(&r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\FirstFolder"));
    }
}
