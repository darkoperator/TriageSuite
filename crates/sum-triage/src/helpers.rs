//! Pure conversion helpers for SUM values (ported from SumData/Sum.cs).

use chrono::{Duration, NaiveDate};
use triage_core::timestamp::WinTimestamp;

/// Windows FILETIME (i64, 100ns ticks since 1601-01-01) → `WinTimestamp`.
/// SUM stores all timestamps as Int64 FILETIME columns; `<= 0` is unset.
pub fn filetime_to_wints(ft: i64) -> WinTimestamp {
    if ft <= 0 {
        WinTimestamp::none()
    } else {
        WinTimestamp::from_filetime(ft as u64)
    }
}

/// 16 GUID bytes → canonical lowercase, no braces (C# `Guid.ToString("D")`).
/// Data1 (4 bytes), Data2 (2), Data3 (2) are little-endian; Data4 (8) is in
/// stored order. Returns "" if fewer than 16 bytes.
pub fn format_guid(b: &[u8]) -> String {
    if b.len() < 16 {
        return String::new();
    }
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// CLIENTS `Address` bytes → IP string (SumData `ConvertBytesToIpAddress`).
/// `len > 10` → IPv6 (uppercase hex, colon every 2 bytes); else IPv4 dotted.
pub fn bytes_to_ip(raw: &[u8]) -> String {
    if raw.is_empty() {
        return String::new();
    }
    if raw.len() > 10 {
        let mut s = String::new();
        for (i, byte) in raw.iter().enumerate() {
            s.push_str(&format!("{byte:02X}"));
            if (i + 1) % 2 == 0 {
                s.push(':');
            }
        }
        s.trim_end_matches(':').to_string()
    } else {
        let mut s = String::new();
        for byte in raw {
            s.push_str(&format!("{byte}."));
        }
        s.trim_end_matches('.').to_string()
    }
}

/// ClientsDetailed `Date` (C# `new DateTimeOffset(year,1,1).AddDays(n-1)`),
/// formatted `yyyy-MM-dd`. `day_number` is 1-based (1..=366).
pub fn day_to_date(year: i32, day_number: i64) -> String {
    let base = NaiveDate::from_ymd_opt(year, 1, 1).expect("Jan 1 is always valid");
    let d = base + Duration::days(day_number - 1);
    d.format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_is_dotted_decimal() {
        assert_eq!(bytes_to_ip(&[10, 200, 200, 10]), "10.200.200.10");
    }

    #[test]
    fn ipv6_loopback_is_colon_hex_uppercase() {
        let mut b = [0u8; 16];
        b[15] = 1;
        assert_eq!(bytes_to_ip(&b), "0000:0000:0000:0000:0000:0000:0000:0001");
    }

    #[test]
    fn empty_address_is_empty_string() {
        assert_eq!(bytes_to_ip(&[]), "");
    }

    #[test]
    fn guid_renders_lowercase_no_braces_mixed_endian() {
        // C# new Guid(bytes).ToString("D") layout: Data1/2/3 little-endian.
        let b = [
            0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        assert_eq!(format_guid(&b), "00112233-4455-6677-8899-aabbccddeeff");
    }

    #[test]
    fn guid_short_input_is_empty() {
        assert_eq!(format_guid(&[0, 1, 2]), "");
    }

    #[test]
    fn filetime_zero_is_unset() {
        assert!(filetime_to_wints(0).is_none());
        assert!(filetime_to_wints(-5).is_none());
    }

    #[test]
    fn day_to_date_offsets_from_jan1() {
        // DayNumber is 1-based: Day 1 == Jan 1; Day 40 of 2026 == Feb 9.
        assert_eq!(day_to_date(2026, 1), "2026-01-01");
        assert_eq!(day_to_date(2026, 40), "2026-02-09");
    }
}
