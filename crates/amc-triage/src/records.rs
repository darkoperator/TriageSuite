//! Amcache dataset record structs and pure builders. Populated in Tasks 3–9.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use triage_core::error::TriageError;
use triage_core::output::router::OutputRouter;
use triage_registry::hive::Hive;

use crate::values::{self, ValueMap};

/// ProgramEntries row — 26 columns, exact AmcacheParser header order.
#[derive(Debug, Clone, Serialize)]
pub struct ProgramRecord {
    #[serde(rename = "ProgramId")]
    pub program_id: String,
    #[serde(rename = "KeyLastWriteTimestamp")]
    pub key_last_write: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "Publisher")]
    pub publisher: String,
    #[serde(rename = "InstallDateArpLastModified")]
    pub install_date_arp_last_modified: String,
    #[serde(rename = "InstallDate")]
    pub install_date: String,
    #[serde(rename = "InstallDateMsi")]
    pub install_date_msi: String,
    #[serde(rename = "OSVersionAtInstallTime")]
    pub os_version_at_install_time: String,
    #[serde(rename = "InstallDateFromLinkFile")]
    pub install_date_from_link_file: String,
    #[serde(rename = "BundleManifestPath")]
    pub bundle_manifest_path: String,
    #[serde(rename = "HiddenArp")]
    pub hidden_arp: String,
    #[serde(rename = "InboxModernApp")]
    pub inbox_modern_app: String,
    #[serde(rename = "Language")]
    pub language: String,
    #[serde(rename = "ManifestPath")]
    pub manifest_path: String,
    #[serde(rename = "MsiPackageCode")]
    pub msi_package_code: String,
    #[serde(rename = "MsiProductCode")]
    pub msi_product_code: String,
    #[serde(rename = "PackageFullName")]
    pub package_full_name: String,
    #[serde(rename = "ProgramInstanceId")]
    pub program_instance_id: String,
    #[serde(rename = "RegistryKeyPath")]
    pub registry_key_path: String,
    #[serde(rename = "RootDirPath")]
    pub root_dir_path: String,
    #[serde(rename = "Type")]
    pub type_: String,
    #[serde(rename = "Source")]
    pub source: String,
    #[serde(rename = "StoreAppType")]
    pub store_app_type: String,
    #[serde(rename = "UninstallString")]
    pub uninstall_string: String,
    #[serde(rename = "Manufacturer")]
    pub manufacturer: String,
}

/// Build one ProgramRecord from a subkey's last-write + values.
pub fn build_program_record(key_last_write: DateTime<Utc>, vm: &ValueMap) -> ProgramRecord {
    ProgramRecord {
        program_id: vm.str("ProgramId"),
        key_last_write: values::ts_string(key_last_write),
        name: vm.str("Name"),
        version: vm.str("Version"),
        publisher: vm.str("Publisher"),
        install_date_arp_last_modified: vm.str("InstallDateArpLastModified"),
        install_date: values::ts_opt(values::parse_dotnet_date(&vm.str("InstallDate"))),
        install_date_msi: values::ts_opt(values::parse_dotnet_date(&vm.str("InstallDateMsi"))),
        os_version_at_install_time: vm.str("OSVersionAtInstallTime"),
        install_date_from_link_file: vm.str("InstallDateFromLinkFile"),
        bundle_manifest_path: vm.str("BundleManifestPath"),
        hidden_arp: vm.dotnet_bool("HiddenArp"),
        inbox_modern_app: vm.dotnet_bool("InboxModernApp"),
        language: {
            let raw = vm.str("Language");
            if raw.is_empty() {
                "0".to_string()
            } else {
                values::parse_int(&raw).to_string()
            }
        },
        manifest_path: vm.str("ManifestPath"),
        msi_package_code: vm.str("MsiPackageCode"),
        msi_product_code: vm.str("MsiProductCode"),
        package_full_name: vm.str("PackageFullName"),
        program_instance_id: vm.str("ProgramInstanceId"),
        registry_key_path: vm.str("RegistryKeyPath"),
        root_dir_path: vm.str("RootDirPath"),
        type_: vm.str("Type"),
        source: vm.str("Source"),
        store_app_type: vm.str("StoreAppType"),
        uninstall_string: vm.str("UninstallString"),
        manufacturer: vm.str("Manufacturer"),
    }
}

/// Walk Root\InventoryApplication subkeys; emit ProgramEntries; collect
/// ProgramId -> Name (Task 4 uses the name to set ApplicationName on
/// associated file entries).
pub fn emit_program_entries(
    hive: &mut Hive,
    out: &mut OutputRouter,
) -> Result<(u64, HashMap<String, String>), TriageError> {
    let mut count = 0u64;
    let mut names = HashMap::new();
    let Some(mut root) = hive.get_key(r"Root\InventoryApplication") else {
        return Ok((0, names));
    };
    for sub in hive.sub_keys(&mut root) {
        let lw = values::key_last_write(&sub);
        let vm = ValueMap::from_key(&sub);
        let rec = build_program_record(lw, &vm);
        if !rec.program_id.is_empty() {
            names.insert(rec.program_id.clone(), rec.name.clone());
        }
        out.write("program_entries", &rec)?;
        count += 1;
    }
    Ok((count, names))
}

/// FileEntries row — 21 columns (Unassociated & Associated share this schema).
#[derive(Debug, Clone, Serialize)]
pub struct FileEntryRecord {
    #[serde(rename = "ApplicationName")]
    pub application_name: String,
    #[serde(rename = "ProgramId")]
    pub program_id: String,
    #[serde(rename = "FileKeyLastWriteTimestamp")]
    pub file_key_last_write: String,
    #[serde(rename = "SHA1")]
    pub sha1: String,
    #[serde(rename = "IsOsComponent")]
    pub is_os_component: String,
    #[serde(rename = "FullPath")]
    pub full_path: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "FileExtension")]
    pub file_extension: String,
    #[serde(rename = "LinkDate")]
    pub link_date: String,
    #[serde(rename = "ProductName")]
    pub product_name: String,
    #[serde(rename = "Size")]
    pub size: String,
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "ProductVersion")]
    pub product_version: String,
    #[serde(rename = "LongPathHash")]
    pub long_path_hash: String,
    #[serde(rename = "BinaryType")]
    pub binary_type: String,
    #[serde(rename = "IsPeFile")]
    pub is_pe_file: String,
    #[serde(rename = "BinFileVersion")]
    pub bin_file_version: String,
    #[serde(rename = "BinProductVersion")]
    pub bin_product_version: String,
    #[serde(rename = "Usn")]
    pub usn: String,
    #[serde(rename = "Language")]
    pub language: String,
    #[serde(rename = "Description")]
    pub description: String,
}

/// Faithful cross-platform port of .NET Path.GetExtension for Windows paths:
/// last '.' after the final '\\' or '/'; includes the dot; empty if none or trailing dot.
fn dotnet_get_extension(p: &str) -> String {
    for (i, ch) in p.char_indices().rev() {
        match ch {
            '.' => {
                return if i + ch.len_utf8() != p.len() {
                    p[i..].to_string()
                } else {
                    String::new()
                };
            }
            '\\' | '/' => return String::new(),
            _ => {}
        }
    }
    String::new()
}

/// FileExtension = extension of FullPath, else of Name (AmcacheNew L49-63).
fn file_extension(full_path: &str, name: &str) -> String {
    let e = dotnet_get_extension(full_path);
    if e.is_empty() {
        dotnet_get_extension(name)
    } else {
        e
    }
}

/// Build one FileEntryRecord from a subkey's last-write + values.
/// `program_name`: Some(name) when the entry's ProgramId is associated to a
/// known program (ApplicationName = that name); None → "Unassociated".
pub fn build_file_entry_record(
    sub_last_write: DateTime<Utc>,
    vm: &ValueMap,
    program_name: Option<&str>,
) -> FileEntryRecord {
    let full_path = vm.str("LowerCaseLongPath");
    let name = vm.str("Name");
    // LinkDate: parse failure → DateTimeOffset.MinValue in C#, which renders as
    // "0001-01-01T00:00:00.0000000Z". We reproduce that literal on parse
    // failure of a NON-empty value; empty stays empty.
    let link_raw = vm.str("LinkDate");
    let link_date = if link_raw.is_empty() {
        String::new()
    } else {
        match values::parse_dotnet_date(&link_raw) {
            Some(dt) => values::ts_string(dt),
            None => "0001-01-01T00:00:00.0000000Z".to_string(),
        }
    };
    FileEntryRecord {
        application_name: program_name.unwrap_or("Unassociated").to_string(),
        program_id: vm.str("ProgramId"),
        file_key_last_write: values::ts_string(sub_last_write),
        sha1: values::strip_sha1(&vm.str("FileId")),
        is_os_component: vm.dotnet_bool("IsOsComponent"),
        full_path: full_path.clone(),
        name: name.clone(),
        file_extension: file_extension(&full_path, &name),
        link_date,
        product_name: vm.str("ProductName"),
        size: values::parse_size(&vm.str("Size")).to_string(),
        version: vm.str("Version"),
        product_version: vm.str("ProductVersion"),
        long_path_hash: vm.str("LongPathHash"),
        binary_type: vm.str("BinaryType"),
        is_pe_file: vm.dotnet_bool("IsPeFile"),
        bin_file_version: vm.str("BinFileVersion"),
        bin_product_version: vm.str("BinProductVersion"),
        usn: values::parse_usn(&vm.str("Usn")),
        language: values::parse_int(&vm.str("Language")).to_string(),
        description: vm.str("Description"),
    }
}

/// Walk Root\InventoryApplicationFile; route each entry to associated/unassociated
/// based on whether its ProgramId is a known program (Task 3's ProgramId->Name map).
pub fn emit_file_entries(
    hive: &mut Hive,
    out: &mut OutputRouter,
    program_names: &HashMap<String, String>,
) -> Result<u64, TriageError> {
    let mut count = 0u64;
    let Some(mut root) = hive.get_key(r"Root\InventoryApplicationFile") else {
        return Ok(0);
    };
    for sub in hive.sub_keys(&mut root) {
        let lw = values::key_last_write(&sub);
        let vm = ValueMap::from_key(&sub);
        let pid = vm.str("ProgramId");
        let program_name = program_names.get(&pid).map(|s| s.as_str());
        let rec = build_file_entry_record(lw, &vm, program_name);
        if program_name.is_some() {
            out.write("associated_file_entries", &rec)?;
        } else {
            out.write("unassociated_file_entries", &rec)?;
        }
        count += 1;
    }
    Ok(count)
}

/// ShortCuts row — KeyName, LnkName, KeyLastWriteTimestamp.
#[derive(Debug, Clone, Serialize)]
pub struct ShortcutRecord {
    #[serde(rename = "KeyName")]
    pub key_name: String,
    #[serde(rename = "LnkName")]
    pub lnk_name: String,
    #[serde(rename = "KeyLastWriteTimestamp")]
    pub key_last_write: String,
}

/// Walk Root\InventoryApplicationShortcut subkeys; emit ShortCuts.
/// LnkName = the first value's data (not matched by name); empty if no values.
pub fn emit_shortcuts(hive: &mut Hive, out: &mut OutputRouter) -> Result<u64, TriageError> {
    let mut count = 0u64;
    let Some(mut root) = hive.get_key(r"Root\InventoryApplicationShortcut") else {
        return Ok(0);
    };
    for sub in hive.sub_keys(&mut root) {
        let lw = values::key_last_write(&sub);
        // LnkName = first value's ValueData (AmcacheNew L560-563).
        let lnk_name = sub
            .value_iter()
            .next()
            .map(|v| {
                let (c, _) = v.get_content();
                triage_registry::value::render_value_data(&c)
            })
            .unwrap_or_default();
        let rec = ShortcutRecord {
            key_name: sub.key_name.clone(),
            lnk_name,
            key_last_write: values::ts_string(lw),
        };
        out.write("shortcuts", &rec)?;
        count += 1;
    }
    Ok(count)
}

/// DriveBinaries row — 20 columns.
#[derive(Debug, Clone, Serialize)]
pub struct DriverBinaryRecord {
    #[serde(rename = "KeyName")]
    pub key_name: String,
    #[serde(rename = "KeyLastWriteTimestamp")]
    pub key_last_write: String,
    #[serde(rename = "DriverTimeStamp")]
    pub driver_time_stamp: String,
    #[serde(rename = "DriverLastWriteTime")]
    pub driver_last_write_time: String,
    #[serde(rename = "DriverName")]
    pub driver_name: String,
    #[serde(rename = "DriverInBox")]
    pub driver_in_box: String,
    #[serde(rename = "DriverIsKernelMode")]
    pub driver_is_kernel_mode: String,
    #[serde(rename = "DriverSigned")]
    pub driver_signed: String,
    #[serde(rename = "DriverCheckSum")]
    pub driver_check_sum: String,
    #[serde(rename = "DriverCompany")]
    pub driver_company: String,
    #[serde(rename = "DriverId")]
    pub driver_id: String,
    #[serde(rename = "DriverPackageStrongName")]
    pub driver_package_strong_name: String,
    #[serde(rename = "DriverType")]
    pub driver_type: String,
    #[serde(rename = "DriverVersion")]
    pub driver_version: String,
    #[serde(rename = "ImageSize")]
    pub image_size: String,
    #[serde(rename = "Inf")]
    pub inf: String,
    #[serde(rename = "Product")]
    pub product: String,
    #[serde(rename = "ProductVersion")]
    pub product_version: String,
    #[serde(rename = "Service")]
    pub service: String,
    #[serde(rename = "WdfVersion")]
    pub wdf_version: String,
}

/// Build one DriverBinaryRecord from a subkey's name, last-write + values.
pub fn build_driver_binary_record(
    key_name: String,
    lw: DateTime<Utc>,
    vm: &ValueMap,
) -> DriverBinaryRecord {
    DriverBinaryRecord {
        key_name,
        key_last_write: values::ts_string(lw),
        driver_time_stamp: values::ts_opt(values::epoch_secs_to_dt(&vm.str("DriverTimeStamp"))),
        driver_last_write_time: values::ts_opt(values::parse_dotnet_date(
            &vm.str("DriverLastWriteTime"),
        )),
        driver_name: vm.str("DriverName"),
        driver_in_box: vm.dotnet_bool("DriverInBox"),
        driver_is_kernel_mode: vm.dotnet_bool("DriverIsKernelMode"),
        driver_signed: vm.dotnet_bool("DriverSigned"),
        driver_check_sum: values::parse_int(&vm.str("DriverCheckSum")).to_string(),
        driver_company: vm.str("DriverCompany"),
        driver_id: values::strip_prefix4(&vm.str("DriverId")),
        driver_package_strong_name: vm.str("DriverPackageStrongName"),
        driver_type: vm.str("DriverType"),
        driver_version: vm.str("DriverVersion"),
        image_size: values::parse_int(&vm.str("ImageSize")).to_string(),
        inf: vm.str("Inf"),
        product: vm.str("Product"),
        product_version: vm.str("ProductVersion"),
        service: vm.str("Service"),
        wdf_version: vm.str("WdfVersion"),
    }
}

/// Walk Root\InventoryDriverBinary subkeys; emit DriveBinaries.
pub fn emit_drive_binaries(hive: &mut Hive, out: &mut OutputRouter) -> Result<u64, TriageError> {
    let mut count = 0u64;
    let Some(mut root) = hive.get_key(r"Root\InventoryDriverBinary") else {
        return Ok(0);
    };
    for sub in hive.sub_keys(&mut root) {
        let lw = values::key_last_write(&sub);
        let vm = ValueMap::from_key(&sub);
        let rec = build_driver_binary_record(sub.key_name.clone(), lw, &vm);
        out.write("drive_binaries", &rec)?;
        count += 1;
    }
    Ok(count)
}

/// DeviceContainers row — 17 columns.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceContainerRecord {
    #[serde(rename = "KeyName")]
    pub key_name: String,
    #[serde(rename = "KeyLastWriteTimestamp")]
    pub key_last_write: String,
    #[serde(rename = "Categories")]
    pub categories: String,
    #[serde(rename = "DiscoveryMethod")]
    pub discovery_method: String,
    #[serde(rename = "FriendlyName")]
    pub friendly_name: String,
    #[serde(rename = "Icon")]
    pub icon: String,
    #[serde(rename = "IsActive")]
    pub is_active: String,
    #[serde(rename = "IsConnected")]
    pub is_connected: String,
    #[serde(rename = "IsMachineContainer")]
    pub is_machine_container: String,
    #[serde(rename = "IsNetworked")]
    pub is_networked: String,
    #[serde(rename = "IsPaired")]
    pub is_paired: String,
    #[serde(rename = "Manufacturer")]
    pub manufacturer: String,
    #[serde(rename = "ModelId")]
    pub model_id: String,
    #[serde(rename = "ModelName")]
    pub model_name: String,
    #[serde(rename = "ModelNumber")]
    pub model_number: String,
    #[serde(rename = "PrimaryCategory")]
    pub primary_category: String,
    #[serde(rename = "State")]
    pub state: String,
}

/// Build one DeviceContainerRecord from a subkey's name, last-write + values.
pub fn build_device_container_record(
    key_name: String,
    lw: DateTime<Utc>,
    vm: &ValueMap,
) -> DeviceContainerRecord {
    DeviceContainerRecord {
        key_name,
        key_last_write: values::ts_string(lw),
        categories: vm.str("Categories"),
        discovery_method: vm.str("DiscoveryMethod"),
        friendly_name: vm.str("FriendlyName"),
        icon: vm.str("Icon"),
        is_active: vm.dotnet_bool("IsActive"),
        is_connected: vm.dotnet_bool("IsConnected"),
        is_machine_container: vm.dotnet_bool("IsMachineContainer"),
        is_networked: vm.dotnet_bool("IsNetworked"),
        is_paired: vm.dotnet_bool("IsPaired"),
        manufacturer: vm.str("Manufacturer"),
        model_id: vm.str("ModelId"),
        model_name: vm.str("ModelName"),
        model_number: vm.str("ModelNumber"),
        primary_category: vm.str("PrimaryCategory"),
        state: vm.str("State"),
    }
}

/// Walk Root\InventoryDeviceContainer subkeys; emit DeviceContainers.
pub fn emit_device_containers(hive: &mut Hive, out: &mut OutputRouter) -> Result<u64, TriageError> {
    let mut count = 0u64;
    let Some(mut root) = hive.get_key(r"Root\InventoryDeviceContainer") else {
        return Ok(0);
    };
    for sub in hive.sub_keys(&mut root) {
        let lw = values::key_last_write(&sub);
        let vm = ValueMap::from_key(&sub);
        let rec = build_device_container_record(sub.key_name.clone(), lw, &vm);
        out.write("device_containers", &rec)?;
        count += 1;
    }
    Ok(count)
}

/// DriverPackages row — 12 columns.
#[derive(Debug, Clone, Serialize)]
pub struct DriverPackageRecord {
    #[serde(rename = "KeyName")]
    pub key_name: String,
    #[serde(rename = "KeyLastWriteTimestamp")]
    pub key_last_write: String,
    #[serde(rename = "Date")]
    pub date: String,
    #[serde(rename = "Class")]
    pub class: String,
    #[serde(rename = "Directory")]
    pub directory: String,
    #[serde(rename = "DriverInBox")]
    pub driver_in_box: String,
    #[serde(rename = "Hwids")]
    pub hwids: String,
    #[serde(rename = "Inf")]
    pub inf: String,
    #[serde(rename = "Provider")]
    pub provider: String,
    #[serde(rename = "SubmissionId")]
    pub submission_id: String,
    #[serde(rename = "SYSFILE")]
    pub sysfile: String,
    #[serde(rename = "Version")]
    pub version: String,
}

/// Build one DriverPackageRecord from a subkey's name, last-write + values.
pub fn build_driver_package_record(
    key_name: String,
    lw: DateTime<Utc>,
    vm: &ValueMap,
) -> DriverPackageRecord {
    DriverPackageRecord {
        key_name,
        key_last_write: values::ts_string(lw),
        date: values::ts_opt(values::parse_dotnet_date(&vm.str("Date"))),
        class: vm.str("Class"),
        directory: vm.str("Directory"),
        driver_in_box: vm.dotnet_bool("DriverInBox"),
        hwids: vm.str("Hwids"),
        inf: vm.str("Inf"),
        provider: vm.str("Provider"),
        submission_id: vm.str("SubmissionId"),
        sysfile: vm.str("SYSFILE"),
        version: vm.str("Version"),
    }
}

/// Walk Root\InventoryDriverPackage subkeys; emit DriverPackages.
pub fn emit_driver_packages(hive: &mut Hive, out: &mut OutputRouter) -> Result<u64, TriageError> {
    let mut count = 0u64;
    let Some(mut root) = hive.get_key(r"Root\InventoryDriverPackage") else {
        return Ok(0);
    };
    for sub in hive.sub_keys(&mut root) {
        let lw = values::key_last_write(&sub);
        let vm = ValueMap::from_key(&sub);
        let rec = build_driver_package_record(sub.key_name.clone(), lw, &vm);
        out.write("driver_packages", &rec)?;
        count += 1;
    }
    Ok(count)
}

/// DevicePnps row — 25 columns.
#[derive(Debug, Clone, Serialize)]
pub struct DevicePnpRecord {
    #[serde(rename = "KeyName")]
    pub key_name: String,
    #[serde(rename = "KeyLastWriteTimestamp")]
    pub key_last_write: String,
    #[serde(rename = "BusReportedDescription")]
    pub bus_reported_description: String,
    #[serde(rename = "Class")]
    pub class: String,
    #[serde(rename = "ClassGuid")]
    pub class_guid: String,
    #[serde(rename = "Compid")]
    pub compid: String,
    #[serde(rename = "ContainerId")]
    pub container_id: String,
    #[serde(rename = "Description")]
    pub description: String,
    #[serde(rename = "DriverId")]
    pub driver_id: String,
    #[serde(rename = "DriverPackageStrongName")]
    pub driver_package_strong_name: String,
    #[serde(rename = "DriverName")]
    pub driver_name: String,
    #[serde(rename = "DriverVerDate")]
    pub driver_ver_date: String,
    #[serde(rename = "DriverVerVersion")]
    pub driver_ver_version: String,
    #[serde(rename = "Enumerator")]
    pub enumerator: String,
    #[serde(rename = "HWID")]
    pub hwid: String,
    #[serde(rename = "Inf")]
    pub inf: String,
    #[serde(rename = "InstallState")]
    pub install_state: String,
    #[serde(rename = "Manufacturer")]
    pub manufacturer: String,
    #[serde(rename = "MatchingId")]
    pub matching_id: String,
    #[serde(rename = "Model")]
    pub model: String,
    #[serde(rename = "ParentId")]
    pub parent_id: String,
    #[serde(rename = "ProblemCode")]
    pub problem_code: String,
    #[serde(rename = "Provider")]
    pub provider: String,
    #[serde(rename = "Service")]
    pub service: String,
    #[serde(rename = "Stackid")]
    pub stackid: String,
}

/// Build one DevicePnpRecord from a subkey's name, last-write + values.
pub fn build_device_pnp_record(
    key_name: String,
    lw: DateTime<Utc>,
    vm: &ValueMap,
) -> DevicePnpRecord {
    DevicePnpRecord {
        key_name,
        key_last_write: values::ts_string(lw),
        bus_reported_description: vm.str("BusReportedDescription"),
        class: vm.str("Class"),
        class_guid: vm.str("ClassGuid"),
        compid: vm.str("COMPID"),
        container_id: vm.str("ContainerId"),
        description: vm.str("Description"),
        driver_id: vm.str("DriverId"),
        driver_package_strong_name: vm.str("DriverPackageStrongName"),
        driver_name: vm.str("DriverName"),
        driver_ver_date: vm.str("DriverVerDate"),
        driver_ver_version: vm.str("DriverVerVersion"),
        enumerator: vm.str("Enumerator"),
        hwid: vm.str("HWID"),
        inf: vm.str("Inf"),
        install_state: vm.str("InstallState"),
        manufacturer: vm.str("Manufacturer"),
        matching_id: vm.str("MatchingID"),
        model: vm.str("Model"),
        parent_id: vm.str("ParentId"),
        problem_code: vm.str("ProblemCode"),
        provider: vm.str("Provider"),
        service: vm.str("Service"),
        stackid: vm.str("STACKID"),
    }
}

/// Walk Root\InventoryDevicePnp subkeys; emit DevicePnps.
pub fn emit_device_pnps(hive: &mut Hive, out: &mut OutputRouter) -> Result<u64, TriageError> {
    let mut count = 0u64;
    let Some(mut root) = hive.get_key(r"Root\InventoryDevicePnp") else {
        return Ok(0);
    };
    for sub in hive.sub_keys(&mut root) {
        let lw = values::key_last_write(&sub);
        let vm = ValueMap::from_key(&sub);
        let rec = build_device_pnp_record(sub.key_name.clone(), lw, &vm);
        out.write("device_pnps", &rec)?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use notatin::cell_value::CellValue;

    fn s(v: &str) -> CellValue {
        CellValue::String(v.into())
    }

    #[test]
    fn program_record_header_matches_amcacheparser() {
        let vm = ValueMap::from_pairs(vec![("ProgramId", s("abc")), ("Name", s("App"))]);
        let lw = chrono::DateTime::parse_from_rfc3339("2026-02-12T18:09:31Z")
            .unwrap()
            .with_timezone(&Utc);
        let rec = build_program_record(lw, &vm);
        let mut w = csv::Writer::from_writer(vec![]);
        w.serialize(&rec).unwrap();
        let out = String::from_utf8(w.into_inner().unwrap()).unwrap();
        let header = out.lines().next().unwrap();
        assert_eq!(header, "ProgramId,KeyLastWriteTimestamp,Name,Version,Publisher,InstallDateArpLastModified,InstallDate,InstallDateMsi,OSVersionAtInstallTime,InstallDateFromLinkFile,BundleManifestPath,HiddenArp,InboxModernApp,Language,ManifestPath,MsiPackageCode,MsiProductCode,PackageFullName,ProgramInstanceId,RegistryKeyPath,RootDirPath,Type,Source,StoreAppType,UninstallString,Manufacturer");
    }

    #[test]
    fn program_record_language_empty_becomes_zero() {
        let vm = ValueMap::from_pairs(vec![("ProgramId", s("abc"))]);
        let lw = Utc::now();
        let rec = build_program_record(lw, &vm);
        assert_eq!(rec.language, "0");
        assert_eq!(rec.hidden_arp, "False");
    }

    #[test]
    fn file_entry_header_and_association() {
        let vm = ValueMap::from_pairs(vec![
            ("ProgramId", s("PID1")),
            ("FileId", s("00066ed68cc4acf8062b33da67d0246d04d5c454cbaa")),
            ("LowerCaseLongPath", s(r"c:\x\accesschk.exe")),
            ("Name", s("accesschk.exe")),
            ("Size", s("1468320")),
        ]);
        let lw = Utc::now();
        // Unassociated (no program):
        let un = build_file_entry_record(lw, &vm, None);
        assert_eq!(un.application_name, "Unassociated");
        assert_eq!(un.sha1, "6ed68cc4acf8062b33da67d0246d04d5c454cbaa");
        assert_eq!(un.file_extension, ".exe");
        assert_eq!(un.size, "1468320");
        // Associated:
        let a = build_file_entry_record(lw, &vm, Some("My App"));
        assert_eq!(a.application_name, "My App");
        // Header:
        let mut w = csv::Writer::from_writer(vec![]);
        w.serialize(&un).unwrap();
        let out = String::from_utf8(w.into_inner().unwrap()).unwrap();
        assert_eq!(out.lines().next().unwrap(),
            "ApplicationName,ProgramId,FileKeyLastWriteTimestamp,SHA1,IsOsComponent,FullPath,Name,FileExtension,LinkDate,ProductName,Size,Version,ProductVersion,LongPathHash,BinaryType,IsPeFile,BinFileVersion,BinProductVersion,Usn,Language,Description");
    }

    #[test]
    fn file_extension_ignores_interior_dot_in_dir_segment() {
        // Interior dot in a directory segment (john.doe) must NOT count as an
        // extension on non-Windows builds where Path::extension() is fooled
        // into treating the whole backslash path as one opaque component.
        assert_eq!(
            file_extension(r"c:\Users\john.doe\AppData\Local\Temp\file", "file"),
            ""
        );
    }

    #[test]
    fn file_extension_ignores_interior_dot_with_space_segment() {
        assert_eq!(
            file_extension(r"c:\Program Files\App v1.2\bin\tool", "tool"),
            ""
        );
    }

    #[test]
    fn file_extension_finds_extension_after_last_separator() {
        assert_eq!(
            file_extension(r"c:\x\accesschk.exe", "accesschk.exe"),
            ".exe"
        );
    }

    #[test]
    fn file_extension_falls_back_to_name_when_full_path_has_none() {
        assert_eq!(file_extension(r"c:\x\noext", "thing.dll"), ".dll");
    }

    #[test]
    fn dotnet_get_extension_trailing_dot_is_empty() {
        assert_eq!(dotnet_get_extension("file."), "");
    }

    #[test]
    fn dotnet_get_extension_last_dot_wins() {
        assert_eq!(dotnet_get_extension("a.b.c"), ".c");
    }

    #[test]
    fn shortcut_header() {
        let rec = ShortcutRecord {
            key_name: "k".into(),
            lnk_name: "x.lnk".into(),
            key_last_write: "".into(),
        };
        let mut w = csv::Writer::from_writer(vec![]);
        w.serialize(&rec).unwrap();
        let out = String::from_utf8(w.into_inner().unwrap()).unwrap();
        assert_eq!(
            out.lines().next().unwrap(),
            "KeyName,LnkName,KeyLastWriteTimestamp"
        );
    }

    #[test]
    fn driver_binary_header() {
        let vm = ValueMap::from_pairs(vec![]);
        let rec = build_driver_binary_record("k".into(), Utc::now(), &vm);
        let mut w = csv::Writer::from_writer(vec![]);
        w.serialize(&rec).unwrap();
        let out = String::from_utf8(w.into_inner().unwrap()).unwrap();
        assert_eq!(out.lines().next().unwrap(),
            "KeyName,KeyLastWriteTimestamp,DriverTimeStamp,DriverLastWriteTime,DriverName,DriverInBox,DriverIsKernelMode,DriverSigned,DriverCheckSum,DriverCompany,DriverId,DriverPackageStrongName,DriverType,DriverVersion,ImageSize,Inf,Product,ProductVersion,Service,WdfVersion");
    }

    #[test]
    fn device_container_header() {
        let vm = ValueMap::from_pairs(vec![]);
        let rec = build_device_container_record("k".into(), Utc::now(), &vm);
        let mut w = csv::Writer::from_writer(vec![]);
        w.serialize(&rec).unwrap();
        let out = String::from_utf8(w.into_inner().unwrap()).unwrap();
        assert_eq!(out.lines().next().unwrap(),
            "KeyName,KeyLastWriteTimestamp,Categories,DiscoveryMethod,FriendlyName,Icon,IsActive,IsConnected,IsMachineContainer,IsNetworked,IsPaired,Manufacturer,ModelId,ModelName,ModelNumber,PrimaryCategory,State");
    }

    #[test]
    fn driver_package_header() {
        let vm = ValueMap::from_pairs(vec![]);
        let rec = build_driver_package_record("k".into(), Utc::now(), &vm);
        let mut w = csv::Writer::from_writer(vec![]);
        w.serialize(&rec).unwrap();
        let out = String::from_utf8(w.into_inner().unwrap()).unwrap();
        assert_eq!(out.lines().next().unwrap(),
            "KeyName,KeyLastWriteTimestamp,Date,Class,Directory,DriverInBox,Hwids,Inf,Provider,SubmissionId,SYSFILE,Version");
    }

    #[test]
    fn device_pnp_header_and_raw_driverid() {
        let vm = ValueMap::from_pairs(vec![("DriverId", s("machine.inf_amd64_d807fc8146278f4c"))]);
        let rec = build_device_pnp_record("k".into(), Utc::now(), &vm);
        assert_eq!(rec.driver_id, "machine.inf_amd64_d807fc8146278f4c"); // NOT stripped
        let mut w = csv::Writer::from_writer(vec![]);
        w.serialize(&rec).unwrap();
        let out = String::from_utf8(w.into_inner().unwrap()).unwrap();
        assert_eq!(out.lines().next().unwrap(),
            "KeyName,KeyLastWriteTimestamp,BusReportedDescription,Class,ClassGuid,Compid,ContainerId,Description,DriverId,DriverPackageStrongName,DriverName,DriverVerDate,DriverVerVersion,Enumerator,HWID,Inf,InstallState,Manufacturer,MatchingId,Model,ParentId,ProblemCode,Provider,Service,Stackid");
    }
}
