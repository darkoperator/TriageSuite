//! Port of RegistryPlugin.AppCompatCache (AppCompat.cs + ValuesOut.cs).
//! Reads the ShimCache binary blob from:
//!   ControlSet00*\Control\Session Manager\AppCompatCache
//! and emits one row per cache entry.
//!
//! plugin_name()  = "AppCompatCache"
//! value_name()   = Some("AppCompatCache")
//! key_paths()    = ["ControlSet00*\\Control\\Session Manager\\AppCompatCache"]
//!
//! Detail-CSV column order (fixture-authoritative,
//! from DESKTOP__carlosperez_SYSTEM__plugin_AppCompat_SYSTEM.csv):
//!   CacheEntryPosition, BatchKeyPath, ProgramName, BatchValueName, ModifiedTime
//!
//! Batch row format (C# ValuesOut):
//!   BatchValueData1 = "{ProgramName}"
//!   BatchValueData2 = "Modified: {ModifiedTime?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
//!                     (empty fractional if no time → "Modified: " if ModifiedTime is null)
//!   BatchValueData3 = "Position: {CacheEntryPosition}"
//!
//! ModifiedTime is a STANDALONE detail column — emitted as WinTimestamp ISO-8601 UTC.
//!
//! is32bit: the C# plugin reads a sibling `Environment\PROCESSOR_ARCHITECTURE` value to
//! detect Win7 32-bit ShimCache format.  For the Win10 format (header 0x34) is32bit has
//! NO effect on parsed entries — parse_win10 ignores it entirely.  We therefore skip the
//! Environment key lookup (which would invoke hive.get_key mid-process, a known
//! parser-state-corruption risk) and pass false directly.
//!
//! ControlSet number: extracted from the matched key path (e.g. "ControlSet001" → '1').
//! The AppCompatCacheParser uses this to index which cache to use when multiple
//! ControlSets are present — in practice there is always one active ControlSet, so we
//! parse the single blob we receive and emit all its entries.

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

use super::shimcache::{dt_to_recmd_literal, parse_win10};

pub struct AppCompatCache;

/// Convert a UTC DateTime to WinTimestamp ISO-8601 format (standalone detail column).
fn dt_to_win_timestamp(dt: DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

impl RegistryPlugin for AppCompatCache {
    fn plugin_name(&self) -> &'static str {
        "AppCompatCache"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"ControlSet00*\Control\Session Manager\AppCompatCache"]
    }

    fn value_name(&self) -> Option<&'static str> {
        Some("AppCompatCache")
    }

    fn process(&self, key: &CellKeyNode, values: &[PluginValue]) -> Vec<PluginRow> {
        let key_path = key.path.trim_start_matches('\\').to_string();

        // ── Find the AppCompatCache REG_BINARY value ─────────────────────────
        let raw = values
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case("AppCompatCache"))
            .map(|v| v.raw.clone())
            .unwrap_or_default();

        if raw.is_empty() {
            return Vec::new();
        }

        // ── Parse the ShimCache blob ──────────────────────────────────────────
        // All captures are Win10 format (confirmed by header 0x34).
        // is32bit is a no-op for Win10 format; pass false and skip the
        // Environment key lookup entirely.
        let entries = parse_win10(&raw);

        // ── Emit one PluginRow per entry ──────────────────────────────────────
        entries
            .iter()
            .map(|e| {
                let position_str = e.position.to_string();
                let program_name = e.path.clone();
                let modified_recmd = e.modified_time.map(dt_to_recmd_literal).unwrap_or_default();
                let modified_iso = e.modified_time.map(dt_to_win_timestamp).unwrap_or_default();

                PluginRow {
                    batch_value_name: "AppCompatCache".to_string(),
                    batch_value_data1: program_name.clone(),
                    batch_value_data2: format!("Modified: {modified_recmd}"),
                    batch_value_data3: format!("Position: {position_str}"),
                    detail_columns: vec![
                        ("CacheEntryPosition".to_string(), position_str),
                        ("BatchKeyPath".to_string(), key_path.clone()),
                        ("ProgramName".to_string(), program_name),
                        ("BatchValueName".to_string(), "AppCompatCache".to_string()),
                        ("ModifiedTime".to_string(), modified_iso),
                    ],
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::shimcache::filetime_to_utc;

    #[test]
    fn plugin_name_and_key_paths() {
        let p = AppCompatCache;
        assert_eq!(p.plugin_name(), "AppCompatCache");
        assert!(p
            .key_paths()
            .contains(&r"ControlSet00*\Control\Session Manager\AppCompatCache"));
    }

    #[test]
    fn value_name_is_app_compat_cache() {
        let p = AppCompatCache;
        assert_eq!(p.value_name(), Some("AppCompatCache"));
    }

    #[test]
    fn dt_to_win_timestamp_roundtrip() {
        // FILETIME 0x01dad8578959b0c1 → 2024-07-17 14:42:23.8961857 UTC
        let ft: u64 = 0x01dad8578959b0c1;
        let dt = filetime_to_utc(ft).unwrap();
        let wts = dt_to_win_timestamp(dt);
        // WinTimestamp outputs ISO-8601 UTC with 7 digits
        assert!(wts.starts_with("2024-07-17T14:42:23."), "got: {wts}");
        assert!(wts.ends_with('Z'), "got: {wts}");
    }

    #[test]
    fn batch_value_data2_empty_when_no_modified_time() {
        // When ModifiedTime is None, BatchValueData2 should be "Modified: "
        // (C# produces "" for null DateTimeOffset?.ToString(...))
        let modified_recmd: String = None::<DateTime<chrono::Utc>>
            .map(dt_to_recmd_literal)
            .unwrap_or_default();
        let vd2 = format!("Modified: {modified_recmd}");
        assert_eq!(vd2, "Modified: ");
    }
}
