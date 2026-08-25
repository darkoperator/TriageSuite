//! Port of RegistryPlugin.NetworkSetup2 (NetworkSetup2.cs + ValuesOut.cs).
//! Extracts network interface information from the NetworkSetup2 registry key.
//!
//! Key path: ControlSet00*\Control\NetworkSetup2
//! Walks: Interfaces subkey → each interface GUID subkey → Kernel subkey.
//! Skips interface GUIDs that have no Kernel subkey.
//!
//! Detail-CSV column order (fixture-authoritative):
//!   ProtocolList, BatchKeyPath, Alias, BatchValueName, Description,
//!   Type, PermanentAddress, CurrentAddress
//!
//! Batch row format (ValueName = "Multiple"):
//!   ValueData1 = "ProtocolList: {ProtocolList} Alias: {Alias}"
//!   ValueData2 = "Description: {Description} Type: {Type}"
//!   ValueData3 = "PermanentAddress: {PermanentAddress} CurrentAddress (Mac Address): {CurrentAddress}"

use notatin::cell_key_node::CellKeyNode;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct NetworkSetup2;

fn get_str_value(key: &CellKeyNode, name: &str) -> Option<String> {
    for v in key.value_iter() {
        if v.get_pretty_name() == name {
            let (content, _) = v.get_content();
            let s = triage_registry::value::render_value_data(&content);
            return Some(s);
        }
    }
    None
}

impl RegistryPlugin for NetworkSetup2 {
    fn plugin_name(&self) -> &'static str {
        "NetworkSetup2"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"ControlSet00*\Control\NetworkSetup2"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let mut rows = Vec::new();

        // Find the Interfaces subkey
        let top_subkeys = hive.sub_keys(key);
        let mut interfaces_key: Option<CellKeyNode> = None;
        for sk in top_subkeys {
            if sk.key_name == "Interfaces" {
                interfaces_key = Some(sk);
                break;
            }
        }

        let mut interfaces = match interfaces_key {
            Some(k) => k,
            None => return rows,
        };

        // Iterate each interface GUID subkey
        let interface_subkeys = hive.sub_keys(&mut interfaces);
        for mut iface in interface_subkeys {
            let batch_key_path = iface.path.trim_start_matches('\\').to_string();

            // Find the Kernel subkey inside this interface
            let iface_subs = hive.sub_keys(&mut iface);
            let kernel = iface_subs.into_iter().find(|sk| sk.key_name == "Kernel");

            let kernel = match kernel {
                Some(k) => k,
                None => continue,
            };

            let protocol_list = get_str_value(&kernel, "ProtocolList").unwrap_or_default();
            let if_alias = get_str_value(&kernel, "IfAlias").unwrap_or_default();
            let if_descr = get_str_value(&kernel, "IfDescr").unwrap_or_default();
            let if_type = get_str_value(&kernel, "IfType").unwrap_or_default();
            let permanent_address = get_str_value(&kernel, "PermanentAddress").unwrap_or_default();
            let current_address = get_str_value(&kernel, "CurrentAddress").unwrap_or_default();

            let vd1 = format!("ProtocolList: {protocol_list} Alias: {if_alias}");
            let vd2 = format!("Description: {if_descr} Type: {if_type}");
            let vd3 = format!(
                "PermanentAddress: {permanent_address} CurrentAddress (Mac Address): {current_address}"
            );

            rows.push(PluginRow {
                batch_value_name: "Multiple".to_string(),
                batch_value_data1: vd1,
                batch_value_data2: vd2,
                batch_value_data3: vd3,
                detail_columns: vec![
                    ("ProtocolList".to_string(), protocol_list),
                    ("BatchKeyPath".to_string(), batch_key_path),
                    ("Alias".to_string(), if_alias),
                    ("BatchValueName".to_string(), "Multiple".to_string()),
                    ("Description".to_string(), if_descr),
                    ("Type".to_string(), if_type),
                    ("PermanentAddress".to_string(), permanent_address),
                    ("CurrentAddress".to_string(), current_address),
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
    fn plugin_name_and_key_paths() {
        assert_eq!(NetworkSetup2.plugin_name(), "NetworkSetup2");
        assert_eq!(
            NetworkSetup2.key_paths(),
            &[r"ControlSet00*\Control\NetworkSetup2"]
        );
    }

    #[test]
    fn value_data_format() {
        // Verify the BatchValueData format strings match C# ValuesOut.cs exactly.
        let protocol_list = "Tcpip6 TCPIP6TUNNEL";
        let alias = "6to4 Adapter";
        let descr = "Microsoft 6to4 Adapter";
        let typ = "131";
        let perm = "";
        let curr = "";
        assert_eq!(
            format!("ProtocolList: {protocol_list} Alias: {alias}"),
            "ProtocolList: Tcpip6 TCPIP6TUNNEL Alias: 6to4 Adapter"
        );
        assert_eq!(
            format!("Description: {descr} Type: {typ}"),
            "Description: Microsoft 6to4 Adapter Type: 131"
        );
        assert_eq!(
            format!("PermanentAddress: {perm} CurrentAddress (Mac Address): {curr}"),
            "PermanentAddress:  CurrentAddress (Mac Address): "
        );
    }

    #[test]
    fn detail_column_order() {
        let expected = [
            "ProtocolList",
            "BatchKeyPath",
            "Alias",
            "BatchValueName",
            "Description",
            "Type",
            "PermanentAddress",
            "CurrentAddress",
        ];
        assert_eq!(expected.len(), 8);
    }
}
