//! A row's place in what the grid shows (`row_offset`).

mod common;

use common::{loaded, n};
use oxl_engine::{
    page, parse_filter, row_offset, Chip, ChipOrigin, GridState, SearchMode, SearchState, SortKey,
};

const EVENTS: &str = "evtxtriage_events";

fn chip(id: &str, filter: &str) -> Chip {
    Chip {
        id: id.into(),
        label: format!("chip {id}"),
        enabled: true,
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

#[test]
fn row_offset_is_the_rows_place_in_the_shown_order() {
    let (_d, case) = loaded();
    // In view: every row's offset is its index in the page the grid shows,
    // in the default order and in a sorted one.
    for descending in [false, true] {
        let mut s = state(Vec::new());
        s.sort = vec![SortKey {
            column: "EventId".into(),
            descending,
        }];
        let shown = page(&case, &s.query().unwrap(), 100, 0).unwrap().row_ids;
        assert!(shown.len() > 5);
        for (i, id) in shown.iter().enumerate() {
            assert_eq!(row_offset(&case, &s, *id).unwrap(), Some(i as u64));
        }
    }
}

#[test]
fn a_row_the_chips_leave_out_has_no_place() {
    let (_d, case) = loaded();
    let s = state(vec![chip(
        "c1",
        r#"{"column":"Channel","op":"eq","value":"Security"}"#,
    )]);
    let sysmon = n(
        &case,
        "select min(_ox_row) from evtxtriage_events where \"Channel\" <> 'Security'",
    );
    assert_eq!(row_offset(&case, &s, sysmon).unwrap(), None);
    // The rows it keeps are numbered within the filtered list.
    let shown = page(&case, &s.query().unwrap(), 100, 0).unwrap().row_ids;
    assert_eq!(row_offset(&case, &s, shown[2]).unwrap(), Some(2));
}

#[test]
fn a_row_of_another_relation_or_no_row_has_no_place() {
    let (_d, case) = loaded();
    let pf = n(&case, "select min(_ox_row) from petriage_timeline");
    assert_eq!(row_offset(&case, &state(Vec::new()), pf).unwrap(), None);
    assert_eq!(row_offset(&case, &state(Vec::new()), -1).unwrap(), None);
}

#[test]
fn find_mode_keeps_every_rows_place_and_filter_mode_hides_it() {
    let (_d, case) = loaded();
    let mut s = state(Vec::new());
    s.search = SearchState {
        text: "rclone".into(),
        regex: false,
        case_sensitive: false,
        mode: SearchMode::Find,
    };
    let all = page(&case, &s.query().unwrap(), 100, 0).unwrap().row_ids;
    assert_eq!(row_offset(&case, &s, all[3]).unwrap(), Some(3));
    s.search.mode = SearchMode::Filter;
    assert_eq!(row_offset(&case, &s, all[3]).unwrap(), None);
}
