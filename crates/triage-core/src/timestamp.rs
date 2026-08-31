use chrono::{DateTime, Utc};
use std::fmt;

/// Seconds between 1601-01-01 (FILETIME epoch) and 1970-01-01 (Unix epoch).
const FILETIME_UNIX_OFFSET_SECS: i64 = 11_644_473_600;

/// Microseconds between 1601-01-01 (FILETIME / WebKit epoch) and 1970-01-01.
const WEBKIT_UNIX_OFFSET_MICROS: i64 = FILETIME_UNIX_OFFSET_SECS * 1_000_000;

/// FILETIME for 9999-12-31T23:59:59.9999999Z (.NET DateTime.MaxValue).
/// Anything later cannot be a real Windows timestamp and cannot be
/// rendered as plain 4-digit-year ISO 8601; it is treated as unset.
const FILETIME_MAX: u64 = 2_650_467_743_999_999_999;

/// A UTC timestamp with 100-nanosecond precision, serialized as ISO 8601
/// terminated by `Z` with exactly seven fractional digits (spec section 3.4).
/// `None` represents an unset/invalid/sentinel source value and serializes
/// as JSON null / empty CSV field. The implementation never invents a
/// timestamp or substitutes the epoch for an unset value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinTimestamp(Option<DateTime<Utc>>);

impl WinTimestamp {
    pub fn none() -> Self {
        WinTimestamp(None)
    }

    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }

    /// From Windows FILETIME (100ns ticks since 1601-01-01). 0 is the
    /// conventional "unset" sentinel and maps to None, as does any value
    /// outside chrono's representable range.
    pub fn from_filetime(ft: u64) -> Self {
        if ft == 0 {
            return WinTimestamp(None);
        }
        if ft > FILETIME_MAX {
            return WinTimestamp(None);
        }
        let secs = (ft / 10_000_000) as i64 - FILETIME_UNIX_OFFSET_SECS;
        let nanos = ((ft % 10_000_000) * 100) as u32;
        WinTimestamp(DateTime::from_timestamp(secs, nanos))
    }

    /// From Unix epoch seconds. Values before -11_644_473_600 (1601-01-01,
    /// the FILETIME epoch — no Windows artifact predates it) or after
    /// 253_402_300_799 (9999-12-31T23:59:59Z) are out-of-range and map to
    /// None.
    pub fn from_unix(secs: i64) -> Self {
        if !(-FILETIME_UNIX_OFFSET_SECS..=253_402_300_799).contains(&secs) {
            return WinTimestamp(None);
        }
        WinTimestamp(DateTime::from_timestamp(secs, 0))
    }

    /// From Unix epoch seconds plus sub-second nanoseconds (full precision,
    /// e.g. converted from a `std::time::SystemTime`). Same bounds policy as
    /// `from_unix`: before 1601-01-01 or after 9999-12-31T23:59:59 maps to
    /// None. `nanos` beyond 999_999_999 is invalid and also maps to None.
    pub fn from_unix_nanos(secs: i64, nanos: u32) -> Self {
        if !(-FILETIME_UNIX_OFFSET_SECS..=253_402_300_799).contains(&secs) {
            return WinTimestamp(None);
        }
        WinTimestamp(DateTime::from_timestamp(secs, nanos))
    }

    /// From a WebKit/Chrome timestamp: microseconds since 1601-01-01 UTC, i.e.
    /// `base::Time::ToDeltaSinceWindowsEpoch().InMicroseconds()`. This is the
    /// FILETIME epoch with a 1000x coarser tick, used by Chromium's
    /// `urls.last_visit_time`, `visits.visit_time`, `downloads.*_time`,
    /// `cookies.*_utc`, `logins.date_*`, and the `date_added` fields in
    /// `Bookmarks`/`Preferences` JSON.
    ///
    /// 0 is Chromium's universal "unset" sentinel and maps to None, as does any
    /// negative value (it would predate the epoch) or anything out of range.
    pub fn from_webkit_micros(micros: i64) -> Self {
        if micros <= 0 {
            return WinTimestamp(None);
        }
        // micros > 0, so this cannot underflow.
        Self::from_unix_micros_allowing_zero(micros - WEBKIT_UNIX_OFFSET_MICROS)
    }

    /// From Mozilla PRTime: microseconds since 1970-01-01 UTC. Used by
    /// `moz_places.last_visit_date`, `moz_historyvisits.visit_date`,
    /// `moz_cookies.creationTime`/`lastAccessed`, `moz_formhistory.firstUsed`/
    /// `lastUsed`, and `moz_bookmarks.dateAdded`/`lastModified`.
    ///
    /// 0 maps to None (Firefox's "never"); same bounds policy as `from_unix`.
    pub fn from_unix_micros(micros: i64) -> Self {
        if micros == 0 {
            return WinTimestamp(None);
        }
        Self::from_unix_micros_allowing_zero(micros)
    }

    /// From Unix epoch milliseconds. Used by Firefox `logins.json`
    /// (`timeCreated`/`timeLastUsed`/`timePasswordChanged`), `extensions.json`
    /// (`installDate`/`updateDate`), and `moz_places_metadata`.
    ///
    /// 0 maps to None; same bounds policy as `from_unix`.
    pub fn from_unix_millis(millis: i64) -> Self {
        if millis == 0 {
            return WinTimestamp(None);
        }
        // Euclidean, not truncating: for -1 ms the answer is
        // 1969-12-31T23:59:59.999, and truncating division would instead give
        // secs = 0 with a negative remainder, which is not representable.
        let secs = millis.div_euclid(1_000);
        let nanos = (millis.rem_euclid(1_000) as u32) * 1_000_000;
        Self::from_unix_nanos(secs, nanos)
    }

    /// Shared microsecond split, without the zero-sentinel check. Private so
    /// that each public constructor applies its own sentinel policy exactly
    /// once, and so the range check stays solely in `from_unix_nanos`.
    fn from_unix_micros_allowing_zero(micros: i64) -> Self {
        let secs = micros.div_euclid(1_000_000);
        let nanos = (micros.rem_euclid(1_000_000) as u32) * 1_000;
        Self::from_unix_nanos(secs, nanos)
    }
}

/// An unset timestamp. Spelled out rather than derived so it is unmistakable
/// that the default is "absent", never the 1601 or 1970 epoch — the same rule
/// the constructors follow for their zero sentinels.
impl Default for WinTimestamp {
    fn default() -> Self {
        WinTimestamp(None)
    }
}

impl fmt::Display for WinTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            None => Ok(()),
            Some(dt) => {
                let ticks = dt.timestamp_subsec_nanos() / 100;
                write!(f, "{}.{:07}Z", dt.format("%Y-%m-%dT%H:%M:%S"), ticks)
            }
        }
    }
}

impl serde::Serialize for WinTimestamp {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            None => s.serialize_none(),
            Some(_) => s.serialize_str(&self.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// `time`-crate ISO 8601 helpers
//
// The suite's standard rendering for Windows timestamps that carry 100ns
// precision: `yyyy-MM-ddTHH:mm:ss.fffffffZ` (7 fractional digits, trailing Z).
// `WinTimestamp` above is the `chrono`-based equivalent used by the ESE-derived
// tools (SRUM/SUM/WxT); these helpers serve the `time`-crate parsers (EvtxTriage,
// MFTriage) so the format and FILETIME math live in exactly one place rather
// than being copy-pasted per crate.
//
// NOTE: unlike `WinTimestamp::from_filetime`, `filetime_to_iso8601` renders a 0
// tick value as the 1601 epoch (not "absent"); callers that treat 0 as empty
// must guard before calling. This preserves the historical MFTriage behavior.
// ---------------------------------------------------------------------------

/// ISO 8601 UTC with 7 fractional digits and a trailing `Z`.
pub const ISO8601_UTC: &[time::format_description::FormatItem<'_>] = time::macros::format_description!(
    "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:7]Z"
);

/// Render a `time::OffsetDateTime` as ISO 8601 UTC (7 fractional digits, `Z`).
pub fn format_iso8601(dt: time::OffsetDateTime) -> String {
    dt.format(ISO8601_UTC).unwrap_or_default()
}

/// Convert a Windows FILETIME (100ns ticks since 1601-01-01) to an ISO 8601 UTC
/// string. Callers pass `filetime as i128` (matching both the `i64` USN-journal
/// and `u64` $MFT representations exactly). Returns `None` only when the instant
/// is outside the representable range.
pub fn filetime_to_iso8601(filetime_ticks: i128) -> Option<String> {
    const FILETIME_UNIX_EPOCH_DELTA_SECS: i128 = 11_644_473_600;
    const NANOS_PER_SEC: i128 = 1_000_000_000;
    const NANOS_PER_FILETIME_TICK: i128 = 100;
    let unix_nanos =
        filetime_ticks * NANOS_PER_FILETIME_TICK - FILETIME_UNIX_EPOCH_DELTA_SECS * NANOS_PER_SEC;
    Some(format_iso8601(
        time::OffsetDateTime::from_unix_timestamp_nanos(unix_nanos).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_renders_iso8601_utc_with_seven_fractional_digits() {
        // 2025-01-23T08:11:07.9390033Z as FILETIME (100ns ticks since 1601-01-01)
        // (1737619867 + 11644473600) * 10_000_000 + 9_390_033
        let ft: u64 = 133_820_934_679_390_033;
        let ts = WinTimestamp::from_filetime(ft);
        assert_eq!(ts.to_string(), "2025-01-23T08:11:07.9390033Z");
    }

    #[test]
    fn filetime_to_iso8601_renders_full_100ns_precision() {
        // Shared `time`-based path: same instant the MFTriage/USN tests assert,
        // proving the consolidated helper matches the old per-crate formatters.
        assert_eq!(
            filetime_to_iso8601(134_214_381_591_095_084).as_deref(),
            Some("2026-04-23T17:15:59.1095084Z"),
        );
    }

    #[test]
    fn filetime_to_iso8601_out_of_range_is_none() {
        // The realistic upper bound a caller can produce: a u64::MAX FILETIME
        // cast to i128 (as the $MFT/USN call sites do). It is far beyond year
        // 9999, so the datetime is out of range -> None, matching the previous
        // per-crate behavior. (i128::MAX itself is not a valid input — it would
        // overflow the *100 tick conversion; real ticks come from i64/u64.)
        assert_eq!(filetime_to_iso8601(u64::MAX as i128), None);
        assert_eq!(filetime_to_iso8601(i64::MAX as i128), None);
    }

    #[test]
    fn zero_filetime_is_none_and_renders_empty() {
        let ts = WinTimestamp::from_filetime(0);
        assert!(ts.is_none());
        assert_eq!(ts.to_string(), "");
    }

    #[test]
    fn unix_seconds_render_with_zero_fraction() {
        let ts = WinTimestamp::from_unix(1_700_000_000);
        assert_eq!(ts.to_string(), "2023-11-14T22:13:20.0000000Z");
    }

    #[test]
    fn unix_nanos_preserve_subsecond_precision() {
        let ts = WinTimestamp::from_unix_nanos(1_700_000_000, 123_456_789);
        // 123_456_789 ns = 1_234_567 ticks (truncated to 100ns precision)
        assert_eq!(ts.to_string(), "2023-11-14T22:13:20.1234567Z");
        // zero nanos behaves exactly like from_unix
        assert_eq!(
            WinTimestamp::from_unix_nanos(1_700_000_000, 0),
            WinTimestamp::from_unix(1_700_000_000)
        );
        // bounds match from_unix: pre-1601 and post-9999 are None
        assert!(WinTimestamp::from_unix_nanos(-11_644_473_601, 0).is_none());
        assert!(WinTimestamp::from_unix_nanos(253_402_300_800, 0).is_none());
        assert_eq!(
            WinTimestamp::from_unix_nanos(253_402_300_799, 999_999_999).to_string(),
            "9999-12-31T23:59:59.9999999Z"
        );
        // invalid nanos (>= 2 seconds' worth) is None, never a wrong time
        assert!(WinTimestamp::from_unix_nanos(0, 2_000_000_000).is_none());
    }

    #[test]
    fn out_of_range_values_are_none() {
        assert!(WinTimestamp::from_filetime(u64::MAX).is_none());
        assert!(WinTimestamp::from_filetime(FILETIME_MAX + 1).is_none());
        // max valid FILETIME renders as a plain 4-digit year
        assert_eq!(
            WinTimestamp::from_filetime(FILETIME_MAX).to_string(),
            "9999-12-31T23:59:59.9999999Z"
        );
        assert!(WinTimestamp::from_unix(253_402_300_800).is_none());
        assert!(WinTimestamp::from_unix(-11_644_473_601).is_none());
        // FILETIME epoch itself is valid
        assert_eq!(
            WinTimestamp::from_unix(-11_644_473_600).to_string(),
            "1601-01-01T00:00:00.0000000Z"
        );
    }

    #[test]
    fn serializes_as_string_or_null() {
        #[derive(serde::Serialize)]
        struct Row {
            t: WinTimestamp,
        }
        let some = Row {
            t: WinTimestamp::from_unix(0),
        };
        let none = Row {
            t: WinTimestamp::none(),
        };
        assert_eq!(
            serde_json::to_string(&some).unwrap(),
            r#"{"t":"1970-01-01T00:00:00.0000000Z"}"#
        );
        assert_eq!(serde_json::to_string(&none).unwrap(), r#"{"t":null}"#);
    }

    #[test]
    fn none_is_bare_empty_csv_cell_and_json_null() {
        #[derive(serde::Serialize)]
        struct Row {
            #[serde(rename = "A")]
            a: u32,
            #[serde(rename = "Ts")]
            ts: WinTimestamp,
            #[serde(rename = "B")]
            b: u32,
        }
        let row = Row {
            a: 1,
            ts: WinTimestamp::none(),
            b: 2,
        };
        let mut buf = Vec::new();
        {
            let mut w = csv::Writer::from_writer(&mut buf);
            w.serialize(&row).unwrap();
            w.flush().unwrap();
        }
        assert_eq!(String::from_utf8(buf).unwrap(), "A,Ts,B\n1,,2\n");
        assert_eq!(
            serde_json::to_string(&row).unwrap(),
            r#"{"A":1,"Ts":null,"B":2}"#
        );
        // Some(_) round-trips through CSV intact
        let row2 = Row {
            a: 1,
            ts: WinTimestamp::from_unix(0),
            b: 2,
        };
        let mut buf2 = Vec::new();
        {
            let mut w = csv::Writer::from_writer(&mut buf2);
            w.serialize(&row2).unwrap();
            w.flush().unwrap();
        }
        assert_eq!(
            String::from_utf8(buf2).unwrap(),
            "A,Ts,B\n1,1970-01-01T00:00:00.0000000Z,2\n"
        );
    }

    #[test]
    fn webkit_micros_match_a_known_chrome_timestamp() {
        // Unix 1_700_000_000s expressed on the 1601 epoch in microseconds.
        assert_eq!(
            WinTimestamp::from_webkit_micros(13_344_473_600_000_000).to_string(),
            "2023-11-14T22:13:20.0000000Z"
        );
        // Sub-second precision survives, unlike SQLECmd's `datetime(...)` maps.
        assert_eq!(
            WinTimestamp::from_webkit_micros(13_344_473_600_123_456).to_string(),
            "2023-11-14T22:13:20.1234560Z"
        );
    }

    #[test]
    fn webkit_one_microsecond_is_just_past_the_1601_epoch() {
        assert_eq!(
            WinTimestamp::from_webkit_micros(1).to_string(),
            "1601-01-01T00:00:00.0000010Z"
        );
    }

    #[test]
    fn webkit_zero_and_negative_are_none() {
        assert!(WinTimestamp::from_webkit_micros(0).is_none());
        assert!(WinTimestamp::from_webkit_micros(-1).is_none());
    }

    #[test]
    fn prtime_micros_round_trip_and_zero_is_none() {
        assert_eq!(
            WinTimestamp::from_unix_micros(1_700_000_000_123_456).to_string(),
            "2023-11-14T22:13:20.1234560Z"
        );
        assert!(WinTimestamp::from_unix_micros(0).is_none());
        assert_eq!(
            WinTimestamp::from_unix_micros(1_700_000_000_000_000),
            WinTimestamp::from_unix(1_700_000_000)
        );
    }

    #[test]
    fn unix_millis_round_trip_and_zero_is_none() {
        assert_eq!(
            WinTimestamp::from_unix_millis(1_700_000_000_123).to_string(),
            "2023-11-14T22:13:20.1230000Z"
        );
        assert!(WinTimestamp::from_unix_millis(0).is_none());
        assert_eq!(
            WinTimestamp::from_unix_millis(1_700_000_000_000),
            WinTimestamp::from_unix(1_700_000_000)
        );
    }

    /// Regression: truncating division would give secs = 0 with a negative
    /// remainder here, which is not representable as (secs, nanos).
    #[test]
    fn negative_sub_second_values_floor_correctly() {
        assert_eq!(
            WinTimestamp::from_unix_micros(-1).to_string(),
            "1969-12-31T23:59:59.9999990Z"
        );
        assert_eq!(
            WinTimestamp::from_unix_millis(-1).to_string(),
            "1969-12-31T23:59:59.9990000Z"
        );
    }

    #[test]
    fn out_of_range_values_are_none_for_every_epoch() {
        for extreme in [i64::MAX, i64::MIN] {
            assert!(WinTimestamp::from_webkit_micros(extreme).is_none());
            assert!(WinTimestamp::from_unix_micros(extreme).is_none());
            assert!(WinTimestamp::from_unix_millis(extreme).is_none());
        }
    }
}
