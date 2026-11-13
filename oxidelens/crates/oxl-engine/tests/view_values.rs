//! A column's values with their counts under the other chips, the grid's
//! row counts, and the columns with nothing in them.

mod common;

use common::loaded;
use oxl_engine::{
    count_view, empty_columns, parse_filter, values, Case, Chip, ChipOrigin, EngineError,
    GridState, ValueCount, ValueOrder,
};

const EVENTS: &str = "evtxtriage_events";

fn chip(id: &str, filter: &str) -> Chip {
    Chip {
        id: id.into(),
        label: id.into(),
        enabled: true,
        origin: ChipOrigin::Values,
        filter: parse_filter(filter).unwrap(),
    }
}

fn state(chips: Vec<Chip>) -> GridState {
    GridState {
        relation: EVENTS.into(),
        chips,
        search: Default::default(),
        hosts: Vec::new(),
        sort: Vec::new(),
        columns: None,
    }
}

fn s(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

fn pairs(items: &[ValueCount]) -> Vec<(Option<&str>, u64)> {
    items
        .iter()
        .map(|v| (v.value.as_deref(), v.count))
        .collect()
}

/// Every value of `column` in `order`, on one page.
fn listed(
    case: &Case,
    st: &GridState,
    column: &str,
    except: &[&str],
    contains: Option<&str>,
    order: ValueOrder,
) -> Vec<(Option<String>, u64)> {
    let out = values(case, st, column, &s(except), contains, order, 1000, 0).unwrap();
    assert!(!out.truncated);
    out.items
        .into_iter()
        .map(|v| {
            assert!(!v.clipped);
            (v.value, v.count)
        })
        .collect()
}

fn some(pairs: &[(&str, u64)]) -> Vec<(Option<String>, u64)> {
    pairs
        .iter()
        .map(|(v, n)| (Some(v.to_string()), *n))
        .collect()
}

#[test]
fn values_respect_other_chips() {
    let (_d, case) = loaded();
    let st = state(vec![
        chip("c1", r#"{"column":"Channel","op":"eq","value":"Security"}"#),
        chip("c2", r#"{"column":"EventId","op":"in","value":["4624"]}"#),
    ]);
    let commonest = ValueOrder::Commonest;
    // Without c1, the channels of the 4624 rows: WS01's one and SRV01's two
    // (one of them the blank-time row), all Security. Sysmon has no 4624.
    assert_eq!(
        listed(&case, &st, "Channel", &["c1"], None, commonest),
        some(&[("Security", 3)])
    );
    // Under both chips, just Security.
    assert_eq!(
        listed(&case, &st, "Channel", &[], None, commonest),
        some(&[("Security", 3)])
    );
    // Without either chip, every channel of the relation.
    assert_eq!(
        listed(&case, &st, "Channel", &["c1", "c2"], None, commonest),
        some(&[("Security", 8), ("Sysmon", 3)])
    );
    // Without c2, the event ids of the Security rows: 4624 (WS01 one, SRV01
    // two), 1102 (the identical pair), 4625, 4688 and 5140 once each.
    assert_eq!(
        listed(&case, &st, "EventId", &["c2"], None, commonest),
        some(&[
            ("4624", 3),
            ("1102", 2),
            ("4625", 1),
            ("4688", 1),
            ("5140", 1)
        ])
    );
    // An id that names no chip leaves every chip applied.
    assert_eq!(
        listed(&case, &st, "Channel", &["gone"], None, commonest),
        some(&[("Security", 3)])
    );
    // A disabled chip is not applied whether or not it is excepted.
    let mut off = st.clone();
    off.chips[1].enabled = false;
    assert_eq!(
        listed(&case, &off, "EventId", &[], None, commonest).len(),
        5
    );
}

#[test]
fn values_orders() {
    let (_d, case) = loaded();
    let st = state(Vec::new());
    // EventId over all 11 rows: 4624 three times (WS01 once, SRV01 twice),
    // 1 twice (the two Sysmon process creations), 1102 twice (the identical
    // pair), and 3, 4625, 4688 and 5140 once each. A typed column's values
    // sort by the typed value, so 3 comes before 1102.
    assert_eq!(
        listed(&case, &st, "EventId", &[], None, ValueOrder::Commonest),
        some(&[
            ("4624", 3),
            ("1", 2),
            ("1102", 2),
            ("3", 1),
            ("4625", 1),
            ("4688", 1),
            ("5140", 1)
        ])
    );
    assert_eq!(
        listed(&case, &st, "EventId", &[], None, ValueOrder::Rarest),
        some(&[
            ("3", 1),
            ("4625", 1),
            ("4688", 1),
            ("5140", 1),
            ("1", 2),
            ("1102", 2),
            ("4624", 3)
        ])
    );
    assert_eq!(
        listed(&case, &st, "EventId", &[], None, ValueOrder::Value),
        some(&[
            ("1", 2),
            ("3", 1),
            ("1102", 2),
            ("4624", 3),
            ("4625", 1),
            ("4688", 1),
            ("5140", 1)
        ])
    );
    // An untyped column sorts its text as the grid does, without regard to
    // case: svc_backup between Administrator and SYSTEM.
    assert_eq!(
        listed(&case, &st, "UserName", &[], None, ValueOrder::Value),
        some(&[("Administrator", 1), ("svc_backup", 9), ("SYSTEM", 1)])
    );

    // `contains` is a case-insensitive literal substring.
    for needle in ["sysm", "SYSM", "ysmo"] {
        assert_eq!(
            listed(&case, &st, "Channel", &[], Some(needle), ValueOrder::Value),
            some(&[("Sysmon", 3)])
        );
    }
    // `_` and `%` are characters, not wildcards: only svc_backup has an
    // underscore, and no value has a percent sign.
    assert_eq!(
        listed(&case, &st, "UserName", &[], Some("_"), ValueOrder::Value),
        some(&[("svc_backup", 9)])
    );
    assert_eq!(
        listed(&case, &st, "UserName", &[], Some("c_b"), ValueOrder::Value),
        some(&[("svc_backup", 9)])
    );
    assert!(listed(&case, &st, "UserName", &[], Some("%"), ValueOrder::Value).is_empty());
    assert!(listed(&case, &st, "UserName", &[], Some("s%"), ValueOrder::Value).is_empty());
    // An empty `contains` narrows nothing.
    assert_eq!(
        listed(&case, &st, "UserName", &[], Some(""), ValueOrder::Value).len(),
        3
    );

    // Paged.
    let first = values(&case, &st, "EventId", &[], None, ValueOrder::Value, 2, 0).unwrap();
    assert_eq!(pairs(&first.items), [(Some("1"), 2), (Some("3"), 1)]);
    assert!(first.truncated);
    let last = values(&case, &st, "EventId", &[], None, ValueOrder::Value, 2, 6).unwrap();
    assert_eq!(pairs(&last.items), [(Some("5140"), 1)]);
    assert!(!last.truncated);

    let invalid = |r: oxl_engine::Result<_>| matches!(r, Err(EngineError::Invalid(_)));
    assert!(invalid(values(
        &case,
        &st,
        "EventId",
        &[],
        None,
        ValueOrder::Value,
        0,
        0
    )));
    assert!(invalid(values(
        &case,
        &st,
        "EventId",
        &[],
        None,
        ValueOrder::Value,
        10,
        u64::MAX
    )));
    assert!(matches!(
        values(&case, &st, "Nope", &[], None, ValueOrder::Value, 10, 0),
        Err(EngineError::NotFound(_))
    ));
    assert_eq!(
        serde_json::to_value(ValueOrder::Commonest).unwrap(),
        "commonest"
    );
}

#[test]
fn values_list_empty_cells_once() {
    let (_d, case) = loaded();
    let st = state(Vec::new());
    let all = listed(&case, &st, "TimeCreated", &[], None, ValueOrder::Value);
    // Eight distinct times (the identical 1102 pair shares one), the text
    // that did not convert, and SRV01's blank time as one empty entry, last.
    assert_eq!(all.len(), 10);
    assert_eq!(all.iter().filter(|(v, _)| v.is_none()).count(), 1);
    assert_eq!(all[8], (Some("not-a-time".to_string()), 1));
    assert_eq!(all[9], (None, 1));
    assert_eq!(
        all[0],
        (Some("2026-01-05T02:13:58.2040000Z".to_string()), 1)
    );

    // Narrowed to SRV01, its two times and the blank one.
    let mut srv = st.clone();
    srv.hosts = vec!["SRV01".into()];
    assert_eq!(
        listed(&case, &srv, "TimeCreated", &[], None, ValueOrder::Commonest),
        vec![
            (Some("2026-01-05T02:22:03.1180000Z".to_string()), 1),
            (Some("2026-01-05T02:22:04.5510000Z".to_string()), 1),
            (None, 1),
        ]
    );
    // The `_mark` pseudo-column lists marks: none here.
    assert_eq!(
        listed(&case, &st, "_mark", &[], None, ValueOrder::Value),
        vec![(None, 11)]
    );
}

#[test]
fn counts_report_failed_values() {
    let (_d, case) = loaded();
    let all = count_view(&case, &state(Vec::new())).unwrap();
    // WS01's `not-a-time` TimeCreated is the one typed cell that did not
    // convert; SRV01's blank one is empty, not failed.
    assert_eq!((all.rows, all.total, all.failed_values), (11, 11, 1));

    let srv = count_view(
        &case,
        &state(vec![chip(
            "c1",
            r#"{"column":"Computer","op":"eq","value":"SRV01"}"#,
        )]),
    )
    .unwrap();
    assert_eq!((srv.rows, srv.total, srv.failed_values), (3, 11, 0));

    let mut ws = state(Vec::new());
    ws.hosts = vec!["ws01".into()];
    let ws = count_view(&case, &ws).unwrap();
    assert_eq!((ws.rows, ws.total, ws.failed_values), (8, 11, 1));
}

#[test]
fn empty_columns_under_a_filter() {
    let (_d, case) = loaded();
    // The fixture's run writes no identity metadata, so `_triage_identity`
    // is empty on every row. Every other column has text on some row:
    // TimeCreated is blank only on SRV01's last one.
    let identity = "_triage_identity";
    assert_eq!(
        empty_columns(&case, &state(Vec::new())).unwrap(),
        [identity]
    );

    // The Sysmon rows: RemoteHost is empty on the two process creations but
    // not on the network connection (203.0.113.7), so it is not listed. No
    // data column is empty on all three.
    let sysmon = chip("c1", r#"{"column":"Channel","op":"eq","value":"Sysmon"}"#);
    let st = state(vec![sysmon.clone()]);
    assert_eq!(empty_columns(&case, &st).unwrap(), [identity]);

    // The two Sysmon process creations (EventId 1): RemoteHost is empty on
    // both, and it is the only data column that is. Listed in display
    // order, where the metadata columns come first.
    let st = state(vec![
        sysmon,
        chip("c2", r#"{"column":"EventId","op":"eq","value":"1"}"#),
    ]);
    assert_eq!(empty_columns(&case, &st).unwrap(), [identity, "RemoteHost"]);

    // No row at all: every catalogued column is empty.
    let st = state(vec![chip(
        "c1",
        r#"{"column":"Channel","op":"eq","value":"nothing"}"#,
    )]);
    let names: Vec<String> = oxl_engine::columns(&case, EVENTS)
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(empty_columns(&case, &st).unwrap(), names);
}
