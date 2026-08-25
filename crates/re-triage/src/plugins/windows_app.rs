//! Port of RegistryPlugin.WindowsApp (WindowsApp.cs + ValuesOut.cs).
//! Lists installed Windows (UWP/AppX) packages from the AppModel Repository.
//!
//! Fires on UsrClass.dat at:
//!   `Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository`
//! and on NTUSER.dat at:
//!   `Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository`
//!
//! plugin_name() = "Windows App" (matches C# PluginName; note the space).
//!
//! Detail-CSV column order (fixture-authoritative, from
//! `STCL1__administrator_UsrClass__plugin_WindowsApp_UsrClass.dat.csv`):
//!   KeyName, BatchKeyPath, DisplayName, BatchValueName, InstallTime, PackageRootFolder
//!
//! Batch row format (C# ValuesOut):
//!   ValueData1 = "KeyName: {KeyName} DisplayName: {DisplayName} PackageRootFolder: {PackageRootFolder}"
//!   ValueData2 = "InstallTime: {InstallTime?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
//!   ValueData3 = ""
//!
//! Design: iterate `Families` subkeys → each Family's subkeys (one per package).
//!   Each package key has an `InstallTime` value (a REG_SZ decimal string
//!   representing a Windows FILETIME). Cross-reference with the `Packages`
//!   subkey for `DisplayName` and `PackageRootFolder` values.
//!
//! The `InstallTime` column is a STANDALONE timestamp. The testkit normalizes
//! it from RECmd's "yyyy-MM-dd HH:mm:ss.fffffff" to ISO-8601 UTC. We emit
//! ISO-8601 UTC directly — no AcceptedDelta needed for InstallTime.
//!
//! The embedded "InstallTime: ..." in ValueData2 is a FREE-TEXT field (not
//! normalized by the testkit). We emit RECmd's literal format there.

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct WindowsApp;

// ─── Timestamp helpers ────────────────────────────────────────────────────────

/// Convert a Windows FILETIME (100-ns ticks since 1601-01-01 UTC) to UTC DateTime.
fn filetime_to_datetime(ft: i64) -> Option<DateTime<chrono::Utc>> {
    const FILETIME_TO_UNIX_SECS: i64 = 11_644_473_600;
    let secs = (ft / 10_000_000) - FILETIME_TO_UNIX_SECS;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    DateTime::from_timestamp(secs, nanos)
}

/// Parse the `InstallTime` string value (a decimal integer FILETIME) and convert
/// to UTC DateTime. Mirrors C# `GetDateTimeOffset`:
///   `DateTime.FromFileTime(Convert.ToInt64(timestamp)).ToUniversalTime()`.
fn parse_install_time(s: &str) -> Option<DateTime<chrono::Utc>> {
    let ft: i64 = s.trim().parse().ok()?;
    filetime_to_datetime(ft)
}

/// Format `DateTime<Utc>` as RECmd's literal "yyyy-MM-dd HH:mm:ss.fffffff".
/// Used in the embedded free-text "InstallTime: ..." ValueData2 field.
fn dt_to_recmd_literal(dt: DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{ticks:07}", dt.format("%Y-%m-%d %H:%M:%S"))
}

/// Format `DateTime<Utc>` as ISO-8601 UTC with 7 fractional digits.
/// Used for the standalone `InstallTime` detail column (auto-normalized).
fn dt_to_iso8601(dt: DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

// ─── RegistryPlugin impl ──────────────────────────────────────────────────────

impl RegistryPlugin for WindowsApp {
    fn plugin_name(&self) -> &'static str {
        "Windows App"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[
            // UsrClass.dat path
            r"Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository",
            // NTUSER.dat path
            r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository",
        ]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        // Locate `Families` and `Packages` subkeys of the matched Repository key.
        // C# uses LINQ .SingleOrDefault(t => t.KeyName == "Families").
        // We iterate sub_keys (safe inside process_with_hive) and collect.
        let mut families_node: Option<CellKeyNode> = None;
        let mut packages_node: Option<CellKeyNode> = None;

        for sub in hive.sub_keys(key) {
            match sub.key_name.as_str() {
                "Families" => families_node = Some(sub),
                "Packages" => packages_node = Some(sub),
                _ => {}
            }
        }

        let mut families = match families_node {
            Some(n) => n,
            None => return Vec::new(),
        };
        let mut packages = match packages_node {
            Some(n) => n,
            None => return Vec::new(),
        };

        // Pre-collect the Packages subkeys so we can look up by KeyName.
        // C# does: packagesKey.SubKeys.SingleOrDefault(t => t.KeyName == AppPath.KeyName)
        // We build a HashMap<key_name, (display_name, package_root_folder)>.
        let mut packages_map: std::collections::HashMap<String, (Option<String>, Option<String>)> =
            std::collections::HashMap::new();

        for pkg in hive.sub_keys(&mut packages) {
            let pkg_name = pkg.key_name.clone();
            let mut display_name: Option<String> = None;
            let mut package_root_folder: Option<String> = None;
            for v in pkg.value_iter() {
                match v.get_pretty_name().as_str() {
                    "DisplayName" => {
                        let (content, _) = v.get_content();
                        display_name = Some(triage_registry::value::render_value_data(&content));
                    }
                    "PackageRootFolder" => {
                        let (content, _) = v.get_content();
                        package_root_folder =
                            Some(triage_registry::value::render_value_data(&content));
                    }
                    _ => {}
                }
            }
            packages_map.insert(pkg_name, (display_name, package_root_folder));
        }

        // Now iterate Families → each family's subkeys (app package keys).
        // C# code:
        //   foreach (var subKey in familiesKey.SubKeys)
        //     foreach (var AppPath in subKey.SubKeys)
        //       var installTime = GetDateTimeOffset(AppPath.Values.SingleOrDefault(...))
        //       var packagesPath = packagesKey.SubKeys.SingleOrDefault(...)
        //       ...
        let mut rows = Vec::new();

        for mut family in hive.sub_keys(&mut families) {
            let family_key_path = family.path.trim_start_matches('\\').to_string();

            for app_path in hive.sub_keys(&mut family) {
                let app_key_name = app_path.key_name.clone();

                // Read InstallTime from this app subkey's values.
                let mut install_time_str: Option<String> = None;
                for v in app_path.value_iter() {
                    if v.get_pretty_name() == "InstallTime" {
                        let (content, _) = v.get_content();
                        install_time_str =
                            Some(triage_registry::value::render_value_data(&content));
                        break;
                    }
                }

                // Parse InstallTime as a filetime integer string.
                let install_time: Option<DateTime<chrono::Utc>> =
                    install_time_str.as_deref().and_then(parse_install_time);

                // Look up DisplayName and PackageRootFolder from the Packages map.
                let (display_name, package_root_folder) = packages_map
                    .get(&app_key_name)
                    .cloned()
                    .unwrap_or((None, None));

                let install_time_recmd = install_time.map(dt_to_recmd_literal).unwrap_or_default();
                let install_time_iso = install_time.map(dt_to_iso8601).unwrap_or_default();

                let dn = display_name.unwrap_or_default();
                let prf = package_root_folder.unwrap_or_default();

                rows.push(PluginRow {
                    batch_value_name: "Multiple".to_string(),
                    batch_value_data1: format!(
                        "KeyName: {app_key_name} DisplayName: {dn} PackageRootFolder: {prf}"
                    ),
                    batch_value_data2: format!("InstallTime: {install_time_recmd}"),
                    batch_value_data3: String::new(),
                    detail_columns: vec![
                        ("KeyName".to_string(), app_key_name),
                        ("BatchKeyPath".to_string(), family_key_path.clone()),
                        ("DisplayName".to_string(), dn),
                        ("BatchValueName".to_string(), "Multiple".to_string()),
                        ("InstallTime".to_string(), install_time_iso),
                        ("PackageRootFolder".to_string(), prf),
                    ],
                });
            }
        }

        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_install_time_known() {
        // FILETIME for 2024-04-16 18:26:14.0416864 UTC.
        // unix_secs = 1713291974
        // ft = (1713291974 + 11644473600) * 10_000_000 + 416864 = 133577655740416864
        let ft_str = "133577655740416864";
        let dt = parse_install_time(ft_str).expect("should parse");
        assert_eq!(
            dt.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2024-04-16 18:26:14"
        );
    }

    #[test]
    fn parse_install_time_invalid() {
        assert!(parse_install_time("").is_none());
        assert!(parse_install_time("not-a-number").is_none());
    }

    #[test]
    fn dt_to_recmd_literal_format() {
        // 2024-04-16 18:26:14.0416864
        let ft: i64 = 133_577_655_740_416_864;
        let dt = filetime_to_datetime(ft).unwrap();
        let s = dt_to_recmd_literal(dt);
        assert_eq!(s, "2024-04-16 18:26:14.0416864");
    }

    #[test]
    fn plugin_name_and_key_paths() {
        let p = WindowsApp;
        assert_eq!(p.plugin_name(), "Windows App");
        let kps = p.key_paths();
        assert!(kps.contains(
            &r"Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository"
        ));
        assert!(kps.contains(
            &r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository"
        ));
    }
}
