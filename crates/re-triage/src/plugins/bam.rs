//! Port of RegistryPlugin.Bam (BamDam.cs + ValuesOut.cs). Extracts program
//! execution times from bam/dam UserSettings keys (8-byte FILETIME per value).
//!
//! Detail-CSV column order (fixture-authoritative, from BamDam_SYSTEM.csv):
//!   Program, BatchKeyPath, ExecutionTime, BatchValueName
//!
//! Batch row format (from SYSTEM batch.csv "Background Activity Moderator" rows):
//!   ValueData  = "Program: <name>"
//!   ValueData2 = "Execution time: <yyyy-MM-dd HH:mm:ss.fffffff>"   (UTC)
//!   ValueData3 = ""

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct BamDam;

/// FILETIME (100-nanosecond intervals since 1601-01-01 UTC) → RECmd's literal
/// `yyyy-MM-dd HH:mm:ss.fffffff` format in UTC.
///
/// The compat harness does NOT normalize timestamps embedded inside free-text
/// columns (ValueData/ValueData2/detail cells) — only standalone timestamp
/// columns are normalized. The embedded form must therefore match RECmd's
/// `.ToString("yyyy-MM-dd HH:mm:ss.fffffff")` output byte-for-byte.
///
/// RECmd's BamDam uses `ExecutionTime.ToUniversalTime()` in ValuesOut.cs,
/// so the timestamp is UTC (confirmed by fixture cross-check).
fn filetime_to_recmd_literal(ft: u64) -> Option<String> {
    // FILETIME epoch is 1601-01-01; Unix epoch is 1970-01-01.
    // Difference = 11644473600 seconds.
    let secs = (ft / 10_000_000) as i64 - 11_644_473_600;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    let dt: DateTime<chrono::Utc> = DateTime::from_timestamp(secs, nanos)?;
    // C# "yyyy-MM-dd HH:mm:ss.fffffff": 7 fractional-second digits at 100ns
    // resolution (.fffffff in C# = 100-nanosecond ticks).
    // chrono's %.7f specifier panics (unsupported in chrono 0.4); instead we
    // extract subsecond nanoseconds, convert to 100ns ticks, and zero-pad to
    // 7 digits. This exactly matches C#'s fffffff output.
    let ticks = dt.timestamp_subsec_nanos() / 100;
    Some(format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks))
}

impl BamDam {
    /// Core logic: build PluginRows from a value list, independent of the
    /// CellKeyNode (unit-testable without a live parser handle).
    ///
    /// `key_path` is the key's `path` field as stored by notatin (may include a
    /// leading backslash, e.g. `\ROOT\ControlSet001\...`). We forward it as-is
    /// to `BatchKeyPath`; the fixture shows RECmd stores the full path including
    /// root prefix (e.g. `ROOT\ControlSet001\...`), so the caller should pass
    /// `key.path.trim_start_matches('\\')` — but we store whatever we receive
    /// so the caller controls stripping, matching the batch-record convention.
    pub fn rows_from_values(&self, key_path: &str, values: &[PluginValue]) -> Vec<PluginRow> {
        let mut rows = Vec::new();
        for v in values {
            // Skip the two metadata values RECmd's BamDam.cs skips.
            if v.name == "Version" || v.name == "SequenceNumber" {
                continue;
            }
            // Need at least 8 bytes for a FILETIME.
            let Some(bytes) = v.raw.get(0..8) else {
                continue;
            };
            let ft = u64::from_le_bytes(bytes.try_into().unwrap());
            let Some(ts) = filetime_to_recmd_literal(ft) else {
                continue;
            };
            rows.push(PluginRow {
                batch_value_name: v.name.clone(),
                batch_value_data1: format!("Program: {}", v.name),
                batch_value_data2: format!("Execution time: {ts}"),
                batch_value_data3: String::new(),
                // Column order matches the fixture header exactly:
                //   Program, BatchKeyPath, ExecutionTime, BatchValueName
                detail_columns: vec![
                    ("Program".to_string(), v.name.clone()),
                    ("BatchKeyPath".to_string(), key_path.to_string()),
                    ("ExecutionTime".to_string(), ts),
                    ("BatchValueName".to_string(), v.name.clone()),
                ],
            });
        }
        rows
    }
}

impl RegistryPlugin for BamDam {
    fn plugin_name(&self) -> &'static str {
        "BamDam"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[
            r"ControlSet00*\Services\bam\UserSettings\*",
            r"ControlSet00*\Services\dam\UserSettings\*",
            r"ControlSet00*\Services\bam\State\UserSettings\*",
        ]
    }

    fn process(&self, key: &CellKeyNode, values: &[PluginValue]) -> Vec<PluginRow> {
        // Strip the leading backslash notatin prepends so BatchKeyPath matches
        // the fixture (fixture has "ROOT\ControlSet001\..." not "\ROOT\...").
        let key_path = key.path.trim_start_matches('\\');
        self.rows_from_values(key_path, values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notatin::cell_key_value::CellKeyValueDataTypes;

    fn val(name: &str, ft: u64) -> PluginValue {
        PluginValue {
            name: name.into(),
            raw: ft.to_le_bytes().to_vec(),
            value_data: String::new(),
            data_type: CellKeyValueDataTypes::REG_BIN,
        }
    }

    #[test]
    fn skips_version_and_sequencenumber() {
        let bam = BamDam;
        let vals = vec![
            val("Version", 0),
            val("SequenceNumber", 0),
            val(r"\Device\HarddiskVolume2\app.exe", 0x01CA_0451_AD6E_4000),
        ];
        let rows = bam.rows_from_values(
            r"ControlSet001\Services\bam\State\UserSettings\S-1-5-21",
            &vals,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].batch_value_name, r"\Device\HarddiskVolume2\app.exe");
        assert!(
            rows[0].batch_value_data1.starts_with("Program: "),
            "got {:?}",
            rows[0].batch_value_data1
        );
        assert!(
            rows[0]
                .batch_value_data2
                .starts_with("Execution time: 2009-07-14"),
            "got {:?}",
            rows[0].batch_value_data2
        );
    }

    #[test]
    fn detail_columns_order_matches_fixture() {
        let bam = BamDam;
        let vals = vec![val(
            r"\Device\HarddiskVolume3\Windows\explorer.exe",
            0x01DA_0000_0000_0000,
        )];
        let rows = bam.rows_from_values(
            r"ROOT\ControlSet001\Services\bam\State\UserSettings\S-1-5-90-0-2",
            &vals,
        );
        assert_eq!(rows.len(), 1);
        let cols: Vec<&str> = rows[0]
            .detail_columns
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(
            cols,
            &["Program", "BatchKeyPath", "ExecutionTime", "BatchValueName"]
        );
    }

    #[test]
    fn skips_values_with_fewer_than_8_bytes() {
        let bam = BamDam;
        let short = PluginValue {
            name: "short".into(),
            raw: vec![0u8; 4],
            value_data: String::new(),
            data_type: CellKeyValueDataTypes::REG_BIN,
        };
        let rows = bam.rows_from_values("SomePath", &[short]);
        assert!(rows.is_empty(), "should skip values with < 8 bytes");
    }

    #[test]
    fn known_filetime_converts_to_utc() {
        // 2026-03-12 22:52:48.7227042 UTC from the DESKTOP SYSTEM fixture
        // (dwm.exe execution time in BamDam_SYSTEM.csv).
        // FILETIME value computed: 134178295687227042
        // (verified round-trip: converts back to "2026-03-12 22:52:48.7227042")
        let bam = BamDam;
        let vals = vec![val(
            r"\Device\HarddiskVolume3\Windows\System32\dwm.exe",
            134_178_295_687_227_042,
        )];
        let rows = bam.rows_from_values("SomePath", &vals);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].batch_value_data2, "Execution time: 2026-03-12 22:52:48.7227042",
            "full timestamp must match RECmd's fffffff format byte-for-byte"
        );
    }
}
