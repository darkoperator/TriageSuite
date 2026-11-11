//! Grouping and the time histogram: the aggregate views the grid needs.

mod common;

use common::{analyst, loaded, n};
use oxl_engine::{
    count, group_filter, groups, histogram, load_file, parse_filter, FileOptions, GridQuery,
    BUCKET_LADDER,
};
use oxl_fixtures::{empty_dataset, loose_fw_csv, new_case};

const EVENTS: &str = "evtxtriage_events";

fn q(relation: &str) -> GridQuery {
    GridQuery {
        relation: relation.into(),
        ..Default::default()
    }
}

fn s(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

fn pairs(g: &[oxl_engine::GroupRow]) -> Vec<(Option<String>, u64)> {
    g.iter().map(|g| (g.key.clone(), g.count)).collect()
}

#[test]
fn top_level_groups_with_counts_and_spans() {
    let (_d, case) = loaded();
    let g = groups(&case, &q(EVENTS), &s(&["Channel"]), &[], 50, 0).unwrap();
    assert_eq!(
        g.iter()
            .map(|g| (g.key.clone().unwrap(), g.count))
            .collect::<Vec<_>>(),
        [("Security".to_string(), 8), ("Sysmon".to_string(), 3)]
    );
    assert_eq!(
        (g[1].first.as_deref(), g[1].last.as_deref()),
        (
            Some("2026-01-05T02:15:44.380000Z"),
            Some("2026-01-05T02:17:02.645000Z")
        )
    );
    // The blank and the unconvertible time are not part of the span.
    assert_eq!(
        (g[0].first.as_deref(), g[0].last.as_deref()),
        (
            Some("2026-01-05T02:13:58.204000Z"),
            Some("2026-01-05T02:33:20.461000Z")
        )
    );
    assert!(g.iter().all(|g| g.suspicious == 0));
}

#[test]
fn limit_and_offset_page_the_groups() {
    let (_d, case) = loaded();
    let by = s(&["Channel"]);
    let first = groups(&case, &q(EVENTS), &by, &[], 1, 0).unwrap();
    let second = groups(&case, &q(EVENTS), &by, &[], 1, 1).unwrap();
    assert_eq!(first[0].key.as_deref(), Some("Security"));
    assert_eq!(second[0].key.as_deref(), Some("Sysmon"));
    assert!(groups(&case, &q(EVENTS), &by, &[], 1, 2)
        .unwrap()
        .is_empty());
}

#[test]
fn nested_groups_under_a_parent_key() {
    let (_d, case) = loaded();
    let g = groups(
        &case,
        &q(EVENTS),
        &s(&["Channel", "_triage_host"]),
        &[Some("Security".into())],
        50,
        0,
    )
    .unwrap();
    assert_eq!(
        g.iter()
            .map(|g| (g.key.clone().unwrap(), g.count))
            .collect::<Vec<_>>(),
        [("WS01".to_string(), 5), ("SRV01".to_string(), 3)]
    );
    // A parent key is a value: a quote in it selects nothing and breaks nothing.
    let none = groups(
        &case,
        &q(EVENTS),
        &s(&["Channel", "_triage_host"]),
        &[Some("x' OR '1'='1".into())],
        50,
        0,
    )
    .unwrap();
    assert!(none.is_empty());
}

#[test]
fn group_filter_selects_exactly_the_group() {
    let (_d, case) = loaded();
    let by = s(&["Channel", "_triage_host"]);
    let k = [Some("Security".to_string()), Some("SRV01".to_string())];
    let sel = group_filter(&q(EVENTS), &by, &k);
    assert_eq!(count(&case, &sel).unwrap(), 3);
    // It matches the group's own count, and sits on top of the query's filter.
    let g = groups(&case, &q(EVENTS), &by, &[Some("Security".into())], 50, 0).unwrap();
    assert_eq!(
        g.iter()
            .find(|g| g.key.as_deref() == Some("SRV01"))
            .unwrap()
            .count,
        3
    );
    let base = GridQuery {
        filter: Some(parse_filter(r#"{"column":"EventId","op":"eq","value":"4624"}"#).unwrap()),
        ..q(EVENTS)
    };
    assert_eq!(count(&case, &group_filter(&base, &by, &k)).unwrap(), 2);
    // Keys match exactly, case included.
    let lower = [Some("security".to_string())];
    assert_eq!(
        count(&case, &group_filter(&q(EVENTS), &by[..1], &lower)).unwrap(),
        0
    );
}

#[test]
fn an_empty_key_is_its_own_group() {
    let (_d, case) = loaded();
    let by = s(&["RemoteHost"]);
    let g = groups(&case, &q(EVENTS), &by, &[], 50, 0).unwrap();
    let blank = g.iter().find(|g| g.key.is_none()).unwrap();
    assert_eq!(blank.count, 5);
    // Ordered by count, so the blanks lead; the rest follow by count then key.
    assert_eq!(
        pairs(&g),
        vec![
            (None, 5),
            (Some("203.0.113.7".to_string()), 3),
            (Some("WS01".to_string()), 2),
            (Some("10.0.0.5".to_string()), 1)
        ]
    );
    let sel = group_filter(&q(EVENTS), &by, &[None]);
    assert_eq!(count(&case, &sel).unwrap(), 5);
    // And a None parent key expands that group.
    let nested = groups(
        &case,
        &q(EVENTS),
        &s(&["RemoteHost", "Channel"]),
        &[None],
        50,
        0,
    )
    .unwrap();
    assert_eq!(
        pairs(&nested),
        vec![
            (Some("Security".to_string()), 3),
            (Some("Sysmon".to_string()), 2)
        ]
    );
}

#[test]
fn groups_count_marks() {
    let (_d, case) = loaded();
    n(
        &case,
        "insert into ox.row_mark (row_id, mark, marked_by, marked_at) \
         select _ox_row, 'suspicious', 'analyst:tester', '2026-01-05T00:00:00.000000Z' \
         from evtxtriage_events where \"PayloadData1\" = 'rclone.exe' returning row_id",
    );
    let g = groups(&case, &q(EVENTS), &s(&["Channel"]), &[], 50, 0).unwrap();
    assert_eq!(
        g.iter()
            .map(|g| (g.key.clone().unwrap(), g.suspicious))
            .collect::<Vec<_>>(),
        [("Security".to_string(), 0), ("Sysmon".to_string(), 1)]
    );
    // Counts are rows, not rows times marks.
    assert_eq!(g[1].count, 3);
    let h = histogram(&case, &q(EVENTS), None, 60).unwrap();
    assert_eq!(h.buckets.iter().map(|b| b.suspicious).sum::<u64>(), 1);
    assert_eq!(
        h.buckets.iter().find(|b| b.suspicious == 1).unwrap().start,
        "2026-01-05T02:17:00.000000Z"
    );
}

#[test]
fn groups_follow_the_query_scope() {
    let (_d, case) = loaded();
    let hosts = GridQuery {
        hosts: vec!["SRV01".into()],
        ..q(EVENTS)
    };
    let g = groups(&case, &hosts, &s(&["Channel"]), &[], 50, 0).unwrap();
    assert_eq!(pairs(&g), vec![(Some("Security".to_string()), 3)]);
    // A loose file's typed time column is its primary time.
    let d = tempfile::tempdir().unwrap();
    let mut other = new_case(d.path());
    let fw = loose_fw_csv(d.path());
    let report = load_file(&mut other, &fw, FileOptions::default(), &analyst()).unwrap();
    let rel = report.relations[0].name.clone();
    let g = groups(&other, &q(&rel), &s(&["action"]), &[], 50, 0).unwrap();
    assert_eq!(
        pairs(&g),
        vec![
            (Some("allow".to_string()), 2),
            (Some("deny".to_string()), 1)
        ]
    );
    assert_eq!(g[0].first.as_deref(), Some("2026-01-05T02:14:00.000000Z"));
    assert_eq!(g[0].last.as_deref(), Some("2026-01-05T02:20:00.000000Z"));
}

#[test]
fn histogram_buckets_the_filtered_rows_in_utc() {
    let (_d, case) = loaded();
    let h = histogram(&case, &q(EVENTS), None, 60).unwrap();
    assert_eq!(
        (h.column.as_str(), h.bucket_seconds, h.untimed),
        ("TimeCreated", 60, 2)
    );
    assert_eq!(h.buckets.iter().map(|b| b.count).sum::<u64>(), 9);
    assert_eq!(h.buckets[0].start, "2026-01-05T02:13:00.000000Z");
    assert_eq!(
        histogram(&case, &q(EVENTS), None, 5)
            .unwrap()
            .bucket_seconds,
        300
    );
    // Empty buckets are omitted: 02:18 to 02:21 hold nothing.
    let starts: Vec<_> = h.buckets.iter().map(|b| b.start.as_str()).collect();
    assert!(!starts.contains(&"2026-01-05T02:19:00.000000Z"));
    assert!(starts.windows(2).all(|w| w[0] < w[1]));
    // The span fits in 21 minute-buckets, so 21 would fit and 20 would not.
    assert_eq!(
        histogram(&case, &q(EVENTS), None, 21)
            .unwrap()
            .bucket_seconds,
        60
    );
    assert_eq!(
        histogram(&case, &q(EVENTS), None, 20)
            .unwrap()
            .bucket_seconds,
        300
    );
    // A column can be named; the result carries its name.
    let named = histogram(&case, &q(EVENTS), Some("TimeCreated"), 60).unwrap();
    assert_eq!(named.buckets.len(), h.buckets.len());
}

#[test]
fn histogram_follows_the_filter_and_picks_wider_steps() {
    let (_d, case) = loaded();
    let ws01 = GridQuery {
        hosts: vec!["WS01".into()],
        ..q("petriage_timeline")
    };
    // 2026-01-04 09:00 to 2026-01-05 02:33 touches four epoch-aligned 6-hour buckets, two day buckets.
    let h = histogram(&case, &ws01, None, 4).unwrap();
    assert!(BUCKET_LADDER.contains(&h.bucket_seconds));
    assert_eq!(h.bucket_seconds, 21_600);
    assert_eq!(h.buckets.iter().map(|b| b.count).sum::<u64>(), 4);
    assert_eq!(h.buckets[0].start, "2026-01-04T06:00:00.000000Z");
    // One bucket takes the first step that covers the span: weeks align to the epoch (a Thursday).
    let one = histogram(&case, &q("petriage_timeline"), None, 1).unwrap();
    assert_eq!(one.bucket_seconds, 604_800);
    assert_eq!(one.buckets[0].start, "2026-01-01T00:00:00.000000Z");
    assert_eq!(one.buckets.len(), 1);
    assert_eq!(one.buckets[0].count, 5);
}

#[test]
fn histogram_on_an_app_typed_column() {
    let (_d, case) = loaded();
    let h = histogram(&case, &q("ext_hayabusa_csv_timeline"), None, 60).unwrap();
    assert_eq!(h.buckets.iter().map(|b| b.count).sum::<u64>(), 4);
    assert_eq!(h.untimed, 1);
    assert_eq!(h.column, "Timestamp");
    assert_eq!(h.buckets[0].start, "2026-01-05T02:13:00.000000Z");
}

#[test]
fn empty_inputs_give_empty_results() {
    let d = tempfile::tempdir().unwrap();
    let root = empty_dataset(d.path());
    let mut case = new_case(d.path());
    oxl_engine::load_run(&mut case, &root, &analyst()).unwrap();
    let g = groups(&case, &q("rbtriage_main"), &s(&["FileName"]), &[], 50, 0).unwrap();
    assert!(g.is_empty());
    let h = histogram(&case, &q("rbtriage_main"), None, 60).unwrap();
    assert_eq!(h.column, "DeletedOn");
    assert_eq!((h.buckets.len(), h.untimed), (0, 0));

    let (_d, case) = loaded();
    let none = GridQuery {
        filter: Some(parse_filter(r#"{"column":"Channel","op":"eq","value":"Nope"}"#).unwrap()),
        ..q(EVENTS)
    };
    assert!(groups(&case, &none, &s(&["Channel"]), &[], 50, 0)
        .unwrap()
        .is_empty());
    let h = histogram(&case, &none, None, 60).unwrap();
    assert_eq!((h.buckets.len(), h.untimed), (0, 0));
    assert_eq!(h.column, "TimeCreated");
}

#[test]
fn bad_arguments_are_invalid() {
    let (_d, case) = loaded();
    let code = |r: oxl_engine::Result<_>| r.map(|_: Vec<_>| ()).unwrap_err().code();
    let one = s(&["Channel"]);
    assert_eq!(code(groups(&case, &q(EVENTS), &[], &[], 50, 0)), "invalid");
    let five = s(&["Channel", "Computer", "UserName", "EventId", "RemoteHost"]);
    assert_eq!(
        code(groups(&case, &q(EVENTS), &five, &[], 50, 0)),
        "invalid"
    );
    assert_eq!(
        code(groups(
            &case,
            &q(EVENTS),
            &one,
            &[Some("Security".into())],
            50,
            0
        )),
        "invalid"
    );
    assert_eq!(
        code(groups(&case, &q(EVENTS), &s(&["nope"]), &[], 50, 0)),
        "not_found"
    );
    assert_eq!(
        code(groups(&case, &q("nope"), &one, &[], 50, 0)),
        "not_found"
    );

    let hcode = |r: oxl_engine::Result<oxl_engine::Histogram>| r.unwrap_err().code();
    assert_eq!(hcode(histogram(&case, &q(EVENTS), None, 0)), "invalid");
    // A column that is not a timestamp, and one the relation lacks.
    assert_eq!(
        hcode(histogram(&case, &q(EVENTS), Some("Channel"), 60)),
        "invalid"
    );
    assert_eq!(
        hcode(histogram(&case, &q(EVENTS), Some("nope"), 60)),
        "not_found"
    );

    // A relation with no timestamp column.
    let d = tempfile::tempdir().unwrap();
    let mut other = new_case(d.path());
    let text_only = d.path().join("names.csv");
    std::fs::write(
        &text_only,
        "name,owner
alpha,WS01
beta,SRV01
",
    )
    .unwrap();
    let rel = load_file(&mut other, &text_only, FileOptions::default(), &analyst())
        .unwrap()
        .relations[0]
        .name
        .clone();
    let err = histogram(&other, &q(&rel), None, 60).unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert!(err.to_string().contains("no timestamp column"), "{err}");
}
