//! Marks, tags, notes, the history they write and the row detail view: the
//! writes an analyst and an agent share, and the rules on who may change
//! what.

mod common;

use common::{agent, analyst, copy_tree, loaded, n};
use oxl_engine::{
    add_note, add_tag, clear_suspicious, delete_note, history, load_run, mark_suspicious, marks,
    notes, remove_source, remove_tag, row_detail, tags, Actor, Case, MarkState,
    AGENT_ROWS_PER_CALL, MAX_NOTE,
};
use std::path::{Path, PathBuf};

fn id(case: &Case, sql: &str) -> i64 {
    n(case, sql)
}

const LOGON_WS01: &str =
    "select _ox_row from evtxtriage_events where EventId = 4624 and _triage_host = 'WS01'";

fn logon(case: &Case) -> i64 {
    id(case, LOGON_WS01)
}

fn mark_rows(case: &Case, row: i64) -> i64 {
    n(
        case,
        &format!("select count(*) from ox.row_mark where row_id = {row}"),
    )
}

#[test]
fn an_analyst_marks_and_clears() {
    let (_d, mut case) = loaded();
    let analyst = analyst();
    let r = logon(&case);
    assert!(mark_suspicious(&mut case, &[r], &analyst).unwrap()[0].done);
    let (listed, more) = marks(&case, None, 50, 0).unwrap();
    assert!(!more);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].marked_by, "analyst:tester");
    assert_eq!(listed[0].row_id, r);
    assert_eq!(listed[0].relation, "evtxtriage_events");
    assert_eq!(listed[0].host.as_deref(), Some("WS01"));
    assert_eq!(listed[0].mark, MarkState::Suspicious);
    assert!(!listed[0].superseded);
    assert!(clear_suspicious(&mut case, &[r], &analyst).unwrap()[0].done);
    assert!(marks(&case, None, 50, 0).unwrap().0.is_empty());
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where target_kind = 'row_mark'"
        ),
        2
    );
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.history where target_kind = 'row_mark' \
                 and target_id = '{r}' and \"by\" = 'analyst:tester' \
                 and action in ('mark_set', 'mark_clear')"
            )
        ),
        2
    );
}

#[test]
fn ownership_between_agent_and_analyst() {
    let (_d, mut case) = loaded();
    let (analyst, agent, other) = (analyst(), agent(), Actor::Agent("other".into()));
    let r = logon(&case);

    assert!(mark_suspicious(&mut case, &[r], &agent).unwrap()[0].done);
    let again = &mark_suspicious(&mut case, &[r], &agent).unwrap()[0];
    assert!(!again.done);
    assert!(
        again.reason.as_deref().unwrap().contains("already marked"),
        "{again:?}"
    );
    // A different agent is a different owner.
    assert!(!clear_suspicious(&mut case, &[r], &other).unwrap()[0].done);
    assert_eq!(
        marks(&case, None, 50, 0).unwrap().0[0].marked_by,
        "agent:claude"
    );

    assert!(mark_suspicious(&mut case, &[r], &analyst).unwrap()[0].done);
    assert_eq!(
        marks(&case, None, 50, 0).unwrap().0[0].marked_by,
        "analyst:tester"
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'mark_take_over' \
             and json_extract_string(before_json, '$.marked_by') = 'agent:claude' \
             and json_extract_string(after_json, '$.marked_by') = 'analyst:tester'"
        ),
        1
    );
    let refused = &clear_suspicious(&mut case, &[r], &agent).unwrap()[0];
    assert!(!refused.done);
    assert!(
        refused
            .reason
            .as_deref()
            .unwrap()
            .contains("only an analyst"),
        "{refused:?}"
    );
    assert!(clear_suspicious(&mut case, &[r], &analyst).unwrap()[0].done);
    assert_eq!(mark_rows(&case, r), 0);
}

#[test]
fn an_agent_clears_its_own_mark_and_lists_by_owner() {
    let (_d, mut case) = loaded();
    let (analyst, agent) = (analyst(), agent());
    let r = logon(&case);
    let other = id(
        &case,
        "select _ox_row from evtxtriage_events where EventId = 4625",
    );
    mark_suspicious(&mut case, &[r], &agent).unwrap();
    mark_suspicious(&mut case, &[other], &analyst).unwrap();
    let mine = marks(&case, Some(&agent), 50, 0).unwrap().0;
    assert_eq!(mine.iter().map(|m| m.row_id).collect::<Vec<_>>(), [r]);
    assert!(clear_suspicious(&mut case, &[r], &agent).unwrap()[0].done);
    assert!(marks(&case, Some(&agent), 50, 0).unwrap().0.is_empty());
    assert_eq!(marks(&case, None, 50, 0).unwrap().0.len(), 1);
}

#[test]
fn one_mark_per_row_under_every_path() {
    let (_d, mut case) = loaded();
    let (analyst, agent) = (analyst(), agent());
    let r = logon(&case);
    // The same row twice in one batch.
    let out = mark_suspicious(&mut case, &[r, r], &agent).unwrap();
    assert_eq!(
        out.iter().map(|o| o.done).collect::<Vec<_>>(),
        [true, false]
    );
    assert_eq!(mark_rows(&case, r), 1);
    // A take-over replaces the owner, not adds a mark.
    mark_suspicious(&mut case, &[r, r], &analyst).unwrap();
    assert_eq!(mark_rows(&case, r), 1);
    // Another analyst finds it marked already.
    let colleague = Actor::Analyst("colleague".into());
    assert!(mark_suspicious(&mut case, &[r], &colleague).unwrap()[0].done);
    assert_eq!(mark_rows(&case, r), 1);
    assert_eq!(
        marks(&case, None, 50, 0).unwrap().0[0].marked_by,
        "analyst:tester"
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where target_kind = 'row_mark'"
        ),
        2
    );
}

#[test]
fn per_row_outcomes_do_not_fail_the_batch() {
    let (_d, mut case) = loaded();
    let r = logon(&case);
    let out = mark_suspicious(&mut case, &[r, 999_999_999, -1], &analyst()).unwrap();
    assert_eq!(
        out.iter().map(|o| o.done).collect::<Vec<_>>(),
        [true, false, false]
    );
    assert_eq!(
        out.iter().map(|o| o.row_id).collect::<Vec<_>>(),
        [r, 999_999_999, -1]
    );
    assert!(out[1].reason.as_deref().unwrap().contains("no row"));
    assert!(out[0].reason.is_none());
    let cleared = clear_suspicious(&mut case, &[999_999_999], &analyst()).unwrap();
    assert!(!cleared[0].done);
}

#[test]
fn an_agent_is_capped_per_call_and_an_analyst_is_not() {
    let (_d, mut case) = loaded();
    let r = logon(&case);
    let mut many: Vec<i64> = (0..AGENT_ROWS_PER_CALL as i64).map(|i| -i - 1).collect();
    many.push(r);
    assert_eq!(many.len(), 1_001);
    let e = mark_suspicious(&mut case, &many, &agent()).unwrap_err();
    assert_eq!(e.code(), "refused", "{e}");
    assert_eq!(
        add_tag(&mut case, &many, "ai:x", &agent())
            .unwrap_err()
            .code(),
        "refused"
    );
    assert_eq!(mark_rows(&case, r), 0);
    let out = mark_suspicious(&mut case, &many, &analyst()).unwrap();
    assert_eq!(out.len(), 1_001);
    assert_eq!(out.iter().filter(|o| o.done).count(), 1);
    // Exactly the cap is allowed.
    assert!(clear_suspicious(&mut case, &many[1..], &agent()).is_ok());
    // An empty list is a whole-call problem.
    assert_eq!(
        mark_suspicious(&mut case, &[], &analyst())
            .unwrap_err()
            .code(),
        "invalid"
    );
}

#[test]
fn the_same_mark_twice_writes_one_history_row() {
    let (_d, mut case) = loaded();
    let r = logon(&case);
    assert!(mark_suspicious(&mut case, &[r], &analyst()).unwrap()[0].done);
    assert!(mark_suspicious(&mut case, &[r], &analyst()).unwrap()[0].done);
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where target_kind = 'row_mark'"
        ),
        1
    );
}

#[test]
fn tag_rules() {
    let (_d, mut case) = loaded();
    let (analyst, agent) = (analyst(), agent());
    let r = logon(&case);
    assert_eq!(
        add_tag(&mut case, &[r], "exfil", &agent)
            .unwrap_err()
            .code(),
        "refused"
    );
    assert_eq!(
        add_tag(&mut case, &[r], "ai:exfil", &analyst)
            .unwrap_err()
            .code(),
        "invalid"
    );
    let long = "a".repeat(65);
    for bad in ["", "Has Space", "é", long.as_str(), "a;drop"] {
        assert_eq!(
            add_tag(&mut case, &[r], bad, &analyst).unwrap_err().code(),
            "invalid",
            "{bad:?}"
        );
    }
    assert!(add_tag(&mut case, &[r], &"a".repeat(64), &analyst).unwrap()[0].done);
    assert!(remove_tag(&mut case, &[r], &"a".repeat(64), &analyst).unwrap()[0].done);
    assert!(add_tag(&mut case, &[r], "Initial-Access", &analyst).unwrap()[0].done);
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.row_tag where row_id = {r} and tag = 'initial-access'"
            )
        ),
        1
    );
    assert!(add_tag(&mut case, &[r], "ai:rdp", &agent).unwrap()[0].done);
    // Again: done, unchanged, no second row and no history row.
    let before = n(&case, "select count(*) from ox.history");
    assert!(add_tag(&mut case, &[r], "ai:rdp", &agent).unwrap()[0].done);
    assert_eq!(n(&case, "select count(*) from ox.history"), before);
    assert_eq!(
        n(
            &case,
            &format!("select count(*) from ox.row_tag where row_id = {r}")
        ),
        2
    );
    let listed = tags(&case).unwrap();
    assert_eq!(
        listed.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["a".repeat(64).as_str(), "ai:rdp", "initial-access"]
    );
    let rdp = listed.iter().find(|t| t.name == "ai:rdp").unwrap();
    assert_eq!((rdp.rows, rdp.created_by.as_str()), (1, "agent:claude"));

    assert!(!remove_tag(&mut case, &[r], "initial-access", &agent).unwrap()[0].done);
    assert!(remove_tag(&mut case, &[r], "initial-access", &analyst).unwrap()[0].done);
    // Nothing left to remove.
    assert!(!remove_tag(&mut case, &[r], "initial-access", &analyst).unwrap()[0].done);
    // An agent removes its own; a different agent does not.
    assert!(!remove_tag(&mut case, &[r], "ai:rdp", &Actor::Agent("other".into())).unwrap()[0].done);
    assert!(remove_tag(&mut case, &[r], "AI:RDP", &agent).unwrap()[0].done);
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where target_kind = 'row_tag' and action = 'tag_add'"
        ),
        3
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where target_kind = 'row_tag' and action = 'tag_remove'"
        ),
        3
    );
    assert_eq!(
        add_tag(&mut case, &[], "x", &analyst).unwrap_err().code(),
        "invalid"
    );
}

#[test]
fn notes_are_added_listed_and_tombstoned() {
    let (_d, mut case) = loaded();
    let (analyst, agent) = (analyst(), agent());
    let r = logon(&case);
    let mine = add_note(&mut case, r, "first logon from 203.0.113.7", &analyst).unwrap();
    let theirs = add_note(&mut case, r, "matches the rclone run", &agent).unwrap();
    assert_eq!(
        (mine.row_id, mine.written_by.as_str()),
        (r, "analyst:tester")
    );
    let listed = notes(&case, r).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].text, "first logon from 203.0.113.7");

    assert_eq!(
        delete_note(&mut case, mine.id, &agent).unwrap_err().code(),
        "refused"
    );
    delete_note(&mut case, mine.id, &analyst).unwrap();
    let listed = notes(&case, r).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, theirs.id);
    // A tombstone: the note is kept, flagged.
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.note where id = {} and deleted",
                mine.id
            )
        ),
        1
    );
    // An agent deletes its own.
    delete_note(&mut case, theirs.id, &agent).unwrap();
    assert!(notes(&case, r).unwrap().is_empty());
    assert_eq!(
        delete_note(&mut case, theirs.id, &analyst)
            .unwrap_err()
            .code(),
        "refused"
    );
    assert_eq!(
        delete_note(&mut case, 999_999, &analyst)
            .unwrap_err()
            .code(),
        "not_found"
    );

    assert_eq!(
        add_note(&mut case, r, "", &analyst).unwrap_err().code(),
        "invalid"
    );
    assert_eq!(
        add_note(&mut case, r, &"x".repeat(MAX_NOTE + 1), &analyst)
            .unwrap_err()
            .code(),
        "invalid"
    );
    assert!(add_note(&mut case, r, &"é".repeat(MAX_NOTE), &analyst).is_ok());
    assert_eq!(
        add_note(&mut case, 999_999_999, "x", &analyst)
            .unwrap_err()
            .code(),
        "not_found"
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where target_kind = 'note' and action = 'note_add'"
        ),
        3
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where target_kind = 'note' and action = 'note_delete'"
        ),
        2
    );
}

#[test]
fn row_detail_gathers_everything() {
    let (_d, mut case) = loaded();
    let r = logon(&case);
    mark_suspicious(&mut case, &[r], &agent()).unwrap();
    add_tag(&mut case, &[r], "lateral", &analyst()).unwrap();
    add_note(&mut case, r, "check the source", &analyst()).unwrap();

    let d = row_detail(&case, r).unwrap();
    assert_eq!(
        (d.relation.as_str(), d.host.as_deref(), d.active),
        ("evtxtriage_events", Some("WS01"), true)
    );
    assert_eq!(d.row_id, r);
    assert_eq!(d.source_id, 1);
    assert_eq!(d.hash.len(), 64);
    assert_eq!(
        d.evidence_path.as_deref(),
        Some(r"C:\Windows\System32\winevt\Logs\Security.evtx")
    );
    assert_eq!(
        d.fields
            .iter()
            .find(|(k, _)| k == "TimeCreated")
            .unwrap()
            .1
            .text
            .as_deref(),
        Some("2026-01-05T02:14:07.9110000Z")
    );
    assert!(d.output_file.as_deref().unwrap().ends_with("evtx.csv"));
    assert_eq!(d.mark, Some(MarkState::Suspicious));
    assert_eq!(d.marked_by.as_deref(), Some("agent:claude"));
    assert_eq!(d.tags, ["lateral"]);
    assert_eq!(d.notes.len(), 1);
    assert!(d.fields.iter().all(|(_, c)| !c.failed && !c.clipped));
    assert_eq!(
        row_detail(&case, 999_999_999).unwrap_err().code(),
        "not_found"
    );
}

#[test]
fn row_detail_flags_a_failed_conversion() {
    let (_d, case) = loaded();
    let r = id(
        &case,
        "select _ox_row from evtxtriage_events where EventId = 4688",
    );
    let d = row_detail(&case, r).unwrap();
    let time = &d.fields.iter().find(|(k, _)| k == "TimeCreated").unwrap().1;
    assert_eq!(time.text.as_deref(), Some("not-a-time"));
    assert!(time.failed);
    assert_eq!((d.mark, d.marked_by), (None, None));
}

#[test]
fn history_is_append_only_and_newest_first() {
    let (_d, mut case) = loaded();
    let r = logon(&case);
    let base = n(&case, "select count(*) from ox.history");
    mark_suspicious(&mut case, &[r], &analyst()).unwrap();
    add_tag(&mut case, &[r], "lateral", &analyst()).unwrap();
    add_note(&mut case, r, "seen", &agent()).unwrap();
    assert_eq!(n(&case, "select count(*) from ox.history"), base + 3);

    let (rows, more) = history(&case, None, 3, 0).unwrap();
    assert!(more, "the source_load row is older still");
    assert_eq!(
        rows.iter().map(|h| h.action.as_str()).collect::<Vec<_>>(),
        ["note_add", "tag_add", "mark_set"]
    );
    assert!(rows.windows(2).all(|w| w[0].id > w[1].id));
    assert_eq!(rows[0].by, "agent:claude");
    assert_eq!(rows[0].target_kind, "note");
    assert_eq!(rows[2].target_id, r.to_string());
    assert!(rows[2].before.is_none());
    assert_eq!(
        rows[2].after.as_ref().unwrap()["marked_by"],
        "analyst:tester"
    );
    let (marks_only, _) = history(&case, Some("row_mark"), 50, 0).unwrap();
    assert_eq!(marks_only.len(), 1);
    let (all, more) = history(&case, None, 50, 0).unwrap();
    assert!(!more);
    assert_eq!(all.last().unwrap().action, "source_load");
    assert_eq!(
        history(&case, Some("bogus"), 50, 0).unwrap_err().code(),
        "invalid"
    );
    assert_eq!(history(&case, None, 1001, 0).unwrap_err().code(), "invalid");
}

/// A copy of the run at `rerun` (a `rerun_ws01_prefetch` tree) under `to`,
/// without its SVCH0ST.EXE row.
fn without_svch0st(rerun: &Path, to: &Path) -> PathBuf {
    copy_tree(rerun, to);
    let csv = to
        .join("Processed-WS01-20260102000000")
        .join("Execution")
        .join("pf.csv");
    let kept: String = std::fs::read_to_string(&csv)
        .unwrap()
        .lines()
        .filter(|l| !l.contains("SVCH0ST.EXE"))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&csv, kept).unwrap();
    to.to_path_buf()
}

const SVCH0ST_WS01: &str = "select _ox_row from petriage_timeline \
                            where ExecutableName = 'SVCH0ST.EXE' and _triage_host = 'WS01'";

#[test]
fn marks_on_superseded_rows_are_flagged() {
    let (d, mut case) = loaded();
    let old = id(&case, SVCH0ST_WS01);
    assert!(mark_suspicious(&mut case, &[old], &analyst()).unwrap()[0].done);
    let rerun = oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b"));
    let lacking = without_svch0st(&rerun, &d.path().join("c"));
    load_run(&mut case, &lacking, &analyst()).unwrap();

    let listed = marks(&case, None, 50, 0).unwrap().0;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].row_id, old);
    assert!(listed[0].superseded);
    let detail = row_detail(&case, old).unwrap();
    assert!(!detail.active);
    assert_eq!(detail.host.as_deref(), Some("WS01"));
    assert_eq!(
        detail
            .fields
            .iter()
            .find(|(k, _)| k == "ExecutableName")
            .unwrap()
            .1
            .text
            .as_deref(),
        Some("SVCH0ST.EXE")
    );
    assert_eq!(detail.mark, Some(MarkState::Suspicious));

    // Another row of the superseded run takes no new annotation by id.
    let other_old = id(
        &case,
        "select _ox_row from src_1.petriage_timeline \
         where ExecutableName = 'CHROME.EXE' and _triage_host = 'WS01'",
    );
    let out = mark_suspicious(&mut case, &[other_old], &analyst()).unwrap();
    assert!(!out[0].done);
    assert!(
        out[0].reason.as_deref().unwrap().contains("superseded"),
        "{:?}",
        out[0]
    );
    let tagged = add_tag(&mut case, &[other_old], "lateral", &analyst()).unwrap();
    assert!(!tagged[0].done);
    assert!(tagged[0].reason.as_deref().unwrap().contains("superseded"));
    let e = add_note(&mut case, other_old, "x", &analyst()).unwrap_err();
    assert_eq!(e.code(), "refused");
    assert!(e.to_string().contains("superseded"), "{e}");
    // Clearing a left-behind mark is still allowed.
    assert!(clear_suspicious(&mut case, &[old], &analyst()).unwrap()[0].done);
}

#[test]
fn rows_of_a_removed_source_take_no_annotation() {
    let (_d, mut case) = loaded();
    let r = logon(&case);
    remove_source(&mut case, 1, true, &analyst()).unwrap();
    let out = mark_suspicious(&mut case, &[r], &analyst()).unwrap();
    assert!(!out[0].done);
    assert!(
        out[0].reason.as_deref().unwrap().contains("removed source"),
        "{:?}",
        out[0]
    );
    let tagged = add_tag(&mut case, &[r], "lateral", &analyst()).unwrap();
    assert!(tagged[0]
        .reason
        .as_deref()
        .unwrap()
        .contains("removed source"));
    let e = add_note(&mut case, r, "x", &analyst()).unwrap_err();
    assert_eq!(e.code(), "refused");
    assert!(e.to_string().contains("removed source"), "{e}");
    assert_eq!(row_detail(&case, r).unwrap_err().code(), "not_found");
}

#[test]
fn an_orphaned_mark_is_listed_as_superseded() {
    let (_d, mut case) = loaded();
    let r = logon(&case);
    mark_suspicious(&mut case, &[r], &analyst()).unwrap();
    remove_source(&mut case, 1, true, &analyst()).unwrap();
    let listed = marks(&case, None, 50, 0).unwrap().0;
    assert_eq!(listed.len(), 1);
    assert!(listed[0].superseded);
    assert_eq!(listed[0].relation, "evtxtriage_events");
    assert_eq!(listed[0].host, None);
}

#[test]
fn hostile_tags_notes_and_row_ids_return_rather_than_panic() {
    let (_d, mut case) = loaded();
    let r = logon(&case);
    let long = "x".repeat(100_000);
    for text in [
        "\0",
        "ai:",
        "İ",
        "ß",
        "a\u{301}",
        "'; drop table ox.tag; --",
        "\u{feff}a",
        &long,
    ] {
        let _ = add_tag(&mut case, &[r], text, &analyst());
        let _ = add_tag(&mut case, &[r], text, &agent());
        let _ = remove_tag(&mut case, &[r], text, &agent());
        let _ = add_note(&mut case, r, text, &agent());
    }
    let extremes = [i64::MIN, i64::MIN + 1, -1, 0, i64::MAX - 1, i64::MAX];
    let out = mark_suspicious(&mut case, &extremes, &analyst()).unwrap();
    assert!(out.iter().all(|o| !o.done));
    assert!(clear_suspicious(&mut case, &extremes, &analyst())
        .unwrap()
        .iter()
        .all(|o| !o.done));
    for id in extremes {
        assert_eq!(row_detail(&case, id).unwrap_err().code(), "not_found");
        assert!(notes(&case, id).unwrap().is_empty());
        let _ = delete_note(&mut case, id, &analyst());
    }
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.tag where name like '%drop%'"
        ),
        0
    );
}

#[test]
fn marks_page() {
    let (_d, mut case) = loaded();
    let rows: Vec<i64> = (0..3)
        .map(|i| {
            id(
                &case,
                &format!(
                    "select _ox_row from petriage_timeline order by _ox_row limit 1 offset {i}"
                ),
            )
        })
        .collect();
    mark_suspicious(&mut case, &rows, &analyst()).unwrap();
    let (first, more) = marks(&case, None, 2, 0).unwrap();
    assert_eq!((first.len(), more), (2, true));
    let (rest, more) = marks(&case, None, 2, 2).unwrap();
    assert_eq!((rest.len(), more), (1, false));
    assert_eq!(marks(&case, None, 1001, 0).unwrap_err().code(), "invalid");
}
