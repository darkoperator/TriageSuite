//! Timeline entries and their evidence rows: M (promote), A (attach),
//! detach, edit, status, delete and re-check, and who may do which.

mod common;

use common::{agent, analyst, loaded, n};
use oxl_engine::{
    attach_rows, delete_entry, detach_row, edit_entry, entries, evidence, history, load_file,
    mark_suspicious, new_entry, promote_rows, set_entry_status, set_primary_time, Case, Entry,
    EntryEdit, EntryOrder, EntryQuery, EntryStatus, EvidenceStanding, FileOptions,
};
use std::cell::RefCell;
use triage_testkit::boundary::assert_total;

fn code<T>(result: oxl_engine::Result<T>) -> (&'static str, String) {
    match result {
        Ok(_) => ("ok", String::new()),
        Err(e) => (e.code(), e.to_string()),
    }
}

const FAILED_LOGON_WS01: &str =
    "select _ox_row from evtxtriage_events where MapDescription = 'Failed logon'";
const SHARE_SRV01: &str =
    "select _ox_row from evtxtriage_events where MapDescription = 'Network share accessed'";
const RCLONE_WS01: &str = "select _ox_row from evtxtriage_events \
                           where PayloadData1 = 'rclone.exe' and _triage_host = 'WS01'";
const NOT_A_TIME_WS01: &str = "select _ox_row from evtxtriage_events where EventId = 4688";
const BLANK_TIME_SRV01: &str = "select _ox_row from evtxtriage_events \
                                where TimeCreated__text is null and _triage_host = 'SRV01'";

/// Every live entry, in the Timeline's order.
fn all(case: &Case) -> Vec<Entry> {
    entries(case, &EntryQuery::default(), 100, 0).unwrap().items
}

fn one(case: &Case, id: i64) -> Entry {
    all(case)
        .into_iter()
        .find(|e| e.id == id)
        .unwrap_or_else(|| panic!("no live entry {id}"))
}

/// The mark `ox.row_state` gives `row`, or empty when it gives none.
fn state(case: &Case, row: i64) -> String {
    let rows = n(
        case,
        &format!("select count(*) from ox.row_state where row_id = {row}"),
    );
    assert!(rows <= 1, "row {row} has {rows} rows in ox.row_state");
    ["malicious", "agent_malicious", "suspicious"]
        .into_iter()
        .find(|mark| {
            n(
                case,
                &format!(
                    "select count(*) from ox.row_state where row_id = {row} and mark = '{mark}'"
                ),
            ) == 1
        })
        .unwrap_or_default()
        .to_string()
}

/// Promote `row` as `actor`; the new entry's id.
fn promote(case: &mut Case, row: i64, actor: &oxl_engine::Actor) -> i64 {
    let out = promote_rows(case, &[row], actor).unwrap();
    assert_eq!(out.len(), 1);
    assert!(out[0].created, "{out:?}");
    out[0].entry_id.unwrap()
}

/// Promote the one row `select` selects as `actor`; the new entry's id.
fn promote_at(case: &mut Case, select: &str, actor: &oxl_engine::Actor) -> i64 {
    let row = n(case, select);
    promote(case, row, actor)
}

fn titled(title: &str) -> EntryEdit {
    EntryEdit {
        title: Some(title.to_string()),
        ..Default::default()
    }
}

#[test]
fn m_makes_one_draft_per_row_with_evidence() {
    let (_d, mut case) = loaded();
    let share = n(&case, SHARE_SRV01);
    let failed = n(&case, FAILED_LOGON_WS01);
    // The later row first: the Timeline orders by time, not by creation.
    let out = promote_rows(&mut case, &[share, failed, share], &analyst()).unwrap();
    assert_eq!(out.len(), 2, "a row named twice is one row: {out:?}");
    assert_eq!(
        out.iter()
            .map(|o| (o.row_id, o.created))
            .collect::<Vec<_>>(),
        [(share, true), (failed, true)]
    );
    assert!(out.iter().all(|o| o.reason.is_none()), "{out:?}");
    let share_entry = out[0].entry_id.unwrap();
    let failed_entry = out[1].entry_id.unwrap();
    assert_ne!(share_entry, failed_entry);

    let listed = all(&case);
    assert_eq!(
        listed.iter().map(|e| e.id).collect::<Vec<_>>(),
        [failed_entry, share_entry]
    );
    let e = &listed[0];
    assert_eq!(e.status, EntryStatus::Draft);
    assert_eq!(e.time.as_deref(), Some("2026-01-05T02:13:58.204000Z"));
    assert_eq!(e.time_source.as_deref(), Some("row"));
    assert!(!e.needs_time);
    assert_eq!(e.title, "Security · 4625 · Failed logon · Administrator");
    assert_eq!(e.description, "");
    assert_eq!(e.created_by, "analyst:tester");
    assert!(!e.agent_added);
    assert_eq!(e.evidence_rows, 1);
    assert_eq!(e.hosts, ["WS01"]);
    assert_eq!(listed[1].hosts, ["SRV01"]);
    assert_eq!(
        listed[1].time.as_deref(),
        Some("2026-01-05T02:22:04.551000Z")
    );

    let rows = evidence(&case, failed_entry).unwrap();
    assert_eq!(rows.len(), 1);
    let ev = &rows[0];
    assert_eq!(ev.row_id, failed);
    assert_eq!(ev.relation, "evtxtriage_events");
    assert_eq!(ev.host.as_deref(), Some("WS01"));
    assert_eq!(ev.standing, EvidenceStanding::Unchanged);
    assert_eq!(ev.summary, e.title);
    assert_eq!(ev.attached_by, "analyst:tester");
    // The hash and the raw row were kept with it.
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.evidence v join evtxtriage_events r \
                 on r._ox_row = v.row_id where v.row_id = {failed} and v.row_hash = r._ox_hash \
                 and json_extract_string(v.snapshot_json, '$.MapDescription') = 'Failed logon'"
            )
        ),
        1
    );
    // S7: each host has a record, an analyst's reviewed, and the entry links it.
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.host where name in ('WS01', 'SRV01') \
             and status = 'reviewed' and created_by = 'analyst:tester'"
        ),
        2
    );
    assert_eq!(state(&case, failed), "malicious");
    assert_eq!(state(&case, share), "malicious");
}

#[test]
fn an_agents_m_makes_an_unreviewed_entry_and_host_record() {
    let (_d, mut case) = loaded();
    let failed = n(&case, FAILED_LOGON_WS01);
    let id = promote(&mut case, failed, &agent());
    let e = one(&case, id);
    assert_eq!(e.status, EntryStatus::Unreviewed);
    assert!(e.agent_added);
    assert_eq!(e.created_by, "agent:claude");
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.host where name = 'WS01' \
             and status = 'unreviewed' and created_by = 'agent:claude'"
        ),
        1
    );
    assert_eq!(state(&case, failed), "agent_malicious");
    // A second row from the same host reuses its record.
    let rclone = n(&case, RCLONE_WS01);
    promote(&mut case, rclone, &analyst());
    assert_eq!(n(&case, "select count(*) from ox.host"), 1);
    // An analyst reviewing the agent's entry makes its row malicious; the
    // agent's label stays.
    set_entry_status(&mut case, id, EntryStatus::Reviewed, &analyst()).unwrap();
    let e = one(&case, id);
    assert!(e.agent_added);
    assert_eq!(e.reviewed_by.as_deref(), Some("analyst:tester"));
    assert_eq!(state(&case, failed), "malicious");
}

#[test]
fn m_removes_the_suspicious_mark_in_the_same_transaction() {
    let (_d, mut case) = loaded();
    let failed = n(&case, FAILED_LOGON_WS01);
    assert!(mark_suspicious(&mut case, &[failed], &analyst()).unwrap()[0].done);
    let marked = format!("select count(*) from ox.row_mark where row_id = {failed}");

    // One transaction: M writes five history rows, entry_create,
    // evidence_attach, host_create, link_add and last mark_clear, after the
    // mark is deleted and the entry, evidence, host and link are written. A
    // row planted on the fifth id makes that last write fail, and nothing
    // of the call is left.
    let history = n(&case, "select count(*) from ox.history");
    let next = n(&case, "select nextval('ox.history_seq')");
    let fault = next + 5;
    n(
        &case,
        &format!(
            "insert into ox.history values ({fault}, '2026-01-01T00:00:00.000000Z', \
             'analyst:x', 'case', 'x', 'fault', NULL, NULL) returning 1"
        ),
    );
    let (c, message) = code(promote_rows(&mut case, &[failed], &analyst()));
    assert_eq!(c, "engine", "{message}");
    assert_eq!(n(&case, &marked), 1, "the mark's removal was rolled back");
    for table in ["ox.entry", "ox.evidence", "ox.host", "ox.entry_host"] {
        assert_eq!(
            n(&case, &format!("select count(*) from {table}")),
            0,
            "{table} was rolled back"
        );
    }
    assert_eq!(n(&case, "select count(*) from ox.history"), history + 1);
    n(
        &case,
        &format!("delete from ox.history where id = {fault} returning 1"),
    );

    promote(&mut case, failed, &analyst());
    assert_eq!(n(&case, &marked), 0);
    // The fifth id is the clear's: the call writes five history rows, the
    // clear last, so the fault hit after the mark was deleted.
    assert_eq!(n(&case, "select count(*) from ox.history"), history + 5);
    assert_eq!(
        case.scalar_text(
            "select action || ':' || target_id from ox.history order by id desc limit 1"
        )
        .unwrap()
        .unwrap(),
        format!("mark_clear:{failed}")
    );

    // An agent clears only its own mark: an analyst's stays (under the
    // derived mark), the agent's own goes.
    let rclone = n(&case, RCLONE_WS01);
    let share = n(&case, SHARE_SRV01);
    assert!(mark_suspicious(&mut case, &[rclone], &analyst()).unwrap()[0].done);
    assert!(mark_suspicious(&mut case, &[share], &agent()).unwrap()[0].done);
    let out = promote_rows(&mut case, &[rclone, share], &agent()).unwrap();
    assert!(out.iter().all(|o| o.created), "{out:?}");
    let left = |row: i64| {
        n(
            &case,
            &format!("select count(*) from ox.row_mark where row_id = {row}"),
        )
    };
    assert_eq!((left(rclone), left(share)), (1, 0));
    assert_eq!(state(&case, rclone), "agent_malicious");
}

#[test]
fn m_on_a_malicious_row_returns_its_entry() {
    let (_d, mut case) = loaded();
    let failed = n(&case, FAILED_LOGON_WS01);
    let id = promote(&mut case, failed, &analyst());
    for actor in [analyst(), agent()] {
        let out = promote_rows(&mut case, &[failed], &actor).unwrap();
        assert_eq!(
            (out[0].entry_id, out[0].created, out[0].reason.clone()),
            (Some(id), false, None),
            "{actor:?}"
        );
    }
    let rclone = n(&case, RCLONE_WS01);
    let agents = promote(&mut case, rclone, &agent());
    let out = promote_rows(&mut case, &[rclone], &analyst()).unwrap();
    assert_eq!(
        (out[0].entry_id, out[0].created, out[0].reason.as_deref()),
        (Some(agents), false, Some("agent entry; review it"))
    );
    assert_eq!(all(&case).len(), 2);

    // A row no source loaded makes nothing and says why.
    let out = promote_rows(&mut case, &[999_999_999], &analyst()).unwrap();
    assert_eq!((out[0].entry_id, out[0].created), (None, false));
    assert!(
        out[0].reason.as_deref().unwrap().contains("no row"),
        "{out:?}"
    );
    assert_eq!(code(promote_rows(&mut case, &[], &analyst())).0, "invalid");
}

#[test]
fn m_on_an_untimed_row_makes_an_undated_entry() {
    let (d, mut case) = loaded();
    // A typed UTC column whose text failed to convert.
    let failed_conversion = promote_at(&mut case, NOT_A_TIME_WS01, &analyst());
    // A blank cell.
    let blank = promote_at(&mut case, BLANK_TIME_SRV01, &analyst());
    // A ?tz column: a time with no zone, never converted.
    let notes = oxl_fixtures::untimed_file(d.path());
    load_file(&mut case, &notes, FileOptions::default(), &analyst()).unwrap();
    let wiped = n(
        &case,
        "select _ox_row from notes_csv where What = 'disk wiped'",
    );
    let no_zone = promote(&mut case, wiped, &analyst());
    // A dataset with no primary time.
    let plain = d.path().join("plain.csv");
    std::fs::write(&plain, "What,Where\nstaged archive,C:\\stage\n").unwrap();
    load_file(&mut case, &plain, FileOptions::default(), &analyst()).unwrap();
    set_primary_time(&mut case, "plain_csv", None, &analyst()).unwrap();
    let no_time = promote_at(&mut case, "select _ox_row from plain_csv", &analyst());
    // And one timed entry, made last, which still sorts first.
    let timed = promote_at(&mut case, FAILED_LOGON_WS01, &analyst());

    let listed = all(&case);
    assert_eq!(
        listed.iter().map(|e| e.id).collect::<Vec<_>>(),
        [timed, failed_conversion, blank, no_zone, no_time]
    );
    for e in &listed[1..] {
        assert_eq!((e.time.as_deref(), e.time_source.as_deref()), (None, None));
        assert!(e.needs_time, "{e:?}");
        assert_eq!(e.status, EntryStatus::Draft);
    }
    let description = |id: i64| one(&case, id).description;
    assert!(
        description(failed_conversion).contains("not-a-time"),
        "{}",
        description(failed_conversion)
    );
    assert!(
        description(no_zone).contains("2026-01-05 10:00:00"),
        "{}",
        description(no_zone)
    );
    assert_eq!(description(blank), "");
    assert_eq!(description(no_time), "");
}

#[test]
fn an_undated_entry_cannot_be_reviewed() {
    let (_d, mut case) = loaded();
    let id = promote_at(&mut case, NOT_A_TIME_WS01, &analyst());
    let (c, message) = code(set_entry_status(
        &mut case,
        id,
        EntryStatus::Reviewed,
        &analyst(),
    ));
    assert_eq!(c, "refused", "{message}");
    assert!(message.contains("needs a time"), "{message}");
    assert_eq!(one(&case, id).status, EntryStatus::Draft);

    let bad = EntryEdit {
        time: Some(Some("2026-01-05 10:00:00".to_string())),
        ..Default::default()
    };
    assert_eq!(
        code(edit_entry(&mut case, id, &bad, &analyst())).0,
        "invalid"
    );
    let typed = EntryEdit {
        time: Some(Some("2026-01-05T10:00:00Z".to_string())),
        ..Default::default()
    };
    let e = edit_entry(&mut case, id, &typed, &analyst()).unwrap();
    assert_eq!(
        (e.time.as_deref(), e.time_source.as_deref(), e.needs_time),
        (Some("2026-01-05T10:00:00.000000Z"), Some("typed"), false)
    );
    // Only an analyst reviews.
    let (c, message) = code(set_entry_status(
        &mut case,
        id,
        EntryStatus::Reviewed,
        &agent(),
    ));
    assert_eq!(c, "refused", "{message}");
    let e = set_entry_status(&mut case, id, EntryStatus::Reviewed, &analyst()).unwrap();
    assert_eq!(e.status, EntryStatus::Reviewed);
    assert_eq!(e.reviewed_by.as_deref(), Some("analyst:tester"));
    // A reviewed entry keeps its time.
    let clear = EntryEdit {
        time: Some(None),
        ..Default::default()
    };
    assert_eq!(
        code(edit_entry(&mut case, id, &clear, &analyst())).0,
        "refused"
    );
    assert!(one(&case, id).time.is_some());
}

#[test]
fn detaching_the_last_evidence_leaves_the_row_unmarked() {
    let (_d, mut case) = loaded();
    let failed = n(&case, FAILED_LOGON_WS01);
    let rclone = n(&case, RCLONE_WS01);
    assert!(mark_suspicious(&mut case, &[failed], &analyst()).unwrap()[0].done);
    let id = promote(&mut case, failed, &analyst());
    let out = attach_rows(&mut case, id, &[rclone], &analyst()).unwrap();
    assert!(out[0].done && out[0].reason.is_none(), "{out:?}");
    assert_eq!(one(&case, id).evidence_rows, 2);

    detach_row(&mut case, id, rclone, &analyst()).unwrap();
    assert_eq!(state(&case, rclone), "");
    assert_eq!(state(&case, failed), "malicious");
    detach_row(&mut case, id, failed, &analyst()).unwrap();
    // M removed the Suspicious mark, so nothing is left on the row.
    assert_eq!(state(&case, failed), "");
    assert!(evidence(&case, id).unwrap().is_empty());
    let e = one(&case, id);
    assert_eq!(e.evidence_rows, 0);
    // The host link stays: it was the entry's, not the row's.
    assert_eq!(e.hosts, ["WS01"]);

    assert_eq!(
        code(detach_row(&mut case, id, failed, &analyst())).0,
        "not_found"
    );
    assert_eq!(
        code(detach_row(&mut case, 999_999, failed, &analyst())).0,
        "not_found"
    );
}

#[test]
fn attach_links_the_host_and_refuses_an_agent_on_an_analysts_entry() {
    let (_d, mut case) = loaded();
    let e = new_entry(&mut case, &titled("Lateral movement"), &analyst()).unwrap();
    assert_eq!(
        (e.status, e.needs_time, e.evidence_rows),
        (EntryStatus::Draft, true, 0)
    );
    assert!(e.hosts.is_empty());
    let share = n(&case, SHARE_SRV01);
    let failed = n(&case, FAILED_LOGON_WS01);
    let out = attach_rows(&mut case, e.id, &[share, failed, 999_999_999], &analyst()).unwrap();
    assert_eq!(
        out.iter().map(|o| o.done).collect::<Vec<_>>(),
        [true, true, false]
    );
    assert_eq!(one(&case, e.id).hosts, ["SRV01", "WS01"]);
    // Attaching a row already there changes nothing.
    let again = attach_rows(&mut case, e.id, &[share], &analyst()).unwrap();
    assert!(again[0].done, "{again:?}");
    assert!(
        again[0].reason.as_deref().unwrap().contains("already"),
        "{again:?}"
    );
    assert_eq!(one(&case, e.id).evidence_rows, 2);
    assert_eq!(n(&case, "select count(*) from ox.entry_host"), 2);

    // S3: an agent's change to an analyst's entry is refused naming the owner.
    for (what, result) in [
        (
            "attach",
            code(attach_rows(&mut case, e.id, &[share], &agent())),
        ),
        ("detach", code(detach_row(&mut case, e.id, share, &agent()))),
        (
            "edit",
            code(edit_entry(&mut case, e.id, &titled("x"), &agent())),
        ),
        ("delete", code(delete_entry(&mut case, e.id, &agent()))),
    ] {
        assert_eq!(result.0, "refused", "{what}: {}", result.1);
        assert!(result.1.contains("analyst:tester"), "{what}: {}", result.1);
        assert!(result.1.contains("Review tab"), "{what}: {}", result.1);
    }
    // An agent changes its own unreviewed entry.
    let mine = new_entry(&mut case, &titled("Agent's lead"), &agent()).unwrap();
    assert_eq!(mine.status, EntryStatus::Unreviewed);
    assert!(attach_rows(&mut case, mine.id, &[share], &agent()).unwrap()[0].done);
    edit_entry(
        &mut case,
        mine.id,
        &titled("Agent's lead, edited"),
        &agent(),
    )
    .unwrap();
    // Once an analyst reviews it, it is no longer the agent's to change.
    let mine = edit_entry(
        &mut case,
        mine.id,
        &EntryEdit {
            time: Some(Some("2026-01-05T02:22:04Z".into())),
            ..Default::default()
        },
        &analyst(),
    )
    .unwrap();
    set_entry_status(&mut case, mine.id, EntryStatus::Reviewed, &analyst()).unwrap();
    assert_eq!(
        code(edit_entry(&mut case, mine.id, &titled("y"), &agent())).0,
        "refused"
    );
}

#[test]
fn delete_is_a_tombstone_that_drops_links() {
    let (_d, mut case) = loaded();
    let failed = n(&case, FAILED_LOGON_WS01);
    let id = promote(&mut case, failed, &analyst());
    let keep = promote_at(&mut case, RCLONE_WS01, &analyst());
    assert_eq!(n(&case, "select count(*) from ox.entry_host"), 2);

    delete_entry(&mut case, id, &analyst()).unwrap();
    assert_eq!(all(&case).iter().map(|e| e.id).collect::<Vec<_>>(), [keep]);
    // Kept and flagged, its links gone, its evidence kept but marking nothing.
    assert_eq!(
        n(
            &case,
            &format!("select count(*) from ox.entry where id = {id} and deleted")
        ),
        1
    );
    assert_eq!(
        n(
            &case,
            &format!("select count(*) from ox.entry_host where entry_id = {id}")
        ),
        0
    );
    assert_eq!(
        n(
            &case,
            &format!("select count(*) from ox.entry_host where entry_id = {keep}")
        ),
        1
    );
    assert_eq!(
        n(&case, "select count(*) from ox.host where not deleted"),
        1
    );
    assert_eq!(
        n(
            &case,
            &format!("select count(*) from ox.evidence where entry_id = {id}")
        ),
        1
    );
    assert_eq!(state(&case, failed), "");
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.history where target_kind = 'link' \
                 and action = 'link_remove' and target_id = 'entry_host:{id}:' || \
                 (select min(id) from ox.host)"
            )
        ),
        1
    );
    // A deleted entry takes no further change, and M on its row makes a new one.
    assert_eq!(code(delete_entry(&mut case, id, &analyst())).0, "refused");
    assert_eq!(
        code(edit_entry(&mut case, id, &titled("x"), &analyst())).0,
        "refused"
    );
    assert_eq!(
        code(delete_entry(&mut case, 999_999, &analyst())).0,
        "not_found"
    );
    let again = promote(&mut case, failed, &analyst());
    assert_ne!(again, id);
}

#[test]
fn technique_ids_are_validated() {
    let (_d, case) = loaded();
    let case = RefCell::new(case);
    let id = new_entry(&mut case.borrow_mut(), &titled("lead"), &analyst())
        .unwrap()
        .id;
    let long = "T".repeat(5_000);
    let hostile: Vec<&str> = vec![
        "",
        " ",
        "T",
        "T123",
        "T12345",
        "t1059",
        " T1059",
        "T1059 ",
        "T1059\n",
        "T1059.01",
        "T1059.0001",
        "T1059.001.001",
        "T1059.",
        "TA0001",
        "T1059,T1003",
        "T\u{0661}\u{0660}\u{0665}\u{0669}",
        "T1059\u{0}",
        "T１０５９",
        "T1059\u{202E}",
        "=HYPERLINK(\"http://x\")",
        &long,
    ];
    let attempt = |value: &str| {
        let edit = EntryEdit {
            technique_ids: Some(vec!["T1003".to_string(), value.to_string()]),
            ..Default::default()
        };
        code(edit_entry(&mut case.borrow_mut(), id, &edit, &analyst())).0
    };
    assert_total("record::technique_ids", &hostile, attempt);
    for value in &hostile {
        assert_eq!(attempt(value), "invalid", "{value:?}");
    }
    // Nothing a refused edit carried was kept.
    assert!(all(&case.borrow())[0].technique_ids.is_empty());

    let good = EntryEdit {
        technique_ids: Some(vec!["T1059".into(), "T1059.001".into(), "T1059".into()]),
        ..Default::default()
    };
    let e = edit_entry(&mut case.borrow_mut(), id, &good, &analyst()).unwrap();
    assert_eq!(e.technique_ids, ["T1059", "T1059.001"]);
}

#[test]
fn tactics_and_titles_are_validated() {
    let (_d, mut case) = loaded();
    let id = new_entry(&mut case, &titled("lead"), &analyst())
        .unwrap()
        .id;
    for tactic in ["Bogus", "execution ", "", "TA0002"] {
        let edit = EntryEdit {
            tactic: Some(Some(tactic.into())),
            ..Default::default()
        };
        assert_eq!(
            code(edit_entry(&mut case, id, &edit, &analyst())).0,
            "invalid",
            "{tactic:?}"
        );
    }
    // The fourteen ATT&CK Enterprise tactics, as attack.mitre.org lists them
    // (https://attack.mitre.org/tactics/enterprise/, TA0043 through TA0040).
    for tactic in [
        "Reconnaissance",
        "Resource Development",
        "Initial Access",
        "Execution",
        "Persistence",
        "Privilege Escalation",
        "Defense Evasion",
        "Credential Access",
        "Discovery",
        "Lateral Movement",
        "Collection",
        "Command and Control",
        "Exfiltration",
        "Impact",
    ] {
        let edit = EntryEdit {
            tactic: Some(Some(tactic.into())),
            ..Default::default()
        };
        let e = edit_entry(&mut case, id, &edit, &analyst()).unwrap();
        assert_eq!(e.tactic.as_deref(), Some(tactic));
    }
    assert_eq!(oxl_engine::TACTICS.len(), 14);
    // A title is required and bounded.
    for title in ["", "   ", &"x".repeat(201)] {
        assert_eq!(
            code(new_entry(&mut case, &titled(title), &analyst())).0,
            "invalid",
            "{title:?}"
        );
    }
}

#[test]
fn every_change_writes_history() {
    let (_d, mut case) = loaded();
    let actions = |case: &Case, kind: &str| -> Vec<String> {
        let mut out: Vec<String> = history(case, Some(kind), None, 100, 0)
            .unwrap()
            .items
            .into_iter()
            .map(|h| h.action)
            .collect();
        out.reverse();
        out
    };
    let failed = n(&case, FAILED_LOGON_WS01);
    let rclone = n(&case, RCLONE_WS01);
    let id = promote(&mut case, failed, &analyst());
    assert_eq!(actions(&case, "entry"), ["entry_create"]);
    assert_eq!(actions(&case, "evidence"), ["evidence_attach"]);
    assert_eq!(actions(&case, "host"), ["host_create"]);
    assert_eq!(actions(&case, "link"), ["link_add"]);

    attach_rows(&mut case, id, &[rclone], &analyst()).unwrap();
    detach_row(&mut case, id, rclone, &analyst()).unwrap();
    assert_eq!(
        actions(&case, "evidence"),
        ["evidence_attach", "evidence_attach", "evidence_detach"]
    );
    edit_entry(&mut case, id, &titled("Brute force"), &analyst()).unwrap();
    set_entry_status(&mut case, id, EntryStatus::Reviewed, &analyst()).unwrap();
    let other = new_entry(&mut case, &titled("Typed by hand"), &analyst()).unwrap();
    delete_entry(&mut case, other.id, &analyst()).unwrap();
    assert_eq!(
        actions(&case, "entry"),
        [
            "entry_create",
            "entry_edit",
            "entry_status",
            "entry_create",
            "entry_delete"
        ]
    );

    // Before and after are the entry as it was and became.
    let edit = history(&case, Some("entry"), None, 100, 0)
        .unwrap()
        .items
        .into_iter()
        .find(|h| h.action == "entry_edit")
        .unwrap();
    assert_eq!(edit.target_id, id.to_string());
    assert_eq!(edit.by, "analyst:tester");
    assert_eq!(
        edit.before.unwrap()["title"],
        "Security · 4625 · Failed logon · Administrator"
    );
    assert_eq!(edit.after.unwrap()["title"], "Brute force");
    let detach = history(&case, Some("evidence"), None, 1, 0)
        .unwrap()
        .items
        .remove(0);
    assert_eq!(detach.target_id, format!("{id}:{rclone}"));
    assert!(detach.before.is_some() && detach.after.is_none());
    // A refused change writes nothing.
    let before = n(&case, "select count(*) from ox.history");
    let _ = edit_entry(&mut case, id, &titled(""), &analyst());
    let _ = set_entry_status(&mut case, id, EntryStatus::Draft, &agent());
    assert_eq!(n(&case, "select count(*) from ox.history"), before);
}

#[test]
fn recheck_detects_a_changed_row() {
    let (_d, mut case) = loaded();
    let failed = n(&case, FAILED_LOGON_WS01);
    let rclone = n(&case, RCLONE_WS01);
    let id = promote(&mut case, failed, &analyst());
    attach_rows(&mut case, id, &[rclone], &analyst()).unwrap();
    let standings = |case: &Case| -> Vec<(i64, EvidenceStanding)> {
        let mut out: Vec<_> = evidence(case, id)
            .unwrap()
            .into_iter()
            .map(|e| (e.row_id, e.standing))
            .collect();
        out.sort();
        out
    };
    assert_eq!(
        standings(&case),
        [
            (failed, EvidenceStanding::Unchanged),
            (rclone, EvidenceStanding::Unchanged)
        ]
    );
    // The case file is written behind the engine's back.
    let source = n(&case, "select min(id) from ox.source");
    n(
        &case,
        &format!(
            "update src_{source}.evtxtriage_events set MapDescription = 'Logon' \
             where _ox_row = {rclone} returning 1"
        ),
    );
    assert_eq!(
        standings(&case),
        [
            (failed, EvidenceStanding::Unchanged),
            (rclone, EvidenceStanding::Changed)
        ]
    );
}

/// Promoting a selection is a few statements per relation, not a few per
/// row: S10 lets the analyst press M on up to 10,000 rows.
#[test]
fn a_promote_costs_a_few_statements_whatever_its_size() {
    let cost = |rows: usize| -> usize {
        let (_d, mut case) = loaded();
        // One host's rows, so every size links the same one host record.
        let ids: Vec<i64> = (0..rows)
            .map(|i| {
                n(
                    &case,
                    &format!(
                        "select _ox_row from petriage_timeline where _triage_host = 'WS01'                          order by _ox_row limit 1 offset {i}"
                    ),
                )
            })
            .collect();
        let (out, ran) = case
            .statements_during_mut(|c| promote_rows(c, &ids, &analyst()).unwrap())
            .unwrap();
        assert!(out.iter().all(|o| o.created), "{out:?}");
        ran.len()
    };
    assert_eq!(cost(1), cost(4));
}

#[test]
fn a_case_below_format_3_lists_no_entries() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("old.case.duckdb");
    drop(Case::create_at_format_2(&path, "Old", &analyst()).unwrap());
    let case = Case::open_read_only(&path).unwrap();
    assert!(case.format() < 3);
    assert!(entries(&case, &EntryQuery::default(), 10, 0)
        .unwrap()
        .items
        .is_empty());
    assert!(evidence(&case, 1).unwrap().is_empty());
}

#[test]
fn entries_filter_by_status_host_tactic_and_text() {
    let (_d, mut case) = loaded();
    let failed = promote_at(&mut case, FAILED_LOGON_WS01, &analyst());
    let share = promote_at(&mut case, SHARE_SRV01, &agent());
    edit_entry(
        &mut case,
        failed,
        &EntryEdit {
            tactic: Some(Some("Credential Access".into())),
            description: Some("Password spray from 203.0.113.7".into()),
            ..Default::default()
        },
        &analyst(),
    )
    .unwrap();
    let ids = |q: EntryQuery| -> Vec<i64> {
        entries(&case, &q, 100, 0)
            .unwrap()
            .items
            .into_iter()
            .map(|e| e.id)
            .collect()
    };
    assert_eq!(
        ids(EntryQuery {
            status: vec![EntryStatus::Unreviewed],
            ..Default::default()
        }),
        [share]
    );
    assert_eq!(
        ids(EntryQuery {
            host: Some("srv01".into()),
            ..Default::default()
        }),
        [share]
    );
    assert_eq!(
        ids(EntryQuery {
            tactic: Some("Credential Access".into()),
            ..Default::default()
        }),
        [failed]
    );
    assert_eq!(
        ids(EntryQuery {
            text: Some("SPRAY".into()),
            ..Default::default()
        }),
        [failed]
    );
    assert_eq!(
        code(entries(
            &case,
            &EntryQuery {
                tactic: Some("Bogus".into()),
                ..Default::default()
            },
            10,
            0
        ))
        .0,
        "invalid"
    );
    let page = entries(&case, &EntryQuery::default(), 1, 0).unwrap();
    assert_eq!((page.items.len(), page.truncated), (1, true));
    assert_eq!(
        code(entries(&case, &EntryQuery::default(), 0, 0)).0,
        "invalid"
    );
}

#[test]
fn entries_filter_by_an_evidence_row() {
    let (_d, mut case) = loaded();
    let row = n(&case, FAILED_LOGON_WS01);
    let other = n(&case, SHARE_SRV01);
    let first = promote(&mut case, row, &analyst());
    let second = new_entry(&mut case, &titled("Second look"), &analyst())
        .unwrap()
        .id;
    let unrelated = promote(&mut case, other, &analyst());
    attach_rows(&mut case, second, &[row], &analyst()).unwrap();
    let ids = |case: &Case, row: i64| -> Vec<i64> {
        let q = EntryQuery {
            row: Some(row),
            ..Default::default()
        };
        entries(case, &q, 100, 0)
            .unwrap()
            .items
            .into_iter()
            .map(|e| e.id)
            .collect()
    };
    // Every live entry holding the row, and no other: the unrelated entry holds a different row.
    let mut both = ids(&case, row);
    both.sort_unstable();
    assert_eq!(both, [first, second]);
    assert_eq!(ids(&case, other), [unrelated]);
    // A detached row and a deleted entry drop out.
    detach_row(&mut case, second, row, &analyst()).unwrap();
    assert_eq!(ids(&case, row), [first]);
    delete_entry(&mut case, first, &analyst()).unwrap();
    assert!(ids(&case, row).is_empty());
}

/// The written time an undated entry quotes is kept whole up to the
/// description's own bound; only what would pass `MAX_DESCRIPTION` is cut,
/// and the description says so.
#[test]
fn an_undated_entry_quotes_the_written_time_whole() {
    let (_d, mut case) = loaded();
    let source = n(&case, "select min(id) from ox.source");
    let rows = n(&case, "select count(*) from evtxtriage_events");
    let mut written = |text: &str| -> String {
        let row = n(&case, NOT_A_TIME_WS01);
        n(
            &case,
            &format!(
                "update src_{source}.evtxtriage_events set TimeCreated__text = '{text}' \
                 where _ox_row = {row} returning 1"
            ),
        );
        let id = promote(&mut case, row, &analyst());
        let description = one(&case, id).description;
        delete_entry(&mut case, id, &analyst()).unwrap();
        description
    };
    let long = "9".repeat(5_000);
    assert_eq!(written(&long), format!("TimeCreated as written: {long}"));

    let longer = "8".repeat(12_000);
    let description = written(&longer);
    assert_eq!(description.chars().count(), oxl_engine::MAX_DESCRIPTION);
    assert!(
        description.starts_with(&format!("TimeCreated as written: {}", "8".repeat(9_000))),
        "{}",
        &description[..60]
    );
    assert!(
        description.ends_with("(cut to fit the description)"),
        "{}",
        &description[description.len() - 60..]
    );
    assert_eq!(n(&case, "select count(*) from evtxtriage_events"), rows);
}

#[test]
fn entry_reads_one_live_entry_by_id() {
    let (_d, mut case) = loaded();
    let id = promote_at(&mut case, FAILED_LOGON_WS01, &analyst());
    let e = oxl_engine::entry(&case, id).unwrap();
    assert_eq!((e.id, e.evidence_rows), (id, 1));
    assert_eq!(e, one(&case, id));
    assert_eq!(code(oxl_engine::entry(&case, id + 99)).0, "not_found");
    delete_entry(&mut case, id, &analyst()).unwrap();
    assert_eq!(code(oxl_engine::entry(&case, id)).0, "not_found");
}

/// S10 in the engine, for every actor and every batch write that shares the
/// check: 10,000 distinct rows pass it, 10,001 are refused with the count.
#[test]
fn a_batch_takes_at_most_max_action_rows_from_anyone() {
    let (_d, mut case) = loaded();
    let entry_id = promote_at(&mut case, FAILED_LOGON_WS01, &analyst());
    let max = oxl_engine::MAX_ACTION_ROWS as i64;
    // Ids no row has: the call-wide check is all that is reached.
    let ok_rows: Vec<i64> = (1_000_000..1_000_000 + max).collect();
    let too_many: Vec<i64> = (1_000_000..=1_000_000 + max).collect();
    let mut with_dups = too_many.clone();
    with_dups.extend(&ok_rows);
    let a = analyst();
    for (what, result) in [
        ("mark ok", code(mark_suspicious(&mut case, &ok_rows, &a))),
        (
            "tag ok",
            code(oxl_engine::add_tag(&mut case, &ok_rows, "t", &a)),
        ),
        ("promote ok", code(promote_rows(&mut case, &ok_rows, &a))),
        (
            "attach ok",
            code(attach_rows(&mut case, entry_id, &ok_rows, &a)),
        ),
    ] {
        assert_eq!(result.0, "ok", "{what}: {}", result.1);
    }
    for (what, result) in [
        ("mark", code(mark_suspicious(&mut case, &too_many, &a))),
        (
            "tag",
            code(oxl_engine::add_tag(&mut case, &too_many, "t", &a)),
        ),
        ("promote", code(promote_rows(&mut case, &too_many, &a))),
        (
            "attach",
            code(attach_rows(&mut case, entry_id, &too_many, &a)),
        ),
        // Counted after de-duplication: repeats do not add to the count.
        (
            "mark dups",
            code(mark_suspicious(&mut case, &with_dups, &a)),
        ),
    ] {
        assert_eq!(result.0, "refused", "{what}");
        assert!(
            result
                .1
                .contains("select at most 10,000 rows (you have 10,001)"),
            "{what}: {}",
            result.1
        );
    }
    // Repeats of 10,000 distinct rows are still 10,000.
    let mut repeated = ok_rows.clone();
    repeated.extend(&ok_rows);
    assert_eq!(code(mark_suspicious(&mut case, &repeated, &a)).0, "ok");
}

/// The attach picker's order: the most recently created or changed entry
/// first, whatever the entries' times; the Timeline's order is unchanged.
#[test]
fn entries_list_newest_first_on_request() {
    let (d, mut case) = loaded();
    let early = promote_at(&mut case, FAILED_LOGON_WS01, &analyst());
    let late = promote_at(&mut case, SHARE_SRV01, &analyst());
    let undated = new_entry(&mut case, &titled("Undated"), &analyst())
        .unwrap()
        .id;
    let tied = new_entry(&mut case, &titled("Tied"), &analyst())
        .unwrap()
        .id;
    std::thread::sleep(std::time::Duration::from_millis(5));
    // The first entry is changed last: it is the most recent.
    edit_entry(&mut case, early, &titled("Changed last"), &analyst()).unwrap();
    // `late` and `tied` are given one change time, older than every other: the tie goes to the newer id.
    let path = case.path().to_path_buf();
    drop(case);
    {
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE ox.entry SET modified_at = '2026-01-01T00:00:00.000000Z' WHERE id IN (?, ?)",
            duckdb::params![late, tied],
        )
        .unwrap();
    }
    let case = Case::open(&path).unwrap();
    let ids = |q: EntryQuery, limit: u32| -> Vec<i64> {
        entries(&case, &q, limit, 0)
            .unwrap()
            .items
            .into_iter()
            .map(|e| e.id)
            .collect()
    };
    let recent = || EntryQuery {
        order: Some(EntryOrder::Recent),
        ..Default::default()
    };
    assert!(late < tied);
    // Newest change first (early, then undated), then the tied pair by id descending.
    assert_eq!(ids(recent(), 100), [early, undated, tied, late]);
    // The Timeline's order is unchanged: dated by time, the undated last by id.
    let timeline = ids(EntryQuery::default(), 100);
    assert_eq!(timeline[2..], [undated, tied]);
    // The order is applied before the page.
    assert_eq!(ids(recent(), 1), [early]);
    // The case closes before its folder goes, or Windows cannot remove the open file.
    drop(case);
    drop(d);
}
