//! `triage_core::timestamp::zone`: UTC text shown in an analyst-chosen zone.
//!
//! The DST edges are the point of this file. America/New_York in 2026 springs
//! forward at 2026-03-08 02:00 local (07:00Z) and falls back at 2026-11-01
//! 02:00 local (06:00Z), so 02:30 on 8 March never happens there and 01:30 on
//! 1 November happens twice.

use triage_core::timestamp::zone::{
    is_zone, local_midnights, local_to_utc, names, render_utc_text, utc_offset_seconds,
    windows_to_iana, LocalError, Rendered,
};
use triage_testkit::boundary::{assert_total, assert_total2};

const NY: &str = "America/New_York";

fn rendered(local: &str, offset: &str) -> Option<Rendered> {
    Some(Rendered {
        local: local.to_string(),
        offset: offset.to_string(),
    })
}

#[test]
fn renders_utc_into_new_york_across_dst() {
    // The last second of EST, then the first instant of EDT: local 02:xx is
    // skipped, so the wall clock jumps from 01:59:59 to 03:00:00.
    assert_eq!(
        render_utc_text("2026-03-08T06:59:59.0000000Z", NY),
        rendered("2026-03-08 01:59:59.0000000", "-05:00")
    );
    assert_eq!(
        render_utc_text("2026-03-08T07:00:00Z", NY),
        rendered("2026-03-08 03:00:00", "-04:00")
    );
    // Fall back: the same local hour twice, told apart only by the offset.
    assert_eq!(
        render_utc_text("2026-11-01T05:30:00Z", NY),
        rendered("2026-11-01 01:30:00", "-04:00")
    );
    assert_eq!(
        render_utc_text("2026-11-01T06:30:00Z", NY),
        rendered("2026-11-01 01:30:00", "-05:00")
    );
    // Crossing local midnight backwards changes the date.
    assert_eq!(
        render_utc_text("2026-01-05T02:13:58.2040000Z", NY),
        rendered("2026-01-04 21:13:58.2040000", "-05:00")
    );
}

#[test]
fn fraction_is_carried_as_written() {
    let at = |text: &str| render_utc_text(text, NY).map(|r| r.local);
    assert_eq!(
        at("2026-01-05T02:13:58.2040000Z").as_deref(),
        Some("2026-01-04 21:13:58.2040000")
    );
    assert_eq!(
        at("2026-01-05T02:13:58Z").as_deref(),
        Some("2026-01-04 21:13:58")
    );
    assert_eq!(
        at("2026-01-05T02:13:58.123456789Z").as_deref(),
        Some("2026-01-04 21:13:58.123456789")
    );
    // One digit and trailing zeros are kept too: never re-rounded, never padded.
    assert_eq!(
        at("2026-01-05T02:13:58.5Z").as_deref(),
        Some("2026-01-04 21:13:58.5")
    );
    assert_eq!(
        at("2026-01-05T02:13:58.000000Z").as_deref(),
        Some("2026-01-04 21:13:58.000000")
    );
}

#[test]
fn utc_zone_renders_with_z() {
    assert_eq!(
        render_utc_text("2026-01-04T21:13:58.2040000Z", "UTC"),
        rendered("2026-01-04 21:13:58.2040000", "Z")
    );
    assert_eq!(
        render_utc_text("2026-01-04T21:13:58Z", "Etc/UTC"),
        rendered("2026-01-04 21:13:58", "Z")
    );
    // A zone that merely sits at offset zero in winter is not UTC: it says so.
    assert_eq!(
        render_utc_text("2026-01-04T21:13:58Z", "Europe/London"),
        rendered("2026-01-04 21:13:58", "+00:00")
    );
    assert_eq!(utc_offset_seconds("2026-01-04T21:13:58Z", "UTC"), Some(0));
}

#[test]
fn ambiguous_local_time_is_refused() {
    let err = local_to_utc("2026-11-01 01:30:00", NY).unwrap_err();
    assert_eq!(
        err,
        LocalError::Ambiguous {
            earlier: "2026-11-01T05:30:00.000000Z".to_string(),
            later: "2026-11-01T06:30:00.000000Z".to_string(),
        }
    );
    // The message names both instants, so the analyst can pick one.
    let message = err.to_string();
    assert!(message.contains("2026-11-01T05:30:00.000000Z"), "{message}");
    assert!(message.contains("2026-11-01T06:30:00.000000Z"), "{message}");
}

#[test]
fn nonexistent_local_time_is_refused() {
    assert_eq!(
        local_to_utc("2026-03-08 02:30:00", NY),
        Err(LocalError::Nonexistent)
    );
}

#[test]
fn local_times_convert_to_r1() {
    assert_eq!(
        local_to_utc("2026-03-08 01:59:59", NY).as_deref(),
        Ok("2026-03-08T06:59:59.000000Z")
    );
    assert_eq!(
        local_to_utc("2026-03-08 03:00:00", NY).as_deref(),
        Ok("2026-03-08T07:00:00.000000Z")
    );
    assert_eq!(
        local_to_utc("2026-01-04 21:13:58", "UTC").as_deref(),
        Ok("2026-01-04T21:13:58.000000Z")
    );
    assert_eq!(
        local_to_utc("2026-01-04 21:13:58", "America/Nowhere"),
        Err(LocalError::UnknownZone)
    );
}

#[test]
fn local_midnights_follow_dst() {
    assert_eq!(
        local_midnights("2026-03-07T00:00:00Z", "2026-03-10T00:00:00Z", NY, 1, 100),
        [
            "2026-03-06T05:00:00.000000Z",
            "2026-03-07T05:00:00.000000Z",
            "2026-03-08T05:00:00.000000Z",
            "2026-03-09T04:00:00.000000Z",
        ]
    );
}

#[test]
fn local_midnights_step_cap_and_degenerate_spans() {
    let from = "2026-03-07T00:00:00Z";
    let to = "2026-03-10T00:00:00Z";
    // Step 2: every other local day, starting at the day that holds `from`.
    assert_eq!(
        local_midnights(from, to, NY, 2, 100),
        ["2026-03-06T05:00:00.000000Z", "2026-03-08T05:00:00.000000Z"]
    );
    // The cap is a hard stop.
    assert_eq!(
        local_midnights(from, to, NY, 1, 2),
        ["2026-03-06T05:00:00.000000Z", "2026-03-07T05:00:00.000000Z"]
    );
    assert!(local_midnights(from, to, NY, 1, 0).is_empty());
    // A step of zero would never advance; a reversed span covers nothing.
    assert!(local_midnights(from, to, NY, 0, 100).is_empty());
    assert!(local_midnights(to, from, NY, 1, 100).is_empty());
    assert!(local_midnights(from, to, "America/Nowhere", 1, 100).is_empty());
    // A single instant is covered by the day that holds it.
    assert_eq!(
        local_midnights(from, from, NY, 1, 100),
        ["2026-03-06T05:00:00.000000Z"]
    );
    // Four centuries at a cap of three is three, and returns at once.
    assert_eq!(
        local_midnights("1700-01-01T00:00:00Z", "9999-12-31T23:59:59Z", "UTC", 1, 3),
        [
            "1700-01-01T00:00:00.000000Z",
            "1700-01-02T00:00:00.000000Z",
            "1700-01-03T00:00:00.000000Z",
        ]
    );
}

#[test]
fn a_day_that_starts_late_or_never_starts_is_handled() {
    // America/Havana springs forward at local midnight: 2026-03-08 00:00 does
    // not exist there, and the day begins at 01:00 CDT (05:00Z).
    assert_eq!(
        local_to_utc("2026-03-08 00:00:00", "America/Havana"),
        Err(LocalError::Nonexistent)
    );
    assert_eq!(
        local_midnights(
            "2026-03-07T12:00:00Z",
            "2026-03-08T12:00:00Z",
            "America/Havana",
            1,
            100
        ),
        ["2026-03-07T05:00:00.000000Z", "2026-03-08T05:00:00.000000Z"]
    );
    // Samoa skipped 2011-12-30 entirely when it crossed the date line: at
    // 2011-12-30T10:00Z the wall clock went from 29 December 24:00 (-10:00)
    // to 31 December 00:00 (+14:00). The skipped day and the day after it
    // start at the same instant, which is listed once.
    assert_eq!(
        local_midnights(
            "2011-12-29T12:00:00Z",
            "2011-12-31T12:00:00Z",
            "Pacific/Apia",
            1,
            100
        ),
        [
            "2011-12-29T10:00:00.000000Z",
            "2011-12-30T10:00:00.000000Z",
            "2011-12-31T10:00:00.000000Z",
        ]
    );
}

#[test]
fn half_hour_zones() {
    assert_eq!(
        render_utc_text("2026-01-04T21:13:58Z", "Asia/Kolkata"),
        rendered("2026-01-05 02:43:58", "+05:30")
    );
    assert_eq!(
        utc_offset_seconds("2026-01-04T21:13:58Z", "Asia/Kolkata"),
        Some(19_800)
    );
    // Quarter-hour too.
    assert_eq!(
        render_utc_text("2026-01-04T21:13:58Z", "Asia/Kathmandu").map(|r| r.offset),
        Some("+05:45".to_string())
    );
}

#[test]
fn offsets_follow_dst() {
    assert_eq!(
        utc_offset_seconds("2026-03-08T06:59:59Z", NY),
        Some(-18_000)
    );
    assert_eq!(
        utc_offset_seconds("2026-03-08T07:00:00Z", NY),
        Some(-14_400)
    );
    assert_eq!(utc_offset_seconds("2026-03-08T07:00:00Z", "Nowhere"), None);
    // The recurring rule still applies past 2038, where a 32-bit time_t ends.
    assert_eq!(
        utc_offset_seconds("2050-01-15T12:00:00Z", NY),
        Some(-18_000)
    );
    assert_eq!(
        utc_offset_seconds("2050-07-15T12:00:00Z", NY),
        Some(-14_400)
    );
    // chrono-tz expands recurring rules through 2099 only; after that each
    // zone keeps its last offset (standard time here). Pinned so a chrono-tz
    // upgrade that moves the horizon is noticed.
    assert_eq!(
        utc_offset_seconds("2099-07-15T12:00:00Z", NY),
        Some(-14_400)
    );
    assert_eq!(
        utc_offset_seconds("2100-07-15T12:00:00Z", NY),
        Some(-18_000)
    );
    // Before standard time, New York kept local mean time, -4:56:02.
    assert_eq!(
        render_utc_text("1880-01-01T12:00:00Z", NY).map(|r| r.offset),
        Some("-04:56:02".to_string())
    );
}

#[test]
fn names_are_sorted_with_utc_first() {
    let all = names();
    assert_eq!(all.first(), Some(&"UTC"));
    let rest = &all[1..];
    assert!(rest.windows(2).all(|w| w[0] < w[1]), "sorted and unique");
    assert!(!rest.contains(&"UTC"));
    for name in [
        NY,
        "Asia/Kolkata",
        "Asia/Calcutta",
        "Europe/Berlin",
        "Etc/UTC",
    ] {
        assert!(all.contains(&name), "{name}");
    }
    assert!(all.iter().all(|n| is_zone(n)));
    assert!(all.len() > 400, "the whole IANA database, links included");
}

#[test]
fn is_zone_is_exact() {
    assert!(is_zone(NY));
    assert!(is_zone("UTC"));
    assert!(!is_zone("america/new_york"));
    assert!(!is_zone(" America/New_York"));
    assert!(!is_zone("Eastern Standard Time"));
    assert!(!is_zone(""));
}

/// Each malformed input, in the UTC form and the local form.
const BAD_UTC: &[&str] = &[
    "",
    "2026-03-08 06:59:59Z",            // no T
    "2026-13-08T06:59:59Z",            // month 13
    "2026-03-08T06:59:59",             // no Z
    "2026-03-08T06:59:59z",            // lower-case z
    "2026-02-30T06:59:59Z",            // 30 February
    "2026-03-08T24:00:00Z",            // hour 24
    "2026-03-08T23:59:60Z",            // leap second
    "2026-03-08T06:59:59.Z",           // empty fraction
    "2026-03-08T06:59:59.1234567890Z", // ten fraction digits
    "+2026-03-08T06:59:59Z",           // signed year
    "2026-3-8T6:59:59Z",               // unpadded fields
    "２０２６-03-08T06:59:59Z",        // full-width digits
    "٢٠٢٦-03-08T06:59:59Z",            // Arabic-Indic digits
    "2026-03-08T06:59:5\u{e9}Z",       // multi-byte where a digit belongs
    "2026-03-08T06:59:59Z\u{202e}",    // trailing bidi override
];

const BAD_LOCAL: &[&str] = &[
    "",
    "2026-03-08T02:30:00",     // a T where the space belongs
    "2026-13-08 02:30:00",     // month 13
    "2026-03-08 02:30:00Z",    // a Z: local text carries no zone
    "2026-03-08 02:30:00.5",   // local text carries no fraction
    "2026-03-08 02:30",        // no seconds
    "2026-02-30 02:30:00",     // 30 February
    "2026-03-08 24:00:00",     // hour 24
    "２０２６-03-08 02:30:00", // full-width digits
    "٢٠٢٦-03-08 02:30:00",     // Arabic-Indic digits
    "2026-03-08 02:30:0\u{e9}",
];

#[test]
fn bad_inputs_are_none_or_errors() {
    let long = "9".repeat(10_000);
    let long_ts = format!("2026-03-08T06:59:59.{long}Z");

    let mut utc_inputs: Vec<&str> = BAD_UTC.to_vec();
    utc_inputs.extend([long.as_str(), long_ts.as_str()]);
    let mut local_inputs: Vec<&str> = BAD_LOCAL.to_vec();
    local_inputs.push(long.as_str());

    for &text in &utc_inputs {
        assert_eq!(render_utc_text(text, NY), None, "{text:?}");
        assert_eq!(utc_offset_seconds(text, NY), None, "{text:?}");
        assert!(local_midnights(text, "2026-03-10T00:00:00Z", NY, 1, 10).is_empty());
        assert!(local_midnights("2026-03-07T00:00:00Z", text, NY, 1, 10).is_empty());
    }
    for &text in &local_inputs {
        assert_eq!(local_to_utc(text, NY), Err(LocalError::BadText), "{text:?}");
    }

    // Totality: every function, every input, every zone, including the valid
    // inputs at the edges of the representable range and zones far from UTC.
    let mut utc_all = utc_inputs.clone();
    utc_all.extend([
        "0000-01-01T00:00:00Z",
        "9999-12-31T23:59:59.9999999Z",
        "2026-03-08T07:00:00Z",
        "2026-11-01T06:30:00.123456789Z",
    ]);
    let mut local_all = local_inputs.clone();
    local_all.extend([
        "0000-01-01 00:00:00",
        "9999-12-31 23:59:59",
        "2026-03-08 02:30:00",
        "2026-11-01 01:30:00",
    ]);
    let zones: Vec<&str> = vec![
        "",
        "UTC",
        NY,
        "Pacific/Kiritimati",
        "Etc/GMT+12",
        "Pacific/Apia",
        "America/Havana",
        "Asia/Kolkata",
        "Nowhere/Never",
        long.as_str(),
    ];
    assert_total2("zone::render_utc_text", &utc_all, &zones, render_utc_text);
    assert_total2(
        "zone::utc_offset_seconds",
        &utc_all,
        &zones,
        utc_offset_seconds,
    );
    assert_total2("zone::local_to_utc", &local_all, &zones, local_to_utc);
    assert_total2(
        "zone::local_midnights(from)",
        &utc_all,
        &zones,
        |from, z| local_midnights(from, "9999-12-31T23:59:59Z", z, 1, 50),
    );
    assert_total2("zone::local_midnights(to)", &utc_all, &zones, |to, z| {
        local_midnights("0000-01-01T00:00:00Z", to, z, 1, 50)
    });
    assert_total(
        "zone::local_midnights(step)",
        &[0u32, 1, 2, 7, 30, u32::MAX - 1, u32::MAX],
        |step| local_midnights("0000-01-01T00:00:00Z", "9999-12-31T23:59:59Z", NY, step, 50),
    );
    // A cap of usize::MAX is only an upper bound: the span still ends it.
    assert_total(
        "zone::local_midnights(cap)",
        &[0usize, 1, usize::MAX],
        |cap| local_midnights("2026-03-07T00:00:00Z", "2026-03-10T00:00:00Z", NY, 1, cap),
    );
    assert_total("zone::is_zone", &zones, is_zone);
    assert_total("zone::windows_to_iana", &zones, windows_to_iana);
}

#[test]
fn the_ends_of_the_range_are_absent_not_wrapped() {
    // Local years outside 0000..=9999 cannot be written with four digits.
    assert_eq!(
        render_utc_text("9999-12-31T23:59:59.9999999Z", "Pacific/Kiritimati"),
        None
    );
    assert_eq!(render_utc_text("0000-01-01T00:00:00Z", NY), None);
    assert_eq!(
        render_utc_text("9999-12-31T23:59:59.9999999Z", "UTC"),
        rendered("9999-12-31 23:59:59.9999999", "Z")
    );
    assert_eq!(
        local_to_utc("0000-01-01 00:00:00", "Asia/Tokyo"),
        Err(LocalError::BadText)
    );
}

/// Transcribed from Unicode CLDR release 49 (tag `release-49`),
/// `common/supplemental/windowsZones.xml`, the `territory="001"` mapZone
/// entries: the Windows zone name and its golden IANA zone. CLDR keeps the
/// older IANA spellings (`Asia/Calcutta`, not `Asia/Kolkata`).
#[test]
fn windows_names_map_to_cldr() {
    for (windows, iana) in [
        ("Eastern Standard Time", "America/New_York"),
        ("Pacific Standard Time", "America/Los_Angeles"),
        ("UTC", "Etc/UTC"),
        ("W. Europe Standard Time", "Europe/Berlin"),
        ("India Standard Time", "Asia/Calcutta"),
        ("Tokyo Standard Time", "Asia/Tokyo"),
        // The first and last entries of the file, and a few awkward names.
        ("Dateline Standard Time", "Etc/GMT+12"),
        ("Line Islands Standard Time", "Pacific/Kiritimati"),
        ("UTC-11", "Etc/GMT+11"),
        ("UTC+13", "Etc/GMT-13"),
        ("Pacific Standard Time (Mexico)", "America/Tijuana"),
        ("Cen. Australia Standard Time", "Australia/Adelaide"),
        ("Nepal Standard Time", "Asia/Katmandu"),
        ("FLE Standard Time", "Europe/Kiev"),
    ] {
        assert_eq!(windows_to_iana(windows), Some(iana), "{windows}");
        assert!(is_zone(iana), "{iana} is a zone this build knows");
    }
    assert_eq!(windows_to_iana("Mars Standard Time"), None);
    assert_eq!(windows_to_iana("eastern standard time"), None);
    assert_eq!(windows_to_iana("Eastern Standard Time "), None);
    assert_eq!(windows_to_iana(""), None);
}
