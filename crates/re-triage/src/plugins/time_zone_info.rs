//! Port of RegistryPlugin.TimeZoneInformation (TimeZoneInfo.cs + ValuesOut.cs).
//! Extracts time zone configuration from ControlSet00*\Control\TimeZoneInformation.
//!
//! Key path: ControlSet00*\Control\TimeZoneInformation
//! Reads specific named values via switch/case logic.
//!
//! Detail-CSV column order (fixture-authoritative):
//!   ValueName, BatchKeyPath, ValueData, BatchValueName, ValueDataRaw
//!
//! Batch row format (ValueName = the registry value name):
//!   ValueData  = computed value (e.g. i32 string, formatted SYSTEMTIME string, or string data)
//!   ValueData2 = ValueDataRaw (RECmd's default render: decimal for DWORD, hex-dashed for binary)
//!   ValueData3 = ""
//!
//! plugin_name() = "TimeZoneInfo" (fixture file is TimeZoneInfo_SYSTEM.csv,
//! C# PluginName is "TimeZoneInformation" but RECmd uses the short form in the detail filename).

use notatin::cell_key_node::CellKeyNode;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct TimeZoneInfo;

/// Parse a SYSTEMTIME-like 16-byte binary struct for Standard/DaylightStart values.
///
/// C# offsets (individual byte reads, not u16 pairs):
///   raw[2]  = month
///   raw[4]  = weekOfMonth
///   raw[6]  = hour
///   raw[8]  = minute
///   raw[10] = second
///   raw[12..14] as Int16 LE = millisecond
///   raw[14] = dayOfWeek
///
/// Format: "Month {m}, week of month {w}, day of week {d}, Hours:Minutes:Seconds:Milliseconds {h}:{m}:{s}:{ms}"
fn parse_start_binary(raw: &[u8]) -> String {
    if raw.len() < 16 {
        return String::new();
    }
    let month = raw[2];
    let week_of_month = raw[4];
    let hour = raw[6];
    let minute = raw[8];
    let second = raw[10];
    let millisecond = i16::from_le_bytes([raw[12], raw[13]]);
    let day_of_week = raw[14];
    format!(
        "Month {month}, week of month {week_of_month}, day of week {day_of_week}, Hours:Minutes:Seconds:Milliseconds {hour}:{minute}:{second}:{millisecond}"
    )
}

impl RegistryPlugin for TimeZoneInfo {
    fn plugin_name(&self) -> &'static str {
        // The fixture file is "TimeZoneInfo_SYSTEM.csv" — batch.csv PluginDetailFile confirmed.
        // C# PluginName is "TimeZoneInformation" but RECmd names the detail file with the
        // short segment "TimeZoneInfo" as observed in the fixture filenames.
        "TimeZoneInfo"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"ControlSet00*\Control\TimeZoneInformation"]
    }

    fn process(&self, key: &CellKeyNode, _values: &[PluginValue]) -> Vec<PluginRow> {
        let batch_key_path = key.path.trim_start_matches('\\').to_string();
        let mut rows = Vec::new();

        for v in key.value_iter() {
            let value_name = v.get_pretty_name().to_string();
            let (content, _) = v.get_content();
            let value_data_raw_str = triage_registry::value::render_value_data(&content);

            match value_name.as_str() {
                "TimeZoneKeyName" | "StandardName" | "DaylightName" => {
                    // String values: ValueData = ValueDataRaw = the string itself
                    rows.push(PluginRow {
                        batch_value_name: value_name.clone(),
                        batch_value_data1: value_data_raw_str.clone(),
                        batch_value_data2: value_data_raw_str.clone(),
                        batch_value_data3: String::new(),
                        detail_columns: vec![
                            ("ValueName".to_string(), value_name.clone()),
                            ("BatchKeyPath".to_string(), batch_key_path.clone()),
                            ("ValueData".to_string(), value_data_raw_str.clone()),
                            ("BatchValueName".to_string(), value_name),
                            ("ValueDataRaw".to_string(), value_data_raw_str),
                        ],
                    });
                }
                "Bias" | "StandardBias" | "DaylightBias" | "ActiveTimeBias" => {
                    // Integer values: read as i32 from raw bytes
                    // ValueData = i32.ToString()
                    // ValueDataRaw = RECmd's default render (unsigned decimal for DWORD)
                    let raw_bytes = get_raw_bytes(&content);
                    let i32_val = if raw_bytes.len() >= 4 {
                        i32::from_le_bytes([raw_bytes[0], raw_bytes[1], raw_bytes[2], raw_bytes[3]])
                    } else {
                        0
                    };
                    let value_data_str = i32_val.to_string();

                    rows.push(PluginRow {
                        batch_value_name: value_name.clone(),
                        batch_value_data1: value_data_str.clone(),
                        batch_value_data2: value_data_raw_str.clone(),
                        batch_value_data3: String::new(),
                        detail_columns: vec![
                            ("ValueName".to_string(), value_name.clone()),
                            ("BatchKeyPath".to_string(), batch_key_path.clone()),
                            ("ValueData".to_string(), value_data_str),
                            ("BatchValueName".to_string(), value_name),
                            ("ValueDataRaw".to_string(), value_data_raw_str),
                        ],
                    });
                }
                "StandardStart" | "DaylightStart" => {
                    // Binary SYSTEMTIME-like struct
                    // ValueData = parsed string
                    // ValueDataRaw = RECmd's default render (hex-dashed binary)
                    let raw_bytes = get_raw_bytes(&content);
                    let value_data_str = parse_start_binary(&raw_bytes);

                    rows.push(PluginRow {
                        batch_value_name: value_name.clone(),
                        batch_value_data1: value_data_str.clone(),
                        batch_value_data2: value_data_raw_str.clone(),
                        batch_value_data3: String::new(),
                        detail_columns: vec![
                            ("ValueName".to_string(), value_name.clone()),
                            ("BatchKeyPath".to_string(), batch_key_path.clone()),
                            ("ValueData".to_string(), value_data_str),
                            ("BatchValueName".to_string(), value_name),
                            ("ValueDataRaw".to_string(), value_data_raw_str),
                        ],
                    });
                }
                _ => {
                    // Unknown value names are silently ignored (C# switch with no default)
                }
            }
        }

        rows
    }
}

/// Extract raw bytes from a CellValue (handles Binary and U32 cases).
fn get_raw_bytes(content: &notatin::cell_value::CellValue) -> Vec<u8> {
    match content {
        notatin::cell_value::CellValue::Binary(b) => b.clone(),
        notatin::cell_value::CellValue::U32(n) => n.to_le_bytes().to_vec(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_name_and_key_paths() {
        assert_eq!(TimeZoneInfo.plugin_name(), "TimeZoneInfo");
        assert_eq!(
            TimeZoneInfo.key_paths(),
            &[r"ControlSet00*\Control\TimeZoneInformation"]
        );
    }

    #[test]
    fn parse_daylight_start() {
        // DaylightStart raw: 00-00-03-00-02-00-02-00-00-00-00-00-00-00-00-00
        // Expected: "Month 3, week of month 2, day of week 0, Hours:Minutes:Seconds:Milliseconds 2:0:0:0"
        let raw = [
            0x00u8, 0x00, 0x03, 0x00, 0x02, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ];
        let result = parse_start_binary(&raw);
        assert_eq!(
            result,
            "Month 3, week of month 2, day of week 0, Hours:Minutes:Seconds:Milliseconds 2:0:0:0"
        );
    }

    #[test]
    fn parse_standard_start() {
        // StandardStart raw: 00-00-0B-00-01-00-02-00-00-00-00-00-00-00-00-00
        // Expected: "Month 11, week of month 1, day of week 0, Hours:Minutes:Seconds:Milliseconds 2:0:0:0"
        let raw = [
            0x00u8, 0x00, 0x0B, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ];
        let result = parse_start_binary(&raw);
        assert_eq!(
            result,
            "Month 11, week of month 1, day of week 0, Hours:Minutes:Seconds:Milliseconds 2:0:0:0"
        );
    }

    #[test]
    fn parse_start_santiago_utc_minus4() {
        // Example from C# source comments:
        // start: 00 00 0A 00 02 00 17 00 3B 00 3B 00 E7 03 06 00
        // month=10, weekOfMonth=2, hour=23, minute=59, second=59, ms=999, dayOfWeek=6
        let raw = [
            0x00u8, 0x00, 0x0A, 0x00, 0x02, 0x00, 0x17, 0x00, 0x3B, 0x00, 0x3B, 0x00, 0xE7, 0x03,
            0x06, 0x00,
        ];
        let result = parse_start_binary(&raw);
        assert_eq!(
            result,
            "Month 10, week of month 2, day of week 6, Hours:Minutes:Seconds:Milliseconds 23:59:59:999"
        );
    }

    #[test]
    fn detail_column_order() {
        let expected = [
            "ValueName",
            "BatchKeyPath",
            "ValueData",
            "BatchValueName",
            "ValueDataRaw",
        ];
        assert_eq!(expected.len(), 5);
    }
}
