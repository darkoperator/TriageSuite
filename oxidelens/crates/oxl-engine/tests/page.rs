//! The grid's data contract: pages of cells, counts, and the scope every
//! query starts from (sort, filter, search, hosts, columns).

mod common;

use common::{analyst, loaded, n};
use oxl_engine::{
    count, load_file, load_run, page, parse_filter, FileOptions, GridQuery, MarkState, SortKey,
    CELL_CAP, MAX_LIMIT,
};
use oxl_fixtures::{empty_dataset, loose_fw_csv, new_case};

const EVENTS: &str = "evtxtriage_events";

fn events() -> GridQuery {
    GridQuery {
        relation: EVENTS.into(),
        ..Default::default()
    }
}

fn code<T>(result: oxl_engine::Result<T>) -> (&'static str, String) {
    match result {
        Ok(_) => ("ok", String::new()),
        Err(e) => (e.code(), e.to_string()),
    }
}

#[test]
fn the_default_page_is_time_ordered_and_shows_full_precision_text() {
    let (_d, case) = loaded();
    let p = page(&case, &events(), 50, 0).unwrap();
    assert_eq!((p.row_ids.len(), p.truncated), (11, false));
    assert_eq!((p.marks.len(), p.cells.len()), (11, 11));
    let t = p.columns.iter().position(|c| c == "TimeCreated").unwrap();
    assert_eq!(
        p.cells[0][t].text.as_deref(),
        Some("2026-01-05T02:13:58.2040000Z")
    );
    let failed: Vec<_> = p
        .cells
        .iter()
        .filter(|r| r[t].failed)
        .map(|r| r[t].text.clone())
        .collect();
    assert_eq!(failed, vec![Some("not-a-time".to_string())]);
    // The nine timed rows come first, in time order.
    assert!(p.cells[..9]
        .iter()
        .all(|r| !r[t].failed && r[t].text.is_some()));
    let times: Vec<_> = p.cells[..9].iter().map(|r| r[t].text.clone()).collect();
    let mut sorted = times.clone();
    sorted.sort();
    assert_eq!(times, sorted);
    // Then the failed one and the blank one, in row-id order.
    assert_eq!(
        p.cells[9..].iter().filter(|r| r[t].text.is_none()).count(),
        1
    );
    assert!(p.row_ids[9] < p.row_ids[10]);
    // Every catalogued column, none of the engine's own.
    assert!(p.columns.iter().all(|c| !c.starts_with("_ox_")));
    assert!(p.columns.iter().any(|c| c == "_triage_host"));
}

#[test]
fn limit_offset_and_truncated() {
    let (_d, case) = loaded();
    let first = page(&case, &events(), 4, 0).unwrap();
    assert_eq!((first.row_ids.len(), first.truncated), (4, true));
    let second = page(&case, &events(), 4, 4).unwrap();
    assert_eq!((second.row_ids.len(), second.truncated), (4, true));
    let last = page(&case, &events(), 4, 8).unwrap();
    assert_eq!((last.row_ids.len(), last.truncated), (3, false));
    let mut all: Vec<i64> = [first, second, last]
        .into_iter()
        .flat_map(|p| p.row_ids)
        .collect();
    assert_eq!(all.len(), 11);
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), 11);
    let every = page(&case, &events(), 50, 0).unwrap().row_ids;
    let mut every_sorted = every.clone();
    every_sorted.sort_unstable();
    assert_eq!(all, every_sorted);

    let past = page(&case, &events(), 4, 11).unwrap();
    assert_eq!((past.row_ids.len(), past.truncated), (0, false));
}

#[test]
fn sort_filter_search_hosts_and_columns_compose() {
    let (_d, case) = loaded();
    let q = GridQuery {
        relation: EVENTS.into(),
        filter: Some(parse_filter(r#"{"column":"Channel","op":"eq","value":"Security"}"#).unwrap()),
        search: Some("svc_backup".into()),
        hosts: vec!["WS01".into()],
        sort: vec![SortKey {
            column: "EventId".into(),
            descending: true,
        }],
        columns: Some(vec!["EventId".into(), "UserName".into()]),
        ..Default::default()
    };
    let p = page(&case, &q, 50, 0).unwrap();
    assert_eq!(p.columns, vec!["EventId", "UserName"]);
    assert_eq!(
        p.cells
            .iter()
            .map(|r| r[0].text.clone().unwrap())
            .collect::<Vec<_>>(),
        ["4624", "1102", "1102"]
    );
    assert!(p.cells.iter().all(|r| r.len() == 2));
    assert_eq!(count(&case, &q).unwrap(), 3);

    // Each part narrows on its own.
    let hosts = |h: &[&str]| GridQuery {
        hosts: h.iter().map(|s| s.to_string()).collect(),
        ..events()
    };
    assert_eq!(count(&case, &hosts(&["SRV01"])).unwrap(), 3);
    assert_eq!(count(&case, &hosts(&["SRV01", "WS01"])).unwrap(), 11);
    assert_eq!(count(&case, &hosts(&["nope"])).unwrap(), 0);
    let search = GridQuery {
        search: Some("^svch0st".into()),
        search_regex: true,
        ..events()
    };
    assert_eq!(count(&case, &search).unwrap(), 2);
    let case_sensitive = GridQuery {
        search: Some("SVC_BACKUP".into()),
        search_case_sensitive: true,
        ..events()
    };
    assert_eq!(count(&case, &case_sensitive).unwrap(), 0);
}

#[test]
fn numeric_sort_uses_the_typed_value() {
    let (_d, case) = loaded();
    let q = GridQuery {
        sort: vec![SortKey {
            column: "EventId".into(),
            descending: false,
        }],
        columns: Some(vec!["EventId".into()]),
        ..events()
    };
    let p = page(&case, &q, 50, 0).unwrap();
    let ids: Vec<_> = p.cells.iter().map(|r| r[0].text.clone().unwrap()).collect();
    assert_eq!(ids[0], "1");
    assert_eq!(
        ids,
        ["1", "1", "3", "1102", "1102", "4624", "4624", "4624", "4625", "4688", "5140"]
    );

    // A text column sorts case-insensitively by its text.
    let q = GridQuery {
        sort: vec![SortKey {
            column: "UserName".into(),
            descending: false,
        }],
        columns: Some(vec!["UserName".into()]),
        ..events()
    };
    let names: Vec<_> = page(&case, &q, 50, 0)
        .unwrap()
        .cells
        .iter()
        .map(|r| r[0].text.clone().unwrap())
        .collect();
    assert_eq!(names[0], "Administrator");
    assert_eq!(names[1], "svc_backup");
    assert_eq!(names[10], "SYSTEM");
}

#[test]
fn marks_come_back_with_the_page() {
    let (_d, case) = loaded();
    let marked = n(
        &case,
        "insert into ox.row_mark (row_id, mark, marked_by, marked_at) \
         select _ox_row, 'suspicious', 'analyst:tester', '2026-01-05T00:00:00.000000Z' \
         from evtxtriage_events where \"PayloadData1\" = 'rclone.exe' returning row_id",
    );
    let p = page(&case, &events(), 50, 0).unwrap();
    let positions: Vec<usize> = p
        .marks
        .iter()
        .enumerate()
        .filter(|(_, m)| m.is_some())
        .map(|(i, _)| i)
        .collect();
    assert_eq!(positions.len(), 1);
    assert_eq!(p.marks[positions[0]], Some(MarkState::Suspicious));
    assert_eq!(p.row_ids[positions[0]], marked);

    // The `_mark` pseudo-column filters through the same scope.
    let q = GridQuery {
        filter: Some(parse_filter(r#"{"column":"_mark","op":"eq","value":"suspicious"}"#).unwrap()),
        ..events()
    };
    assert_eq!(page(&case, &q, 50, 0).unwrap().row_ids, vec![marked]);
    assert_eq!(count(&case, &q).unwrap(), 1);
}

#[test]
fn long_cells_are_clipped_and_flagged() {
    let d = tempfile::tempdir().unwrap();
    let mut case = new_case(d.path());
    let p = d.path().join("long.csv");
    std::fs::write(&p, format!("a,b\n{},short\n", "é".repeat(5_000))).unwrap();
    let r = load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    let q = GridQuery {
        relation: r.relations[0].name.clone(),
        ..Default::default()
    };
    let page = page(&case, &q, 50, 0).unwrap();
    let a = page.columns.iter().position(|c| c == "a").unwrap();
    let b = page.columns.iter().position(|c| c == "b").unwrap();
    let cell = &page.cells[0][a];
    assert_eq!(cell.text.as_ref().unwrap().chars().count(), CELL_CAP);
    assert!(cell.clipped);
    assert_eq!(page.cells[0][b].text.as_deref(), Some("short"));
    assert!(!page.cells[0][b].clipped);
}

#[test]
fn an_empty_relation_and_an_all_excluding_filter_return_empty_pages() {
    let d = tempfile::tempdir().unwrap();
    let mut case = new_case(d.path());
    load_run(&mut case, &empty_dataset(d.path()), &analyst()).unwrap();
    let q = GridQuery {
        relation: "rbtriage_main".into(),
        ..Default::default()
    };
    let p = page(&case, &q, 50, 0).unwrap();
    assert_eq!((p.row_ids.len(), p.truncated), (0, false));
    assert!(!p.columns.is_empty());
    assert_eq!(count(&case, &q).unwrap(), 0);

    let (_d, case) = loaded();
    let q = GridQuery {
        filter: Some(parse_filter(r#"{"column":"EventId","op":"eq","value":"9"}"#).unwrap()),
        ..events()
    };
    let p = page(&case, &q, 50, 0).unwrap();
    assert_eq!((p.row_ids.len(), p.truncated), (0, false));
    assert_eq!(count(&case, &q).unwrap(), 0);
}

#[test]
fn errors_are_typed() {
    let (d, mut case) = loaded();
    let nope = GridQuery {
        relation: "nope".into(),
        ..Default::default()
    };
    assert_eq!(code(page(&case, &nope, 50, 0)).0, "not_found");
    assert_eq!(code(count(&case, &nope)).0, "not_found");
    let sort = GridQuery {
        sort: vec![SortKey {
            column: "nope".into(),
            descending: false,
        }],
        ..events()
    };
    assert_eq!(code(page(&case, &sort, 50, 0)).0, "not_found");
    let cols = GridQuery {
        columns: Some(vec!["nope".into()]),
        ..events()
    };
    assert_eq!(code(page(&case, &cols, 50, 0)).0, "not_found");
    assert_eq!(code(page(&case, &events(), MAX_LIMIT + 1, 0)).0, "invalid");
    assert_eq!(code(page(&case, &events(), MAX_LIMIT, 0)).0, "ok");

    load_file(
        &mut case,
        &loose_fw_csv(d.path()),
        FileOptions::default(),
        &analyst(),
    )
    .unwrap();
    let fw = GridQuery {
        relation: "fw_csv".into(),
        hosts: vec!["WS01".into()],
        ..Default::default()
    };
    assert_eq!(code(page(&case, &fw, 50, 0)).0, "invalid");
    assert_eq!(code(count(&case, &fw)).0, "invalid");
}

#[test]
fn a_regex_duckdb_rejects_is_invalid() {
    let (_d, case) = loaded();
    let q = GridQuery {
        filter: Some(
            parse_filter(r#"{"column":"PayloadData1","op":"regex","value":"("}"#).unwrap(),
        ),
        ..events()
    };
    let (c, m) = code(page(&case, &q, 50, 0));
    assert_eq!(c, "invalid", "{m}");
    assert!(m.contains("\"(\" is not a valid regex"), "{m}");
    assert_eq!(code(count(&case, &q)).0, "invalid");

    // Nested under `not` and `or` it is found all the same.
    let q = GridQuery {
        filter: Some(
            parse_filter(
                r#"{"or":[{"column":"EventId","op":"eq","value":"1"},{"not":{"column":"UserName","op":"regex","value":"a[b"}}]}"#,
            )
            .unwrap(),
        ),
        ..events()
    };
    assert_eq!(code(count(&case, &q)).0, "invalid");
    // A valid one runs.
    let q = GridQuery {
        filter: Some(
            parse_filter(r#"{"column":"PayloadData1","op":"regex","value":"^svch0st"}"#).unwrap(),
        ),
        ..events()
    };
    assert_eq!(count(&case, &q).unwrap(), 2);
}

#[test]
fn a_search_regex_duckdb_rejects_is_invalid() {
    let (_d, case) = loaded();
    // The `regex` crate accepts both; DuckDB's RE2 refuses both.
    for pattern in ["a{1001}", "(?<n>a)"] {
        let q = GridQuery {
            search: Some(pattern.into()),
            search_regex: true,
            ..events()
        };
        // The probe's wording, not parse_search's, so the probe is what refused it.
        let probe = format!("{pattern:?} is not a valid regex");
        let (c, m) = code(page(&case, &q, 50, 0));
        assert_eq!(c, "invalid", "{pattern}: {m}");
        assert!(m.contains(&probe), "{pattern}: {m}");
        let (c, m) = code(count(&case, &q));
        assert_eq!(c, "invalid", "{pattern}: {m}");
        assert!(m.contains(&probe), "{pattern}: {m}");
    }
}

#[test]
fn the_grid_query_round_trips_through_json() {
    let q: GridQuery = serde_json::from_str(r#"{"relation":"evtxtriage_events"}"#).unwrap();
    assert!(q.filter.is_none() && q.sort.is_empty() && q.columns.is_none());
    let q: GridQuery = serde_json::from_str(
        r#"{"relation":"evtxtriage_events","sort":[{"column":"EventId"}],
            "filter":{"column":"Channel","op":"eq","value":"Security"},"hosts":["WS01"]}"#,
    )
    .unwrap();
    assert!(!q.sort[0].descending);
    let (_d, case) = loaded();
    assert_eq!(count(&case, &q).unwrap(), 5);
    let again: GridQuery = serde_json::from_str(&serde_json::to_string(&q).unwrap()).unwrap();
    assert_eq!(count(&case, &again).unwrap(), 5);
}
