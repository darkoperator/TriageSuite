//! Pure decode helpers shared by the table readers. No I/O.

use triage_core::timestamp::WinTimestamp;

/// Decode a 16-byte .NET GUID blob to canonical lowercase text, matching
/// `System.Guid(byte[]).ToString("D")`: the first three fields are
/// little-endian (4, 2, 2 bytes), the last two are byte-order-preserved (2, 6).
/// Returns `None` if the slice is not exactly 16 bytes.
pub fn guid_from_blob(b: &[u8]) -> Option<String> {
    if b.len() != 16 {
        return None;
    }
    let d1 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let d2 = u16::from_le_bytes([b[4], b[5]]);
    let d3 = u16::from_le_bytes([b[6], b[7]]);
    Some(format!(
        "{d1:08x}-{d2:04x}-{d3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    ))
}

/// Convert a Unix epoch-SECONDS value (as WxTCmd reads these columns) to a
/// `WinTimestamp`. A value of 0 (or negative) is treated as "no timestamp"
/// (empty), matching WxTCmd's `val == 0 -> null`.
pub fn epoch_ts(secs: i64) -> WinTimestamp {
    if secs <= 0 {
        WinTimestamp::none()
    } else {
        WinTimestamp::from_unix(secs)
    }
}

/// Format a duration (end - start) in .NET `TimeSpan` constant ("c") form:
/// `[d.]hh:mm:ss` (the Timeline stores whole seconds, so no fractional part).
/// Returns an empty string when `end` is absent, equal to `start`, or has a
/// year <= 1970 — matching WxTCmd's guard.
pub fn duration_str(start_secs: i64, end_secs: Option<i64>) -> String {
    let Some(end) = end_secs else {
        return String::new();
    };
    if end == start_secs {
        return String::new();
    }
    // year > 1970 guard: 1971-01-01T00:00:00Z = 31_536_000.
    if end < 31_536_000 {
        return String::new();
    }
    let total = end - start_secs;
    if total <= 0 {
        return String::new();
    }
    let days = total / 86_400;
    let rem = total % 86_400;
    let hh = rem / 3_600;
    let mm = (rem % 3_600) / 60;
    let ss = rem % 60;
    if days > 0 {
        format!("{days}.{hh:02}:{mm:02}:{ss:02}")
    } else {
        format!("{hh:02}:{mm:02}:{ss:02}")
    }
}

/// Map the integer ActivityType to WxTCmd's enum name; unknown values render
/// as the raw integer. `ActivityTypeOrg` always carries the raw integer.
pub fn activity_type_name(t: i64) -> String {
    match t {
        2 => "ToastNotification".to_string(),
        5 => "ExecuteOpen".to_string(),
        6 => "InFocus".to_string(),
        10 => "CloudClipboard".to_string(),
        16 => "CopyPaste".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_blob_decodes_dotnet_mixed_endian() {
        let bytes = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        assert_eq!(
            guid_from_blob(&bytes).unwrap(),
            "33221100-5544-7766-8899-aabbccddeeff"
        );
    }

    #[test]
    fn guid_blob_wrong_length_is_none() {
        assert_eq!(guid_from_blob(&[0u8; 15]), None);
    }

    #[test]
    fn epoch_zero_is_empty_nonzero_renders_utc() {
        assert_eq!(epoch_ts(0).to_string(), "");
        // 1_681_999_196 Unix seconds = 2023-04-20T13:59:56Z (UTC).
        // WinTimestamp::Display always emits 7 fractional digits.
        assert_eq!(
            epoch_ts(1_681_999_196).to_string(),
            "2023-04-20T13:59:56.0000000Z"
        );
    }

    #[test]
    fn duration_formats_and_guards() {
        // 13 seconds.
        assert_eq!(duration_str(1_681_999_196, Some(1_681_999_209)), "00:00:13");
        // > 1 day: 1 day, 2h, 3m, 4s = 93_784 s.
        assert_eq!(
            duration_str(31_536_000, Some(31_536_000 + 93_784)),
            "1.02:03:04"
        );
        // equal start/end -> empty.
        assert_eq!(duration_str(1_681_999_196, Some(1_681_999_196)), "");
        // missing end -> empty.
        assert_eq!(duration_str(1_681_999_196, None), "");
        // end year <= 1970 -> empty.
        assert_eq!(duration_str(0, Some(100)), "");
    }

    #[test]
    fn activity_type_known_and_unknown() {
        assert_eq!(activity_type_name(5), "ExecuteOpen");
        assert_eq!(activity_type_name(6), "InFocus");
        assert_eq!(activity_type_name(99), "99");
    }
}
