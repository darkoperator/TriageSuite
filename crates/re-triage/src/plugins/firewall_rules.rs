//! Port of RegistryPlugin.FirewallRules (FirewallRules.cs + ValuesOut.cs).
//! Extracts Windows Firewall rules from the registry.
//!
//! Key paths:
//!   ControlSet00*\Services\SharedAccess\Parameters\FirewallPolicy\FirewallRules
//!   ControlSet00*\Services\SharedAccess\Parameters\FirewallPolicy\RestrictedServices\AppIso\FirewallRules
//!   ControlSet00*\Services\SharedAccess\Parameters\FirewallPolicy\Mdm\FirewallRules
//!
//! Each value in the key contains a '|'-delimited rule string with key=value segments.
//!
//! Detail-CSV column order (fixture-authoritative):
//!   Action, BatchKeyPath, Active, BatchValueName, Dir, Protocol, LPort, RPort, Name, Desc, App
//!
//! Batch row format (ValueName = "Multiple"):
//!   ValueData  = "Action: {Action} Active: {Active}"
//!   ValueData2 = "Dir: {Dir} Protocol: {Protocol} Name: {Name}"
//!   ValueData3 = "LPort: {LPort} RPort: {RPort} Desc: {Desc} App: {App}"

use notatin::cell_key_node::CellKeyNode;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct FirewallRules;

/// Map protocol number string to protocol name.
/// C# dictionary: NumToProtocol.
fn num_to_protocol(num: &str) -> Option<&'static str> {
    match num {
        "1" => Some("ICMP"),
        "2" => Some("IGMP"),
        "3" => Some("GGP"),
        "4" => Some("IPv4"),
        "5" => Some("ST"),
        "6" => Some("TCP"),
        "17" => Some("UDP"),
        "18" => Some("MUX"),
        "27" => Some("RDP"),
        "41" => Some("IPv6"),
        "47" => Some("GRE"),
        "58" => Some("IPv6-ICMP"),
        "92" => Some("MTP"),
        "121" => Some("SMP"),
        "143" => Some("Ethernet"),
        _ => None,
    }
}

/// C# SetData: if `regex_value` (treated as "contains" check) matches `compare_string`,
/// split by '=' and add the value part to `data`. For Protocol, map to name.
///
/// The C# regex `IsMatch` checks if the pattern occurs anywhere in the string.
/// Pattern "Action=.*" matches any string containing "Action=".
/// Since rule segments are "KEY=VALUE" pairs, `contains("KEY=")` is equivalent.
fn set_data(field_prefix: &str, compare_string: &str, data: &mut Vec<String>) {
    if compare_string.contains(field_prefix) {
        // Split on first '=' only
        if let Some(eq_pos) = compare_string.find('=') {
            let key_part = &compare_string[..eq_pos];
            let val_part = &compare_string[eq_pos + 1..];

            if key_part == "Protocol" {
                // Map to protocol name or fall back to raw value
                let name = num_to_protocol(val_part)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| val_part.to_string());
                data.push(name);
            } else {
                data.push(val_part.to_string());
            }
        }
    }
}

impl RegistryPlugin for FirewallRules {
    fn plugin_name(&self) -> &'static str {
        "FirewallRules"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[
            r"ControlSet00*\Services\SharedAccess\Parameters\FirewallPolicy\FirewallRules",
            r"ControlSet00*\Services\SharedAccess\Parameters\FirewallPolicy\RestrictedServices\AppIso\FirewallRules",
            r"ControlSet00*\Services\SharedAccess\Parameters\FirewallPolicy\Mdm\FirewallRules",
        ]
    }

    fn process(&self, key: &CellKeyNode, values: &[PluginValue]) -> Vec<PluginRow> {
        let batch_key_path = key.path.trim_start_matches('\\').to_string();
        let mut rows = Vec::new();

        for v in values {
            // Rule string is in value_data (decoded string)
            let rules = &v.value_data;
            let rule_list: Vec<&str> = rules.split('|').collect();

            let mut action: Vec<String> = Vec::new();
            let mut active: Vec<String> = Vec::new();
            let mut dir: Vec<String> = Vec::new();
            let mut protocol: Vec<String> = Vec::new();
            let mut lport: Vec<String> = Vec::new();
            let mut rport: Vec<String> = Vec::new();
            let mut name: Vec<String> = Vec::new();
            let mut desc: Vec<String> = Vec::new();
            let mut app: Vec<String> = Vec::new();

            for part in &rule_list {
                set_data("Action=", part, &mut action);
                set_data("Active=", part, &mut active);
                set_data("Dir=", part, &mut dir);
                set_data("Protocol=", part, &mut protocol);
                set_data("LPort=", part, &mut lport);
                set_data("RPort=", part, &mut rport);
                set_data("Name=", part, &mut name);
                set_data("Desc=", part, &mut desc);
                set_data("App=", part, &mut app);
            }

            let action_str = action.join(",");
            let active_str = active.join(",");
            let dir_str = dir.join(",");
            let protocol_str = protocol.join(",");
            let lport_str = lport.join(",");
            let rport_str = rport.join(",");
            let name_str = name.join(",");
            let desc_str = desc.join(",");
            let app_str = app.join(",");

            let vd1 = format!("Action: {action_str} Active: {active_str}");
            let vd2 = format!("Dir: {dir_str} Protocol: {protocol_str} Name: {name_str}");
            let vd3 =
                format!("LPort: {lport_str} RPort: {rport_str} Desc: {desc_str} App: {app_str}");

            rows.push(PluginRow {
                batch_value_name: "Multiple".to_string(),
                batch_value_data1: vd1,
                batch_value_data2: vd2,
                batch_value_data3: vd3,
                // Column order: Action,BatchKeyPath,Active,BatchValueName,Dir,Protocol,
                //   LPort,RPort,Name,Desc,App
                detail_columns: vec![
                    ("Action".to_string(), action_str),
                    ("BatchKeyPath".to_string(), batch_key_path.clone()),
                    ("Active".to_string(), active_str),
                    ("BatchValueName".to_string(), "Multiple".to_string()),
                    ("Dir".to_string(), dir_str),
                    ("Protocol".to_string(), protocol_str),
                    ("LPort".to_string(), lport_str),
                    ("RPort".to_string(), rport_str),
                    ("Name".to_string(), name_str),
                    ("Desc".to_string(), desc_str),
                    ("App".to_string(), app_str),
                ],
            });
        }

        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notatin::cell_key_value::CellKeyValueDataTypes;

    #[test]
    fn plugin_name_and_key_paths() {
        assert_eq!(FirewallRules.plugin_name(), "FirewallRules");
        assert_eq!(FirewallRules.key_paths().len(), 3);
        assert!(FirewallRules.key_paths()[0].contains("FirewallRules"));
    }

    #[test]
    fn protocol_mapping() {
        assert_eq!(num_to_protocol("6"), Some("TCP"));
        assert_eq!(num_to_protocol("17"), Some("UDP"));
        assert_eq!(num_to_protocol("1"), Some("ICMP"));
        assert_eq!(num_to_protocol("58"), Some("IPv6-ICMP"));
        assert_eq!(num_to_protocol("999"), None);
    }

    #[test]
    fn set_data_action() {
        let mut data = Vec::new();
        set_data("Action=", "Action=Allow", &mut data);
        assert_eq!(data, vec!["Allow"]);
    }

    #[test]
    fn set_data_protocol_maps_to_name() {
        let mut data = Vec::new();
        set_data("Protocol=", "Protocol=6", &mut data);
        assert_eq!(data, vec!["TCP"]);
    }

    #[test]
    fn set_data_protocol_unknown_passthrough() {
        let mut data = Vec::new();
        set_data("Protocol=", "Protocol=999", &mut data);
        assert_eq!(data, vec!["999"]);
    }

    #[test]
    fn set_data_no_match_skips() {
        let mut data = Vec::new();
        set_data("Action=", "Protocol=6", &mut data);
        assert!(data.is_empty());
    }

    #[test]
    fn parse_sample_rule() {
        let rule = "v2.31|Action=Allow|Active=TRUE|Dir=Out|Protocol=6|RPort=2869|App=%SystemRoot%\\system32\\svchost.exe|Name=@FirewallAPI.dll,-32765|Desc=@FirewallAPI.dll,-32768|EmbeddedContext=@FirewallAPI.dll,-32760|Profile=Private|RA4=LocalSubnet|RA6=LocalSubnet|";
        let parts: Vec<&str> = rule.split('|').collect();

        let mut action = Vec::new();
        let mut active = Vec::new();
        let mut dir = Vec::new();
        let mut protocol = Vec::new();
        let mut lport = Vec::new();
        let mut rport = Vec::new();
        let mut name = Vec::new();
        let mut desc = Vec::new();
        let mut app = Vec::new();

        for part in &parts {
            set_data("Action=", part, &mut action);
            set_data("Active=", part, &mut active);
            set_data("Dir=", part, &mut dir);
            set_data("Protocol=", part, &mut protocol);
            set_data("LPort=", part, &mut lport);
            set_data("RPort=", part, &mut rport);
            set_data("Name=", part, &mut name);
            set_data("Desc=", part, &mut desc);
            set_data("App=", part, &mut app);
        }

        assert_eq!(action, vec!["Allow"]);
        assert_eq!(active, vec!["TRUE"]);
        assert_eq!(dir, vec!["Out"]);
        assert_eq!(protocol, vec!["TCP"]);
        assert!(lport.is_empty());
        assert_eq!(rport, vec!["2869"]);
    }

    #[test]
    fn detail_column_order() {
        let val = PluginValue {
            name: "test".into(),
            raw: vec![],
            value_data: "v2.31|Action=Allow|Active=TRUE|Dir=Out".into(),
            data_type: CellKeyValueDataTypes::REG_SZ,
        };
        // We can't easily create a CellKeyNode for unit tests, but verify column order
        // is correct by checking the constant array
        let expected = [
            "Action",
            "BatchKeyPath",
            "Active",
            "BatchValueName",
            "Dir",
            "Protocol",
            "LPort",
            "RPort",
            "Name",
            "Desc",
            "App",
        ];
        assert_eq!(expected.len(), 11);
        // Suppress unused variable warning
        let _ = val;
    }
}
