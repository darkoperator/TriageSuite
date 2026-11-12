//! Find everywhere, the time pivot and the merged rows both open.

mod common;

use common::{analyst, loaded, n};
use oxl_engine::{
    anchor_of, around, find, load_file, load_run, mark_suspicious, merged_rows, set_primary_time,
    EngineError, FileOptions, FindMode, FindRequest, Listing, MarkState, MergedSource,
    TimeSemantics, MAX_VALUE,
};
use oxl_fixtures::{empty_dataset, loose_fw_csv, new_case};

fn req(v: &str, mode: FindMode) -> FindRequest {
    FindRequest {
        value: v.into(),
        mode,
        hosts: vec![],
        from: None,
        to: None,
    }
}

fn s(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

fn around_ws01() -> MergedSource {
    MergedSource::Around {
        host: "WS01".into(),
        at: "2026-01-05T02:15:44.380000Z".into(),
        window_seconds: 60,
    }
}

/// The `_ox_row` of the Sysmon 1 process creation of svch0st.exe on WS01.
fn svch0st_row(case: &oxl_engine::Case) -> i64 {
    n(
        case,
        "select _ox_row from evtxtriage_events where \"PayloadData1\" = 'svch0st.exe'",
    )
}

/// How many statements `call` makes the case run.
fn statements(case: &oxl_engine::Case, call: impl FnOnce(&oxl_engine::Case)) -> usize {
    case.statements_during(|| call(case)).unwrap().1.len()
}

/// Found on the first real run: a find and a pivot read the catalog again
/// for every relation, about ten statements each, so over 164 relations a
/// pivot spent three quarters of its time on statements that count nothing.
#[test]
fn a_pivot_reads_the_catalog_once_however_many_relations_there_are() {
    let (d, mut case) = loaded();
    let costs = |case: &oxl_engine::Case| {
        let find_req = || req("203.0.113.7", FindMode::Contains);
        [
            statements(case, |c| {
                find(c, &find_req(), &mut |_, _| true).unwrap();
            }),
            statements(case, |c| {
                around(c, "WS01", "2026-01-05T02:15:44.380000Z", 60).unwrap();
            }),
            statements(case, |c| {
                merged_rows(c, &MergedSource::Find(find_req()), 50, 0).unwrap();
            }),
            statements(case, |c| {
                merged_rows(c, &around_ws01(), 50, 0).unwrap();
            }),
        ]
    };
    let before = costs(&case);
    // One relation more, with a host and a time, so every pivot covers it.
    load_file(
        &mut case,
        &loose_fw_csv(d.path()),
        FileOptions {
            host: Some("WS01".into()),
            ..FileOptions::default()
        },
        &analyst(),
    )
    .unwrap();
    let after = costs(&case);
    let grew: Vec<usize> = after.iter().zip(before).map(|(a, b)| a - b).collect();
    // A find asks the new relation its one question and a pivot its one
    // count. A merged list is one statement whatever it covers, and one more
    // for each relation with rows on the page: the find's page has two rows
    // of the new relation, the pivot's has none.
    assert_eq!(grew, [1, 1, 1, 0], "before {before:?}, after {after:?}");
}

/// Found on the first real run: the merged list worked out a summary and
/// looked up a mark for every row of the pivot before keeping the first
/// page of them, which was 0.95 s of the 1.4 s a pivot's row list took.
#[test]
fn a_merged_list_reads_summaries_and_marks_for_its_page_only() {
    let (_d, mut case) = loaded();
    let row = svch0st_row(&case);
    assert!(mark_suspicious(&mut case, &[row], &analyst()).unwrap()[0].done);
    for source in [
        around_ws01(),
        MergedSource::Find(req("svc_backup", FindMode::Contains)),
    ] {
        let (page, statements) = case
            .statements_during(|| merged_rows(&case, &source, 2, 0).unwrap())
            .unwrap();
        // The statement that orders every row and keeps a page of them.
        let paging: Vec<&String> = statements
            .iter()
            .filter(|sql| sql.contains("UNION ALL"))
            .collect();
        assert_eq!(paging.len(), 1, "{statements:#?}");
        assert!(
            !paging[0].contains("ox.row_mark") && !paging[0].contains("array_to_string"),
            "the paging statement reads marks or builds summaries: {}",
            paging[0]
        );
        // The page has them all the same.
        assert_eq!((page.items.len(), page.truncated), (2, true));
        assert!(page.items.iter().all(|r| !r.summary.is_empty()));
        assert!(page.items.iter().all(|r| r.host.is_some()));
        let marked: Vec<i64> = page
            .items
            .iter()
            .filter(|r| r.mark == Some(MarkState::Suspicious))
            .map(|r| r.row_id)
            .collect();
        assert_eq!(marked, [row], "{source:?}");
    }
}

/// Found on the first real run: `--host desktop-…` matched nothing on a case
/// whose host is `DESKTOP-…`, and a host the case does not have gave zero
/// counts with no word of why.
#[test]
fn a_host_matches_without_regard_to_case_and_an_unknown_one_is_not_found() {
    let (_d, case) = loaded();
    let at = "2026-01-05T02:15:44.380000Z";
    let exact = around(&case, "WS01", at, 60).unwrap();
    let lower = around(&case, "ws01", at, 60).unwrap();
    let counts = |c: &[oxl_engine::AroundCount]| c.iter().map(|x| x.count).collect::<Vec<_>>();
    assert_eq!(counts(&lower), counts(&exact));
    let rows = merged_rows(
        &case,
        &MergedSource::Around {
            host: "Ws01".into(),
            at: at.into(),
            window_seconds: 60,
        },
        50,
        0,
    )
    .unwrap();
    assert_eq!(rows.items.len(), 4);
    // Rows show the case's own spelling.
    assert!(rows.items.iter().all(|r| r.host.as_deref() == Some("WS01")));

    let mut by_host = req("svc_backup", FindMode::Exact);
    by_host.hosts = s(&["srv01"]);
    let found = find(&case, &by_host, &mut |_, _| true).unwrap();
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].host.as_deref(), Some("SRV01"));
    assert!(
        found.hosts_not_seen.is_empty(),
        "{:?}",
        found.hosts_not_seen
    );

    for e in [
        around(&case, "NOPE", at, 60).unwrap_err(),
        find(
            &case,
            &FindRequest {
                hosts: s(&["WS01", "NOPE"]),
                ..req("x", FindMode::Contains)
            },
            &mut |_, _| true,
        )
        .unwrap_err(),
        merged_rows(
            &case,
            &MergedSource::Around {
                host: "NOPE".into(),
                at: at.into(),
                window_seconds: 60,
            },
            50,
            0,
        )
        .unwrap_err(),
    ] {
        assert_eq!(e.code(), "not_found", "{e}");
        let message = e.to_string();
        assert!(
            message.contains("NOPE") && message.contains("SRV01, WS01"),
            "{message}"
        );
    }
}

/// Found on the first real run: outside the event log and Hayabusa a
/// merged row's summary was its first four values, which for the MFT are
/// entry and sequence numbers. The relations a pivot reaches say what a row
/// is: a path, a program, a task.
#[test]
fn merged_rows_outside_the_event_log_have_readable_summaries() {
    let d = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::app_timed(&d.path().join("run"));
    let mut case = new_case(d.path());
    load_run(&mut case, &run, &analyst()).unwrap();
    let rows = merged_rows(
        &case,
        &MergedSource::Around {
            host: "WS01".into(),
            at: "2026-01-05T02:17:00Z".into(),
            window_seconds: 300,
        },
        50,
        0,
    )
    .unwrap()
    .items;
    let summary = |rel: &str| {
        rows.iter()
            .find(|r| r.relation == rel)
            .unwrap_or_else(|| panic!("no {rel} row in {rows:#?}"))
            .summary
            .clone()
    };
    assert_eq!(
        summary("appcompattriage_appcompat"),
        r"C:\Tools\rclone.exe · Yes"
    );
    assert_eq!(summary("retriage_taskcache_software"), r"\Updater");
    assert_eq!(summary("sbetriage_shellbags"), r"C:\Users\svc_backup\stage");
    assert_eq!(summary("retriage_userassist_ntuser_dat"), "rclone.exe · 3");
}

#[test]
fn every_pivot_says_how_its_times_are_zoned() {
    let (d, mut case) = loaded();
    load_file(
        &mut case,
        &loose_fw_csv(d.path()),
        FileOptions {
            host: Some("WS01".into()),
            ..FileOptions::default()
        },
        &analyst(),
    )
    .unwrap();
    let (host, at, window) = ("WS01", "2026-01-05T02:15:44.380000Z", 300);
    let semantics_of = |rel: &str, counts: &[oxl_engine::AroundCount]| {
        counts
            .iter()
            .find(|c| c.relation == rel)
            .unwrap()
            .time_semantics
    };
    // A time written without a zone is compared as written, and labelled so.
    let counts = around(&case, host, at, window).unwrap();
    assert_eq!(
        semantics_of("fw_csv", &counts),
        Some(TimeSemantics::Unknown)
    );
    assert!(
        counts
            .iter()
            .find(|c| c.relation == "fw_csv")
            .unwrap()
            .count
            > 0
    );
    assert_eq!(
        semantics_of("evtxtriage_events", &counts),
        Some(TimeSemantics::Utc)
    );

    let source = MergedSource::Around {
        host: host.into(),
        at: at.into(),
        window_seconds: window,
    };
    let rows = merged_rows(&case, &source, 1000, 0).unwrap().items;
    let fw: Vec<_> = rows.iter().filter(|r| r.relation == "fw_csv").collect();
    assert!(!fw.is_empty());
    assert!(fw
        .iter()
        .all(|r| r.time_semantics == Some(TimeSemantics::Unknown)));
    assert!(rows
        .iter()
        .filter(|r| r.relation == "evtxtriage_events")
        .all(|r| r.time_semantics == Some(TimeSemantics::Utc)));
    // Serialised as the catalog spells it.
    assert_eq!(
        serde_json::to_value(fw[0]).unwrap()["time_semantics"],
        "unknown"
    );

    let found = find(&case, &req("203.0.113.7", FindMode::Exact), &mut |_, _| {
        true
    })
    .unwrap();
    let hit = |rel: &str| {
        found
            .hits
            .iter()
            .find(|h| h.relation == rel)
            .unwrap()
            .time_semantics
    };
    assert_eq!(hit("fw_csv"), Some(TimeSemantics::Unknown));
    assert_eq!(hit("evtxtriage_events"), Some(TimeSemantics::Utc));

    let q = |rel: &str| oxl_engine::GridQuery {
        relation: rel.into(),
        ..Default::default()
    };
    let h = oxl_engine::histogram(&case, &q("fw_csv"), None, 60).unwrap();
    assert_eq!(h.time_semantics, Some(TimeSemantics::Unknown));
    let st = oxl_engine::stack(
        &case,
        &q("evtxtriage_events"),
        &s(&["Channel"]),
        oxl_engine::StackOrder::Rarest,
        50,
        0,
    )
    .unwrap();
    assert_eq!(st.time_semantics, Some(TimeSemantics::Utc));
    // A relation with no primary time says nothing.
    set_primary_time(&mut case, "fw_csv", None, &analyst()).unwrap();
    let st = oxl_engine::stack(
        &case,
        &q("fw_csv"),
        &s(&["action"]),
        oxl_engine::StackOrder::Rarest,
        50,
        0,
    )
    .unwrap();
    assert_eq!(st.time_semantics, None);
    let found = find(&case, &req("203.0.113.7", FindMode::Exact), &mut |_, _| {
        true
    })
    .unwrap();
    assert_eq!(
        found
            .hits
            .iter()
            .find(|h| h.relation == "fw_csv")
            .unwrap()
            .time_semantics,
        None
    );
}

#[test]
fn exact_find_attributes_hits_to_dataset_column_and_host() {
    let (_d, case) = loaded();
    let r = find(&case, &req("203.0.113.7", FindMode::Exact), &mut |_, _| {
        true
    })
    .unwrap();
    assert_eq!(r.hits.len(), 1);
    let h = &r.hits[0];
    assert_eq!(
        (
            h.relation.as_str(),
            h.column.as_str(),
            h.host.as_deref(),
            h.count
        ),
        ("evtxtriage_events", "RemoteHost", Some("WS01"), 3)
    );
    assert_eq!(
        (h.first.as_deref(), h.last.as_deref()),
        (
            Some("2026-01-05T02:13:58.204000Z"),
            Some("2026-01-05T02:15:46.018000Z")
        )
    );
    assert_eq!(
        r.searched,
        s(&[
            "evtxtriage_events",
            "ext_hayabusa_csv_timeline",
            "petriage_timeline"
        ])
    );
    assert_eq!(
        r.no_hit,
        vec![
            "ext_hayabusa_csv_timeline".to_string(),
            "petriage_timeline".to_string()
        ]
    );
    assert_eq!(r.hosts_not_seen, vec!["SRV01".to_string()]);
    assert!(!r.cancelled);
}

#[test]
fn contains_find_reaches_packed_fields_and_loose_files() {
    let (d, mut case) = loaded();
    load_file(
        &mut case,
        &loose_fw_csv(d.path()),
        FileOptions::default(),
        &analyst(),
    )
    .unwrap();
    let r = find(
        &case,
        &req("203.0.113.7", FindMode::Contains),
        &mut |_, _| true,
    )
    .unwrap();
    let got: Vec<_> = r
        .hits
        .iter()
        .map(|h| (h.relation.as_str(), h.column.as_str(), h.count))
        .collect();
    assert_eq!(
        got,
        [
            ("evtxtriage_events", "RemoteHost", 3),
            ("ext_hayabusa_csv_timeline", "Details", 1),
            ("fw_csv", "src_ip", 2)
        ]
    );
    // A loose file loaded without a host has hits with no host.
    assert_eq!(r.hits[2].host, None);
    assert_eq!(r.no_hit, s(&["petriage_timeline"]));
}

#[test]
fn find_is_case_insensitive_and_literal() {
    let (_d, case) = loaded();
    let r = find(&case, &req("SVCH0ST.EXE", FindMode::Exact), &mut |_, _| {
        true
    })
    .unwrap();
    // evtx PayloadData1 1 + prefetch 1; "svch0st.exe:443" is not a whole-cell match.
    assert_eq!(r.hits.iter().map(|h| h.count).sum::<u64>(), 2);
    assert!(find(
        &case,
        &req("svc%backup", FindMode::Contains),
        &mut |_, _| true
    )
    .unwrap()
    .hits
    .is_empty());
    assert!(
        find(&case, &req("s_c", FindMode::Contains), &mut |_, _| true)
            .unwrap()
            .hits
            .is_empty()
    );
}

#[test]
fn the_host_column_is_not_searched() {
    let (_d, case) = loaded();
    // Computer is the source's own column; _triage_host only restates the host.
    let r = find(&case, &req("SRV01", FindMode::Exact), &mut |_, _| true).unwrap();
    let got: Vec<_> = r
        .hits
        .iter()
        .map(|h| (h.relation.as_str(), h.column.as_str(), h.count))
        .collect();
    assert_eq!(got, [("evtxtriage_events", "Computer", 3)]);
}

#[test]
fn host_and_time_scope_narrow_the_search() {
    let (_d, mut case) = loaded();
    let mut by_host = req("svc_backup", FindMode::Exact);
    by_host.hosts = s(&["SRV01"]);
    let r = find(&case, &by_host, &mut |_, _| true).unwrap();
    let got: Vec<_> = r
        .hits
        .iter()
        .map(|h| {
            (
                h.relation.as_str(),
                h.column.as_str(),
                h.host.as_deref(),
                h.count,
            )
        })
        .collect();
    assert_eq!(got, [("evtxtriage_events", "UserName", Some("SRV01"), 3)]);
    assert!(r.hosts_not_seen.is_empty());

    let mut by_time = req("203.0.113.7", FindMode::Exact);
    by_time.from = Some("2026-01-05T02:15:00Z".into());
    by_time.to = Some("2026-01-05T02:16:00Z".into());
    let r = find(&case, &by_time, &mut |_, _| true).unwrap();
    assert_eq!(r.hits.len(), 1);
    assert_eq!(r.hits[0].count, 1);
    assert_eq!(
        (r.hits[0].first.as_deref(), r.hits[0].last.as_deref()),
        (
            Some("2026-01-05T02:15:46.018000Z"),
            Some("2026-01-05T02:15:46.018000Z")
        )
    );

    // A time scope leaves out a relation with no primary time.
    set_primary_time(&mut case, "petriage_timeline", None, &analyst()).unwrap();
    let r = find(&case, &by_time, &mut |_, _| true).unwrap();
    assert_eq!(
        r.searched,
        s(&["evtxtriage_events", "ext_hayabusa_csv_timeline"])
    );
}

#[test]
fn a_host_scope_skips_relations_without_a_host_column() {
    let (d, mut case) = loaded();
    load_file(
        &mut case,
        &loose_fw_csv(d.path()),
        FileOptions::default(),
        &analyst(),
    )
    .unwrap();
    let mut q = req("203.0.113.7", FindMode::Contains);
    q.hosts = s(&["WS01"]);
    let r = find(&case, &q, &mut |_, _| true).unwrap();
    assert!(!r.searched.contains(&"fw_csv".to_string()));
    assert!(r.hits.iter().all(|h| h.host.as_deref() == Some("WS01")));
}

#[test]
fn progress_is_called_per_relation_and_can_cancel() {
    let (_d, case) = loaded();
    let mut seen: Vec<(String, usize)> = Vec::new();
    let r = find(
        &case,
        &req("203.0.113.7", FindMode::Contains),
        &mut |rel, hits| {
            seen.push((rel.to_string(), hits.len()));
            true
        },
    )
    .unwrap();
    assert_eq!(
        seen,
        [
            ("evtxtriage_events".to_string(), 1),
            ("ext_hayabusa_csv_timeline".to_string(), 1),
            ("petriage_timeline".to_string(), 0)
        ]
    );
    assert!(!r.cancelled);

    let mut calls = 0;
    let r = find(
        &case,
        &req("203.0.113.7", FindMode::Contains),
        &mut |_, _| {
            calls += 1;
            false
        },
    )
    .unwrap();
    assert_eq!(calls, 1);
    assert!(r.cancelled);
    assert_eq!(r.searched.len(), 1);
    assert_eq!(r.hits.len(), 1);
}

#[test]
fn an_empty_relation_is_searched_without_hits() {
    let d = tempfile::tempdir().unwrap();
    let mut case = new_case(d.path());
    load_run(&mut case, &empty_dataset(d.path()), &analyst()).unwrap();
    let r = find(&case, &req("x", FindMode::Contains), &mut |_, _| true).unwrap();
    assert!(r.hits.is_empty());
    assert_eq!(r.no_hit, s(&["rbtriage_main"]));
    let Listing {
        items: rows,
        truncated,
    } = merged_rows(
        &case,
        &MergedSource::Find(req("x", FindMode::Contains)),
        50,
        0,
    )
    .unwrap();
    assert!(rows.is_empty() && !truncated);
}

#[test]
fn empty_or_oversized_values_are_invalid() {
    let (_d, case) = loaded();
    let long = "a".repeat(MAX_VALUE + 1);
    for value in ["", long.as_str()] {
        let e = find(&case, &req(value, FindMode::Contains), &mut |_, _| true).unwrap_err();
        assert!(matches!(e, EngineError::Invalid(_)), "{e}");
        let e = merged_rows(
            &case,
            &MergedSource::Find(req(value, FindMode::Exact)),
            50,
            0,
        )
        .unwrap_err();
        assert!(matches!(e, EngineError::Invalid(_)), "{e}");
    }
    // The cap counts characters, not bytes.
    let at_cap = "é".repeat(MAX_VALUE);
    assert!(find(&case, &req(&at_cap, FindMode::Contains), &mut |_, _| true).is_ok());

    for bad in ["2026-01-05 02:15:00", "yesterday", "2026-02-30T00:00:00Z"] {
        let mut q = req("x", FindMode::Contains);
        q.from = Some(bad.into());
        let e = find(&case, &q, &mut |_, _| true).unwrap_err();
        assert!(matches!(e, EngineError::Invalid(_)), "{bad}: {e}");
    }
}

#[test]
fn around_counts_every_relation_on_the_host() {
    let (_d, case) = loaded();
    let a = around(&case, "WS01", "2026-01-05T02:15:44.380000Z", 60).unwrap();
    assert_eq!(
        a.iter()
            .map(|c| (c.relation.as_str(), c.count))
            .collect::<Vec<_>>(),
        [
            ("evtxtriage_events", 2),
            ("ext_hayabusa_csv_timeline", 1),
            ("petriage_timeline", 1)
        ]
    );
    // Zero counts are listed too.
    let quiet = around(&case, "SRV01", "2026-01-05T02:15:44.380000Z", 60).unwrap();
    assert_eq!(
        quiet
            .iter()
            .map(|c| (c.relation.as_str(), c.count))
            .collect::<Vec<_>>(),
        [
            ("evtxtriage_events", 0),
            ("ext_hayabusa_csv_timeline", 0),
            ("petriage_timeline", 0)
        ]
    );
}

#[test]
fn around_skips_relations_without_a_host_or_a_time() {
    let (d, mut case) = loaded();
    load_file(
        &mut case,
        &loose_fw_csv(d.path()),
        FileOptions::default(),
        &analyst(),
    )
    .unwrap();
    set_primary_time(&mut case, "petriage_timeline", None, &analyst()).unwrap();
    let a = around(&case, "WS01", "2026-01-05T02:15:44.380000Z", 60).unwrap();
    assert_eq!(
        a.iter().map(|c| c.relation.as_str()).collect::<Vec<_>>(),
        ["evtxtriage_events", "ext_hayabusa_csv_timeline"]
    );
}

#[test]
fn around_refuses_a_bad_anchor_or_window() {
    let (_d, case) = loaded();
    for at in ["", "2026-01-05 02:15:44", "not-a-time"] {
        let e = around(&case, "WS01", at, 60).unwrap_err();
        assert!(matches!(e, EngineError::Invalid(_)), "{at}: {e}");
    }
    for window in [u64::MAX, i64::MAX as u64, 1 << 50] {
        let e = around(&case, "WS01", "2026-01-05T02:15:44Z", window).unwrap_err();
        assert!(matches!(e, EngineError::Invalid(_)), "{window}: {e}");
    }
}

#[test]
fn anchor_of_reads_host_and_time_from_a_row() {
    let (_d, case) = loaded();
    let row = svch0st_row(&case);
    assert_eq!(
        anchor_of(&case, row).unwrap(),
        (
            "WS01".to_string(),
            "2026-01-05T02:15:44.380000Z".to_string()
        )
    );

    let untimed = n(
        &case,
        "select _ox_row from evtxtriage_events where \"TimeCreated__text\" = 'not-a-time'",
    );
    let e = anchor_of(&case, untimed).unwrap_err();
    assert_eq!(e.code(), "invalid", "{e}");

    let e = anchor_of(&case, -1).unwrap_err();
    assert!(matches!(e, EngineError::NotFound(_)), "{e}");
}

#[test]
fn anchor_of_a_row_with_no_host_is_invalid() {
    let (d, mut case) = loaded();
    load_file(
        &mut case,
        &loose_fw_csv(d.path()),
        FileOptions::default(),
        &analyst(),
    )
    .unwrap();
    let row = n(&case, "select min(_ox_row) from fw_csv");
    let e = anchor_of(&case, row).unwrap_err();
    assert!(matches!(e, EngineError::Invalid(_)), "{e}");
}

#[test]
fn merged_rows_are_time_ordered_with_summaries() {
    let (_d, case) = loaded();
    let Listing {
        items: rows,
        truncated,
    } = merged_rows(&case, &around_ws01(), 50, 0).unwrap();
    assert_eq!((rows.len(), truncated), (4, false));
    assert_eq!(
        rows[0].summary,
        "Sysmon · 1 · Process creation · svc_backup"
    );
    assert_eq!(
        rows.iter().map(|r| r.relation.as_str()).collect::<Vec<_>>(),
        [
            "evtxtriage_events",
            "ext_hayabusa_csv_timeline",
            "petriage_timeline",
            "evtxtriage_events"
        ]
    );
    // Hayabusa's listed columns.
    assert_eq!(
        rows[1].summary,
        "high · Proc Exec · Proc: svch0st.exe ¦ User: svc_backup"
    );
    // Fallback: non-metadata, non-timestamp columns.
    assert_eq!(rows[2].summary, "SVCH0ST.EXE · 1");
    assert_eq!(
        rows.iter().map(|r| r.time.as_deref()).collect::<Vec<_>>(),
        [
            Some("2026-01-05T02:15:44.380000Z"),
            Some("2026-01-05T02:15:44.380000Z"),
            Some("2026-01-05T02:15:44.412000Z"),
            Some("2026-01-05T02:15:46.018000Z")
        ]
    );
    assert!(rows.iter().all(|r| r.host.as_deref() == Some("WS01")));
    assert_eq!(rows[0].row_id, svch0st_row(&case));
    assert!(rows.iter().all(|r| r.mark.is_none()));
}

#[test]
fn merged_rows_page_and_carry_marks() {
    let (_d, mut case) = loaded();
    let row = svch0st_row(&case);
    assert!(mark_suspicious(&mut case, &[row], &analyst()).unwrap()[0].done);
    let Listing {
        items: first,
        truncated,
    } = merged_rows(&case, &around_ws01(), 2, 0).unwrap();
    assert_eq!((first.len(), truncated), (2, true));
    assert_eq!(first[0].mark, Some(MarkState::Suspicious));
    assert_eq!(first[1].mark, None);
    let Listing {
        items: rest,
        truncated,
    } = merged_rows(&case, &around_ws01(), 2, 2).unwrap();
    assert_eq!((rest.len(), truncated), (2, false));
    assert_eq!(rest[0].relation, "petriage_timeline");

    let e = merged_rows(&case, &around_ws01(), 1001, 0).unwrap_err();
    assert!(matches!(e, EngineError::Invalid(_)), "{e}");
}

#[test]
fn merged_rows_of_a_find() {
    let (_d, case) = loaded();
    let Listing {
        items: rows,
        truncated,
    } = merged_rows(
        &case,
        &MergedSource::Find(req("203.0.113.7", FindMode::Exact)),
        50,
        0,
    )
    .unwrap();
    assert_eq!((rows.len(), truncated), (3, false));
    assert!(rows.iter().all(|r| r.relation == "evtxtriage_events"));
    let times: Vec<_> = rows.iter().map(|r| r.time.clone().unwrap()).collect();
    assert_eq!(
        times,
        [
            "2026-01-05T02:13:58.204000Z",
            "2026-01-05T02:14:07.911000Z",
            "2026-01-05T02:15:46.018000Z"
        ]
    );
}

#[test]
fn merged_rows_of_a_find_put_untimed_rows_last() {
    let (_d, case) = loaded();
    // svc_backup on SRV01: two timed logons and one with an empty time.
    let mut q = req("svc_backup", FindMode::Exact);
    q.hosts = s(&["SRV01"]);
    let Listing { items: rows, .. } = merged_rows(&case, &MergedSource::Find(q), 50, 0).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.time.is_some()).collect::<Vec<_>>(),
        [true, true, false]
    );
    assert_eq!(
        rows[2].summary,
        "Security · 4624 · Successful logon · svc_backup"
    );
}
