//! Parse RECmd `.reb` batch files (YAML). Schema = RECmd-master/RECmd/ReBatch.cs.
//! DFIRBatch.reb (518 entries) is embedded and parsed at startup.

use serde::Deserialize;
use triage_registry::hivetype::HiveType;
use triage_registry::value::BinaryConvert;

/// The embedded default batch (RECmd BatchExamples/DFIRBatch.reb).
pub const DFIR_BATCH: &str = include_str!("../../../resources/registry/DFIRBatch.reb");

/// Raw YAML shape of a `.reb` file.
#[derive(Debug, Deserialize)]
pub struct RebFile {
    #[serde(rename = "Description")]
    pub description: String,
    #[serde(rename = "Author")]
    pub author: String,
    #[serde(rename = "Version")]
    pub version: serde_yaml::Value,
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Keys")]
    pub keys: Vec<RebKeyRaw>,
}

/// Raw YAML shape of one batch entry (optional fields default).
#[derive(Debug, Deserialize)]
pub struct RebKeyRaw {
    #[serde(rename = "Description")]
    pub description: String,
    #[serde(rename = "HiveType")]
    pub hive_type: String,
    #[serde(rename = "Category", default)]
    pub category: String,
    #[serde(rename = "KeyPath")]
    pub key_path: String,
    #[serde(rename = "ValueName", default)]
    pub value_name: Option<String>,
    #[serde(rename = "Recursive", default)]
    pub recursive: bool,
    #[serde(rename = "DisablePlugin", default)]
    pub disable_plugin: bool,
    #[serde(rename = "IncludeBinary", default)]
    pub include_binary: bool,
    #[serde(rename = "BinaryConvert", default)]
    pub binary_convert: Option<String>,
    #[serde(rename = "Comment", default)]
    pub comment: Option<String>,
}

/// A resolved batch entry with enums parsed.
#[derive(Debug, Clone)]
pub struct KeyEntry {
    pub description: String,
    pub hive_type: HiveType,
    pub category: String,
    pub key_path: String,
    pub value_name: Option<String>,
    pub recursive: bool,
    pub disable_plugin: bool,
    pub include_binary: bool,
    pub binary_convert: BinaryConvert,
    pub comment: String,
}

/// Parse a `.reb` YAML string into resolved entries.
pub fn parse_reb(yaml: &str) -> Result<Vec<KeyEntry>, String> {
    let file: RebFile = serde_yaml::from_str(yaml).map_err(|e| format!("{e}"))?;
    Ok(file
        .keys
        .into_iter()
        .map(|k| KeyEntry {
            description: k.description,
            hive_type: HiveType::from_reb(&k.hive_type),
            category: k.category,
            key_path: k.key_path,
            value_name: k.value_name.filter(|v| !v.is_empty()),
            recursive: k.recursive,
            disable_plugin: k.disable_plugin,
            include_binary: k.include_binary,
            binary_convert: k
                .binary_convert
                .as_deref()
                .map(BinaryConvert::from_reb)
                .unwrap_or(BinaryConvert::None),
            comment: k.comment.unwrap_or_default(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dfir_batch_parses_all_entries() {
        let entries = parse_reb(DFIR_BATCH).expect("parse DFIRBatch");
        // The file contains 516 parseable YAML entries (some entries are commented out
        // with '#' in the file, e.g. around line 3826-3830, reducing the count from
        // the nominally-referenced 518). Python yaml and serde_yaml both agree on 516.
        assert_eq!(entries.len(), 516, "DFIRBatch entry count");
        // Spot-check the WinLogon entry from the file.
        assert!(entries.iter().any(|e| e.hive_type == HiveType::Software
            && e.key_path == r"Microsoft\Windows NT\CurrentVersion\WinLogon"));
        // A ValueName-bearing entry resolves to Some.
        assert!(entries.iter().any(|e| e.value_name.is_some()));
    }
}
