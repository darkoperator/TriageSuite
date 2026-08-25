//! Port of RegistryPlugin.OpenSavePidlMRU (OpenSavePidlMRU.cs + ValuesOut.cs).
//! Extracts shell items from the ComDlg32 OpenSavePidlMRU subkeys.
//!
//! Fires on NTUSER.DAT at:
//!   `Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\OpenSavePidlMRU`
//!
//! The plugin fires on the ROOT key and iterates its SUBKEYS (each extension
//! subkey like `*`, `exe`, `xml`, etc.). The BatchKeyPath in the output is
//! the subkey's path, not the root key path.
//!
//! Detail-CSV column order (fixture-authoritative):
//!   Extension, BatchKeyPath, ValueName, BatchValueName, MruPosition,
//!   AbsolutePath, OpenedOn, Details
//!
//! Batch row format (C# ValuesOut.cs):
//!   ValueData1 = "Extension: {Extension} Absolute path: {AbsolutePath}"
//!   ValueData2 = "Opened: {OpenedOn?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
//!   ValueData3 = "MRU: {MruPosition} Details: {Details}"
//!
//! OpenedOn is a standalone column; testkit normalizes the reference from
//! RECmd's "yyyy-MM-dd HH:mm:ss.fffffff" to ISO-8601 UTC. We emit ISO-8601 UTC.
//!
//! Details is a complex multiline ShellBag.ToString() text that we cannot
//! reproduce exactly. We emit an empty string and accept this divergence.
//!
//! Output ordering: sorted by MruPosition ascending across all subkeys.

use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct OpenSavePidlMru;

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

/// Parse MRUListEx binary: build ordered list of entry indices (position → entry_idx).
/// Returns Vec where index = mru_position, value = entry_idx.
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

/// Extract raw bytes from a notatin CellValue.
fn extract_raw(content: &notatin::cell_value::CellValue) -> Vec<u8> {
    triage_registry::value::plugin_raw_bytes(content)
}

impl RegistryPlugin for OpenSavePidlMru {
    fn plugin_name(&self) -> &'static str {
        "ComDlg32 OpenSavePidlMRU"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\OpenSavePidlMRU"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let mut all_rows: Vec<(i32, PluginRow)> = Vec::new();

        for sub in hive.sub_keys(key) {
            let ext = sub.key_name.clone();
            let sub_path = sub.path.trim_start_matches('\\').to_string();
            let sub_lw = sub.last_key_written_date_and_time();

            // Read all values from the subkey.
            let mut mru_raw: Option<Vec<u8>> = None;
            let mut sub_values: Vec<(String, Vec<u8>)> = Vec::new();

            for v in sub.value_iter() {
                let name = v.get_pretty_name();
                let (content, _) = v.get_content();
                let raw = extract_raw(&content);
                if name == "MRUListEx" {
                    mru_raw = Some(raw);
                } else {
                    sub_values.push((name, raw));
                }
            }

            // Parse MRUListEx to get ordered list.
            let mru_order = mru_raw
                .as_deref()
                .map(parse_mru_list_ex_ordered)
                .unwrap_or_default();

            for (value_name, raw) in &sub_values {
                let entry_idx = match value_name.parse::<i32>() {
                    Ok(n) => n,
                    Err(_) => continue,
                };

                // mru_position = index of entry_idx in mru_order
                let mru_pos = mru_order
                    .iter()
                    .position(|&e| e == entry_idx)
                    .map(|p| p as i32)
                    .unwrap_or(-1);

                let opened_on = if mru_pos == 0 { Some(sub_lw) } else { None };

                // Parse PIDL bytes directly (raw IS the PIDL for OpenSavePidlMRU).
                let items = triage_shellitems::parse_id_list(raw);
                let abs_path = triage_shellitems::absolute_path(&items);

                let opened_on_iso = opened_on.map(dt_to_iso8601).unwrap_or_default();
                let opened_on_recmd = opened_on.map(dt_to_recmd_literal).unwrap_or_default();

                // Details: complex ShellBag.ToString() text we cannot reproduce.
                // Declared as AcceptedDelta in compat tests.
                let details = String::new();

                let row = PluginRow {
                    batch_value_name: value_name.clone(),
                    batch_value_data1: format!("Extension: {ext} Absolute path: {abs_path}"),
                    batch_value_data2: format!("Opened: {opened_on_recmd}"),
                    batch_value_data3: format!("MRU: {mru_pos} Details: {details}"),
                    detail_columns: vec![
                        ("Extension".to_string(), ext.clone()),
                        ("BatchKeyPath".to_string(), sub_path.clone()),
                        ("ValueName".to_string(), value_name.clone()),
                        ("BatchValueName".to_string(), value_name.clone()),
                        ("MruPosition".to_string(), mru_pos.to_string()),
                        ("AbsolutePath".to_string(), abs_path),
                        ("OpenedOn".to_string(), opened_on_iso),
                        ("Details".to_string(), details),
                    ],
                };

                all_rows.push((mru_pos, row));
            }
        }

        // Sort by MruPosition ascending (matches C#: OrderBy(t => t.MruPosition)).
        all_rows.sort_by_key(|(pos, _)| *pos);
        all_rows.into_iter().map(|(_, r)| r).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_and_key_paths() {
        let p = OpenSavePidlMru;
        assert_eq!(p.plugin_name(), "ComDlg32 OpenSavePidlMRU");
        let kps = p.key_paths();
        assert!(kps.contains(
            &r"Software\Microsoft\Windows\CurrentVersion\Explorer\ComDlg32\OpenSavePidlMRU"
        ));
    }

    #[test]
    fn parse_mru_list_ex_ordered_basic() {
        // MRUListEx: [3, 1, 0, 2, -1 terminator]
        let raw: Vec<u8> = [3i32, 1, 0, 2, -1]
            .iter()
            .flat_map(|n: &i32| n.to_le_bytes())
            .collect();
        let order = parse_mru_list_ex_ordered(&raw);
        assert_eq!(order, vec![3, 1, 0, 2]);
    }
}
