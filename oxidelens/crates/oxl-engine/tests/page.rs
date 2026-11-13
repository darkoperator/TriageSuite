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

/// DuckDB's OFFSET is a signed 64-bit value: an offset past it was a raw
/// conversion error, code `engine`. It is the caller's input, so `invalid`.
#[test]
fn offset_past_i64_is_invalid() {
    let (_d, case) = loaded();
    let r = n(&case, "select min(_ox_row) from evtxtriage_events");
    let by = vec!["Channel".to_string()];
    let around = oxl_engine::MergedSource::Around {
        host: "WS01".into(),
        at: "2026-01-05T02:15:00Z".into(),
        window_seconds: 300,
    };
    for offset in [u64::MAX, i64::MAX as u64 + 1] {
        let calls: [(&str, (&'static str, String)); 8] = [
            ("page", code(page(&case, &events(), 50, offset))),
            (
                "groups",
                code(oxl_engine::groups(&case, &events(), &by, &[], 50, offset)),
            ),
            (
                "stack",
                code(oxl_engine::stack(
                    &case,
                    &events(),
                    &by,
                    oxl_engine::StackOrder::Rarest,
                    50,
                    offset,
                )),
            ),
            ("marks", code(oxl_engine::marks(&case, None, 50, offset))),
            (
                "history",
                code(oxl_engine::history(&case, None, 50, offset)),
            ),
            (
                "merged_rows",
                code(oxl_engine::merged_rows(&case, &around, 50, offset)),
            ),
            ("tags", code(oxl_engine::tags(&case, 50, offset))),
            ("notes", code(oxl_engine::notes(&case, r, 50, offset))),
        ];
        for (name, (c, m)) in calls {
            assert_eq!(c, "invalid", "{name} with offset {offset}: {m}");
        }
    }
    // The largest offset DuckDB takes is fine, and past every row.
    let p = page(&case, &events(), 50, i64::MAX as u64).unwrap();
    assert!(p.row_ids.is_empty() && !p.truncated);
}

/// The grid's `failed` flag, the `conversion_failed` filter and the row
/// detail's `failed` are one predicate: a typed column's text that is there
/// and did not convert. An empty cell, NULL or `''`, has not failed.
#[test]
fn failed_flag_filter_and_detail_agree() {
    let d = tempfile::tempdir().unwrap();
    let mut case = new_case(d.path());
    let p = d.path().join("times.csv");
    std::fs::write(
        &p,
        "t,k\n2026-01-05 02:14:00,set\n,null\n\"\",empty\nnot-a-time,bad\n",
    )
    .unwrap();
    load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    oxl_engine::declare_type(
        &mut case,
        "times_csv",
        "t",
        oxl_engine::SqlType::Timestamp,
        Some(oxl_engine::TimeSemantics::Unknown),
        &analyst(),
    )
    .unwrap();
    // DuckDB reads a quoted empty field as NULL too, so the `''` cell is
    // written in directly, as a source that does write one would leave it.
    let path = case.path().to_path_buf();
    drop(case);
    {
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute_batch("UPDATE src_1.times_csv SET t = '' WHERE k = 'empty'")
            .unwrap();
    }
    let case = oxl_engine::Case::open(&path).unwrap();
    // The three kinds of cell are really there.
    assert_eq!(
        n(&case, "select count(*) from times_csv where t is null"),
        1
    );
    assert_eq!(n(&case, "select count(*) from times_csv where t = ''"), 1);

    let all = GridQuery {
        relation: "times_csv".into(),
        ..Default::default()
    };
    let grid = page(&case, &all, 50, 0).unwrap();
    let t = grid.columns.iter().position(|c| c == "t").unwrap();
    let k = grid.columns.iter().position(|c| c == "k").unwrap();
    let flagged: Vec<(i64, String)> = grid
        .row_ids
        .iter()
        .zip(&grid.cells)
        .filter(|(_, cells)| cells[t].failed)
        .map(|(id, cells)| (*id, cells[k].text.clone().unwrap()))
        .collect();
    let failing = GridQuery {
        filter: Some(parse_filter(r#"{"column":"t","op":"conversion_failed"}"#).unwrap()),
        ..all.clone()
    };
    let filtered = page(&case, &failing, 50, 0).unwrap().row_ids;
    let detailed: Vec<i64> = grid
        .row_ids
        .iter()
        .copied()
        .filter(|id| {
            let detail = oxl_engine::row_detail(&case, *id).unwrap();
            detail
                .fields
                .iter()
                .find(|(name, _)| name == "t")
                .unwrap()
                .1
                .failed
        })
        .collect();
    assert_eq!(
        flagged.iter().map(|(_, k)| k.as_str()).collect::<Vec<_>>(),
        ["bad"]
    );
    let ids: Vec<i64> = flagged.iter().map(|(id, _)| *id).collect();
    assert_eq!(filtered, ids);
    assert_eq!(detailed, ids);
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

/// The text of the first column of every cell row, in page order.
fn first_column(p: &oxl_engine::Page) -> Vec<Option<String>> {
    p.cells.iter().map(|r| r[0].text.clone()).collect()
}

#[test]
fn descending_typed_sort_puts_nulls_last() {
    let d = tempfile::tempdir().unwrap();
    let mut case = new_case(d.path());
    let p = d.path().join("nums.csv");
    std::fs::write(&p, "n,label\n3,c\n,e\n10,a\n1,b\n,f\n").unwrap();
    load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    // The column is typed, so it sorts as numbers (10 after 3 as text would
    // be wrong) and the empty cells are NULL.
    let typed = oxl_engine::columns(&case, "nums_csv").unwrap();
    assert_eq!(
        typed.iter().find(|c| c.name == "n").unwrap().sql_type,
        Some(oxl_engine::SqlType::BigInt)
    );
    let sorted = |descending| {
        let q = GridQuery {
            relation: "nums_csv".into(),
            sort: vec![SortKey {
                column: "n".into(),
                descending,
            }],
            ..Default::default()
        };
        first_column(&page(&case, &q, 50, 0).unwrap())
    };
    let some = |t: &str| Some(t.to_string());
    assert_eq!(sorted(true), [some("10"), some("3"), some("1"), None, None]);
    assert_eq!(
        sorted(false),
        [some("1"), some("3"), some("10"), None, None]
    );
}

#[test]
fn exact_page_boundary_is_not_truncated() {
    let (_d, case) = loaded();
    // 11 rows: a page of exactly 11 is the last, and one short of it is not.
    let exact = page(&case, &events(), 11, 0).unwrap();
    assert_eq!((exact.row_ids.len(), exact.truncated), (11, false));
    let short = page(&case, &events(), 10, 0).unwrap();
    assert_eq!((short.row_ids.len(), short.truncated), (10, true));
    let tail = page(&case, &events(), 10, 1).unwrap();
    assert_eq!((tail.row_ids.len(), tail.truncated), (10, false));
    let at_end = page(&case, &events(), 1, 10).unwrap();
    assert_eq!((at_end.row_ids.len(), at_end.truncated), (1, false));

    // The same at the largest page a caller may ask for.
    let d = tempfile::tempdir().unwrap();
    let mut case = new_case(d.path());
    for (name, rows) in [("full.csv", MAX_LIMIT), ("over.csv", MAX_LIMIT + 1)] {
        let p = d.path().join(name);
        let body: String = (0..rows).map(|i| format!("{i}\n")).collect();
        std::fs::write(&p, format!("v\n{body}")).unwrap();
        load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    }
    let big = |relation: &str| {
        let q = GridQuery {
            relation: relation.into(),
            ..Default::default()
        };
        let p = page(&case, &q, MAX_LIMIT, 0).unwrap();
        (p.row_ids.len(), p.truncated)
    };
    assert_eq!(big("full_csv"), (MAX_LIMIT as usize, false));
    assert_eq!(big("over_csv"), (MAX_LIMIT as usize, true));
}

#[test]
fn hostile_header_relation_sorts_and_scopes_by_host() {
    let d = tempfile::tempdir().unwrap();
    let mut case = new_case(d.path());
    let p = d.path().join("hostile.csv");
    let long = "x".repeat(600);
    let mut w = csv::Writer::from_path(&p).unwrap();
    let header = [
        "a\"b",
        "line\nbreak",
        long.as_str(),
        "dup",
        "dup",
        "_ox_row",
        "",
    ];
    w.write_record(header).unwrap();
    // Seven rows; column c holds a permutation of 1..=7 of its own (7 is
    // prime, so each multiplier gives a different one), so sorting by any
    // other column gives a different row order.
    let multipliers = [1usize, 2, 3, 4, 5, 6, 3];
    let value = |row: usize, col: usize| (row * multipliers[col] + col) % 7 + 1;
    for row in 0..7 {
        let cells: Vec<String> = (0..7).map(|col| value(row, col).to_string()).collect();
        w.write_record(&cells).unwrap();
    }
    w.flush().unwrap();
    let opts = FileOptions {
        host: Some("WS01".into()),
        ..Default::default()
    };
    let report = load_file(&mut case, &p, opts, &analyst()).unwrap();
    let relation = report.relations[0].name.clone();
    let names: Vec<String> = oxl_engine::columns(&case, &relation)
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .filter(|name| name != "_triage_host")
        .collect();
    assert_eq!(names.len(), 7, "{names:?}");
    let base = GridQuery {
        relation: relation.clone(),
        ..Default::default()
    };
    // The rows in file order, by id.
    let mut ids = page(&case, &base, 50, 0).unwrap().row_ids;
    ids.sort_unstable();
    assert_eq!(ids.len(), 7);
    let mut seen_orders = std::collections::HashSet::new();
    for (col, name) in names.iter().enumerate() {
        for descending in [false, true] {
            let q = GridQuery {
                sort: vec![SortKey {
                    column: name.clone(),
                    descending,
                }],
                columns: Some(vec![name.clone()]),
                hosts: vec!["WS01".into()],
                ..base.clone()
            };
            let p = page(&case, &q, 50, 0).unwrap();
            // The order the column's own values call for, as row ids.
            let mut want: Vec<(usize, i64)> = ids.iter().copied().enumerate().collect();
            want.sort_by_key(|(row, _)| value(*row, col));
            if descending {
                want.reverse();
            }
            let want_ids: Vec<i64> = want.iter().map(|(_, id)| *id).collect();
            assert_eq!(
                p.row_ids, want_ids,
                "sorting by {name:?} descending={descending}"
            );
            let want_cells: Vec<String> = want
                .iter()
                .map(|(row, _)| value(*row, col).to_string())
                .collect();
            let got_cells: Vec<String> = first_column(&p).into_iter().map(Option::unwrap).collect();
            assert_eq!(got_cells, want_cells, "{name:?} descending={descending}");
            // No two sorts agree, so a sort on the wrong column fails.
            assert!(
                seen_orders.insert(p.row_ids.clone()),
                "{name:?} descending={descending} orders the rows as another sort does"
            );
        }
    }
    assert_eq!(seen_orders.len(), 14);
    // Scoped to its host the file's rows are all there; another host is a
    // mistake, not an empty page.
    let scoped = |host: &str| GridQuery {
        hosts: vec![host.into()],
        ..base.clone()
    };
    assert_eq!(count(&case, &scoped("WS01")).unwrap(), 7);
    assert_eq!(count(&case, &scoped("ws01")).unwrap(), 7);
    assert_eq!(
        count(&case, &scoped("SRV01")).unwrap_err().code(),
        "not_found"
    );
}
