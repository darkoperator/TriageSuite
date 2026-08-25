//! Decode a subkey's values into the string / bool / int / timestamp forms
//! AmcacheNew.cs reads. Value names are case-sensitive (Amcache uses PascalCase).

use std::collections::HashMap;

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use notatin::cell_key_node::CellKeyNode;
use notatin::cell_value::CellValue;
use triage_core::timestamp::WinTimestamp;
use triage_registry::value::render_value_data;

/// A subkey's values keyed by value name, each decoded to its `CellValue`.
pub struct ValueMap(HashMap<String, CellValue>);

impl ValueMap {
    /// Decode every value on `key` once (get_content) into the map.
    pub fn from_key(key: &CellKeyNode) -> ValueMap {
        let mut m = HashMap::new();
        for v in key.value_iter() {
            let name = v.get_pretty_name();
            let (content, _) = v.get_content();
            m.insert(name, content);
        }
        ValueMap(m)
    }

    /// Test constructor from explicit (name, CellValue) pairs.
    pub fn from_pairs(pairs: Vec<(&str, CellValue)>) -> ValueMap {
        ValueMap(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// RECmd `ValueData` string form of a value; empty string if absent.
    pub fn str(&self, name: &str) -> String {
        self.0.get(name).map(render_value_data).unwrap_or_default()
    }

    /// AmcacheNew bool: `ValueData == "1"`.
    pub fn bool1(&self, name: &str) -> bool {
        self.str(name) == "1"
    }

    /// `.NET` bool literal for CSV: "True" / "False".
    pub fn dotnet_bool(&self, name: &str) -> String {
        if self.bool1(name) {
            "True".to_string()
        } else {
            "False".to_string()
        }
    }
}

/// The subkey's last-write FILETIME as a chrono UTC datetime.
pub fn key_last_write(key: &CellKeyNode) -> DateTime<Utc> {
    key.last_key_written_date_and_time()
}

/// Render a UTC datetime as the suite ISO-8601 timestamp (7-digit, `…Z`).
pub fn ts_string(dt: DateTime<Utc>) -> String {
    WinTimestamp::from_unix_nanos(dt.timestamp(), dt.timestamp_subsec_nanos()).to_string()
}

/// Render an optional datetime; empty string when None.
pub fn ts_opt(dt: Option<DateTime<Utc>>) -> String {
    dt.map(ts_string).unwrap_or_default()
}

/// `int.Parse`-style: decimal i64, 0 on empty/failure.
pub fn parse_int(s: &str) -> i64 {
    s.trim().parse::<i64>().unwrap_or(0)
}

/// `Size` decode: "0x…" hex or decimal; 0 on failure (matches AmcacheNew try/catch).
pub fn parse_size(s: &str) -> i64 {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix("0x") {
        i64::from_str_radix(hex, 16).unwrap_or(0)
    } else {
        t.parse::<i64>().unwrap_or(0)
    }
}

/// `Usn` decode: ulong; empty string if absent/unparseable (rendered as-is).
pub fn parse_usn(s: &str) -> String {
    match s.trim().parse::<u64>() {
        Ok(n) => n.to_string(),
        Err(_) => String::new(),
    }
}

/// Strip a `FileId` into a SHA1: first 4 chars removed, lowercased; empty if len<=4.
///
/// Char-safe (not byte-index) slicing: FileId is ASCII hex in valid Amcache
/// data, but forensic input is never trusted, so we count/skip by `char`
/// rather than indexing bytes (see `crates/wxt-triage/src/appid.rs` for the
/// same convention against multi-byte panics). Boundary note: this uses `> 4`
/// (empty at exactly 4 chars), while `strip_prefix4` below uses `>= 4` (also
/// empty at exactly 4 chars) — both agree at the len==4 boundary; keep them
/// in sync if either changes.
pub fn strip_sha1(file_id: &str) -> String {
    if file_id.chars().count() > 4 {
        file_id
            .chars()
            .skip(4)
            .collect::<String>()
            .to_ascii_lowercase()
    } else {
        String::new()
    }
}

/// Strip the first 4 chars (no lowercasing). Returns raw when len < 4.
///
/// Char-safe slicing, same convention as `strip_sha1` above. Boundary note:
/// this uses `>= 4` (empty at exactly 4 chars), matching `strip_sha1`'s `> 4`
/// (also empty at exactly 4 chars) — keep both in sync if either changes.
pub fn strip_prefix4(s: &str) -> String {
    if s.chars().count() >= 4 {
        s.chars().skip(4).collect::<String>()
    } else {
        s.to_string()
    }
}

/// Unix epoch seconds → UTC datetime, guarded > 0 (AmcacheNew DriverTimeStamp).
pub fn epoch_secs_to_dt(s: &str) -> Option<DateTime<Utc>> {
    let n = s.trim().parse::<i64>().ok()?;
    if n <= 0 {
        return None;
    }
    Utc.timestamp_opt(n, 0).single()
}

/// Lenient `.NET DateTime.Parse(InvariantInfo)` replacement for registry date
/// strings (InstallDate, LinkDate, InstallDateMsi, DriverLastWriteTime, Date).
/// Returns None on empty/unparseable. Formats are refined against the compat
/// gate; this list covers the invariant forms Amcache stores.
pub fn parse_dotnet_date(s: &str) -> Option<DateTime<Utc>> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    // Try RFC3339 first (handles trailing Z / offset).
    if let Ok(dt) = DateTime::parse_from_rfc3339(t) {
        return Some(dt.with_timezone(&Utc));
    }
    const FORMATS: &[&str] = &[
        "%Y-%m-%d %H:%M:%S",
        "%m/%d/%Y %H:%M:%S",
        "%m/%d/%Y %I:%M:%S %p",
        "%Y/%m/%d %H:%M:%S",
        "%m/%d/%Y",
        "%Y-%m-%d",
    ];
    for f in FORMATS {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(t, f) {
            return Some(Utc.from_utc_datetime(&ndt));
        }
        // date-only formats parse as NaiveDate; retry at midnight.
        if let Ok(d) = chrono::NaiveDate::parse_from_str(t, f) {
            return Some(Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_and_bool_and_dotnet_bool() {
        let vm = ValueMap::from_pairs(vec![
            ("IsOsComponent", CellValue::String("1".into())),
            ("IsPeFile", CellValue::String("0".into())),
            ("Name", CellValue::String("accesschk.exe".into())),
        ]);
        assert_eq!(vm.str("Name"), "accesschk.exe");
        assert_eq!(vm.str("Missing"), "");
        assert!(vm.bool1("IsOsComponent"));
        assert!(!vm.bool1("IsPeFile"));
        assert_eq!(vm.dotnet_bool("IsOsComponent"), "True");
        assert_eq!(vm.dotnet_bool("IsPeFile"), "False");
        assert_eq!(vm.dotnet_bool("Missing"), "False");
    }

    #[test]
    fn strip_sha1_removes_first_four_and_lowercases() {
        // FileId "0006<40 hex uppercase>" → drop 4, lowercase.
        assert_eq!(strip_sha1("00066ED68CC4ACF8"), "6ed68cc4acf8");
        assert_eq!(strip_sha1("abc"), ""); // len<=4
    }

    #[test]
    fn strip_sha1_boundary_at_exactly_four_chars() {
        assert_eq!(strip_sha1("0006"), "");
        // Mixed-case input beyond 4 chars strips and lowercases.
        assert_eq!(strip_sha1("0006AbCdEf"), "abcdef");
    }

    #[test]
    fn strip_prefix4_boundaries_and_no_case_change() {
        assert_eq!(strip_prefix4("0006"), ""); // exactly 4 chars
        assert_eq!(strip_prefix4("abc"), "abc"); // len<4 returns raw
        assert_eq!(strip_prefix4("0006ABCDEF"), "ABCDEF"); // no lowercasing
    }

    #[test]
    fn parse_usn_valid_and_invalid() {
        assert_eq!(parse_usn("123456789"), "123456789");
        assert_eq!(parse_usn(""), "");
        assert_eq!(parse_usn("garbage"), "");
    }

    #[test]
    fn parse_size_hex_and_decimal() {
        assert_eq!(parse_size("1468320"), 1468320);
        assert_eq!(parse_size("0x1000"), 4096);
        assert_eq!(parse_size(""), 0);
        assert_eq!(parse_size("garbage"), 0);
    }

    #[test]
    fn epoch_zero_is_none() {
        assert!(epoch_secs_to_dt("0").is_none());
        assert!(epoch_secs_to_dt("").is_none());
        assert!(epoch_secs_to_dt("1254380487").is_some());
    }

    #[test]
    fn dotnet_date_parses_common_forms() {
        assert!(parse_dotnet_date("2024-06-27 00:00:00").is_some());
        assert!(parse_dotnet_date("").is_none());
        let dt = parse_dotnet_date("2024-06-27 00:00:00").unwrap();
        assert_eq!(ts_string(dt), "2024-06-27T00:00:00.0000000Z");
    }

    #[test]
    fn dotnet_date_parses_rfc3339_us_style_and_date_only() {
        let rfc3339 = parse_dotnet_date("2024-06-27T12:00:00Z");
        assert!(rfc3339.is_some());
        assert_eq!(ts_string(rfc3339.unwrap()), "2024-06-27T12:00:00.0000000Z");

        assert!(parse_dotnet_date("06/27/2024 00:00:00").is_some());

        // Date-only fallback branch (NaiveDate retry at midnight).
        let date_only = parse_dotnet_date("2024-06-27");
        assert!(date_only.is_some());
        assert_eq!(
            ts_string(date_only.unwrap()),
            "2024-06-27T00:00:00.0000000Z"
        );
    }
}
