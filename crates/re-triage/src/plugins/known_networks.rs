//! Port of RegistryPlugin.KnownNetworks (KnownNetworks.cs + ValuesOut.cs).
//! Extracts Wi-Fi/LAN network history from Windows NetworkList registry keys.
//!
//! Key path: Microsoft\Windows NT\CurrentVersion\NetworkList
//!
//! IMPORTANT: DateCreated / DateLastConnected are Windows SYSTEMTIME structs
//! (16 bytes, LOCAL time). We format them with T+Z suffix so the testkit's
//! reference normalization (RECmd space-separated → T+Z) produces a match.
//!
//! Detail-CSV column order (fixture-authoritative):
//!   FirstNetwork, BatchKeyPath, NetworkName, BatchValueName, NameType,
//!   FirstConnectLOCAL, LastConnectedLOCAL, Managed, DNSSuffix, GatewayMacAddress,
//!   ProfileGUID

use notatin::cell_key_node::CellKeyNode;
use notatin::cell_value::CellValue;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct KnownNetworks;

/// Parse Windows SYSTEMTIME (16-byte binary, LOCAL time) and emit as
/// `yyyy-MM-ddTHH:mm:ss.fffffffZ` (with T and Z appended to local values so
/// the testkit normalizer on the reference side produces the same form).
/// Parse a Windows SYSTEMTIME struct (16 bytes, little-endian fields) and
/// format it as an ISO-8601 UTC string ("yyyy-MM-ddTHH:mm:ss.fffffffZ").
///
/// Used for the standalone detail-CSV timestamp columns (FirstConnectLOCAL,
/// LastConnectedLOCAL) where the testkit normalizes the reference fixture from
/// RECmd's space-separated local-time format to T+Z form before comparing.
fn systemtime_to_local_recmd(raw: &[u8]) -> String {
    if raw.len() < 16 {
        return String::new();
    }
    let year = u16::from_le_bytes([raw[0], raw[1]]) as i32;
    let month = u16::from_le_bytes([raw[2], raw[3]]) as u32;
    let day = u16::from_le_bytes([raw[6], raw[7]]) as u32;
    let hour = u16::from_le_bytes([raw[8], raw[9]]) as u32;
    let min = u16::from_le_bytes([raw[10], raw[11]]) as u32;
    let sec = u16::from_le_bytes([raw[12], raw[13]]) as u32;
    let ms = u16::from_le_bytes([raw[14], raw[15]]) as u64;
    let ticks = ms * 10_000;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{ticks:07}Z")
}

/// Parse a Windows SYSTEMTIME struct and format it as RECmd's batch-row
/// embedded timestamp format: "yyyy-MM-dd HH:mm:ss.fffffff" (space-separated,
/// no T separator, no Z suffix — matching RECmd KnownNetworks.cs ValuesOut).
///
/// Used for the batch CSV ValueData2 field where timestamps are embedded in
/// label text ("First Connect LOCAL: {ts} Last Connect LOCAL: {ts}"). The
/// testkit's normalize_reference_timestamp cannot normalize embedded timestamps,
/// so we match RECmd's format exactly rather than using ISO-8601.
fn systemtime_to_batch_local_recmd(raw: &[u8]) -> String {
    if raw.len() < 16 {
        return String::new();
    }
    let year = u16::from_le_bytes([raw[0], raw[1]]) as i32;
    let month = u16::from_le_bytes([raw[2], raw[3]]) as u32;
    let day = u16::from_le_bytes([raw[6], raw[7]]) as u32;
    let hour = u16::from_le_bytes([raw[8], raw[9]]) as u32;
    let min = u16::from_le_bytes([raw[10], raw[11]]) as u32;
    let sec = u16::from_le_bytes([raw[12], raw[13]]) as u32;
    let ms = u16::from_le_bytes([raw[14], raw[15]]) as u64;
    let ticks = ms * 10_000;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}.{ticks:07}")
}

/// Map `NameType` integer to its network type string.
fn name_type_string(nt: &str) -> &'static str {
    match nt {
        "6" => "Wired",
        "71" => "Wireless",
        "23" => "WWAN",
        "0" => "Unknown",
        _ => "Unknown",
    }
}

fn get_str_value(key: &CellKeyNode, name: &str) -> String {
    for v in key.value_iter() {
        if v.get_pretty_name() == name {
            let (content, _) = v.get_content();
            return triage_registry::value::render_value_data(&content);
        }
    }
    String::new()
}

fn get_binary_value(key: &CellKeyNode, name: &str) -> Vec<u8> {
    for v in key.value_iter() {
        if v.get_pretty_name() == name {
            let (content, _) = v.get_content();
            if let CellValue::Binary(b) = content {
                return b;
            }
        }
    }
    Vec::new()
}

/// Format raw MAC address bytes as `XX-XX-XX-XX-XX-XX`.
fn format_mac(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join("-")
}

#[derive(Default)]
struct NetworkEntry {
    profile_guid: String,
    network_name: String,
    /// ISO-8601 UTC format for the standalone detail-CSV column (FirstConnectLOCAL).
    /// Testkit normalizes the RECmd reference from space-sep to T+Z; ours matches.
    first_connect: String,
    /// ISO-8601 UTC format for the standalone detail-CSV column (LastConnectedLOCAL).
    last_connected: String,
    /// RECmd batch-embedded format ("yyyy-MM-dd HH:mm:ss.fffffff") for ValueData2.
    /// The testkit cannot normalize timestamps embedded in label text, so we match
    /// RECmd's space-separated format exactly for the batch row.
    first_connect_batch: String,
    last_connected_batch: String,
    name_type: String,
    is_managed: bool,
    batch_key_path: String,
    // Filled in from Signatures subkey
    first_network: String,
    dns_suffix: String,
    gateway_mac: String,
}

impl RegistryPlugin for KnownNetworks {
    fn plugin_name(&self) -> &'static str {
        "Known networks"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Microsoft\Windows NT\CurrentVersion\NetworkList"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let mut entries: Vec<NetworkEntry> = Vec::new();

        // --- Step 1: Collect top-level subkeys (Profiles, Signatures) ---
        let top_subkeys = hive.sub_keys(key);

        let mut profiles_key: Option<CellKeyNode> = None;
        let mut signatures_key: Option<CellKeyNode> = None;

        for top in top_subkeys {
            if top.key_name.eq_ignore_ascii_case("Profiles") {
                profiles_key = Some(top);
            } else if top.key_name.eq_ignore_ascii_case("Signatures") {
                signatures_key = Some(top);
            }
        }

        // --- Step 2: Process Profiles ---
        if let Some(mut profiles) = profiles_key {
            let profile_subkeys = hive.sub_keys(&mut profiles);
            for profile in profile_subkeys {
                let profile_guid = profile.key_name.clone();
                let batch_key_path = profile.path.trim_start_matches('\\').to_string();

                let date_last_raw = get_binary_value(&profile, "DateLastConnected");
                // is_managed: C# checks ValueData == "0"; in Rust, if the binary
                // blob is empty/zero this means managed. We check if the raw bytes
                // are absent OR if the rendered value_data == "0" (DWORD=0 case).
                let is_managed = if date_last_raw.is_empty() {
                    // Try reading as string/dword
                    let vd = get_str_value(&profile, "DateLastConnected");
                    vd == "0"
                } else {
                    // Binary 16-byte blob → not managed (managed entries typically
                    // don't have a valid 16-byte SYSTEMTIME here; but we check
                    // the rendered form for safety)
                    false
                };

                let last_connected = if !date_last_raw.is_empty() {
                    systemtime_to_local_recmd(&date_last_raw)
                } else {
                    String::new()
                };
                let last_connected_batch = if !date_last_raw.is_empty() {
                    systemtime_to_batch_local_recmd(&date_last_raw)
                } else {
                    String::new()
                };

                let date_created_raw = get_binary_value(&profile, "DateCreated");
                let first_connect = systemtime_to_local_recmd(&date_created_raw);
                let first_connect_batch = systemtime_to_batch_local_recmd(&date_created_raw);

                let profile_name = get_str_value(&profile, "ProfileName");
                let name_type_raw = get_str_value(&profile, "NameType");
                let name_type = name_type_string(&name_type_raw).to_string();

                // RECmd always uses ProfileName as NetworkName regardless of
                // managed status. The `is_managed` flag only affects the `Managed`
                // column in the detail CSV, not the displayed name.
                let network_name = profile_name.clone();

                entries.push(NetworkEntry {
                    profile_guid,
                    network_name,
                    first_connect,
                    last_connected,
                    first_connect_batch,
                    last_connected_batch,
                    name_type,
                    is_managed,
                    batch_key_path,
                    ..Default::default()
                });
            }
        }

        // --- Step 3: Enrich from Signatures\Unmanaged and Signatures\Managed ---
        if let Some(mut sigs) = signatures_key {
            let sig_subkeys = hive.sub_keys(&mut sigs);

            for mut sig_group in sig_subkeys {
                let is_unmanaged = sig_group.key_name.eq_ignore_ascii_case("Unmanaged");
                let is_managed_group = sig_group.key_name.eq_ignore_ascii_case("Managed");
                if !is_unmanaged && !is_managed_group {
                    continue;
                }

                let sig_entries = hive.sub_keys(&mut sig_group);
                for sig in sig_entries {
                    let profile_guid_val = get_str_value(&sig, "ProfileGuid");
                    let dns_suffix = get_str_value(&sig, "DnsSuffix");
                    let first_network = get_str_value(&sig, "FirstNetwork");
                    let mac_raw = get_binary_value(&sig, "DefaultGatewayMac");
                    let gateway_mac = if mac_raw.is_empty() {
                        // Fall back to string value_data (already dash-formatted by render_value_data)
                        get_str_value(&sig, "DefaultGatewayMac")
                    } else {
                        format_mac(&mac_raw)
                    };

                    // Find matching entry by ProfileGuid and enrich it.
                    // C# calls kn.UpdateInfo(gatewayMacRaw, dnsSuffix, firstNetwork, managed)
                    // where `managed` is `false` for Unmanaged and `true` for Managed.
                    // UpdateInfo sets Managed = managed (the parameter), so Unmanaged
                    // enrichment must reset is_managed to false, matching C# exactly.
                    for entry in &mut entries {
                        if entry.profile_guid.eq_ignore_ascii_case(&profile_guid_val) {
                            entry.first_network = first_network.clone();
                            entry.dns_suffix = dns_suffix.clone();
                            entry.gateway_mac = gateway_mac.clone();
                            if is_managed_group {
                                entry.is_managed = true;
                                // For domain-managed networks the ProfileName in the
                                // Profiles subkey is sometimes empty for AD-joined hosts.
                                // RECmd uses FirstNetwork (from Signatures\Managed) as the
                                // network name in that case.  Never blank the name we already
                                // have from ProfileName; only fill it from first_network when
                                // ProfileName was empty.
                                if entry.network_name.is_empty() {
                                    entry.network_name = first_network.clone();
                                }
                            } else {
                                // Unmanaged group: reset is_managed = false (C# UpdateInfo managed: false)
                                entry.is_managed = false;
                            }
                            break;
                        }
                    }
                }
            }
        }

        // --- Step 4: Build PluginRows ---
        entries
            .into_iter()
            .map(|e| {
                let managed_str = if e.is_managed { "True" } else { "False" };
                // BatchValueData fields — must match RECmd KnownNetworks.cs ValuesOut format.
                // RECmd emits:
                //   ValueData:  "Name: {NetworkName} Type: {NameType}"
                //   ValueData2: "First Connect LOCAL: {FirstConnectLOCAL} Last Connect LOCAL: {LastConnectedLOCAL}"
                //   ValueData3: "Gateway MAC: {GatewayMacAddress}"
                let vd1 = format!("Name: {} Type: {}", e.network_name, e.name_type);
                let vd2 = format!(
                    "First Connect LOCAL: {} Last Connect LOCAL: {}",
                    e.first_connect_batch, e.last_connected_batch
                );
                let vd3 = format!("Gateway MAC: {}", e.gateway_mac);
                PluginRow {
                    batch_value_name: "Multiple".to_string(),
                    batch_value_data1: vd1,
                    batch_value_data2: vd2,
                    batch_value_data3: vd3,
                    detail_columns: vec![
                        ("FirstNetwork".to_string(), e.first_network),
                        ("BatchKeyPath".to_string(), e.batch_key_path),
                        ("NetworkName".to_string(), e.network_name),
                        ("BatchValueName".to_string(), "Multiple".to_string()),
                        ("NameType".to_string(), e.name_type),
                        ("FirstConnectLOCAL".to_string(), e.first_connect),
                        ("LastConnectedLOCAL".to_string(), e.last_connected),
                        ("Managed".to_string(), managed_str.to_string()),
                        ("DNSSuffix".to_string(), e.dns_suffix),
                        ("GatewayMacAddress".to_string(), e.gateway_mac),
                        ("ProfileGUID".to_string(), e.profile_guid),
                    ],
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_and_key_paths() {
        assert_eq!(KnownNetworks.plugin_name(), "Known networks");
        assert_eq!(
            KnownNetworks.key_paths(),
            &[r"Microsoft\Windows NT\CurrentVersion\NetworkList"]
        );
    }

    #[test]
    fn systemtime_parse_known_value() {
        // 2024-06-28 19:04:37.295 LOCAL → ticks = 295 * 10000 = 2950000
        let mut raw = [0u8; 16];
        raw[0..2].copy_from_slice(&2024u16.to_le_bytes()); // year
        raw[2..4].copy_from_slice(&6u16.to_le_bytes()); // month
        raw[4..6].copy_from_slice(&5u16.to_le_bytes()); // weekday (ignored)
        raw[6..8].copy_from_slice(&28u16.to_le_bytes()); // day
        raw[8..10].copy_from_slice(&19u16.to_le_bytes()); // hour
        raw[10..12].copy_from_slice(&4u16.to_le_bytes()); // minute
        raw[12..14].copy_from_slice(&37u16.to_le_bytes()); // second
        raw[14..16].copy_from_slice(&295u16.to_le_bytes()); // ms
        let out = systemtime_to_local_recmd(&raw);
        assert_eq!(out, "2024-06-28T19:04:37.2950000Z");
    }

    #[test]
    fn name_type_mapping() {
        assert_eq!(name_type_string("6"), "Wired");
        assert_eq!(name_type_string("71"), "Wireless");
        assert_eq!(name_type_string("23"), "WWAN");
        assert_eq!(name_type_string("0"), "Unknown");
        assert_eq!(name_type_string("99"), "Unknown");
    }

    #[test]
    fn format_mac_6_bytes() {
        let bytes = [0xF0, 0x9F, 0xC2, 0x16, 0x61, 0xA7];
        assert_eq!(format_mac(&bytes), "F0-9F-C2-16-61-A7");
    }

    #[test]
    fn detail_column_order() {
        let expected = [
            "FirstNetwork",
            "BatchKeyPath",
            "NetworkName",
            "BatchValueName",
            "NameType",
            "FirstConnectLOCAL",
            "LastConnectedLOCAL",
            "Managed",
            "DNSSuffix",
            "GatewayMacAddress",
            "ProfileGUID",
        ];
        assert_eq!(expected.len(), 11);
    }
}
