//! SearchRecord: the CSV row for search mode output.
//! Mirrors the fields RECmd logs to console for --sk/--sv/--sd hits.
//!
//! AcceptedDelta: RECmd search mode (--sk/--sv/--sd) does NOT write CSV output;
//! it writes only to console/log. RETriage search mode is therefore a pure
//! RETriage extension. The column layout is chosen to mirror what RECmd prints
//! to stdout, not any RECmd-prescribed CSV schema.

use serde::Serialize;

/// One search hit row in the search-mode CSV output.
/// Column order matches what would be useful for analysis (no RECmd reference
/// since RECmd search mode does not write CSV — AcceptedDelta: search CSV is
/// a RETriage extension, not RECmd-prescribed).
#[derive(Debug, Clone, Serialize)]
pub struct SearchRecord {
    /// The hive file path
    #[serde(rename = "HivePath")]
    pub hive_path: String,
    /// Type of hit: KeyName, ValueName, or ValueData
    #[serde(rename = "HitType")]
    pub hit_type: String,
    /// Full key path (root-stripped, backslash-separated)
    #[serde(rename = "KeyPath")]
    pub key_path: String,
    /// Value name (empty for KeyName hits)
    #[serde(rename = "ValueName")]
    pub value_name: String,
    /// Value data (empty for KeyName/ValueName hits)
    #[serde(rename = "ValueData")]
    pub value_data: String,
    /// Whether the key/value is deleted
    #[serde(rename = "Deleted")]
    pub deleted: bool,
    /// Last write timestamp (ISO-8601 UTC)
    #[serde(rename = "LastWriteTimestamp")]
    pub last_write_timestamp: String,
}
