//! The grid's data contract: pages of cells, counts, and the scope every
//! query starts from (sort, filter, search, hosts, columns).

mod common;

use common::{analyst, loaded, n};
use oxl_engine::{
    count, load_file, load_run, mark_suspicious, page, parse_filter, FileOptions, GridQuery,
    MarkState, SortKey, CELL_CAP, MAX_LIMIT,
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
fn every_paged_call_refuses_a_limit_of_0_or_over_the_maximum() {
    let (_d, case) = loaded();
    let r = n(&case, "select min(_ox_row) from evtxtriage_events");
    let by = vec!["Channel".to_string()];
    let around = oxl_engine::MergedSource::Around {
        host: "WS01".into(),
        at: "2026-01-05T02:15:00Z".into(),
        window_seconds: 300,
    };
    for limit in [0, MAX_LIMIT + 1] {
        let calls: [(&str, &'static str); 8] = [
            ("page", code(page(&case, &events(), limit, 0)).0),
            (
                "groups",
                code(oxl_engine::groups(&case, &events(), &by, &[], limit, 0)).0,
            ),
            (
                "stack",
                code(oxl_engine::stack(
                    &case,
                    &events(),
                    &by,
                    oxl_engine::StackOrder::Rarest,
                    limit,
                    0,
                ))
                .0,
            ),
            ("marks", code(oxl_engine::marks(&case, None, limit, 0)).0),
            (
                "history",
                code(oxl_engine::history(&case, None, limit, 0)).0,
            ),
            (
                "merged_rows",
                code(oxl_engine::merged_rows(&case, &around, limit, 0)).0,
            ),
            ("tags", code(oxl_engine::tags(&case, limit, 0)).0),
            ("notes", code(oxl_engine::notes(&case, r, limit, 0)).0),
        ];
        for (name, c) in calls {
            assert_eq!(c, "invalid", "{name} with limit {limit}");
        }
    }
    // The bounds themselves are fine.
    for limit in [1, MAX_LIMIT] {
        assert_eq!(code(page(&case, &events(), limit, 0)).0, "ok");
        assert_eq!(
            code(oxl_engine::groups(&case, &events(), &by, &[], limit, 0)).0,
            "ok"
        );
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
    // A host the case does not have is not an empty narrowing but a mistake.
    assert_eq!(
        count(&case, &hosts(&["nope"])).unwrap_err().code(),
        "not_found"
    );
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

/// Found on the first real run: a full-width page read every cell, cut it
/// to the cap and looked up its mark for every matching row before keeping
/// the first 50, which was most of the 0.6 s a page took.
#[test]
fn a_page_reads_cells_and_marks_for_its_own_rows_only() {
    let (_d, mut case) = loaded();
    let marked = n(
        &case,
        "select _ox_row from evtxtriage_events where \"PayloadData1\" = 'rclone.exe'",
    );
    assert!(mark_suspicious(&mut case, &[marked], &analyst()).unwrap()[0].done);
    let before = page(&case, &events(), 3, 2).unwrap();
    let (p, statements) = case
        .statements_during(|| page(&case, &events(), 3, 2).unwrap())
        .unwrap();
    // The statement that orders the rows and keeps a page of them.
    let paging: Vec<&String> = statements
        .iter()
        .filter(|sql| sql.contains("OFFSET"))
        .collect();
    assert_eq!(paging.len(), 1, "{statements:#?}");
    assert!(
        !paging[0].contains("ox.row_mark") && !paging[0].contains("left("),
        "the paging statement reads marks or cuts cells: {}",
        paging[0]
    );
    // The statement that reads the cells reads only the paged rows.
    let reading: Vec<&String> = statements
        .iter()
        .filter(|sql| sql.contains("left("))
        .collect();
    assert_eq!(reading.len(), 1, "{statements:#?}");
    assert!(
        reading[0].contains("_ox_row IN (?, ?, ?)"),
        "{}",
        reading[0]
    );
    // The page is the same page all the same, cells and marks included.
    assert_eq!(
        (p.row_ids.clone(), p.marks.clone(), p.truncated),
        (
            before.row_ids.clone(),
            before.marks.clone(),
            before.truncated
        )
    );
    assert_eq!(p.row_ids.len(), 3);
    assert!(
        p.marks.contains(&Some(MarkState::Suspicious)),
        "{:?}",
        p.marks
    );
    assert_eq!(
        p.cells
            .iter()
            .map(|row| row.iter().map(|c| c.text.clone()).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        before
            .cells
            .iter()
            .map(|row| row.iter().map(|c| c.text.clone()).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );
}

/// Found on the first real run: a host given in another case matched no
/// row, and a host the case lacks gave an empty page with no word of why.
#[test]
fn a_page_matches_its_hosts_without_regard_to_case() {
    let (_d, case) = loaded();
    let q = |hosts: &[&str]| GridQuery {
        hosts: hosts.iter().map(|h| h.to_string()).collect(),
        ..events()
    };
    assert_eq!(count(&case, &q(&["srv01"])).unwrap(), 3);
    assert_eq!(count(&case, &q(&["SRV01", "ws01"])).unwrap(), 11);
    let e = count(&case, &q(&["srv02"])).unwrap_err();
    assert_eq!(e.code(), "not_found", "{e}");
    let e = page(&case, &q(&["srv02"]), 50, 0).unwrap_err();
    assert_eq!(e.code(), "not_found", "{e}");
}

/// A filtered, sorted page against the same rows asked of the view directly.
#[test]
fn a_filtered_sorted_page_matches_the_view() {
    let (_d, case) = loaded();
    let q = GridQuery {
        filter: Some(parse_filter(r#"{"column":"Channel","op":"eq","value":"Security"}"#).unwrap()),
        sort: vec![SortKey {
            column: "EventId".into(),
            descending: true,
        }],
        columns: Some(vec!["EventId".into(), "UserName".into()]),
        ..events()
    };
    let p = page(&case, &q, 3, 1).unwrap();
    // Security rows by EventId (typed) descending, then row id; skip one, take three.
    let mut stmt_ids = Vec::new();
    for i in 0..3 {
        stmt_ids.push(n(
            &case,
            &format!(
                "select _ox_row from evtxtriage_events where Channel = 'Security' \
                 order by EventId desc nulls last, _ox_row limit 1 offset {}",
                1 + i
            ),
        ));
    }
    assert_eq!(p.row_ids, stmt_ids);
    assert!(p.truncated);
    for (row, id) in p.cells.iter().zip(&p.row_ids) {
        let event = n(
            &case,
            &format!("select EventId from evtxtriage_events where _ox_row = {id}"),
        );
        assert_eq!(row[0].text.as_deref(), Some(event.to_string().as_str()));
    }
}

#[test]
fn marks_come_back_with_the_page() {
    let (_d, mut case) = loaded();
    let marked = n(
        &case,
        "select _ox_row from evtxtriage_events where \"PayloadData1\" = 'rclone.exe'",
    );
    assert!(mark_suspicious(&mut case, &[marked], &analyst()).unwrap()[0].done);
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
