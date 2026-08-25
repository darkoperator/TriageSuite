//! Port of RegistryPlugin.LastVisitedPidlMRU (LastVisitedPidlMRU.cs + ValuesOut.cs).
//! Extracts shell items from the ComDlg32 LastVisitedPidlMRU key.
//!
//! Fires on NTUSER.DAT at:
//!   `Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\LastVisitedPidlMRU`
//!   `Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\LastVisitedPidlMRULegacy`
//!
//! The plugin fires directly on these keys (no subkey iteration). Each value
//! encodes: [UTF-16LE exe name][NUL NUL][PIDL bytes].
//!
//! Detail-CSV column order (fixture-authoritative):
//!   ValueName, BatchKeyPath, MruPosition, BatchValueName, Executable,
//!   AbsolutePath, OpenedOn, Details
//!
//! Batch row format (C# ValuesOut.cs):
//!   ValueData1 = "Exe: {Executable} Folder: {Executable} Absolute path: {AbsolutePath}"
//!   ValueData2 = "Opened: {OpenedOn?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
//!   ValueData3 = "MRU: {MruPosition} Details: {Details}"
//!
//! Note: C# ValuesOut.cs uses `Executable` TWICE in ValueData1:
//!   "Exe: {Executable} Folder: {Executable} Absolute path: {AbsolutePath}"
//!
//! OpenedOn is a standalone column; testkit normalizes the reference from
//! RECmd's "yyyy-MM-dd HH:mm:ss.fffffff" to ISO-8601 UTC. We emit ISO-8601 UTC.
//!
//! Details is a complex multiline ShellBag.ToString() text that we cannot
//! reproduce exactly. We emit an empty string and accept this divergence.
//!
//! Output ordering: sorted by MruPosition ascending.

use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct LastVisitedPidlMru;

/// Format DateTime<Utc> as ISO-8601 UTC with 7 fractional digits.
fn dt_to_iso8601(dt: chrono::DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

/// Format DateTime<Utc> as RECmd literal "yyyy-MM-dd HH:mm:ss.fffffff".
/// Used for embedded free-text ValueData fields.
fn dt_to_recmd_literal(dt: chrono::DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks)
}

/// Parse MRUListEx binary: build ordered list (position → entry_idx).
fn parse_mru_list_ex_ordered(raw: &[u8]) -> Vec<i32> {
    let mut order = Vec::new();
    let mut pos = 0usize;
    while pos + 4 <= raw.len() {
        let entry = i32::from_le_bytes(raw[pos..pos + 4].try_into().unwrap());
        pos += 4;
        if entry == -1 {
            break;
        }
        order.push(entry);
    }
    order
}

impl RegistryPlugin for LastVisitedPidlMru {
    fn plugin_name(&self) -> &'static str {
        "ComDlg32 LastVisitedPidlMRU"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\LastVisitedPidlMRU",
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\LastVisitedPidlMRULegacy",
        ]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        values: &[PluginValue],
        _hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let key_path = key.path.trim_start_matches('\\').to_string();
        let key_lw = key.last_key_written_date_and_time();

        // Build MRU order from MRUListEx.
        let mut mru_order: Vec<i32> = Vec::new();
        for v in values {
            if v.name == "MRUListEx" {
                mru_order = parse_mru_list_ex_ordered(&v.raw);
                break;
            }
        }

        let mut all_rows: Vec<(i32, PluginRow)> = Vec::new();

        for pv in values {
            if pv.name == "MRUListEx" {
                continue;
            }

            let entry_idx = match pv.name.parse::<i32>() {
                Ok(n) => n,
                Err(_) => continue,
            };

            // mru_position = index of entry_idx in mru_order
            let mru_pos = mru_order
                .iter()
                .position(|&e| e == entry_idx)
                .map(|p| p as i32)
                .unwrap_or(-1);

            // Parse value: [UTF-16LE exe name][NUL NUL][PIDL bytes]
            let raw = &pv.raw;
            let words: Vec<u16> = raw
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let full = String::from_utf16_lossy(&words);
            let exe_name = full.split('\0').next().unwrap_or("").to_string();

            let pidl_offset = ((exe_name.len() + 1) * 2).min(raw.len());
            let pidl_bytes = &raw[pidl_offset..];

            // Parse the PIDL bytes.
            let items = triage_shellitems::parse_id_list(pidl_bytes);
            let abs_path = triage_shellitems::absolute_path(&items);

            let opened_on = if mru_pos == 0 { Some(key_lw) } else { None };

            let opened_on_iso = opened_on.map(dt_to_iso8601).unwrap_or_default();
            let opened_on_recmd = opened_on.map(dt_to_recmd_literal).unwrap_or_default();

            // Details: complex ShellBag.ToString() text we cannot reproduce.
            // Declared as AcceptedDelta in compat tests.
            let details = String::new();

            let row = PluginRow {
                batch_value_name: pv.name.clone(),
                // C# ValuesOut: "Exe: {Executable} Folder: {Executable} Absolute path: {AbsolutePath}"
                // — Executable appears TWICE (Exe: and Folder:).
                batch_value_data1: format!(
                    "Exe: {exe_name} Folder: {exe_name} Absolute path: {abs_path}"
                ),
                batch_value_data2: format!("Opened: {opened_on_recmd}"),
                batch_value_data3: format!("MRU: {mru_pos} Details: {details}"),
                detail_columns: vec![
                    ("ValueName".to_string(), pv.name.clone()),
                    ("BatchKeyPath".to_string(), key_path.clone()),
                    ("MruPosition".to_string(), mru_pos.to_string()),
                    ("BatchValueName".to_string(), pv.name.clone()),
                    ("Executable".to_string(), exe_name),
                    ("AbsolutePath".to_string(), abs_path),
                    ("OpenedOn".to_string(), opened_on_iso),
                    ("Details".to_string(), details),
                ],
            };

            all_rows.push((mru_pos, row));
        }

        // Sort by MruPosition ascending.
        all_rows.sort_by_key(|(pos, _)| *pos);
        all_rows.into_iter().map(|(_, r)| r).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_and_key_paths() {
        let p = LastVisitedPidlMru;
        assert_eq!(p.plugin_name(), "ComDlg32 LastVisitedPidlMRU");
        let kps = p.key_paths();
        assert!(kps.contains(
            &r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\LastVisitedPidlMRU"
        ));
        assert!(kps.contains(
            &r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\LastVisitedPidlMRULegacy"
        ));
    }

    #[test]
    fn parse_mru_list_ex_ordered_basic() {
        let raw: Vec<u8> = [2i32, 0, 1, -1]
            .iter()
            .flat_map(|n: &i32| n.to_le_bytes())
            .collect();
        let order = parse_mru_list_ex_ordered(&raw);
        assert_eq!(order, vec![2, 0, 1]);
    }
}
