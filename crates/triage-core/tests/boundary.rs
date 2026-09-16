//! Every timestamp constructor swept over the extremes of its input type.
//!
//! These are the functions the whole suite decodes evidence through, so a
//! panic in one of them aborts a parse on a single crafted cell. The
//! constructors' own unit tests pin what they *return*; this file pins that
//! they return at all. See `triage_testkit::boundary` for why.

use triage_core::timestamp::{
    dt_to_iso8601, dt_to_recmd_literal, filetime_to_datetime, filetime_to_iso8601,
    filetime_to_recmd_literal, WinTimestamp,
};
use triage_testkit::boundary::{
    assert_total, assert_total2, EXTREME_I64, EXTREME_U32, EXTREME_U64,
};

#[test]
fn every_wintimestamp_constructor_is_total() {
    assert_total("WinTimestamp::from_filetime", EXTREME_U64, |v| {
        WinTimestamp::from_filetime(v).to_string()
    });
    assert_total("WinTimestamp::from_unix", EXTREME_I64, |v| {
        WinTimestamp::from_unix(v).to_string()
    });
    assert_total("WinTimestamp::from_webkit_micros", EXTREME_I64, |v| {
        WinTimestamp::from_webkit_micros(v).to_string()
    });
    assert_total("WinTimestamp::from_unix_micros", EXTREME_I64, |v| {
        WinTimestamp::from_unix_micros(v).to_string()
    });
    assert_total("WinTimestamp::from_unix_millis", EXTREME_I64, |v| {
        WinTimestamp::from_unix_millis(v).to_string()
    });
    // The nanosecond field is its own untrusted integer: a caller can hand it
    // a value past one second, and a constructor that trusted it would build
    // an unrepresentable instant.
    assert_total2(
        "WinTimestamp::from_unix_nanos",
        EXTREME_I64,
        EXTREME_U32,
        |secs, nanos| WinTimestamp::from_unix_nanos(secs, nanos).to_string(),
    );
}

#[test]
fn every_filetime_helper_is_total() {
    // i128 callers pass either width through unchanged, so sweep both.
    assert_total("filetime_to_datetime(i64)", EXTREME_I64, |v| {
        filetime_to_datetime(v as i128)
    });
    assert_total("filetime_to_datetime(u64)", EXTREME_U64, |v| {
        filetime_to_datetime(v as i128)
    });
    assert_total("filetime_to_iso8601(i64)", EXTREME_I64, |v| {
        filetime_to_iso8601(v as i128)
    });
    assert_total("filetime_to_iso8601(u64)", EXTREME_U64, |v| {
        filetime_to_iso8601(v as i128)
    });
    assert_total("filetime_to_recmd_literal(u64)", EXTREME_U64, |v| {
        filetime_to_recmd_literal(v as i128)
    });
}

#[test]
fn the_renderers_are_total_for_every_instant_a_decoder_can_produce() {
    // Renderers take a decoded DateTime rather than a raw integer, so the
    // sweep runs over whatever the decoders actually hand them.
    for &ticks in EXTREME_U64 {
        if let Some(dt) = filetime_to_datetime(ticks as i128) {
            let _ = dt_to_iso8601(dt);
            let _ = dt_to_recmd_literal(dt);
        }
    }
}

/// A negative tick count is a misread field, not a pre-1601 instant, and past
/// 9999 cannot be rendered with a 4-digit year. Both must come back absent
/// rather than as a plausible-looking timestamp.
#[test]
fn filetime_rejects_what_it_cannot_represent() {
    assert!(filetime_to_datetime(-1).is_none());
    assert!(filetime_to_datetime(i64::MIN as i128).is_none());
    assert!(filetime_to_datetime(u64::MAX as i128).is_none());
    // 9999-12-31T23:59:59.9999999Z is the last representable tick.
    let max = 2_650_467_743_999_999_999i128;
    assert_eq!(
        filetime_to_datetime(max).map(dt_to_iso8601).as_deref(),
        Some("9999-12-31T23:59:59.9999999Z")
    );
    assert!(filetime_to_datetime(max + 1).is_none());
    // Tick 0 is the 1601 epoch here, not absent: the sentinel policy belongs
    // to the caller, and WinTimestamp::from_filetime is the variant that
    // treats it as unset.
    assert_eq!(
        filetime_to_datetime(0).map(dt_to_iso8601).as_deref(),
        Some("1601-01-01T00:00:00.0000000Z")
    );
    assert!(WinTimestamp::from_filetime(0).is_none());
}
