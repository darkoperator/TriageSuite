//! Port of RegistryPlugin.AppCompatFlags2 (AppCompatFlags2.cs + ValuesOut.cs).
//! Extracts AppCompatFlags Compatibility Assistant and Layers information.
//!
//! Fires on NTUSER.DAT at:
//!   `Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags`
//!
//! Detail-CSV column order (fixture-authoritative, from
//! `STCL1__cperez_NTUSER__plugin_AppCompatFlags2_NTUSER.DAT.csv`):
//!   Path, BatchKeyPath, BatchValueName
//!
//! Batch row format (C# ValuesOut):
//!   ValueData1 = "Path: {Path}"
//!   ValueData2 = ""
//!   ValueData3 = ""
//!
//! Design: the C# plugin iterates subkeys of the AppCompatFlags key.
//!   - "Layers" subkey: each value's name is a path (ValueData1="Path: {valueName}")
//!     BatchKeyPath = Layers subkey path.
//!   - "Compatibility Assistant" subkey: iterates its subkeys (Persisted, Store, etc.)
//!     and for each value in those sub-subkeys: BatchKeyPath = CompAssist subkey path.
//!
//! BatchKeyPath for both cases is the PARENT subkey (Layers or Compatibility Assistant),
//! NOT the leaf key. This matches C# `addData(j, subKey)` where subKey is Layers/CompAssist.

use notatin::cell_key_node::CellKeyNode;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct AppCompatFlags2;

fn make_row(path: &str, batch_key_path: &str) -> PluginRow {
    PluginRow {
        batch_value_name: "Multiple".to_string(),
        batch_value_data1: format!("Path: {path}"),
        batch_value_data2: String::new(),
        batch_value_data3: String::new(),
        detail_columns: vec![
            ("Path".to_string(), path.to_string()),
            ("BatchKeyPath".to_string(), batch_key_path.to_string()),
            ("BatchValueName".to_string(), "Multiple".to_string()),
        ],
    }
}

impl RegistryPlugin for AppCompatFlags2 {
    fn plugin_name(&self) -> &'static str {
        "AppCompatFlags2"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let mut rows = Vec::new();

        // Iterate subkeys of AppCompatFlags key.
        for mut sub_key in hive.sub_keys(key) {
            let sub_path = sub_key.path.trim_start_matches('\\').to_string();

            match sub_key.key_name.as_str() {
                "Layers" => {
                    // Each value's NAME is the path to the executable.
                    for v in sub_key.value_iter() {
                        let path = v.get_pretty_name();
                        rows.push(make_row(&path, &sub_path));
                    }
                }
                "Compatibility Assistant" => {
                    // Iterate sub-sub-keys (Persisted, Store, etc.).
                    // BatchKeyPath = Compatibility Assistant subkey path (not the leaf).
                    for sub_sub in hive.sub_keys(&mut sub_key) {
                        for v in sub_sub.value_iter() {
                            let path = v.get_pretty_name();
                            rows.push(make_row(&path, &sub_path));
                        }
                    }
                }
                _ => {}
            }
        }

        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_row_format() {
        let row = make_row(
            r"C:\Windows\system32\cmd.exe",
            r"ROOT\SOFTWARE\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers",
        );
        assert_eq!(row.batch_value_name, "Multiple");
        assert_eq!(row.batch_value_data1, r"Path: C:\Windows\system32\cmd.exe");
        assert_eq!(row.batch_value_data2, "");
        assert_eq!(row.batch_value_data3, "");
        let col_names: Vec<&str> = row.detail_columns.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(col_names, &["Path", "BatchKeyPath", "BatchValueName"]);
    }

    #[test]
    fn detail_columns_order() {
        let row = make_row("somepath", "somekeypath");
        let col_names: Vec<&str> = row.detail_columns.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(col_names, &["Path", "BatchKeyPath", "BatchValueName"]);
    }

    #[test]
    fn plugin_name_and_key_paths() {
        let p = AppCompatFlags2;
        assert_eq!(p.plugin_name(), "AppCompatFlags2");
        assert!(p
            .key_paths()
            .contains(&r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags"));
    }
}
