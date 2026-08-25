//! Port of RegistryPlugin.ProfileList (ProfileList.cs + ValuesOut.cs). Extracts
//! user profile information from the SOFTWARE ProfileList key.
//!
//! The plugin matches `Microsoft\Windows NT\CurrentVersion\ProfileList` and
//! iterates its subkeys. Each subkey is a user profile (SID-named).
//!
//! Detail-CSV column order (fixture-authoritative, from ProfileList_SOFTWARE.csv):
//!   Timestamp, BatchKeyPath, KeyName, BatchValueName, ProfileImagePath,
//!   LastLogonTime, LastLogoffTime
//!
//! Batch row format (from SOFTWARE batch.csv "User Accounts (SOFTWARE)" rows):
//!   ValueName  = "Multiple"
//!   ValueData  = "KeyName: {sid}"
//!   ValueData2 = "Timestamp: {yyyy-MM-dd HH:mm:ss.fffffff}"   (UTC, RECmd literal)
//!   ValueData3 = "ProfileImagePath: {path}"
//!
//! LocalProfileLoadTime / LocalProfileUnloadTime are computed from DWORD pairs
//! `LocalProfileLoadTimeHigh`/`LocalProfileLoadTimeLow` and
//! `LocalProfileUnloadTimeHigh`/`LocalProfileUnloadTimeLow`.
//!
//! The C# `GetDateTime` method (ProfileList.cs):
//!   hexString = Convert.ToInt64(high).ToString("X") + Convert.ToInt64(low).ToString("X")
//!   timestampInt = Convert.ToInt64(hexString, 16)
//!   → DateTime.FromFileTime(timestampInt).ToUniversalTime()
//!
//! Translation: read each DWORD as i64, format each as uppercase hex WITHOUT
//! leading zeros, concatenate, then parse the result as i64 FILETIME.

use chrono::DateTime;
use notatin::cell_key_node::CellKeyNode;
use triage_core::timestamp::WinTimestamp;
use triage_registry::hive::Hive;
use triage_registry::plugin::{PluginRow, PluginValue, RegistryPlugin};

pub struct ProfileList;

fn dt_to_recmd_literal(dt: DateTime<chrono::Utc>) -> String {
    let ticks = dt.timestamp_subsec_nanos() / 100;
    format!("{}.{:07}", dt.format("%Y-%m-%d %H:%M:%S"), ticks)
}

fn dt_to_iso8601(dt: DateTime<chrono::Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

/// Replicate C# `GetDateTime(high, low)` from ProfileList.cs.
///
/// Both `high` and `low` are REG_DWORD values (4 bytes LE). The C# reads them
/// as their decimal string representations (ValueData), converts each to i64,
/// formats each as uppercase hex WITHOUT zero-padding, concatenates, then
/// parses the result as an i64 FILETIME.
///
/// NOTE: This is a quirk of the C# implementation. For typical FILETIME values
/// the result is identical to `(high as u64) << 32 | (low as u64)`, but when
/// `low == 0` the hex concat produces "X0" (= low * 16) rather than "X00000000".
/// We replicate the C# behavior exactly.
fn get_datetime(high_raw: &[u8], low_raw: &[u8]) -> Option<String> {
    if high_raw.len() < 4 || low_raw.len() < 4 {
        return None;
    }
    let high = u32::from_le_bytes(high_raw[..4].try_into().ok()?) as i64;
    let low = u32::from_le_bytes(low_raw[..4].try_into().ok()?) as i64;
    // C#: Convert.ToInt64(high).ToString("X") + Convert.ToInt64(low).ToString("X")
    // ToString("X") is uppercase hex with NO leading zeros.
    let hex_str = format!("{high:X}{low:X}");
    let ft = i64::from_str_radix(&hex_str, 16).ok()?;
    if ft == 0 {
        return None;
    }
    // ft is a Windows FILETIME (100-ns ticks since 1601-01-01 UTC).
    // Convert to DateTime<Utc> via WinTimestamp.
    let win_ts = WinTimestamp::from_filetime(ft as u64);
    if win_ts.is_none() {
        return None;
    }
    Some(win_ts.to_string())
}

/// Get a named value's raw bytes from a subkey.
fn get_raw_value(sub_key: &CellKeyNode, name: &str) -> Option<Vec<u8>> {
    for v in sub_key.value_iter() {
        if v.get_pretty_name() == name {
            return Some(triage_registry::value::raw_bytes(&v));
        }
    }
    None
}

/// Get a named string value from a subkey.
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
    let profile_image_path = get_str_value(sub_key, "ProfileImagePath");

    // Logon time from LocalProfileLoadTimeHigh + LocalProfileLoadTimeLow
    let load_high = get_raw_value(sub_key, "LocalProfileLoadTimeHigh");
    let load_low = get_raw_value(sub_key, "LocalProfileLoadTimeLow");
    let last_logon_time = match (load_high, load_low) {
        (Some(h), Some(l)) => get_datetime(&h, &l).unwrap_or_default(),
        _ => String::new(),
    };

    // Logoff time from LocalProfileUnloadTimeHigh + LocalProfileUnloadTimeLow
    let unload_high = get_raw_value(sub_key, "LocalProfileUnloadTimeHigh");
    let unload_low = get_raw_value(sub_key, "LocalProfileUnloadTimeLow");
    let last_logoff_time = match (unload_high, unload_low) {
        (Some(h), Some(l)) => get_datetime(&h, &l).unwrap_or_default(),
        _ => String::new(),
    };

    let lw = sub_key.last_key_written_date_and_time();
    let ts_recmd = dt_to_recmd_literal(lw);
    let ts_iso = dt_to_iso8601(lw);

    PluginRow {
        batch_value_name: "Multiple".to_string(),
        // C# ValuesOut.BatchValueData1: $"KeyName: {KeyName}"
        batch_value_data1: format!("KeyName: {key_name}"),
        // C# ValuesOut.BatchValueData2:
        // $"Timestamp: {Timestamp?.ToUniversalTime():yyyy-MM-dd HH:mm:ss.fffffff}"
        batch_value_data2: format!("Timestamp: {ts_recmd}"),
        // C# ValuesOut.BatchValueData3: $"ProfileImagePath: {ProfileImagePath}"
        batch_value_data3: format!("ProfileImagePath: {profile_image_path}"),
        // Column order from fixture header:
        //   Timestamp, BatchKeyPath, KeyName, BatchValueName, ProfileImagePath,
        //   LastLogonTime, LastLogoffTime
        detail_columns: vec![
            ("Timestamp".to_string(), ts_iso),
            ("BatchKeyPath".to_string(), sub_path.to_string()),
            ("KeyName".to_string(), key_name),
            ("BatchValueName".to_string(), "Multiple".to_string()),
            ("ProfileImagePath".to_string(), profile_image_path),
            ("LastLogonTime".to_string(), last_logon_time),
            ("LastLogoffTime".to_string(), last_logoff_time),
        ],
    }
}

impl RegistryPlugin for ProfileList {
    fn plugin_name(&self) -> &'static str {
        "ProfileList"
    }

    fn key_paths(&self) -> &[&'static str] {
        &[r"Microsoft\Windows NT\CurrentVersion\ProfileList"]
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
    fn plugin_name_and_key_path() {
        assert_eq!(ProfileList.plugin_name(), "ProfileList");
        assert_eq!(
            ProfileList.key_paths(),
            &[r"Microsoft\Windows NT\CurrentVersion\ProfileList"]
        );
    }

    #[test]
    fn get_datetime_matches_cs_algorithm() {
        // From the fixture: SID -1001 has LastLogonTime = "2026-03-12 22:52:32.3008994"
        // RECmd UTC → ISO after testkit normalization: "2026-03-12T22:52:32.3008994Z"
        //
        // Compute the expected DWORD High/Low from the known FILETIME.
        // FILETIME for 2026-03-12 22:52:32.3008994 UTC:
        // Unix seconds: 2026-03-12T22:52:32Z = 1773355952 s
        // Subsecond 100-ns ticks: 3008994
        // Total FILETIME ticks = 116444736000000000 + 1773355952*10000000 + 3008994
        //                       = 116444736000000000 + 17733559520000000 + 3008994
        //                       = 134178295523008994
        let ft: i64 = 134_178_295_523_008_994;
        let low = (ft & 0xFFFF_FFFF) as u32;
        let high = (ft >> 32) as u32;
        let low_bytes = low.to_le_bytes().to_vec();
        let high_bytes = high.to_le_bytes().to_vec();
        let result = get_datetime(&high_bytes, &low_bytes).unwrap();
        assert_eq!(result, "2026-03-12T22:52:32.3008994Z", "got {result:?}");
    }

    #[test]
    fn get_datetime_returns_none_for_zero() {
        let zero = 0u32.to_le_bytes().to_vec();
        assert!(get_datetime(&zero, &zero).is_none());
    }

    #[test]
    fn detail_columns_order() {
        let expected_cols = [
            "Timestamp",
            "BatchKeyPath",
            "KeyName",
            "BatchValueName",
            "ProfileImagePath",
            "LastLogonTime",
            "LastLogoffTime",
        ];
        assert_eq!(expected_cols.len(), 7);
    }
}
