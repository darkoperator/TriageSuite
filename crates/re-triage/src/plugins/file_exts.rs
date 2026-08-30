//! Port of RegistryPlugin.FileExts (FileExts.cs + ValuesOut.cs).
//! Extracts file extension associations from the FileExts key.
//!
//! Fires on NTUSER.DAT at:
//!   `Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts`
//!
//! Detail-CSV column order (fixture-authoritative, from
//! `<host>__<user>_NTUSER__plugin_FileExts_NTUSER.DAT.csv`):
//!   Extension, BatchKeyPath, OpensWithExecutables, BatchValueName,
//!   OpensWithProgramIDs, UserChoice
//!
//! Batch row format (C# ValuesOut):
//!   ValueData1 = "Extension: {Extension}"
//!   ValueData2 = "Open Exes: {OpensWithExecutables} Open ProgIds: {OpensWithProgramIDs}"
//!   ValueData3 = "User choice: {UserChoice}"
//!
//! Design: the C# plugin iterates subkeys of the FileExts key. For each
//! extension subkey it looks at:
//!   - "OpenWithList": reads MRUList order and resolves executable names
//!   - "OpenWithProgids": collects progid value *names* (not values)
//!   - "UserChoice": reads the "ProgId" value
//!
//! If an extension subkey has NO subkeys, the plugin checks for a "Progid"
//! value directly on that subkey (no "OpenWithList" or "OpenWithProgids").
//! In that case BatchKeyPath/BatchValueName are NOT set (null/empty in C#).
//!
//! UserChoice default: "(UserChoice key not present)"
//! Missing MRU slot: "(Executable name for MRU slot '{c}' not found!)"

use notatin::cell_key_node::CellKeyNode;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct FileExts;

fn read_string_value(key: &CellKeyNode, name: &str) -> Option<String> {
    key.value_iter().find_map(|v| {
        if v.get_pretty_name().eq_ignore_ascii_case(name) {
            let (content, _) = v.get_content();
            Some(triage_registry::value::render_value_data(&content))
        } else {
            None
        }
    })
}

impl FileExts {
    /// Unit-testable row builder: takes pre-collected extension data.
    pub fn row_from_parts(
        &self,
        ext_name: &str,
        batch_key_path: &str,
        open_exes: Vec<String>,
        open_prog_ids: Vec<String>,
        user_choice: &str,
        has_subkeys: bool,
    ) -> PluginRow {
        if !has_subkeys {
            // No-subkeys branch: BatchKeyPath/BatchValueName not set (match C# null behavior).
            let op = open_prog_ids.join(", ");
            return PluginRow {
                batch_value_name: String::new(),
                batch_value_data1: format!("Extension: {ext_name}"),
                batch_value_data2: format!("Open Exes:  Open ProgIds: {op}"),
                batch_value_data3: format!("User choice: {user_choice}"),
                detail_columns: vec![
                    ("Extension".to_string(), ext_name.to_string()),
                    ("BatchKeyPath".to_string(), String::new()),
                    ("OpensWithExecutables".to_string(), String::new()),
                    ("BatchValueName".to_string(), String::new()),
                    ("OpensWithProgramIDs".to_string(), op),
                    ("UserChoice".to_string(), user_choice.to_string()),
                ],
            };
        }
        let oe_str = open_exes.join(", ");
        let op_str = open_prog_ids.join(", ");
        PluginRow {
            batch_value_name: "Multiple".to_string(),
            batch_value_data1: format!("Extension: {ext_name}"),
            batch_value_data2: format!("Open Exes: {oe_str} Open ProgIds: {op_str}"),
            batch_value_data3: format!("User choice: {user_choice}"),
            detail_columns: vec![
                ("Extension".to_string(), ext_name.to_string()),
                ("BatchKeyPath".to_string(), batch_key_path.to_string()),
                ("OpensWithExecutables".to_string(), oe_str),
                ("BatchValueName".to_string(), "Multiple".to_string()),
                ("OpensWithProgramIDs".to_string(), op_str),
                ("UserChoice".to_string(), user_choice.to_string()),
            ],
        }
    }
}

impl RegistryPlugin for FileExts {
    fn plugin_name(&self) -> &'static str {
        "File Extensions"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let key_path = key.path.trim_start_matches('\\').to_string();
        let mut rows = Vec::new();

        // Iterate subkeys of FileExts — each is a file extension.
        let ext_keys = hive.sub_keys(key);
        for mut ext_key in ext_keys {
            let ext_name = ext_key.key_name.clone();
            let sub_keys = hive.sub_keys(&mut ext_key);

            if sub_keys.is_empty() {
                // No subkeys: check for a "Progid" value directly on this key.
                let progid = read_string_value(&ext_key, "Progid").unwrap_or_default();
                let mut op_list = Vec::new();
                if !progid.is_empty() {
                    op_list.push(progid);
                }
                // C# does NOT set BatchKeyPath/BatchValueName for this branch.
                rows.push(PluginRow {
                    batch_value_name: String::new(),
                    batch_value_data1: format!("Extension: {ext_name}"),
                    batch_value_data2: format!("Open Exes:  Open ProgIds: {}", op_list.join(", ")),
                    batch_value_data3: "User choice: (UserChoice key not present)".to_string(),
                    detail_columns: vec![
                        ("Extension".to_string(), ext_name),
                        ("BatchKeyPath".to_string(), String::new()),
                        ("OpensWithExecutables".to_string(), String::new()),
                        ("BatchValueName".to_string(), String::new()),
                        ("OpensWithProgramIDs".to_string(), op_list.join(", ")),
                        (
                            "UserChoice".to_string(),
                            "(UserChoice key not present)".to_string(),
                        ),
                    ],
                });
                continue;
            }

            let mut open_exes: Vec<String> = Vec::new();
            let mut open_prog_ids: Vec<String> = Vec::new();
            let mut user_choice = "(UserChoice key not present)".to_string();

            for sub in &sub_keys {
                match sub.key_name.as_str() {
                    "OpenWithList" => {
                        let mru_list = read_string_value(sub, "MRUList");
                        if let Some(mru) = mru_list {
                            for c in mru.chars() {
                                let slot_name = c.to_string();
                                match read_string_value(sub, &slot_name) {
                                    Some(exe) => open_exes.push(exe),
                                    None => open_exes.push(format!(
                                        "(Executable name for MRU slot '{c}' not found!)"
                                    )),
                                }
                            }
                        }
                    }
                    "OpenWithProgids" => {
                        for v in sub.value_iter() {
                            open_prog_ids.push(v.get_pretty_name());
                        }
                    }
                    "UserChoice" => {
                        if let Some(prog_id) = read_string_value(sub, "ProgId") {
                            user_choice = prog_id;
                        }
                    }
                    _ => {}
                }
            }

            let oe_str = open_exes.join(", ");
            let op_str = open_prog_ids.join(", ");

            rows.push(PluginRow {
                batch_value_name: "Multiple".to_string(),
                batch_value_data1: format!("Extension: {ext_name}"),
                batch_value_data2: format!("Open Exes: {oe_str} Open ProgIds: {op_str}"),
                batch_value_data3: format!("User choice: {user_choice}"),
                detail_columns: vec![
                    ("Extension".to_string(), ext_name),
                    ("BatchKeyPath".to_string(), key_path.clone()),
                    ("OpensWithExecutables".to_string(), oe_str),
                    ("BatchValueName".to_string(), "Multiple".to_string()),
                    ("OpensWithProgramIDs".to_string(), op_str),
                    ("UserChoice".to_string(), user_choice),
                ],
            });
        }

        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_with_subkeys() {
        let p = FileExts;
        let row = p.row_from_parts(
            ".txt",
            r"ROOT\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\FileExts",
            vec!["notepad.exe".to_string(), "wordpad.exe".to_string()],
            vec![
                "txtfile".to_string(),
                "Applications\\notepad.exe".to_string(),
            ],
            "txtfile",
            true,
        );
        assert_eq!(row.batch_value_name, "Multiple");
        assert_eq!(row.batch_value_data1, "Extension: .txt");
        assert!(row.batch_value_data2.contains("notepad.exe, wordpad.exe"));
        assert!(row.batch_value_data2.contains("txtfile"));
        assert_eq!(row.batch_value_data3, "User choice: txtfile");
    }

    #[test]
    fn extension_no_subkeys_batch_fields_empty() {
        let p = FileExts;
        let row = p.row_from_parts(
            ".xyz",
            "",
            vec![],
            vec!["xyzApp".to_string()],
            "(UserChoice key not present)",
            false,
        );
        // No-subkeys branch: BatchValueName is empty, BatchKeyPath is empty
        assert_eq!(row.batch_value_name, "");
        assert_eq!(row.detail_columns[1].1, ""); // BatchKeyPath empty
        assert_eq!(row.detail_columns[3].1, ""); // BatchValueName empty
    }

    #[test]
    fn user_choice_default_when_absent() {
        let p = FileExts;
        let row = p.row_from_parts(
            ".abc",
            r"ROOT\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\FileExts",
            vec![],
            vec![],
            "(UserChoice key not present)",
            true,
        );
        let uc = &row.detail_columns[5];
        assert_eq!(uc.0, "UserChoice");
        assert_eq!(uc.1, "(UserChoice key not present)");
    }

    #[test]
    fn detail_columns_order() {
        let p = FileExts;
        let row = p.row_from_parts(".foo", "path", vec![], vec![], "someapp", true);
        let col_names: Vec<&str> = row.detail_columns.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            col_names,
            &[
                "Extension",
                "BatchKeyPath",
                "OpensWithExecutables",
                "BatchValueName",
                "OpensWithProgramIDs",
                "UserChoice"
            ]
        );
    }

    #[test]
    fn plugin_name_and_key_paths() {
        let p = FileExts;
        assert_eq!(p.plugin_name(), "File Extensions");
        assert!(p
            .key_paths()
            .contains(&r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts"));
    }
}
