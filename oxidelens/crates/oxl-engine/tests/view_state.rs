//! The grid's state: chips, the search box in filter or find mode, and what
//! it compiles to.

mod common;

use common::{analyst, loaded, n};
use oxl_engine::{
    columns, count_view, group_filter, groups, mark_suspicious, match_count, matches, page,
    parse_filter, relation, Chip, ChipOrigin, EngineError, GridState, SearchMode, SearchState,
    MAX_CHIPS,
};

const EVENTS: &str = "evtxtriage_events";

fn chip(id: &str, filter: &str, enabled: bool) -> Chip {
    Chip {
        id: id.into(),
        label: format!("chip {id}"),
        enabled,
        origin: ChipOrigin::FilterRow,
        filter: parse_filter(filter).unwrap(),
    }
}

fn state(chips: Vec<Chip>) -> GridState {
    GridState {
        relation: EVENTS.into(),
        chips,
        search: SearchState::default(),
        hosts: Vec::new(),
        sort: Vec::new(),
        columns: None,
    }
}

fn is_invalid<T: std::fmt::Debug>(r: oxl_engine::Result<T>) -> bool {
    matches!(r, Err(EngineError::Invalid(_)))
}

#[test]
fn state_ands_enabled_chips() {
    let (_d, case) = loaded();
    let s = state(vec![
        chip(
            "c1",
            r#"{"column":"Channel","op":"eq","value":"Security"}"#,
            true,
        ),
        // Switched off: shown dimmed, not applied. Applied, it would leave
        // only the one SYSTEM row.
        chip(
            "c2",
            r#"{"column":"UserName","op":"eq","value":"SYSTEM"}"#,
            false,
        ),
    ]);
    let counts = count_view(&case, &s).unwrap();
    // The fixture's Security rows: WS01's 4625, 4624, the two identical
    // 1102s and the not-a-time 4688 (5); SRV01's 4624, 5140 and the blank-
    // time 4624 (3). Sysmon's three rows are left out.
    assert_eq!(counts.rows, 8);
    assert_eq!(counts.total, 11);

    // No enabled chip: no filter at all.
    let none = state(vec![chip(
        "c1",
        r#"{"column":"Channel","op":"eq","value":"Security"}"#,
        false,
    )]);
    assert!(none.query().unwrap().filter.is_none());
    assert_eq!(count_view(&case, &none).unwrap().rows, 11);
}

#[test]
fn find_mode_does_not_filter() {
    let (_d, case) = loaded();
    let mut s = state(Vec::new());
    s.search = SearchState {
        text: "rclone".into(),
        regex: false,
        case_sensitive: false,
        mode: SearchMode::Find,
    };
    assert!(s.query().unwrap().search.is_none());
    assert_eq!(count_view(&case, &s).unwrap().rows, 11);
    assert_eq!(match_count(&case, &s).unwrap(), 1);

    let rclone = n(
        &case,
        "select _ox_row from evtxtriage_events where \"PayloadData1\" = 'rclone.exe'",
    );
    let found = matches(&case, &s, None, 10).unwrap();
    // The default order is TimeCreated ascending, untimed last: 02:13:58,
    // 02:14:07, 02:15:44, 02:15:46, then 02:17:02, the rclone row, at 4.
    assert_eq!(found.items.len(), 1);
    assert!(!found.truncated);
    assert_eq!((found.items[0].offset, found.items[0].row_id), (4, rclone));
    // The position is the row's place in the page the grid shows.
    let shown = page(&case, &s.query().unwrap(), 1, 4).unwrap();
    assert_eq!(shown.row_ids, [rclone]);
    // Stepping past the only match finds nothing more.
    assert!(matches(&case, &s, Some(4), 10).unwrap().items.is_empty());
    assert_eq!(matches(&case, &s, Some(3), 10).unwrap().items[0].offset, 4);

    // In filter mode the same search hides the other rows, and find is
    // refused.
    s.search.mode = SearchMode::Filter;
    assert_eq!(count_view(&case, &s).unwrap().rows, 1);
    assert!(is_invalid(matches(&case, &s, None, 10)));
    assert!(is_invalid(match_count(&case, &s)));
}

#[test]
fn find_positions_follow_the_sort_and_the_chips() {
    let (_d, case) = loaded();
    // Every svc_backup row is a match; the chip keeps only SRV01's rows.
    let mut s = state(vec![chip(
        "c1",
        r#"{"column":"Computer","op":"eq","value":"SRV01"}"#,
        true,
    )]);
    s.search = SearchState {
        text: "svc_backup".into(),
        mode: SearchMode::Find,
        ..SearchState::default()
    };
    s.sort = vec![oxl_engine::SortKey {
        column: "EventId".into(),
        descending: true,
    }];
    assert_eq!(match_count(&case, &s).unwrap(), 3);
    let found = matches(&case, &s, None, 2).unwrap();
    assert_eq!(
        found.items.iter().map(|m| m.offset).collect::<Vec<_>>(),
        [0, 1]
    );
    assert!(found.truncated);
    // EventId descending: 5140 first.
    let first = page(&case, &s.query().unwrap(), 1, 0).unwrap();
    assert_eq!(first.row_ids[0], found.items[0].row_id);
    assert!(is_invalid(matches(&case, &s, None, 0)));
    assert!(is_invalid(matches(&case, &s, Some(u64::MAX), 10)));

    // A regex search is checked in find mode as in filter mode.
    s.search.regex = true;
    s.search.text = "(".into();
    assert!(is_invalid(match_count(&case, &s)));
    s.search.text = "^svc_".into();
    assert_eq!(match_count(&case, &s).unwrap(), 3);
    // A blank search finds nothing and narrows nothing.
    s.search.text = "  ".into();
    assert_eq!(match_count(&case, &s).unwrap(), 0);
    assert!(matches(&case, &s, None, 10).unwrap().items.is_empty());
    assert_eq!(count_view(&case, &s).unwrap().rows, 3);
}

#[test]
fn too_many_chips_and_deep_chips_are_invalid() {
    let (_d, case) = loaded();
    let leaf = r#"{"column":"Channel","op":"eq","value":"Security"}"#;

    let many = state(
        (0..=MAX_CHIPS)
            .map(|i| chip(&format!("c{i}"), leaf, false))
            .collect(),
    );
    assert_eq!(many.chips.len(), 51);
    assert!(is_invalid(many.query()));
    assert!(is_invalid(count_view(&case, &many)));
    let fifty = state(
        (0..MAX_CHIPS)
            .map(|i| chip(&format!("c{i}"), leaf, true))
            .collect(),
    );
    assert_eq!(count_view(&case, &fifty).unwrap().rows, 8);

    // Depth 8 on its own is the cap and accepted; the AND the chips are
    // joined in makes it 9.
    let mut deep = leaf.to_string();
    for _ in 0..7 {
        deep = format!(r#"{{"not":{deep}}}"#);
    }
    assert!(parse_filter(&deep).is_ok());
    let deep_state = state(vec![chip("deep", &deep, true)]);
    assert!(is_invalid(deep_state.query()));
    assert!(is_invalid(count_view(&case, &deep_state)));

    let dup = state(vec![chip("same", leaf, true), chip("same", leaf, false)]);
    assert!(is_invalid(dup.query()));

    for bad in ["", "has space", "semi;colon", "ünï", &"x".repeat(65)] {
        let s = state(vec![chip(bad, leaf, true)]);
        assert!(is_invalid(s.query()), "id {bad:?} was accepted");
    }
    let ok = state(vec![chip(&"a-Z_9".repeat(12), leaf, true)]);
    assert_eq!(ok.chips[0].id.len(), 60);
    assert!(ok.query().is_ok());

    let mut long_label = state(vec![chip("c1", leaf, true)]);
    long_label.chips[0].label = "é".repeat(201);
    assert!(is_invalid(long_label.query()));
    long_label.chips[0].label = "é".repeat(200);
    assert!(long_label.query().is_ok());
}

#[test]
fn hostile_chip_values_are_bound() {
    let (_d, case) = loaded();
    let hostile = "'); DROP TABLE ox.note; --";
    let filter = serde_json::json!({"column": "Channel", "op": "eq", "value": hostile});
    let mut s = state(vec![chip("c1", &filter.to_string(), true)]);
    s.chips[0].label = hostile.into();
    assert_eq!(count_view(&case, &s).unwrap().rows, 0);
    assert_eq!(
        n(
            &case,
            "select count(*) from duckdb_tables() \
             where schema_name = 'ox' and table_name = 'note'"
        ),
        1
    );
}

#[test]
fn state_serde_shape() {
    // The app sends the state as JSON; everything but the relation defaults.
    let s: GridState = serde_json::from_str(r#"{"relation":"evtxtriage_events"}"#).unwrap();
    assert!(s.chips.is_empty() && s.hosts.is_empty() && s.sort.is_empty());
    assert!(s.columns.is_none());
    assert_eq!(s.search.mode, SearchMode::Filter);
    assert_eq!(s.search.text, "");

    let full = serde_json::json!({
        "relation": EVENTS,
        "chips": [{
            "id": "c1", "label": "Channel = Security", "enabled": true,
            "origin": "date_range",
            "filter": {"column": "Channel", "op": "eq", "value": "Security", "case_sensitive": false}
        }],
        "search": {"text": "x", "regex": false, "case_sensitive": true, "mode": "find"},
        "hosts": ["WS01"],
        "sort": [{"column": "EventId", "descending": true}],
        "columns": ["Channel"]
    });
    let s: GridState = serde_json::from_value(full.clone()).unwrap();
    assert_eq!(s.chips[0].origin, ChipOrigin::DateRange);
    assert_eq!(s.search.mode, SearchMode::Find);
    assert_eq!(serde_json::to_value(&s).unwrap(), full);
    for origin in [
        ("filter_row", ChipOrigin::FilterRow),
        ("text", ChipOrigin::Text),
        ("values", ChipOrigin::Values),
        ("cell", ChipOrigin::Cell),
        ("histogram", ChipOrigin::Histogram),
        ("advanced", ChipOrigin::Advanced),
        ("group", ChipOrigin::Group),
        ("preset", ChipOrigin::Preset),
    ] {
        assert_eq!(serde_json::to_value(origin.1).unwrap(), origin.0);
    }

    // Unknown fields are refused at every level: the JSON comes from the app.
    for bad in [
        serde_json::json!({"relation": EVENTS, "extra": 1}),
        serde_json::json!({"relation": EVENTS, "search": {"text": "x", "extra": 1}}),
        serde_json::json!({"relation": EVENTS, "chips": [{
            "id": "c1", "label": "", "enabled": true, "origin": "text", "extra": 1,
            "filter": {"column": "Channel", "op": "eq", "value": "Security"}
        }]}),
    ] {
        assert!(serde_json::from_value::<GridState>(bad).is_err());
    }
}

#[test]
fn catalog_types_serialise() {
    let (_d, case) = loaded();
    let cols = serde_json::to_value(columns(&case, EVENTS).unwrap()).unwrap();
    let time = cols
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "TimeCreated")
        .unwrap();
    assert_eq!(
        time,
        &serde_json::json!({
            "name": "TimeCreated",
            "role": "data",
            "text_column": "TimeCreated__text",
            "typed_column": "TimeCreated",
            "sql_type": "TIMESTAMP",
            "time_semantics": "utc",
            "origin": "suite",
        })
    );
    let event_id = cols
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "EventId")
        .unwrap();
    assert_eq!(event_id["sql_type"], "BIGINT");
    assert_eq!(event_id["time_semantics"], serde_json::Value::Null);
    let host = cols
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["role"] == "host")
        .unwrap();
    assert_eq!(host["origin"], serde_json::Value::Null);

    let rel = serde_json::to_value(relation(&case, EVENTS).unwrap()).unwrap();
    assert_eq!(rel["kind"], "dataset");
    assert_eq!(rel["rows"], 11);
    assert_eq!(rel["primary_time"], "TimeCreated");
    assert_eq!(rel["hosts"], serde_json::json!(["SRV01", "WS01"]));
    assert_eq!(
        serde_json::to_value(oxl_engine::TypeOrigin::AppMap).unwrap(),
        "app-map"
    );
    assert_eq!(
        serde_json::to_value(oxl_engine::ColumnRole::OutputFile).unwrap(),
        "output_file"
    );
}

#[test]
fn group_by_mark() {
    let (_d, mut case) = loaded();
    let ids = page(&case, &state(Vec::new()).query().unwrap(), 2, 0)
        .unwrap()
        .row_ids;
    mark_suspicious(&mut case, &ids, &analyst()).unwrap();
    let q = state(Vec::new()).query().unwrap();
    let by = vec!["_mark".to_string()];
    let g = groups(&case, &q, &by, &[], 50, 0).unwrap().items;
    assert_eq!(
        g.iter()
            .map(|g| (g.key.clone(), g.count, g.suspicious))
            .collect::<Vec<_>>(),
        [(None, 9, 0), (Some("suspicious".to_string()), 2, 2)]
    );

    // Nested under a mark key, and drilled into as a filter.
    let by2 = vec!["_mark".to_string(), "Channel".to_string()];
    let inner = groups(&case, &q, &by2, &[Some("suspicious".into())], 50, 0)
        .unwrap()
        .items;
    assert_eq!(inner.iter().map(|g| g.count).sum::<u64>(), 2);
    let unmarked = groups(&case, &q, &by2, &[None], 50, 0).unwrap().items;
    assert_eq!(unmarked.iter().map(|g| g.count).sum::<u64>(), 9);

    let marked = group_filter(&q, &by, &[Some("suspicious".into())]);
    assert_eq!(oxl_engine::count(&case, &marked).unwrap(), 2);
    let none = group_filter(&q, &by, &[None]);
    assert_eq!(oxl_engine::count(&case, &none).unwrap(), 9);
}
