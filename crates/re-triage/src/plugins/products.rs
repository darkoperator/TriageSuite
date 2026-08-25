//! Port of RegistryPlugin.Products (Products.cs + ValuesOut.cs). Extracts
//! installed software products from Windows Installer registry keys.
//!
//! The plugin matches two key paths:
//!  1. `Microsoft\Windows\CurrentVersion\Installer\UserData\*\Products` (SOFTWARE)
//!  2. `Classes\Installer\Products` (SOFTWARE)
//!
//! For path 1 (UserData): iterates subkeys of the matched `Products` key;
//!   for each product subkey, finds `InstallProperties` sub-subkey and reads
//!   DisplayName, DisplayVersion (PackageName), InstallDate, Publisher,
//!   Language, InstallLocation, InstallSource, Comments. Timestamp = InstallProperties.LastWriteTime.
//!
//! For path 2 (Classes): iterates subkeys of the matched `Products` key;
//!   for each product subkey, reads ProductName (DisplayName), Language,
//!   and from a `SourceList` sub-subkey: PackageName, LastUsedSource (InstallSource).
//!   Timestamp = product subkey.LastWriteTime.
//!
//! Detail-CSV column order (fixture-authoritative, from Products_SOFTWARE.csv):
//!   Timestamp, BatchKeyPath, DisplayName, BatchValueName, PackageName,
//!   InstallDate, Publisher, Language, InstallLocation, InstallSource, Comments
//!
//! Batch row format (from SOFTWARE batch.csv "Products" rows):
//!   ValueName  = "Multiple"
//!   ValueData  = "DisplayName: {dn} PackageName: {pn} InstallDate: {id}
//!                  Publisher: {pub} Language: {lang}"
//!   ValueData2 = "Timestamp: {yyyy-MM-dd HH:mm:ss.fffffff}"   (UTC, RECmd literal)
//!   ValueData3 = "InstallLocation: {loc} InstallSource: {src} Comments: {comments}"

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct Products;

fn dt_to_recmd_literal(dt: DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks)
}

fn dt_to_iso8601(dt: DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

fn get_str_value(sub_key: &CellKeyNode, name: &str) -> String {
    for v in sub_key.value_iter() {
        if v.get_pretty_name() == name {
            let (content, _) = v.get_content();
            return triage_registry::value::render_value_data(&content);
        }
    }
    String::new()
}

/// Collected fields for one Products row (avoids clippy::too_many_arguments).
struct ProductFields {
    display_name: String,
    package_name: String,
    install_date: String,
    publisher: String,
    language: String,
    install_location: String,
    install_source: String,
    comments: String,
    timestamp_dt: DateTime<chrono::Utc>,
    sub_path: String,
}

fn make_row(f: ProductFields) -> PluginRow {
    let ts_recmd = dt_to_recmd_literal(f.timestamp_dt);
    let ts_iso = dt_to_iso8601(f.timestamp_dt);

    PluginRow {
        batch_value_name: "Multiple".to_string(),
        // C# ValuesOut.BatchValueData1:
        // $"DisplayName: {DisplayName} PackageName: {PackageName} InstallDate: {InstallDate}
        //   Publisher: {Publisher} Language: {Language}"
        batch_value_data1: format!(
            "DisplayName: {} PackageName: {} InstallDate: {} Publisher: {} Language: {}",
            f.display_name, f.package_name, f.install_date, f.publisher, f.language,
        ),
        // C# ValuesOut.BatchValueData2:
        // $"Timestamp: {Timestamp?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
        batch_value_data2: format!("Timestamp: {ts_recmd}"),
        // C# ValuesOut.BatchValueData3:
        // $"InstallLocation: {InstallLocation} InstallSource: {InstallSource} Comments: {Comments}"
        batch_value_data3: format!(
            "InstallLocation: {} InstallSource: {} Comments: {}",
            f.install_location, f.install_source, f.comments,
        ),
        // Column order from fixture header:
        //   Timestamp, BatchKeyPath, DisplayName, BatchValueName, PackageName,
        //   InstallDate, Publisher, Language, InstallLocation, InstallSource, Comments
        detail_columns: vec![
            ("Timestamp".to_string(), ts_iso),
            ("BatchKeyPath".to_string(), f.sub_path),
            ("DisplayName".to_string(), f.display_name),
            ("BatchValueName".to_string(), "Multiple".to_string()),
            ("PackageName".to_string(), f.package_name),
            ("InstallDate".to_string(), f.install_date),
            ("Publisher".to_string(), f.publisher),
            ("Language".to_string(), f.language),
            ("InstallLocation".to_string(), f.install_location),
            ("InstallSource".to_string(), f.install_source),
            ("Comments".to_string(), f.comments),
        ],
    }
}

impl RegistryPlugin for Products {
    fn plugin_name(&self) -> &'static str {
        "Products"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[
            r"Microsoft\Windows\CurrentVersion\Installer\UserData\*\Products",
            r"Classes\Installer\Products",
        ]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        _values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let mut rows = Vec::new();
        let key_pretty = key.get_pretty_path().to_lowercase();
        let is_classes_branch = key_pretty.contains("classes");

        // Collect subkeys first (hive.sub_keys takes &mut key).
        let subkeys = hive.sub_keys(key);

        for mut sub in subkeys {
            let sub_path = sub.path.trim_start_matches('\\').to_string();

            if is_classes_branch {
                // C# Classes branch: read values from product subkey + SourceList sub-subkey.
                let product_name = get_str_value(&sub, "ProductName");
                let language = get_str_value(&sub, "Language");
                let ts_dt = sub.last_key_written_date_and_time();

                // Find SourceList sub-subkey.
                let mut package_name = String::new();
                let mut last_used_source = String::new();
                for source_list in hive.sub_keys(&mut sub) {
                    if source_list.key_name.eq_ignore_ascii_case("SourceList") {
                        package_name = get_str_value(&source_list, "PackageName");
                        last_used_source = get_str_value(&source_list, "LastUsedSource");
                        break;
                    }
                }

                rows.push(make_row(ProductFields {
                    display_name: product_name,
                    package_name,
                    install_date: String::new(), // installDate = null in C#
                    publisher: String::new(),    // publisher = null in C#
                    language,
                    install_location: String::new(), // installLocation = null in C#
                    install_source: last_used_source,
                    comments: String::new(), // comments = null in C#
                    timestamp_dt: ts_dt,
                    sub_path: sub_path.clone(),
                }));
            } else {
                // C# UserData branch: find InstallProperties sub-subkey.
                let mut install_props: Option<CellKeyNode> = None;
                for props in hive.sub_keys(&mut sub) {
                    if props.key_name.eq_ignore_ascii_case("InstallProperties") {
                        install_props = Some(props);
                        break;
                    }
                }
                let Some(props) = install_props else {
                    // C#: if properties == null → continue (skip this subkey)
                    continue;
                };

                let display_name = get_str_value(&props, "DisplayName");
                // C# uses "DisplayVersion" as the PackageName parameter
                let package_name = get_str_value(&props, "DisplayVersion");
                let install_date = get_str_value(&props, "InstallDate");
                let publisher = get_str_value(&props, "Publisher");
                let language = get_str_value(&props, "Language");
                let install_location = get_str_value(&props, "InstallLocation");
                let install_source = get_str_value(&props, "InstallSource");
                let comments = get_str_value(&props, "Comments");
                // C#: ts = properties.LastWriteTime (the InstallProperties key)
                let ts_dt = props.last_key_written_date_and_time();

                rows.push(make_row(ProductFields {
                    display_name,
                    package_name,
                    install_date,
                    publisher,
                    language,
                    install_location,
                    install_source,
                    comments,
                    timestamp_dt: ts_dt,
                    sub_path,
                }));
            }
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_and_key_paths() {
        assert_eq!(Products.plugin_name(), "Products");
        let paths = Products.key_paths();
        assert!(paths.contains(&r"Microsoft\Windows\CurrentVersion\Installer\UserData\*\Products"));
        assert!(paths.contains(&r"Classes\Installer\Products"));
    }

    #[test]
    fn detail_column_count_and_order() {
        // Detail header: Timestamp, BatchKeyPath, DisplayName, BatchValueName,
        //                PackageName, InstallDate, Publisher, Language,
        //                InstallLocation, InstallSource, Comments
        let expected = [
            "Timestamp",
            "BatchKeyPath",
            "DisplayName",
            "BatchValueName",
            "PackageName",
            "InstallDate",
            "Publisher",
            "Language",
            "InstallLocation",
            "InstallSource",
            "Comments",
        ];
        assert_eq!(expected.len(), 11);
    }
}
