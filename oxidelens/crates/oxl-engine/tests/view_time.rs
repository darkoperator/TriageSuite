//! Time in the display zone: histogram buckets at local midnights, date
//! ranges typed in local time, and group spans rendered in the zone.

mod common;

use common::{analyst, loaded};
use oxl_engine::{
    count, count_view, date_range_chip, declare_type, group_filter, groups_view, histogram,
    histogram_view, load_file, Bucket, Case, ChipOrigin, EngineError, FileOptions, GridQuery,
    GridState, SqlType, TimeSemantics,
};

const EVENTS: &str = "evtxtriage_events";
const NY: &str = "America/New_York";

fn q(relation: &str) -> GridQuery {
    GridQuery {
        relation: relation.into(),
        ..Default::default()
    }
}

fn state(relation: &str) -> GridState {
    GridState {
        relation: relation.into(),
        chips: Vec::new(),
        search: Default::default(),
        hosts: Vec::new(),
        sort: Vec::new(),
        columns: None,
    }
}

fn starts(b: &[Bucket]) -> Vec<&str> {
    b.iter().map(|b| b.start.as_str()).collect()
}

fn labels(b: &[Bucket]) -> Vec<(&str, &str)> {
    b.iter()
        .map(|b| (b.label.as_str(), b.end_label.as_str()))
        .collect()
}

fn indexes(b: &[Bucket]) -> Vec<u64> {
    b.iter().map(|b| b.index).collect()
}

fn counts(b: &[Bucket]) -> Vec<u64> {
    b.iter().map(|b| b.count).collect()
}

/// A loose file `name` holding `body`, loaded into a new case, and the
/// relation's name.
fn loose(dir: &std::path::Path, name: &str, body: &str) -> (Case, String) {
    std::fs::create_dir_all(dir).unwrap();
    let mut case = oxl_fixtures::new_case(dir);
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    let rel = load_file(&mut case, &path, FileOptions::default(), &analyst())
        .unwrap()
        .relations[0]
        .name
        .clone();
    (case, rel)
}

/// `loose`, with its `time` column declared a UTC timestamp.
fn loose_utc(dir: &std::path::Path, body: &str) -> (Case, String) {
    let (mut case, rel) = loose(dir, "times.csv", body);
    declare_type(
        &mut case,
        &rel,
        "time",
        SqlType::Timestamp,
        Some(TimeSemantics::Utc),
        &analyst(),
    )
    .unwrap();
    (case, rel)
}

#[test]
fn histogram_in_a_zone() {
    let (_d, case) = loaded();
    // petriage_timeline in New York: SRV01 at 03:00 on Jan 3, WS01 at 04:00
    // on Jan 4 and three in the evening of Jan 4 (02:15 to 02:33 UTC on
    // Jan 5). Six-hour buckets would take eight, so four is days.
    let h = histogram(&case, &q("petriage_timeline"), None, 4, NY).unwrap();
    assert_eq!((h.bucket_seconds, h.zone.as_str()), (86_400, NY));
    assert_eq!(h.time_semantics, Some(TimeSemantics::Utc));
    assert_eq!(
        starts(&h.buckets),
        ["2026-01-03T05:00:00.000000Z", "2026-01-04T05:00:00.000000Z"]
    );
    assert_eq!(
        labels(&h.buckets),
        [
            ("2026-01-03 00:00:00", "2026-01-03 23:59:59"),
            ("2026-01-04 00:00:00", "2026-01-04 23:59:59")
        ]
    );
    assert_eq!(counts(&h.buckets), [1, 4]);

    // In UTC the same rows fall on three UTC days, and the labels are the
    // starts as written without `T` and `Z`.
    let h = histogram(&case, &q("petriage_timeline"), None, 4, "UTC").unwrap();
    assert_eq!(h.bucket_seconds, 86_400);
    assert_eq!(
        starts(&h.buckets),
        [
            "2026-01-03T00:00:00.000000Z",
            "2026-01-04T00:00:00.000000Z",
            "2026-01-05T00:00:00.000000Z"
        ]
    );
    assert_eq!(h.buckets[0].label, "2026-01-03 00:00:00");
    assert_eq!(h.buckets[2].end_label, "2026-01-05 23:59:59");
    assert_eq!(counts(&h.buckets), [1, 1, 3]);

    // Minute buckets: New York's offset is a whole number of minutes, so
    // they keep the epoch's alignment and are only labelled locally.
    let utc = histogram(&case, &q(EVENTS), None, 60, "UTC").unwrap();
    let ny = histogram(&case, &q(EVENTS), None, 60, NY).unwrap();
    assert_eq!(ny.bucket_seconds, 60);
    assert_eq!(starts(&ny.buckets), starts(&utc.buckets));
    assert_eq!(counts(&ny.buckets), counts(&utc.buckets));
    assert_eq!(ny.buckets[0].start, "2026-01-05T02:13:00.000000Z");
    assert_eq!(
        (
            ny.buckets[0].label.as_str(),
            ny.buckets[0].end_label.as_str()
        ),
        ("2026-01-04 21:13:00", "2026-01-04 21:13:59")
    );
    assert_eq!(
        (
            utc.buckets[0].label.as_str(),
            utc.buckets[0].end_label.as_str()
        ),
        ("2026-01-05 02:13:00", "2026-01-05 02:13:59")
    );

    // An unknown zone is refused.
    let err = histogram(&case, &q(EVENTS), None, 60, "Nowhere/Else").unwrap_err();
    assert_eq!(err.code(), "invalid");
}

#[test]
fn six_hour_buckets_follow_the_local_clock() {
    let (_d, case) = loaded();
    let ws01 = GridQuery {
        hosts: vec!["WS01".into()],
        ..q("petriage_timeline")
    };
    // -05:00 is not a whole number of six hours, so the buckets start at
    // local 00:00, 06:00, 12:00 and 18:00. WS01: Jan 4 04:00 local, and
    // three rows from 21:15 local.
    let h = histogram(&case, &ws01, None, 4, NY).unwrap();
    assert_eq!(h.bucket_seconds, 21_600);
    assert_eq!(
        starts(&h.buckets),
        ["2026-01-04T05:00:00.000000Z", "2026-01-04T23:00:00.000000Z"]
    );
    assert_eq!(
        labels(&h.buckets),
        [
            ("2026-01-04 00:00:00", "2026-01-04 05:59:59"),
            ("2026-01-04 18:00:00", "2026-01-04 23:59:59")
        ]
    );
    assert_eq!(counts(&h.buckets), [1, 3]);
    // 00:00 and 18:00 are positions 0 and 3 of six-hour buckets.
    assert_eq!(indexes(&h.buckets), [0, 3]);
    assert_eq!(h.slots, 4);
    // In UTC they keep the epoch's alignment, as before.
    let h = histogram(&case, &ws01, None, 4, "UTC").unwrap();
    assert_eq!(h.buckets[0].start, "2026-01-04T06:00:00.000000Z");
}

#[test]
fn weeks_start_on_monday_and_months_on_the_first() {
    let (_d, case) = loaded();
    // In New York petriage_timeline runs from Saturday Jan 3 to Sunday Jan 4:
    // one Monday week, starting Monday Dec 29 at local midnight.
    let h = histogram(&case, &q("petriage_timeline"), None, 1, NY).unwrap();
    assert_eq!(h.bucket_seconds, 604_800);
    assert_eq!(starts(&h.buckets), ["2025-12-29T05:00:00.000000Z"]);
    assert_eq!(
        labels(&h.buckets),
        [("2025-12-29 00:00:00", "2026-01-04 23:59:59")]
    );
    assert_eq!(counts(&h.buckets), [5]);
    // In UTC the last rows are on Monday Jan 5, a second week, so one
    // bucket takes a month, which starts on the 1st.
    let h = histogram(&case, &q("petriage_timeline"), None, 1, "UTC").unwrap();
    assert_eq!(h.bucket_seconds, 2_592_000);
    assert_eq!(starts(&h.buckets), ["2026-01-01T00:00:00.000000Z"]);
    assert_eq!(
        labels(&h.buckets),
        [("2026-01-01 00:00:00", "2026-01-31 23:59:59")]
    );
    let h = histogram(&case, &q("petriage_timeline"), None, 2, "UTC").unwrap();
    assert_eq!(h.bucket_seconds, 604_800);
    assert_eq!(
        starts(&h.buckets),
        ["2025-12-29T00:00:00.000000Z", "2026-01-05T00:00:00.000000Z"]
    );
    assert_eq!(counts(&h.buckets), [2, 3]);

    // Month buckets over a span of months, in a zone with DST: each starts
    // at local midnight on the 1st, whatever the offset that day.
    let d = tempfile::tempdir().unwrap();
    let (case, rel) = loose_utc(
        d.path(),
        "time,n\n\
         2026-02-10T12:00:00Z,1\n\
         2026-03-31T12:00:00Z,2\n\
         2026-04-01T03:30:00Z,3\n\
         2026-06-15T12:00:00Z,4\n",
    );
    let h = histogram(&case, &q(&rel), None, 5, NY).unwrap();
    assert_eq!(h.bucket_seconds, 2_592_000);
    assert_eq!(
        starts(&h.buckets),
        [
            "2026-02-01T05:00:00.000000Z",
            "2026-03-01T05:00:00.000000Z",
            "2026-06-01T04:00:00.000000Z"
        ]
    );
    // 2026-04-01T03:30Z is still March 31 in New York.
    assert_eq!(counts(&h.buckets), [1, 2, 1]);
    // April and May hold nothing: the jump shows in the indexes.
    assert_eq!(indexes(&h.buckets), [0, 1, 4]);
    assert_eq!(h.slots, 5);
    assert_eq!(h.buckets[1].end_label, "2026-03-31 23:59:59");
}

#[test]
fn day_buckets_across_spring_forward() {
    let d = tempfile::tempdir().unwrap();
    // Local noon in New York on Mar 7 (EST), Mar 8 (the clocks go forward
    // at 02:00) and Mar 9 (EDT), written in a form the suite would not use,
    // so the cell is rendered from its typed value.
    let (case, rel) = loose_utc(
        d.path(),
        "time,n\n2026-03-07 17:00:00,1\n2026-03-08 16:00:00,2\n2026-03-09 16:00:00,3\n",
    );
    let h = histogram(&case, &q(&rel), None, 3, NY).unwrap();
    assert_eq!(h.bucket_seconds, 86_400);
    assert_eq!(
        starts(&h.buckets),
        [
            "2026-03-07T05:00:00.000000Z",
            "2026-03-08T05:00:00.000000Z",
            "2026-03-09T04:00:00.000000Z"
        ]
    );
    assert_eq!(counts(&h.buckets), [1, 1, 1]);
    // The 23-hour day is one position like any other.
    assert_eq!(indexes(&h.buckets), [0, 1, 2]);
    assert_eq!(h.slots, 3);
    // Mar 8 is 23 hours long, and each day ends a second before the next.
    assert_eq!(
        labels(&h.buckets),
        [
            ("2026-03-07 00:00:00", "2026-03-07 23:59:59"),
            ("2026-03-08 00:00:00", "2026-03-08 23:59:59"),
            ("2026-03-09 00:00:00", "2026-03-09 23:59:59")
        ]
    );

    // Hour buckets across the gap keep the epoch's alignment; the hour
    // after 01:00 local is 03:00.
    let (case, rel) = loose_utc(
        &d.path().join("gap"),
        "time,n\n2026-03-08T06:30:00Z,1\n2026-03-08T07:30:00Z,2\n",
    );
    let h = histogram(&case, &q(&rel), None, 2, NY).unwrap();
    assert_eq!(h.bucket_seconds, 3_600);
    assert_eq!(
        labels(&h.buckets),
        [
            ("2026-03-08 01:00:00", "2026-03-08 01:59:59"),
            ("2026-03-08 03:00:00", "2026-03-08 03:59:59")
        ]
    );
}

#[test]
fn buckets_across_fall_back_and_in_a_half_hour_zone() {
    let d = tempfile::tempdir().unwrap();
    // New York, Nov 1: 00:30 EDT (04:30Z), 01:30 EDT (05:30Z), 01:30 EST
    // (06:30Z), 05:30 EST (10:30Z) and 06:30 EST (11:30Z). The offset
    // changes inside the span, so each row is read with its own: one offset
    // for all would put 04:30Z on Oct 31 (EST) or 10:30Z after 06:00 (EDT).
    // Six-hour buckets on the local clock put the first four in the
    // seven-hour bucket that starts at local midnight.
    let (case, rel) = loose_utc(
        &d.path().join("back"),
        "time,n\n2026-11-01T04:30:00Z,0\n2026-11-01T05:30:00Z,1\n2026-11-01T06:30:00Z,2\n\
         2026-11-01T10:30:00Z,3\n2026-11-01T11:30:00Z,4\n",
    );
    let h = histogram(&case, &q(&rel), None, 2, NY).unwrap();
    assert_eq!(h.bucket_seconds, 21_600);
    assert_eq!(
        starts(&h.buckets),
        ["2026-11-01T04:00:00.000000Z", "2026-11-01T11:00:00.000000Z"]
    );
    assert_eq!(
        labels(&h.buckets),
        [
            ("2026-11-01 00:00:00", "2026-11-01 05:59:59"),
            ("2026-11-01 06:00:00", "2026-11-01 11:59:59")
        ]
    );
    assert_eq!(counts(&h.buckets), [4, 1]);

    // Kolkata is +05:30, not a whole number of hours: hour buckets follow
    // its clock. 02:13Z is 07:43 there.
    let (case, rel) = loose_utc(
        &d.path().join("kolkata"),
        "time,n\n2026-01-05T02:13:00Z,1\n2026-01-05T03:20:00Z,2\n",
    );
    let h = histogram(&case, &q(&rel), None, 2, "Asia/Kolkata").unwrap();
    assert_eq!(h.bucket_seconds, 3_600);
    assert_eq!(
        starts(&h.buckets),
        ["2026-01-05T01:30:00.000000Z", "2026-01-05T02:30:00.000000Z"]
    );
    assert_eq!(
        labels(&h.buckets),
        [
            ("2026-01-05 07:00:00", "2026-01-05 07:59:59"),
            ("2026-01-05 08:00:00", "2026-01-05 08:59:59")
        ]
    );
}

#[test]
fn unknown_columns_bucket_as_written() {
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let fw = oxl_fixtures::loose_fw_csv(d.path());
    let rel = load_file(&mut case, &fw, FileOptions::default(), &analyst())
        .unwrap()
        .relations[0]
        .name
        .clone();
    let utc = histogram(&case, &q(&rel), None, 60, "UTC").unwrap();
    let ny = histogram(&case, &q(&rel), None, 60, NY).unwrap();
    assert_eq!(ny.time_semantics, Some(TimeSemantics::Unknown));
    assert_eq!(ny.bucket_seconds, utc.bucket_seconds);
    assert_eq!(starts(&ny.buckets), starts(&utc.buckets));
    assert_eq!(labels(&ny.buckets), labels(&utc.buckets));
    // The label is the start as written: 23:41 is in the file.
    let first = &ny.buckets[0];
    assert_eq!(first.start, "2026-01-04T23:40:00.000000Z");
    assert_eq!(first.label, "2026-01-04 23:40:00");
}

#[test]
fn histogram_view_takes_the_grid_state() {
    let (_d, case) = loaded();
    let mut st = state(EVENTS);
    st.hosts = vec!["SRV01".into()];
    let h = histogram_view(&case, &st, None, 60, NY).unwrap();
    assert_eq!(h.buckets.iter().map(|b| b.count).sum::<u64>(), 2);
    assert_eq!(h.untimed, 1);
    assert_eq!(h.zone, NY);
}

#[test]
fn date_range_chip_converts_and_refuses() {
    let (_d, case) = loaded();
    let chip = date_range_chip(
        &case,
        EVENTS,
        "TimeCreated",
        Some("2026-01-04 21:00:00"),
        Some("2026-01-04 21:14:59"),
        NY,
    )
    .unwrap();
    assert_eq!(chip.origin, ChipOrigin::DateRange);
    assert!(chip.enabled);
    assert!(chip.id.starts_with("dr-"), "{}", chip.id);
    assert_eq!(
        chip.label,
        "TimeCreated 2026-01-04 21:00:00 → 2026-01-04 21:14:59 America/New_York"
    );
    let mut st = state(EVENTS);
    st.chips = vec![chip.clone()];
    let p = oxl_engine::page(&case, &st.query().unwrap(), 100, 0).unwrap();
    let t = p.columns.iter().position(|c| c == "TimeCreated").unwrap();
    let times: Vec<_> = p.cells.iter().map(|r| r[t].text.clone().unwrap()).collect();
    assert_eq!(
        times,
        [
            "2026-01-05T02:13:58.2040000Z",
            "2026-01-05T02:14:07.9110000Z"
        ]
    );
    // The upper bound is the whole last second: 21:14:59.999 is in, 21:15
    // is not. In UTC the same range is five hours later.
    let utc = date_range_chip(
        &case,
        EVENTS,
        "TimeCreated",
        Some("2026-01-05 02:14:00"),
        Some("2026-01-05 02:14:07"),
        "UTC",
    )
    .unwrap();
    assert_ne!(utc.id, chip.id);
    st.chips.push(utc);
    assert_eq!(count_view(&case, &st).unwrap().rows, 1);

    // One end is enough.
    let from = date_range_chip(
        &case,
        EVENTS,
        "TimeCreated",
        Some("2026-01-04 21:20:00"),
        None,
        NY,
    )
    .unwrap();
    st.chips = vec![from];
    // From 02:20 UTC: SRV01's two and WS01's identical pair.
    assert_eq!(count_view(&case, &st).unwrap().rows, 4);
    let to = date_range_chip(
        &case,
        EVENTS,
        "TimeCreated",
        None,
        Some("2026-01-04 21:14:59"),
        NY,
    )
    .unwrap();
    assert!(to.label.contains("21:14:59"), "{}", to.label);
    st.chips = vec![to];
    assert_eq!(count_view(&case, &st).unwrap().rows, 2);

    // Ambiguous: 01:30 happened at 05:30 and again at 06:30 UTC.
    let err = date_range_chip(
        &case,
        EVENTS,
        "TimeCreated",
        Some("2026-11-01 01:30:00"),
        None,
        NY,
    )
    .unwrap_err();
    assert_eq!(err.code(), "invalid");
    let msg = err.to_string();
    assert!(msg.contains("05:30") && msg.contains("06:30"), "{msg}");
    // Nonexistent: the clocks skipped 02:30 on Mar 8.
    let err = date_range_chip(
        &case,
        EVENTS,
        "TimeCreated",
        None,
        Some("2026-03-08 02:30:00"),
        NY,
    )
    .unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert!(err.to_string().contains("does not exist"), "{err}");

    let invalid =
        |r: oxl_engine::Result<oxl_engine::Chip>| matches!(r, Err(EngineError::Invalid(_)));
    // Neither end; a bad time; a reversed range; a bad zone; a column that
    // is not a timestamp.
    assert!(invalid(date_range_chip(
        &case,
        EVENTS,
        "TimeCreated",
        None,
        None,
        NY
    )));
    for bad in [
        "2026-01-04T21:00:00",
        "2026-02-30 00:00:00",
        "yesterday",
        "' OR 1=1 --",
    ] {
        assert!(
            invalid(date_range_chip(
                &case,
                EVENTS,
                "TimeCreated",
                Some(bad),
                None,
                NY
            )),
            "{bad}"
        );
    }
    assert!(invalid(date_range_chip(
        &case,
        EVENTS,
        "TimeCreated",
        Some("2026-01-05 00:00:00"),
        Some("2026-01-04 00:00:00"),
        NY
    )));
    assert!(invalid(date_range_chip(
        &case,
        EVENTS,
        "TimeCreated",
        Some("2026-01-05 00:00:00"),
        None,
        "Nowhere"
    )));
    assert!(invalid(date_range_chip(
        &case,
        EVENTS,
        "Channel",
        Some("2026-01-05 00:00:00"),
        None,
        NY
    )));
    assert_eq!(
        date_range_chip(&case, EVENTS, "Nope", Some("2026-01-05 00:00:00"), None, NY)
            .unwrap_err()
            .code(),
        "not_found"
    );
}

#[test]
fn a_histogram_bucket_round_trips_through_a_date_range() {
    let (_d, case) = loaded();
    let h = histogram(&case, &q("petriage_timeline"), None, 4, NY).unwrap();
    let b = &h.buckets[1];
    let chip = date_range_chip(
        &case,
        "petriage_timeline",
        "RunTime",
        Some(&b.label),
        Some(&b.end_label),
        NY,
    )
    .unwrap();
    let mut st = state("petriage_timeline");
    st.chips = vec![chip];
    assert_eq!(count_view(&case, &st).unwrap().rows, b.count);
}

#[test]
fn date_ranges_on_unknown_columns_compare_as_written() {
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let fw = oxl_fixtures::loose_fw_csv(d.path());
    let rel = load_file(&mut case, &fw, FileOptions::default(), &analyst())
        .unwrap()
        .relations[0]
        .name
        .clone();
    // The file says 02:14 and 02:20; in New York or not, that is what is
    // compared, and an instant New York skipped is no reason to refuse.
    let chip = date_range_chip(
        &case,
        &rel,
        "time",
        Some("2026-01-05 02:00:00"),
        Some("2026-01-05 02:14:00"),
        NY,
    )
    .unwrap();
    assert!(chip.label.ends_with("(as written)"), "{}", chip.label);
    assert!(!chip.label.contains(NY), "{}", chip.label);
    let mut st = state(&rel);
    st.chips = vec![chip];
    assert_eq!(count_view(&case, &st).unwrap().rows, 1);
    assert!(date_range_chip(&case, &rel, "time", Some("2026-03-08 02:30:00"), None, NY).is_ok());
}

#[test]
fn a_long_column_name_keeps_the_label_within_the_cap() {
    let d = tempfile::tempdir().unwrap();
    let name = "T".repeat(300);
    let (case, rel) = loose(
        d.path(),
        "long.csv",
        &format!("{name},n\n2026-01-05T02:13:58Z,1\n"),
    );
    let chip = date_range_chip(
        &case,
        &rel,
        &name,
        Some("2026-01-04 00:00:00"),
        Some("2026-01-06 00:00:00"),
        NY,
    )
    .unwrap();
    assert!(chip.label.chars().count() <= oxl_engine::MAX_CHIP_LABEL);
    assert!(chip.label.contains("2026-01-06 00:00:00"), "{}", chip.label);
    let mut st = state(&rel);
    st.chips = vec![chip];
    assert_eq!(count_view(&case, &st).unwrap().rows, 1);
}

#[test]
fn group_spans_render_in_zone() {
    let (_d, case) = loaded();
    let by = ["Channel".to_string()];
    let g = groups_view(&case, &state(EVENTS), &by, &[], 50, 0, NY)
        .unwrap()
        .items;
    let security = g
        .iter()
        .find(|g| g.key.as_deref() == Some("Security"))
        .unwrap();
    assert_eq!(security.count, 8);
    // The first and last are the rows' own text, rendered, so the fraction
    // keeps its seven digits.
    assert_eq!(
        security.first.as_deref(),
        Some("2026-01-04 21:13:58.2040000")
    );
    assert_eq!(
        security.last.as_deref(),
        Some("2026-01-04 21:33:20.4610000")
    );
    assert!(!security.key_clipped);
    assert_eq!(security.full_key, None);
    let sysmon = g
        .iter()
        .find(|g| g.key.as_deref() == Some("Sysmon"))
        .unwrap();
    assert_eq!(sysmon.first.as_deref(), Some("2026-01-04 21:15:44.3800000"));

    // In UTC too, without the `T` and `Z`.
    let g = groups_view(&case, &state(EVENTS), &by, &[], 50, 0, "UTC")
        .unwrap()
        .items;
    assert_eq!(g[0].first.as_deref(), Some("2026-01-05 02:13:58.2040000"));

    // An unknown primary time is shown as written.
    let d = tempfile::tempdir().unwrap();
    let mut fw_case = oxl_fixtures::new_case(d.path());
    let fw = oxl_fixtures::loose_fw_csv(d.path());
    let rel = load_file(&mut fw_case, &fw, FileOptions::default(), &analyst())
        .unwrap()
        .relations[0]
        .name
        .clone();
    let g = groups_view(
        &fw_case,
        &state(&rel),
        &["action".to_string()],
        &[],
        50,
        0,
        NY,
    )
    .unwrap()
    .items;
    assert_eq!(g[0].key.as_deref(), Some("allow"));
    assert_eq!(g[0].first.as_deref(), Some("2026-01-05 02:14:00"));
    assert_eq!(g[0].last.as_deref(), Some("2026-01-05 02:20:00"));

    // A bad zone is refused.
    assert_eq!(
        groups_view(&case, &state(EVENTS), &by, &[], 50, 0, "Nowhere")
            .unwrap_err()
            .code(),
        "invalid"
    );
}

#[test]
fn clipped_group_key_drills_with_full_key() {
    let d = tempfile::tempdir().unwrap();
    let long = "k".repeat(5_000);
    let (case, rel) = loose(
        d.path(),
        "keys.csv",
        &format!("key,n\n{long},1\n{long},2\nshort,3\n"),
    );
    let by = ["key".to_string()];
    let st = state(&rel);
    let g = groups_view(&case, &st, &by, &[], 50, 0, "UTC")
        .unwrap()
        .items;
    let clipped = g.iter().find(|g| g.key_clipped).unwrap();
    assert_eq!(clipped.count, 2);
    assert_eq!(
        clipped.key.as_ref().unwrap().chars().count(),
        oxl_engine::CELL_CAP
    );
    let full = clipped.full_key.clone().unwrap();
    assert_eq!(full.chars().count(), 5_000);
    let drilled = group_filter(&st.query().unwrap(), &by, &[Some(full)]);
    assert_eq!(count(&case, &drilled).unwrap(), 2);
    // The cut key selects nothing, which is why the whole one is returned.
    let cut = group_filter(
        &st.query().unwrap(),
        &by,
        std::slice::from_ref(&clipped.key),
    );
    assert_eq!(count(&case, &cut).unwrap(), 0);
    // A key that fits carries no full key.
    let short = g
        .iter()
        .find(|g| g.key.as_deref() == Some("short"))
        .unwrap();
    assert_eq!(
        (short.key_clipped, short.full_key.as_deref()),
        (false, None)
    );
    // The full key is serialised for the app only where it was cut.
    let json = serde_json::to_value(&g).unwrap();
    assert!(json
        .as_array()
        .unwrap()
        .iter()
        .all(|v| v.get("full_key").is_some()));
}
