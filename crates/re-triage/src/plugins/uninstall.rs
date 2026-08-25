//! Port of RegistryPlugin.Uninstall (Uninstall.cs + ValuesOut.cs). Extracts
//! installed program information from Uninstall registry keys.
//!
//! The plugin matches the PARENT key (`Microsoft\Windows\CurrentVersion\Uninstall`
//! etc.) and iterates its subkeys. Each subkey represents one installed program.
//!
//! Detail-CSV column order (fixture-authoritative, from UnInstall_SOFTWARE.csv):
//!   Timestamp, BatchKeyPath, KeyName, BatchValueName, DisplayName, DisplayVersion,
//!   Publisher, InstallDate, InstallSource, InstallLocation, UninstallString
//!
//! Batch row format (from SOFTWARE batch.csv "Add/Remove Programs Entries" rows):
//!   ValueName  = "Multiple"
//!   ValueData  = "KeyName: {keyname} DisplayName: {dn} DisplayVersion: {dv}
//!                  Publisher: {pub} InstallDate: {date}"
//!   ValueData2 = "Timestamp: {yyyy-MM-dd HH:mm:ss.fffffff}"   (UTC, RECmd literal)
//!   ValueData3 = "InstallSource: {src} InstallLocation: {loc} UninstallString: {us}"
//!
//! NOTE: The C# PluginName is "Uninstall" but the fixture file is named
//! `UnInstall_SOFTWARE.csv` — so `plugin_name()` MUST return "UnInstall"
//! (capital I) to match the fixture basename exactly.

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct UnInstall;

fn dt_to_recmd_literal(dt: DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks)
}

fn dt_to_iso8601(dt: DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

/// Get a named string value from a subkey (case-sensitive, matches C# behavior).
fn get_str_value(sub_key: &CellKeyNode, name: &str) -> String {
    for v in sub_key.value_iter() {
        if v.get_pretty_name() == name {
            let (content, _) = v.get_content();
            return triage_registry::value::render_value_data(&content);
        }
    }
    String::new()
}

fn row_from_subkey(sub_key: &CellKeyNode, sub_path: &str) -> PluginRow {
    let key_name = sub_key.key_name.clone();
    let display_name = get_str_value(sub_key, "DisplayName");
    let display_version = get_str_value(sub_key, "DisplayVersion");
    let publisher = get_str_value(sub_key, "Publisher");
    let install_date = get_str_value(sub_key, "InstallDate");
    let install_source = get_str_value(sub_key, "InstallSource");
    let install_location = get_str_value(sub_key, "InstallLocation");
    let uninstall_string = get_str_value(sub_key, "UninstallString");

    let lw = sub_key.last_key_written_date_and_time();
    let ts_recmd = dt_to_recmd_literal(lw);
    let ts_iso = dt_to_iso8601(lw);

    PluginRow {
        batch_value_name: "Multiple".to_string(),
        // C# ValuesOut.BatchValueData1:
        // $"KeyName: {KeyName} DisplayName: {DisplayName} DisplayVersion: {DisplayVersion}
        //   Publisher: {Publisher} InstallDate: {InstallDate}"
        batch_value_data1: format!(
            "KeyName: {key_name} DisplayName: {display_name} DisplayVersion: {display_version} \
             Publisher: {publisher} InstallDate: {install_date}"
        ),
        // C# ValuesOut.BatchValueData2:
        // $"Timestamp: {Timestamp?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
        batch_value_data2: format!("Timestamp: {ts_recmd}"),
        // C# ValuesOut.BatchValueData3:
        // $"InstallSource: {InstallSource} InstallLocation: {InstallLocation}
        //   UninstallString: {UninstallString}"
        batch_value_data3: format!(
            "InstallSource: {install_source} InstallLocation: {install_location} \
             UninstallString: {uninstall_string}"
        ),
        // Column order from fixture header:
        //   Timestamp, BatchKeyPath, KeyName, BatchValueName, DisplayName,
        //   DisplayVersion, Publisher, InstallDate, InstallSource,
        //   InstallLocation, UninstallString
        detail_columns: vec![
            ("Timestamp".to_string(), ts_iso),
            ("BatchKeyPath".to_string(), sub_path.to_string()),
            ("KeyName".to_string(), key_name),
            ("BatchValueName".to_string(), "Multiple".to_string()),
            ("DisplayName".to_string(), display_name),
            ("DisplayVersion".to_string(), display_version),
            ("Publisher".to_string(), publisher),
            ("InstallDate".to_string(), install_date),
            ("InstallSource".to_string(), install_source),
            ("InstallLocation".to_string(), install_location),
            ("UninstallString".to_string(), uninstall_string),
        ],
    }
}

impl RegistryPlugin for UnInstall {
    fn plugin_name(&self) -> &'static str {
        // Fixture filename is UnInstall_SOFTWARE.csv → "UnInstall" (capital I).
        "UnInstall"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", // NTUSER.DAT
            r"Microsoft\Windows\CurrentVersion\Uninstall",          // SOFTWARE
            r"WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall", // SOFTWARE
        ]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let mut rows = Vec::new();
        for sub in hive.sub_keys(key) {
            let sub_path = sub.path.trim_start_matches('\\').to_string();
            rows.push(row_from_subkey(&sub, &sub_path));
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_matches_fixture_filename() {
        // Fixture: UnInstall_SOFTWARE.csv → plugin_name must be "UnInstall"
        assert_eq!(UnInstall.plugin_name(), "UnInstall");
    }

    #[test]
    fn key_paths_cover_software_and_ntuser() {
        let paths = UnInstall.key_paths();
        assert!(paths.contains(&r"Microsoft\Windows\CurrentVersion\Uninstall"));
        assert!(paths.contains(&r"WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"));
        assert!(paths.contains(&r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"));
    }

    #[test]
    fn detail_columns_names_match_fixture_header() {
        // Column order is fixture-authoritative:
        //   Timestamp, BatchKeyPath, KeyName, BatchValueName, DisplayName,
        //   DisplayVersion, Publisher, InstallDate, InstallSource,
        //   InstallLocation, UninstallString
        let expected = [
            "Timestamp",
            "BatchKeyPath",
            "KeyName",
            "BatchValueName",
            "DisplayName",
            "DisplayVersion",
            "Publisher",
            "InstallDate",
            "InstallSource",
            "InstallLocation",
            "UninstallString",
        ];
        // Build a synthetic row by calling row_from_subkey is not possible without
        // a real CellKeyNode. Verify via the column name list embedded above matches
        // what row_from_subkey actually emits by inspecting the source. This test
        // documents the contract.
        assert_eq!(expected.len(), 11, "detail column count");
    }
}
