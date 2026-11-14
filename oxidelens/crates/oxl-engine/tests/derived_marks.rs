//! Malicious and Agent: malicious, derived from the timeline (ruling S2):
//! every reader of a row's mark reads the one view, `ox.row_state`; S and U
//! keep off a row that is evidence; the counts carry every state and each
//! relation's share; the worklist lists every marked row across relations.

mod common;

use common::{agent, analyst, loaded, n};
use oxl_engine::{
    clear_suspicious, count, detach_row, groups, histogram, load_run, mark_counts, mark_suspicious,
    marks, merged_rows, page, page_view, parse_filter, promote_rows, relations, row_detail,
    set_entry_status, values, Actor, Case, EntryStatus, GridQuery, GridState, MarkState,
    MergedSource, PageOptions, ValueOrder,
};
use std::collections::BTreeMap;

const EVENTS: &str = "evtxtriage_events";
const PREFETCH: &str = "petriage_timeline";

const FAILED_LOGON_WS01: &str =
    "select _ox_row from evtxtriage_events where MapDescription = 'Failed logon'";
const SHARE_SRV01: &str =
    "select _ox_row from evtxtriage_events where MapDescription = 'Network share accessed'";
const RCLONE_WS01: &str = "select _ox_row from evtxtriage_events \
                           where PayloadData1 = 'rclone.exe' and _triage_host = 'WS01'";

fn q(relation: &str) -> GridQuery {
    GridQuery {
        relation: relation.into(),
        ..Default::default()
    }
}

fn with_mark(relation: &str, op: &str, value: &str) -> GridQuery {
    GridQuery {
        filter: Some(
            parse_filter(&format!(
                r#"{{"column": "_mark", "op": "{op}", "value": "{value}"}}"#
            ))
            .unwrap(),
        ),
        ..q(relation)
    }
}

fn promote(case: &mut Case, row: i64, actor: &Actor) -> i64 {
    let out = promote_rows(case, &[row], actor).unwrap();
    assert!(out[0].created, "{out:?}");
    out[0].entry_id.unwrap()
}

/// The three rows of one relation, one in each state: an analyst's M gives
/// `malicious`, an agent's M `agent_malicious`, an analyst's S
/// `suspicious`. Each row also carried a mark the derived one must hide or
/// replace: the malicious row was the agent's suspicious row first.
struct Three {
    malicious: i64,
    agent: i64,
    suspicious: i64,
}

fn three(case: &mut Case) -> Three {
    let rows = Three {
        malicious: n(case, FAILED_LOGON_WS01),
        agent: n(case, SHARE_SRV01),
        suspicious: n(case, RCLONE_WS01),
    };
    assert!(mark_suspicious(case, &[rows.malicious], &agent()).unwrap()[0].done);
    promote(case, rows.malicious, &analyst());
    promote(case, rows.agent, &agent());
    assert!(mark_suspicious(case, &[rows.suspicious], &analyst()).unwrap()[0].done);
    rows
}

impl Three {
    fn expected(&self) -> [(i64, MarkState); 3] {
        [
            (self.malicious, MarkState::Malicious),
            (self.agent, MarkState::AgentMalicious),
            (self.suspicious, MarkState::Suspicious),
        ]
    }
}

/// The marked rows of a page, by row id.
fn marked(row_ids: &[i64], marks: &[Option<MarkState>]) -> BTreeMap<i64, MarkState> {
    row_ids
        .iter()
        .zip(marks)
        .filter_map(|(id, m)| Some((*id, (*m)?)))
        .collect()
}

#[test]
fn every_mark_reader_uses_row_state() {
    let (_d, mut case) = loaded();
    let rows = three(&mut case);
    let expected: BTreeMap<i64, MarkState> = rows.expected().into_iter().collect();
    let total = count(&case, &q(EVENTS)).unwrap();

    // The page and the grid's page.
    let p = page(&case, &q(EVENTS), 1000, 0).unwrap();
    assert_eq!(marked(&p.row_ids, &p.marks), expected);
    let state = GridState {
        relation: EVENTS.into(),
        chips: Vec::new(),
        search: Default::default(),
        hosts: Vec::new(),
        sort: Vec::new(),
        columns: None,
    };
    let opts = PageOptions {
        zone: "UTC".into(),
        rules: Vec::new(),
    };
    let gp = page_view(&case, &state, 1000, 0, &opts).unwrap();
    assert_eq!(marked(&gp.row_ids, &gp.marks), expected);

    // The filter, one state at a time, and its complement.
    for (row, mark) in rows.expected() {
        let spelled = serde_json::to_value(mark).unwrap();
        let spelled = spelled.as_str().unwrap();
        let only = with_mark(EVENTS, "eq", spelled);
        assert_eq!(count(&case, &only).unwrap(), 1, "{spelled}");
        assert_eq!(
            page(&case, &only, 10, 0).unwrap().row_ids,
            [row],
            "{spelled}"
        );
        assert_eq!(
            count(&case, &with_mark(EVENTS, "ne", spelled)).unwrap(),
            total - 1
        );
    }
    assert_eq!(
        count(&case, &with_mark(EVENTS, "eq", "none")).unwrap(),
        total - 3
    );

    // Grouped by the mark: one group per state, each counted as marked.
    let by = ["_mark".to_string()];
    let grouped: BTreeMap<Option<String>, (u64, u64)> = groups(&case, &q(EVENTS), &by, &[], 50, 0)
        .unwrap()
        .items
        .into_iter()
        .map(|g| (g.key, (g.count, g.suspicious)))
        .collect();
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        grouped,
        BTreeMap::from([
            (some("agent_malicious"), (1, 1)),
            (some("malicious"), (1, 1)),
            (some("suspicious"), (1, 1)),
            (None, (total - 3, 0)),
        ])
    );

    // The Values tab on the mark.
    let listed: BTreeMap<Option<String>, u64> =
        values(&case, &state, "_mark", &[], None, ValueOrder::Value, 50, 0)
            .unwrap()
            .items
            .into_iter()
            .map(|v| (v.value, v.count))
            .collect();
    assert_eq!(
        listed,
        BTreeMap::from([
            (some("agent_malicious"), 1),
            (some("malicious"), 1),
            (some("suspicious"), 1),
            (None, total - 3),
        ])
    );

    // The histogram's marked count: all three rows are timed.
    let h = histogram(&case, &q(EVENTS), None, 60, "UTC").unwrap();
    assert_eq!(h.buckets.iter().map(|b| b.suspicious).sum::<u64>(), 3);

    // The merged list around WS01's two marked rows, and SRV01's one.
    for (host, at) in [
        ("WS01", "2026-01-05T02:15:00Z"),
        ("SRV01", "2026-01-05T02:30:00Z"),
    ] {
        let source = MergedSource::Around {
            host: host.into(),
            at: at.into(),
            window_seconds: 3600,
        };
        let listed = merged_rows(&case, &source, 1000, 0).unwrap().items;
        let got: BTreeMap<i64, MarkState> = listed
            .iter()
            .filter(|r| r.relation == EVENTS)
            .filter_map(|r| Some((r.row_id, r.mark?)))
            .collect();
        let want: BTreeMap<i64, MarkState> = expected
            .iter()
            .filter(|(id, _)| listed.iter().any(|r| r.row_id == **id))
            .map(|(id, m)| (*id, *m))
            .collect();
        assert!(!want.is_empty(), "{host}: none of the marked rows listed");
        assert_eq!(got, want, "{host}");
    }

    // The row detail and the marks listing, with who each mark is from.
    for (row, mark) in rows.expected() {
        let detail = row_detail(&case, row).unwrap();
        assert_eq!(detail.mark, Some(mark), "row {row}");
    }
    assert_eq!(
        row_detail(&case, rows.agent).unwrap().marked_by.as_deref(),
        Some("agent:claude")
    );
    let listed: BTreeMap<i64, (MarkState, String)> = marks(&case, None, 50, 0)
        .unwrap()
        .items
        .into_iter()
        .map(|m| (m.row_id, (m.mark, m.marked_by)))
        .collect();
    assert_eq!(
        listed,
        BTreeMap::from([
            (
                rows.malicious,
                (MarkState::Malicious, "analyst:tester".to_string())
            ),
            (
                rows.agent,
                (MarkState::AgentMalicious, "agent:claude".to_string())
            ),
            (
                rows.suspicious,
                (MarkState::Suspicious, "analyst:tester".to_string())
            ),
        ])
    );
}

#[test]
fn s_and_u_are_refused_on_a_malicious_row() {
    let (_d, mut case) = loaded();
    let row = n(&case, FAILED_LOGON_WS01);
    let entry = promote(&mut case, row, &analyst());
    for actor in [analyst(), agent()] {
        for (what, out) in [
            ("S", mark_suspicious(&mut case, &[row], &actor).unwrap()),
            ("U", clear_suspicious(&mut case, &[row], &actor).unwrap()),
        ] {
            assert!(!out[0].done, "{what} by {actor:?}: {out:?}");
            let reason = out[0].reason.as_deref().unwrap();
            assert!(
                reason.contains(&format!("E-{entry}")) && reason.contains("detach it there"),
                "{what} by {actor:?}: {reason}"
            );
        }
    }
    assert_eq!(
        n(
            &case,
            &format!("select count(*) from ox.row_mark where row_id = {row}")
        ),
        0
    );
    assert_eq!(
        page(&case, &with_mark(EVENTS, "eq", "malicious"), 10, 0)
            .unwrap()
            .row_ids,
        [row]
    );
}

#[test]
fn an_analysts_s_takes_over_an_agents_mark() {
    let (_d, mut case) = loaded();
    let row = n(&case, RCLONE_WS01);
    assert!(mark_suspicious(&mut case, &[row], &agent()).unwrap()[0].done);
    assert_eq!(
        row_detail(&case, row).unwrap().marked_by.as_deref(),
        Some("agent:claude")
    );
    assert!(mark_suspicious(&mut case, &[row], &analyst()).unwrap()[0].done);
    let detail = row_detail(&case, row).unwrap();
    assert_eq!(detail.mark, Some(MarkState::Suspicious));
    assert_eq!(detail.marked_by.as_deref(), Some("analyst:tester"));
    // Now the analyst's: the agent may no longer clear it.
    let refused = &clear_suspicious(&mut case, &[row], &agent()).unwrap()[0];
    assert!(!refused.done);
    assert!(
        refused
            .reason
            .as_deref()
            .unwrap()
            .contains("analyst:tester"),
        "{refused:?}"
    );
}

#[test]
fn an_agent_may_not_clear_an_analysts_mark() {
    let (_d, mut case) = loaded();
    let row = n(&case, RCLONE_WS01);
    assert!(mark_suspicious(&mut case, &[row], &analyst()).unwrap()[0].done);
    let refused = &clear_suspicious(&mut case, &[row], &agent()).unwrap()[0];
    assert!(!refused.done);
    let reason = refused.reason.as_deref().unwrap();
    assert!(
        reason.contains("analyst:tester") && reason.contains("only an analyst"),
        "{reason}"
    );
    // Nor the analyst's derived mark, whose owner is the entry.
    let held = n(&case, FAILED_LOGON_WS01);
    let entry = promote(&mut case, held, &analyst());
    let refused = &clear_suspicious(&mut case, &[held], &agent()).unwrap()[0];
    assert!(!refused.done);
    assert!(
        refused
            .reason
            .as_deref()
            .unwrap()
            .contains(&format!("E-{entry}")),
        "{refused:?}"
    );
}

#[test]
fn agent_entry_rows_are_agent_malicious_until_reviewed() {
    let (_d, mut case) = loaded();
    let row = n(&case, FAILED_LOGON_WS01);
    let entry = promote(&mut case, row, &agent());
    let only = with_mark(EVENTS, "eq", "agent_malicious");
    let state_of = |case: &Case| {
        let p = page(case, &q(EVENTS), 1000, 0).unwrap();
        marked(&p.row_ids, &p.marks)
    };
    assert_eq!(
        state_of(&case),
        BTreeMap::from([(row, MarkState::AgentMalicious)])
    );
    assert_eq!(page(&case, &only, 10, 0).unwrap().row_ids, [row]);
    let counts = mark_counts(&case).unwrap();
    assert_eq!(
        (counts.suspicious, counts.malicious, counts.agent_malicious),
        (0, 0, 1)
    );
    // An agent may not mark it over: the row is already marked.
    assert!(!mark_suspicious(&mut case, &[row], &agent()).unwrap()[0].done);

    set_entry_status(&mut case, entry, EntryStatus::Reviewed, &analyst()).unwrap();
    assert_eq!(
        state_of(&case),
        BTreeMap::from([(row, MarkState::Malicious)])
    );
    assert_eq!(count(&case, &only).unwrap(), 0);
    let counts = mark_counts(&case).unwrap();
    assert_eq!(
        (counts.suspicious, counts.malicious, counts.agent_malicious),
        (0, 1, 0)
    );
}

#[test]
fn counts_by_relation_match_the_filter() {
    let (_d, mut case) = loaded();
    three(&mut case);
    // A second relation, with marks of its own.
    let prefetch = n(
        &case,
        &format!("select min(_ox_row) from {PREFETCH} where _triage_host = 'WS01'"),
    );
    let prefetch_2 = n(
        &case,
        &format!("select max(_ox_row) from {PREFETCH} where _triage_host = 'WS01'"),
    );
    assert_ne!(prefetch, prefetch_2);
    promote(&mut case, prefetch, &analyst());
    assert!(mark_suspicious(&mut case, &[prefetch_2], &agent()).unwrap()[0].done);

    let counts = mark_counts(&case).unwrap();
    let mut seen = 0;
    for relation in relations(&case).unwrap() {
        let name = &relation.name;
        let of = counts.by_relation.iter().find(|r| &r.relation == name);
        let by_filter = |mark: &str| count(&case, &with_mark(name, "eq", mark)).unwrap();
        let (s, m, a) = (
            by_filter("suspicious"),
            by_filter("malicious"),
            by_filter("agent_malicious"),
        );
        match of {
            Some(r) => {
                seen += 1;
                assert_eq!(
                    (r.suspicious, r.malicious, r.agent_malicious),
                    (s, m, a),
                    "{name}"
                );
                assert!(s + m + a > 0, "{name} is listed with no marks");
            }
            None => assert_eq!((s, m, a), (0, 0, 0), "{name}"),
        }
    }
    assert_eq!(seen, counts.by_relation.len(), "a relation listed twice");
    assert_eq!(
        counts
            .by_relation
            .iter()
            .map(|r| (
                r.relation.as_str(),
                r.suspicious,
                r.malicious,
                r.agent_malicious
            ))
            .collect::<Vec<_>>(),
        [(EVENTS, 1, 1, 1), (PREFETCH, 1, 1, 0)]
    );
    assert_eq!(
        (counts.suspicious, counts.malicious, counts.agent_malicious),
        (2, 2, 1)
    );
}

#[test]
fn the_worklist_lists_every_marked_row_across_relations_in_time_order() {
    let (_d, mut case) = loaded();
    let rows = three(&mut case);
    let prefetch = n(
        &case,
        &format!("select min(_ox_row) from {PREFETCH} where _triage_host = 'WS01'"),
    );
    assert!(mark_suspicious(&mut case, &[prefetch], &analyst()).unwrap()[0].done);

    let listed = |marks: Vec<MarkState>| {
        merged_rows(&case, &MergedSource::Marked { marks }, 1000, 0)
            .unwrap()
            .items
    };
    let all = listed(vec![
        MarkState::Suspicious,
        MarkState::Malicious,
        MarkState::AgentMalicious,
    ]);
    let ids: Vec<i64> = all.iter().map(|r| r.row_id).collect();
    let mut want = vec![rows.malicious, rows.agent, rows.suspicious, prefetch];
    want.sort_unstable();
    let mut got = ids.clone();
    got.sort_unstable();
    assert_eq!(got, want);
    assert!(all.iter().any(|r| r.relation == PREFETCH));
    // Time ascending, untimed last.
    let times: Vec<Option<&str>> = all.iter().map(|r| r.time.as_deref()).collect();
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| match (a, b) {
        (Some(a), Some(b)) => a.cmp(b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    assert_eq!(times, sorted);
    for r in &all {
        assert!(r.mark.is_some(), "{r:?}");
    }

    let suspicious: Vec<i64> = listed(vec![MarkState::Suspicious])
        .iter()
        .map(|r| r.row_id)
        .collect();
    let mut want = vec![rows.suspicious, prefetch];
    want.sort_unstable();
    let mut got = suspicious;
    got.sort_unstable();
    assert_eq!(got, want);
    assert_eq!(
        listed(vec![MarkState::AgentMalicious])
            .iter()
            .map(|r| r.row_id)
            .collect::<Vec<_>>(),
        [rows.agent]
    );
    assert!(listed(Vec::new()).is_empty());

    // As the app sends it.
    let source: MergedSource =
        serde_json::from_str(r#"{"marked": {"marks": ["malicious"]}}"#).unwrap();
    assert_eq!(
        merged_rows(&case, &source, 10, 0)
            .unwrap()
            .items
            .iter()
            .map(|r| r.row_id)
            .collect::<Vec<_>>(),
        [rows.malicious]
    );
}

/// A case opened read-only below format 3 has no `ox.row_state`: every
/// reader falls back to the suspicious marks it holds.
#[test]
fn a_case_below_format_3_reads_its_suspicious_marks() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("v2.case.duckdb");
    let mut case = Case::create_at_format_2(&path, "IR-1", &analyst()).unwrap();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    let row = n(&case, RCLONE_WS01);
    assert!(mark_suspicious(&mut case, &[row], &analyst()).unwrap()[0].done);
    drop(case);

    let case = Case::open_read_only(&path).unwrap();
    assert_eq!(case.format(), 2);
    let p = page(&case, &q(EVENTS), 1000, 0).unwrap();
    assert_eq!(
        marked(&p.row_ids, &p.marks),
        BTreeMap::from([(row, MarkState::Suspicious)])
    );
    assert_eq!(
        page(&case, &with_mark(EVENTS, "eq", "suspicious"), 10, 0)
            .unwrap()
            .row_ids,
        [row]
    );
    assert_eq!(
        count(&case, &with_mark(EVENTS, "eq", "malicious")).unwrap(),
        0
    );
    let counts = mark_counts(&case).unwrap();
    assert_eq!(
        (counts.suspicious, counts.malicious, counts.agent_malicious),
        (1, 0, 0)
    );
    assert_eq!(counts.by_relation.len(), 1);
    assert_eq!(
        row_detail(&case, row).unwrap().mark,
        Some(MarkState::Suspicious)
    );
    assert_eq!(marks(&case, None, 10, 0).unwrap().items.len(), 1);
    let worklist = MergedSource::Marked {
        marks: vec![MarkState::Suspicious, MarkState::Malicious],
    };
    assert_eq!(
        merged_rows(&case, &worklist, 10, 0)
            .unwrap()
            .items
            .iter()
            .map(|r| r.row_id)
            .collect::<Vec<_>>(),
        [row]
    );
}

/// An analyst's S on an Agent: malicious row is allowed and stored under the
/// derived mark; once the agent's evidence is detached the row shows the
/// analyst's Suspicious mark. An agent's S on that row is refused.
#[test]
fn an_analysts_s_on_an_agent_malicious_row_is_kept_under_it() {
    let (_d, mut case) = loaded();
    let row = n(&case, SHARE_SRV01);
    let entry = promote(&mut case, row, &agent());
    assert_eq!(
        row_detail(&case, row).unwrap().mark,
        Some(MarkState::AgentMalicious)
    );

    let refused = &mark_suspicious(&mut case, &[row], &agent()).unwrap()[0];
    assert!(!refused.done, "{refused:?}");
    assert!(
        refused
            .reason
            .as_deref()
            .unwrap()
            .contains(&format!("E-{entry}")),
        "{refused:?}"
    );

    let done = &mark_suspicious(&mut case, &[row], &analyst()).unwrap()[0];
    assert!(done.done, "{done:?}");
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.row_mark where row_id = {row} \
                 and mark = 'suspicious' and marked_by = 'analyst:tester'"
            )
        ),
        1
    );
    // Still Agent: malicious while the agent's entry holds it.
    let detail = row_detail(&case, row).unwrap();
    assert_eq!(detail.mark, Some(MarkState::AgentMalicious));
    assert_eq!(detail.marked_by.as_deref(), Some("agent:claude"));

    detach_row(&mut case, entry, row, &agent()).unwrap();
    let detail = row_detail(&case, row).unwrap();
    assert_eq!(detail.mark, Some(MarkState::Suspicious));
    assert_eq!(detail.marked_by.as_deref(), Some("analyst:tester"));
    assert_eq!(
        page(&case, &with_mark(EVENTS, "eq", "suspicious"), 10, 0)
            .unwrap()
            .row_ids,
        [row]
    );
}
