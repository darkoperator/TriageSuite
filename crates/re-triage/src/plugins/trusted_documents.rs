//! Port of RegistryPlugin.TrustedDocuments (TrustedDocuments.cs + ValuesOut.cs).
//! Extracts Office documents where the user clicked "Enable Editing" or "Enable
//! Content/Macros".
//!
//! Fires on NTUSER.DAT at (15.0 and 16.0, all Office apps):
//!   `Software\Microsoft\Office\15.0\Word\Security\Trusted Documents\TrustRecords`
//!   `Software\Microsoft\Office\15.0\Excel\Security\Trusted Documents\TrustRecords`
//!   `Software\Microsoft\Office\15.0\PowerPoint\Security\Trusted Documents\TrustRecords`
//!   `Software\Microsoft\Office\15.0\Access\Security\Trusted Documents\TrustRecords`
//!   `Software\Microsoft\Office\15.0\OneNote\Security\Trusted Documents\TrustRecords`
//!   `Software\Microsoft\Office\16.0\Word\Security\Trusted Documents\TrustRecords`
//!   `Software\Microsoft\Office\16.0\Excel\Security\Trusted Documents\TrustRecords`
//!   `Software\Microsoft\Office\16.0\PowerPoint\Security\Trusted Documents\TrustRecords`
//!   `Software\Microsoft\Office\16.0\Access\Security\Trusted Documents\TrustRecords`
//!   `Software\Microsoft\Office\16.0\OneNote\Security\Trusted Documents\TrustRecords`
//!
//! Detail-CSV column order (fixture-authoritative, from
//! `<host>__<user>_NTUSER__plugin_TrustedDocuments_NTUSER.DAT.csv`):
//!   EventType, BatchKeyPath, Timestamp, BatchValueName, FileName, Username
//!
//! Batch row format (C# ValuesOut):
//!   ValueData1 = "File Name: {FileName}"
//!   ValueData2 = "File Opened: {Timestamp}"   (DateTimeOffset.ToString(), locale-specific)
//!   ValueData3 = "Event Type: {EventType}"
//!
//! Binary value layout:
//!   - First 8 bytes (LE FILETIME): when the document was trusted
//!   - Last 4 bytes: event type indicator
//!     - [01 00 00 00] = "Enable Editing"
//!     - [FF FF FF 7F] = "Enable Content/Macros"
//!     - anything else = "Unknown value"
//!
//! Username: navigated from the key via:
//!   key.Parent^6 = Microsoft folder
//!   → SubKeys["Internet Explorer"].SubKeys["Suggested Sites"].Values["LogFileFolder"]
//!   → split('\\')[2] (the user profile directory name)
//!
//! The `Timestamp` column is standalone and testkit-normalized. We emit ISO-8601 UTC.

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};
use triage_registry::value::plugin_raw_bytes;

pub struct TrustedDocuments;

/// Convert a Windows FILETIME (100-ns intervals since 1601-01-01 UTC) to UTC DateTime.
fn filetime_to_datetime(ft: i64) -> Option<DateTime<chrono::Utc>> {
    const FILETIME_TO_UNIX_SECS: i64 = 11_644_473_600;
    let secs = (ft / 10_000_000) - FILETIME_TO_UNIX_SECS;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    DateTime::from_timestamp(secs, nanos)
}

/// Format DateTime<Utc> as ISO-8601 UTC with 7 fractional digits.
fn dt_to_iso8601(dt: DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

/// Format DateTime as C# DateTimeOffset.ToString() for the free-text ValueData2 field.
/// C# default ToString() for DateTimeOffset on .NET 6+ en-US:
///   "M/d/yyyy h:mm:ss AM/PM +00:00"
/// But looking at the fixture: Timestamp shows "2023-03-28 21:07:31.7961556"
/// which is the RECmd-formatted standalone column, not the embedded ValueData2.
/// For ValueData2 we use the DateTimeOffset default which is locale-specific.
/// To keep it simple and match C# exactly we use the same format.
fn dt_to_datetimeoffset_string(dt: DateTime<chrono::Utc>) -> String {
    // C# DateTimeOffset.ToString() default (en-US) is like "3/28/2023 9:07:31 PM +00:00"
    // We use this format for the ValueData2 free-text field.
    format!(
        "{}/{}/{} {}{} +00:00",
        dt.format("%-m"),
        dt.format("%-d"),
        dt.format("%Y"),
        dt.format("%-I:%M:%S"),
        dt.format(" %p"),
    )
}

/// Parse the event type from the last 4 bytes of the binary value.
/// Matches C# exactly:
///   "01000000" → "Enable Editing"
///   "FFFFFF7F" → "Enable Content/Macros"
///   anything else → "Unknown value"
fn parse_event_type(raw: &[u8]) -> &'static str {
    if raw.len() < 4 {
        return "Unknown value";
    }
    let last4 = &raw[raw.len() - 4..];
    if last4 == [0x01, 0x00, 0x00, 0x00] {
        "Enable Editing"
    } else if last4 == [0xFF, 0xFF, 0xFF, 0x7F] {
        "Enable Content/Macros"
    } else {
        "Unknown value"
    }
}

/// Parse a FILETIME from the first 8 bytes of the binary value (little-endian).
fn parse_timestamp(raw: &[u8]) -> Option<DateTime<chrono::Utc>> {
    if raw.len() < 8 {
        return None;
    }
    let ft = i64::from_le_bytes(raw[..8].try_into().ok()?);
    filetime_to_datetime(ft)
}

/// Navigate from the TrustRecords key path to find the username via the
/// Internet Explorer Suggested Sites LogFileFolder value.
///
/// Path layout:
///   Software\Microsoft\Office\{ver}\{App}\Security\Trusted Documents\TrustRecords
///   → 6 levels up → Software\Microsoft
///   → Software\Microsoft\Internet Explorer\Suggested Sites → LogFileFolder value
fn get_username(key_path: &str, hive: &mut Hive) -> String {
    // Strip ROOT\ prefix and compute the Microsoft folder path (6 levels up from TrustRecords).
    let rel = key_path.trim_start_matches("ROOT\\");
    let segs: Vec<&str> = rel.split('\\').collect();
    if segs.len() < 7 {
        return String::new();
    }
    // 6 levels up = drop last 6 segments; the result ends at "Microsoft".
    let microsoft_segs = &segs[..segs.len() - 6];
    let microsoft_path = microsoft_segs.join("\\");

    let suggested_sites_path = format!("{microsoft_path}\\Internet Explorer\\Suggested Sites");

    if let Some(ss_key) = hive.get_key(&suggested_sites_path) {
        for v in ss_key.value_iter() {
            if v.get_pretty_name().eq_ignore_ascii_case("LogFileFolder") {
                let (content, _) = v.get_content();
                let val_data = triage_registry::value::render_value_data(&content);
                let parts: Vec<&str> = val_data.split('\\').collect();
                if parts.len() > 2 {
                    return parts[2].to_string();
                }
            }
        }
    }
    String::new()
}

impl TrustedDocuments {
    /// Unit-testable row builder.
    pub fn build_row(
        &self,
        value_name: &str,
        raw: &[u8],
        key_path: &str,
        username: &str,
    ) -> PluginRow {
        let event_type = parse_event_type(raw);
        let ts_opt = parse_timestamp(raw);

        let ts_iso = ts_opt.map(dt_to_iso8601).unwrap_or_default();
        let ts_display = ts_opt.map(dt_to_datetimeoffset_string).unwrap_or_default();

        PluginRow {
            batch_value_name: value_name.to_string(),
            batch_value_data1: format!("File Name: {value_name}"),
            batch_value_data2: format!("File Opened: {ts_display}"),
            batch_value_data3: format!("Event Type: {event_type}"),
            detail_columns: vec![
                ("EventType".to_string(), event_type.to_string()),
                ("BatchKeyPath".to_string(), key_path.to_string()),
                ("Timestamp".to_string(), ts_iso),
                ("BatchValueName".to_string(), value_name.to_string()),
                ("FileName".to_string(), value_name.to_string()),
                ("Username".to_string(), username.to_string()),
            ],
        }
    }
}

impl RegistryPlugin for TrustedDocuments {
    fn plugin_name(&self) -> &'static str {
        "TrustedDocuments"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[
            r"Software\Microsoft\Office\15.0\Word\Security\Trusted Documents\TrustRecords",
            r"Software\Microsoft\Office\15.0\Excel\Security\Trusted Documents\TrustRecords",
            r"Software\Microsoft\Office\15.0\PowerPoint\Security\Trusted Documents\TrustRecords",
            r"Software\Microsoft\Office\15.0\Access\Security\Trusted Documents\TrustRecords",
            r"Software\Microsoft\Office\15.0\OneNote\Security\Trusted Documents\TrustRecords",
            r"Software\Microsoft\Office\16.0\Word\Security\Trusted Documents\TrustRecords",
            r"Software\Microsoft\Office\16.0\Excel\Security\Trusted Documents\TrustRecords",
            r"Software\Microsoft\Office\16.0\PowerPoint\Security\Trusted Documents\TrustRecords",
            r"Software\Microsoft\Office\16.0\Access\Security\Trusted Documents\TrustRecords",
            r"Software\Microsoft\Office\16.0\OneNote\Security\Trusted Documents\TrustRecords",
        ]
    }

    fn process_with_hive(
        &self,
        key: &mut CellKeyNode,
        values: &[PluginValue],
        hive: &mut Hive,
    ) -> Vec<PluginRow> {
        let key_path = key.path.trim_start_matches('\\').to_string();
        let username = get_username(&key_path, hive);

        let mut rows = Vec::new();
        for v in values {
            // Get raw bytes for the binary value.
            let raw = if v.raw.is_empty() {
                // Fallback: re-read from key.
                key.value_iter()
                    .find(|kv| kv.get_pretty_name() == v.name)
                    .map(|kv| {
                        let (content, _) = kv.get_content();
                        plugin_raw_bytes(&content)
                    })
                    .unwrap_or_default()
            } else {
                v.raw.clone()
            };

            let row = self.build_row(&v.name, &raw, &key_path, &username);
            rows.push(row);
        }

        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_trust_record(filetime_le: i64, type_bytes: [u8; 4]) -> Vec<u8> {
        let mut raw = vec![0u8; 16]; // 16 bytes total (8 timestamp + 4 padding + 4 type)
        let ft_bytes = filetime_le.to_le_bytes();
        raw[..8].copy_from_slice(&ft_bytes);
        // Fill middle 4 bytes with zeros
        raw[8..12].copy_from_slice(&[0, 0, 0, 0]);
        raw[12..16].copy_from_slice(&type_bytes);
        raw
    }

    #[test]
    fn enable_editing_event_type() {
        // FILETIME for 2023-03-28 21:07:31.7961556 UTC
        let raw = make_trust_record(133247932517961556i64, [0x01, 0x00, 0x00, 0x00]);
        let p = TrustedDocuments;
        let row = p.build_row(
            r"%USERPROFILE%/Downloads/layer_by_operation.xlsx",
            &raw,
            r"ROOT\SOFTWARE\Microsoft\Office\16.0\Excel\Security\Trusted Documents\TrustRecords",
            "user1",
        );
        assert_eq!(row.detail_columns[0].1, "Enable Editing");
        assert_eq!(
            row.detail_columns[4].1,
            r"%USERPROFILE%/Downloads/layer_by_operation.xlsx"
        );
        assert_eq!(row.detail_columns[5].1, "user1");
        // Timestamp should be non-empty
        assert!(!row.detail_columns[2].1.is_empty());
    }

    #[test]
    fn enable_content_macros_event_type() {
        let raw = make_trust_record(133247932517961556i64, [0xFF, 0xFF, 0xFF, 0x7F]);
        let p = TrustedDocuments;
        let row = p.build_row("doc.docm", &raw, "path", "user");
        assert_eq!(row.detail_columns[0].1, "Enable Content/Macros");
    }

    #[test]
    fn unknown_event_type() {
        let raw = make_trust_record(133247932517961556i64, [0x02, 0x00, 0x00, 0x00]);
        let p = TrustedDocuments;
        let row = p.build_row("doc.docx", &raw, "path", "user");
        assert_eq!(row.detail_columns[0].1, "Unknown value");
    }

    #[test]
    fn detail_columns_order() {
        let raw = make_trust_record(133247932517961556i64, [0x01, 0x00, 0x00, 0x00]);
        let p = TrustedDocuments;
        let row = p.build_row("file.xlsx", &raw, "keypath", "user");
        let col_names: Vec<&str> = row.detail_columns.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            col_names,
            &[
                "EventType",
                "BatchKeyPath",
                "Timestamp",
                "BatchValueName",
                "FileName",
                "Username"
            ]
        );
    }

    #[test]
    fn plugin_name_and_key_paths() {
        let p = TrustedDocuments;
        assert_eq!(p.plugin_name(), "TrustedDocuments");
        assert!(p.key_paths().contains(
            &r"Software\Microsoft\Office\16.0\Excel\Security\Trusted Documents\TrustRecords"
        ));
        assert_eq!(p.key_paths().len(), 10);
    }

    #[test]
    fn batch_value_data_format() {
        let raw = make_trust_record(133247932517961556i64, [0x01, 0x00, 0x00, 0x00]);
        let p = TrustedDocuments;
        let row = p.build_row("file.xlsx", &raw, "path", "user");
        assert_eq!(row.batch_value_data1, "File Name: file.xlsx");
        assert!(row.batch_value_data2.starts_with("File Opened: "));
        assert_eq!(row.batch_value_data3, "Event Type: Enable Editing");
    }
}
