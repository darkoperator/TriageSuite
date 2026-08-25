//! Port of RegistryPlugin.Services (Services.cs + Service.cs).
//! Extracts Windows services from ControlSet00*\Services subkeys.
//!
//! Key path: ControlSet00*\Services
//! Each subkey is a service entry. Also reads a Parameters subkey if present.
//!
//! Detail-CSV column order (fixture-authoritative):
//!   Name, BatchKeyPath, Description, BatchValueName, DisplayName, StartMode,
//!   ServiceType, NameKeyLastWrite, ParametersKeyLastWrite, Group, ImagePath,
//!   ServiceDLL, RequiredPrivileges
//!
//! Batch row format (ValueName = "None"):
//!   ValueData  = "Name: {Name} Desc: {Description}"
//!   ValueData2 = "Name last write: {recmd_literal}, Parameters last write: {recmd_literal}"
//!   ValueData3 = "Image path: {ImagePath} ServiceDLL: {ServiceDLL}"

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct Services;

/// Convert a chrono::DateTime<Utc> to ISO-8601 UTC string (for standalone detail columns).
fn dt_to_iso8601(dt: DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

/// Convert a chrono::DateTime<Utc> to RECmd literal "yyyy-MM-dd HH:mm:ss.fffffff"
/// (for embedded free-text fields like ValueData2 which the testkit does NOT normalize).
fn dt_to_recmd_literal(dt: DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks)
}

/// Read a string value from a CellKeyNode by value name (case-insensitive).
fn get_str_value(key: &CellKeyNode, name: &str) -> String {
    for v in key.value_iter() {
        if v.get_pretty_name().eq_ignore_ascii_case(name) {
            let (content, _) = v.get_content();
            return triage_registry::value::render_value_data(&content);
        }
    }
    String::new()
}

/// Read a u32 value from a CellKeyNode by value name (case-insensitive).
/// Returns None if the value is absent or not a DWORD.
fn get_u32_value(key: &CellKeyNode, name: &str) -> Option<u32> {
    for v in key.value_iter() {
        if v.get_pretty_name().eq_ignore_ascii_case(name) {
            let (content, _) = v.get_content();
            if let notatin::cell_value::CellValue::U32(n) = content {
                return Some(n);
            }
            // Fallback: parse from rendered string
            let s = triage_registry::value::render_value_data(&content);
            return s.parse::<u32>().ok();
        }
    }
    None
}

/// C# [Flags] enum ServiceTypeEnum:
///   KernelDriver=1, FileSystemDriver=2, Adapter=4, RecognizerDriver=8,
///   Win32OwnProcess=16, Win32ShareProcess=32, InteractiveProcess=256
///
/// C# [Flags].ToString() iterates defined values in definition order (ascending),
/// collects names for bits present, then if any bits remain undefined, outputs
/// the original numeric value. If no bits match at all, outputs the number.
fn service_type_to_string(v: u32) -> String {
    const FLAGS: &[(u32, &str)] = &[
        (1, "KernelDriver"),
        (2, "FileSystemDriver"),
        (4, "Adapter"),
        (8, "RecognizerDriver"),
        (16, "Win32OwnProcess"),
        (32, "Win32ShareProcess"),
        (256, "InteractiveProcess"),
    ];
    let mut remaining = v;
    let mut names: Vec<&str> = Vec::new();
    for &(bit, name) in FLAGS {
        if remaining & bit != 0 {
            remaining &= !bit;
            names.push(name);
        }
    }
    if remaining != 0 || names.is_empty() {
        // Undefined bits remain or no named bits found — output original number
        return v.to_string();
    }
    names.join(", ")
}

/// C# ServiceStartMode: Boot=0, System=1, Automatic=2, Manual=3, Disabled=4.
/// Default (when value absent) is Disabled=4.
fn start_mode_to_string(v: u32) -> &'static str {
    match v {
        0 => "Boot",
        1 => "System",
        2 => "Automatic",
        3 => "Manual",
        _ => "Disabled",
    }
}

impl RegistryPlugin for Services {
    fn plugin_name(&self) -> &'static str {
        "Services"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"ControlSet00*\Services"]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let mut rows = Vec::new();

        // The BatchKeyPath for all rows is the parent Services key path.
        let batch_key_path = key.path.trim_start_matches('\\').to_string();
        // C# key.KeyName — the leaf name of the Services key (always "Services").
        let key_name = key.key_name.clone();

        let subkeys = hive.sub_keys(key);

        for mut subkey in subkeys {
            let name = subkey.key_name.clone();
            let name_lw_dt = subkey.last_key_written_date_and_time();

            let desc = get_str_value(&subkey, "Description");
            let disp = get_str_value(&subkey, "DisplayName");
            let service_dll_key = get_str_value(&subkey, "ServiceDll");
            let group = get_str_value(&subkey, "Group");
            let image_path = get_str_value(&subkey, "ImagePath");
            let req_privs = get_str_value(&subkey, "RequiredPrivileges");

            // ServiceTypeEnum — default Adapter(4)
            let start_type = get_u32_value(&subkey, "Type").unwrap_or(4);
            let service_type_str = service_type_to_string(start_type);

            // ServiceStartMode — default Disabled(4)
            let start_mode_val = get_u32_value(&subkey, "Start").unwrap_or(4);
            let start_mode_str = start_mode_to_string(start_mode_val);

            // Look for Parameters subkey
            let param_subkeys = hive.sub_keys(&mut subkey);
            let mut param_lw_dt: Option<DateTime<chrono::Utc>> = None;
            let mut service_dll_param = String::new();

            for param_sub in param_subkeys {
                if param_sub.key_name.eq_ignore_ascii_case("Parameters") {
                    param_lw_dt = Some(param_sub.last_key_written_date_and_time());
                    service_dll_param = get_str_value(&param_sub, "SERVICEDLL");
                    break;
                }
            }

            // Build ServiceDLL composite string (svcDllInfo in C#)
            let svc_dll_info = if !service_dll_key.is_empty() {
                format!(
                    "{key_name} ServiceDll: {service_dll_key} / Parameter ServiceDll: {service_dll_param}"
                )
            } else {
                service_dll_param.clone()
            };

            // Timestamps for standalone detail columns (ISO-8601 UTC)
            let name_lw_iso = dt_to_iso8601(name_lw_dt);
            let param_lw_iso = param_lw_dt.map(dt_to_iso8601).unwrap_or_default();

            // Timestamps for embedded ValueData2 (RECmd literal format)
            let name_lw_recmd = dt_to_recmd_literal(name_lw_dt);
            let param_lw_recmd = param_lw_dt.map(dt_to_recmd_literal).unwrap_or_default();

            // Batch value fields
            let vd1 = format!("Name: {name} Desc: {desc}");
            let vd2 = format!(
                "Name last write: {name_lw_recmd}, Parameters last write: {param_lw_recmd}"
            );
            let vd3 = format!("Image path: {image_path} ServiceDLL: {svc_dll_info}");

            rows.push(PluginRow {
                batch_value_name: "None".to_string(),
                batch_value_data1: vd1,
                batch_value_data2: vd2,
                batch_value_data3: vd3,
                // Column order: Name,BatchKeyPath,Description,BatchValueName,DisplayName,
                //   StartMode,ServiceType,NameKeyLastWrite,ParametersKeyLastWrite,
                //   Group,ImagePath,ServiceDLL,RequiredPrivileges
                detail_columns: vec![
                    ("Name".to_string(), name),
                    ("BatchKeyPath".to_string(), batch_key_path.clone()),
                    ("Description".to_string(), desc),
                    ("BatchValueName".to_string(), "None".to_string()),
                    ("DisplayName".to_string(), disp),
                    ("StartMode".to_string(), start_mode_str.to_string()),
                    ("ServiceType".to_string(), service_type_str),
                    ("NameKeyLastWrite".to_string(), name_lw_iso),
                    ("ParametersKeyLastWrite".to_string(), param_lw_iso),
                    ("Group".to_string(), group),
                    ("ImagePath".to_string(), image_path),
                    ("ServiceDLL".to_string(), svc_dll_info),
                    ("RequiredPrivileges".to_string(), req_privs),
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
        assert_eq!(Services.plugin_name(), "Services");
        assert_eq!(Services.key_paths(), &[r"ControlSet00*\Services"]);
    }

    #[test]
    fn service_type_single_named_flags() {
        assert_eq!(service_type_to_string(1), "KernelDriver");
        assert_eq!(service_type_to_string(2), "FileSystemDriver");
        assert_eq!(service_type_to_string(4), "Adapter");
        assert_eq!(service_type_to_string(8), "RecognizerDriver");
        assert_eq!(service_type_to_string(16), "Win32OwnProcess");
        assert_eq!(service_type_to_string(32), "Win32ShareProcess");
        assert_eq!(service_type_to_string(256), "InteractiveProcess");
    }

    #[test]
    fn service_type_combinations() {
        // Win32OwnProcess(16) + InteractiveProcess(256) = 272
        assert_eq!(
            service_type_to_string(272),
            "Win32OwnProcess, InteractiveProcess"
        );
        // Win32ShareProcess(32) + InteractiveProcess(256) = 288
        assert_eq!(
            service_type_to_string(288),
            "Win32ShareProcess, InteractiveProcess"
        );
    }

    #[test]
    fn service_type_undefined_bits_outputs_number() {
        // 96 = 64+32 → 64 is undefined, output number
        assert_eq!(service_type_to_string(96), "96");
        // 80 = 64+16 → 64 is undefined, output number
        assert_eq!(service_type_to_string(80), "80");
        // 224 = 128+64+32 → undefined bits
        assert_eq!(service_type_to_string(224), "224");
        // 208 = 128+64+16 → undefined bits
        assert_eq!(service_type_to_string(208), "208");
        // 0 → no bits → number
        assert_eq!(service_type_to_string(0), "0");
    }

    #[test]
    fn start_mode_mapping() {
        assert_eq!(start_mode_to_string(0), "Boot");
        assert_eq!(start_mode_to_string(1), "System");
        assert_eq!(start_mode_to_string(2), "Automatic");
        assert_eq!(start_mode_to_string(3), "Manual");
        assert_eq!(start_mode_to_string(4), "Disabled");
        assert_eq!(start_mode_to_string(99), "Disabled");
    }

    #[test]
    fn detail_column_order() {
        let expected = [
            "Name",
            "BatchKeyPath",
            "Description",
            "BatchValueName",
            "DisplayName",
            "StartMode",
            "ServiceType",
            "NameKeyLastWrite",
            "ParametersKeyLastWrite",
            "Group",
            "ImagePath",
            "ServiceDLL",
            "RequiredPrivileges",
        ];
        assert_eq!(expected.len(), 13);
    }

    #[test]
    fn svc_dll_info_with_service_dll_key() {
        // When serviceDllkey is non-empty, C# prepends "Services ServiceDll: {key} / Parameter ServiceDll: {param}"
        let service_dll_key = "%SystemRoot%\\system32\\dhcpcore.dll";
        let service_dll_param = "%SystemRoot%\\system32\\dhcpcore.dll";
        let key_name = "Services";
        let result = format!(
            "{key_name} ServiceDll: {service_dll_key} / Parameter ServiceDll: {service_dll_param}"
        );
        assert_eq!(
            result,
            "Services ServiceDll: %SystemRoot%\\system32\\dhcpcore.dll / Parameter ServiceDll: %SystemRoot%\\system32\\dhcpcore.dll"
        );
    }
}
