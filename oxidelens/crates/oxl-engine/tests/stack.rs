//! Stacking: values counted rarest-first with host counts.

mod common;

use common::{analyst, loaded, n};
use oxl_engine::{
    load_file, load_run, mark_suspicious, stack, EngineError, FileOptions, GridQuery, StackOrder,
    CELL_CAP,
};
use oxl_fixtures::{empty_dataset, loose_fw_csv, new_case};

fn q(relation: &str) -> GridQuery {
    GridQuery {
        relation: relation.into(),
        ..Default::default()
    }
}

fn s(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

#[test]
fn long_keys_are_clipped_in_groups_and_stacks() {
    let d = tempfile::tempdir().unwrap();
    let mut case = new_case(d.path());
    let long = "k".repeat(5_000);
    let p = d.path().join("long.csv");
    std::fs::write(&p, format!("a,b\n{long},1\n{long},2\nshort,3\n")).unwrap();
    load_file(&mut case, &p, FileOptions::default(), &analyst()).unwrap();
    let by = s(&["a"]);

    let g = oxl_engine::groups(&case, &q("long_csv"), &by, &[], 50, 0).unwrap();
    let top = &g.items[0];
    assert_eq!(top.count, 2);
    assert_eq!(
        top.key.as_deref().map(|k| k.chars().count()),
        Some(CELL_CAP)
    );
    assert!(top.key_clipped);
    assert!(!g.items[1].key_clipped);
    // The full key stays with the engine's caller and still selects the
    // group; the clipped text alone is a different key and selects nothing.
    assert_eq!(top.full_key.as_deref(), Some(long.as_str()));
    let full = vec![top.full_key.clone()];
    assert_eq!(
        oxl_engine::count(&case, &oxl_engine::group_filter(&q("long_csv"), &by, &full)).unwrap(),
        2
    );
    let clipped = vec![top.key.clone()];
    assert_eq!(
        oxl_engine::count(
            &case,
            &oxl_engine::group_filter(&q("long_csv"), &by, &clipped)
        )
        .unwrap(),
        0
    );
    // Serialised, a row carries the clipped key only.
    let json = serde_json::to_value(top).unwrap();
    assert!(json.get("full_key").is_none(), "{json}");

    let st = stack(&case, &q("long_csv"), &by, StackOrder::Commonest, 50, 0).unwrap();
    assert!(st.cells_clipped);
    assert_eq!(
        st.rows[0].values[0].as_deref().map(|v| v.chars().count()),
        Some(CELL_CAP)
    );
    assert_eq!(st.rows[1].values[0].as_deref(), Some("short"));
    let short = stack(
        &case,
        &q("long_csv"),
        &s(&["b"]),
        StackOrder::Commonest,
        50,
        0,
    )
    .unwrap();
    assert!(!short.cells_clipped);
}

#[test]
fn rarest_first_with_host_counts() {
    let (_d, case) = loaded();
    let st = stack(
        &case,
        &q("petriage_timeline"),
        &s(&["ExecutableName"]),
        StackOrder::Rarest,
        50,
        0,
    )
    .unwrap();
    assert_eq!((st.hosts_total, st.distinct, st.rows_counted), (2, 4, 5));
    assert!(!st.truncated);
    assert_eq!(
        st.rows
            .iter()
            .map(|r| (r.values[0].clone().unwrap(), r.count, r.hosts))
            .collect::<Vec<_>>(),
        [
            ("RCLONE.EXE".to_string(), 1, 1),
            ("SVCH0ST.EXE".to_string(), 1, 1),
            ("WEVTUTIL.EXE".to_string(), 1, 1),
            ("CHROME.EXE".to_string(), 2, 2)
        ]
    );
    assert_eq!(
        (st.rows[3].first.as_deref(), st.rows[3].last.as_deref()),
        (
            Some("2026-01-03T08:00:00.000000Z"),
            Some("2026-01-04T09:00:00.000000Z")
        )
    );
}

#[test]
fn commonest_first_and_two_columns() {
    let (_d, case) = loaded();
    let c = stack(
        &case,
        &q("petriage_timeline"),
        &s(&["ExecutableName"]),
        StackOrder::Commonest,
        50,
        0,
    )
    .unwrap();
    assert_eq!(c.rows[0].values[0].as_deref(), Some("CHROME.EXE"));
    assert_eq!(c.rows[1].values[0].as_deref(), Some("RCLONE.EXE"));

    let two = stack(
        &case,
        &q("evtxtriage_events"),
        &s(&["Channel", "EventId"]),
        StackOrder::Rarest,
        50,
        0,
    )
    .unwrap();
    assert_eq!(two.distinct, 7);
    assert_eq!(two.rows[0].count, 1);
    assert_eq!(two.rows[0].values.len(), 2);
    assert_eq!(
        two.rows.iter().map(|r| r.count).sum::<u64>(),
        two.rows_counted
    );
}

#[test]
fn a_stack_respects_the_query_filter_and_paging() {
    let (_d, case) = loaded();
    let mut query = q("petriage_timeline");
    query.hosts = vec!["WS01".into()];
    let by = s(&["ExecutableName"]);
    let all = stack(&case, &query, &by, StackOrder::Rarest, 50, 0).unwrap();
    assert_eq!((all.distinct, all.hosts_total), (4, 1));
    assert!(all.rows.iter().all(|r| r.hosts == 1));

    let page = stack(&case, &query, &by, StackOrder::Rarest, 2, 0).unwrap();
    assert_eq!(page.rows.len(), 2);
    assert!(page.truncated);
    // The totals describe the whole stack, not the page.
    assert_eq!((page.distinct, page.rows_counted), (4, 4));
    let rest = stack(&case, &query, &by, StackOrder::Rarest, 2, 2).unwrap();
    assert_eq!(rest.rows.len(), 2);
    assert!(!rest.truncated);
}

#[test]
fn marks_and_empty_values_are_counted() {
    let (_d, mut case) = loaded();
    let chrome = n(
        &case,
        "select _ox_row from petriage_timeline where \"ExecutableName\" = 'CHROME.EXE' \
         order by _ox_row limit 1",
    );
    assert!(mark_suspicious(&mut case, &[chrome], &analyst()).unwrap()[0].done);
    let st = stack(
        &case,
        &q("petriage_timeline"),
        &s(&["ExecutableName"]),
        StackOrder::Commonest,
        50,
        0,
    )
    .unwrap();
    assert_eq!(st.rows[0].values[0].as_deref(), Some("CHROME.EXE"));
    assert_eq!(st.rows[0].suspicious, 1);
    assert_eq!(st.rows[1].suspicious, 0);

    // Empty cells stack as one absent value.
    let r = stack(
        &case,
        &q("evtxtriage_events"),
        &s(&["RemoteHost"]),
        StackOrder::Commonest,
        50,
        0,
    )
    .unwrap();
    let absent = r.rows.iter().find(|r| r.values[0].is_none()).unwrap();
    assert_eq!(absent.count, 5);
    assert_eq!(r.rows_counted, 11);
}

#[test]
fn no_host_column_and_empty_inputs() {
    let d = tempfile::tempdir().unwrap();
    let mut case = new_case(d.path());
    let fw = loose_fw_csv(d.path());
    let report = load_file(&mut case, &fw, FileOptions::default(), &analyst()).unwrap();
    let rel = report.relations[0].name.clone();
    let st = stack(
        &case,
        &q(&rel),
        &s(&["action"]),
        StackOrder::Commonest,
        50,
        0,
    )
    .unwrap();
    assert_eq!(st.hosts_total, 0);
    assert_eq!(
        st.rows
            .iter()
            .map(|r| (r.values[0].clone().unwrap(), r.count, r.hosts))
            .collect::<Vec<_>>(),
        [("allow".to_string(), 2, 0), ("deny".to_string(), 1, 0)]
    );

    let d2 = tempfile::tempdir().unwrap();
    let mut case2 = new_case(d2.path());
    load_run(&mut case2, &empty_dataset(d2.path()), &analyst()).unwrap();
    let e = stack(
        &case2,
        &q("rbtriage_main"),
        &s(&["FileName"]),
        StackOrder::Rarest,
        50,
        0,
    )
    .unwrap();
    assert!(e.rows.is_empty());
    assert_eq!((e.distinct, e.rows_counted, e.hosts_total), (0, 0, 0));
    assert!(!e.truncated);
}

#[test]
fn bad_arguments_are_invalid() {
    let (_d, case) = loaded();
    let none: Vec<String> = vec![];
    let five = s(&["Channel", "EventId", "Computer", "UserName", "RemoteHost"]);
    for by in [none, five] {
        let e = stack(
            &case,
            &q("evtxtriage_events"),
            &by,
            StackOrder::Rarest,
            50,
            0,
        )
        .unwrap_err();
        assert!(matches!(e, EngineError::Invalid(_)), "{e}");
    }
    let e = stack(
        &case,
        &q("evtxtriage_events"),
        &s(&["Nope"]),
        StackOrder::Rarest,
        50,
        0,
    )
    .unwrap_err();
    assert!(matches!(e, EngineError::NotFound(_)), "{e}");
}

#[test]
fn a_filter_that_excludes_every_row_gives_an_empty_stack() {
    let (_d, case) = loaded();
    let mut query = q("petriage_timeline");
    // An unknown host is `not_found` now; a filter is what excludes rows.
    query.filter = Some(
        oxl_engine::parse_filter(
            r#"{"column":"ExecutableName","op":"eq","value":"no-such-program.exe"}"#,
        )
        .unwrap(),
    );
    let st = stack(
        &case,
        &query,
        &s(&["ExecutableName"]),
        StackOrder::Rarest,
        50,
        0,
    )
    .unwrap();
    assert!(st.rows.is_empty());
    assert_eq!(
        (st.distinct, st.rows_counted, st.hosts_total, st.truncated),
        (0, 0, 0, false)
    );
}
