//! Marks, tags, notes, the history they write and the row detail view: the
//! writes an analyst and an agent share, and the rules on who may change
//! what.

mod common;

use common::{agent, analyst, copy_tree, loaded, n};
use oxl_engine::{
    add_note, add_tag, clear_suspicious, delete_note, history, load_run, mark_suspicious, marks,
    notes, remove_source, remove_tag, row_detail, tags, Actor, Case, Listing, MarkState,
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

/// Found on the second real run: marking 1,000 rows took 6.5 s and tagging
/// them 11.5 s, whatever the case's size, because each row was located,
/// checked and written with statements of its own. A batch now reads what
/// it needs and writes what it decided a few statements at a time, however
/// many rows it has.
#[test]
fn a_batch_costs_a_few_statements_whatever_its_size() {
    let (_d, mut case) = loaded();
    let analyst = analyst();
    let mut rows: Vec<i64> = Vec::new();
    for relation in [
        "evtxtriage_events",
        "petriage_timeline",
        "ext_hayabusa_csv_timeline",
    ] {
        // A table's rows take one contiguous run of ids.
        let first = n(&case, &format!("select min(_ox_row) from {relation}"));
        let last = n(&case, &format!("select max(_ox_row) from {relation}"));
        let count = n(&case, &format!("select count(*) from {relation}"));
        assert_eq!(last - first + 1, count, "{relation}");
        rows.extend(first..=last);
    }
    let n_rows = rows.len();
    assert!(n_rows >= 20, "{n_rows}");
    // Fewer than one statement per row.
    let allowed = 20;
    assert!(allowed < n_rows);
    // Listed in an order other than the tables', to see the history follow it.
    rows.reverse();
    type Batch = fn(&mut Case, &[i64], &Actor) -> oxl_engine::Result<Vec<oxl_engine::RowOutcome>>;
    let calls: [(&str, Batch); 4] = [
        ("mark set", |c, r, a| mark_suspicious(c, r, a)),
        ("mark clear", |c, r, a| clear_suspicious(c, r, a)),
        ("tag add", |c, r, a| add_tag(c, r, "batch", a)),
        ("tag remove", |c, r, a| remove_tag(c, r, "batch", a)),
    ];
    for (what, call) in calls {
        let (outcomes, statements) = case
            .statements_during_mut(|case| call(case, &rows, &analyst).unwrap())
            .unwrap();
        assert!(outcomes.iter().all(|o| o.done), "{what}: {outcomes:?}");
        assert!(
            statements.len() <= allowed,
            "{what} ran {} statements for {n_rows} rows (allowed {allowed}): {statements:#?}",
            statements.len()
        );
    }
    // Each call's history rows are numbered in the order its rows were given.
    let given = rows
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    for action in ["mark_set", "mark_clear", "tag_add", "tag_remove"] {
        let in_place = n(
            &case,
            &format!(
                "select count(*) from \
                 (select cast(target_id as bigint) as t, row_number() over (order by id) as r \
                  from ox.history where action = '{action}') h \
                 join (select unnest(l) as t, generate_subscripts(l, 1) as p \
                       from (select [{given}] as l)) g using (t) \
                 where h.r = g.p"
            ),
        );
        assert_eq!(in_place, i64::try_from(n_rows).unwrap(), "{action}");
    }
}

/// Found on the second real run: a tag removed from every row still listed,
/// with 0 rows. No command defines a tag but using it, so a tag is now
/// defined while some row carries it, and the list shows only those.
#[test]
fn a_tag_no_row_carries_is_not_listed() {
    let (_d, mut case) = loaded();
    let r = logon(&case);
    assert!(add_tag(&mut case, &[r], "lateral", &analyst()).unwrap()[0].done);
    assert_eq!(tags(&case, 50, 0).unwrap().items.len(), 1);
    assert!(remove_tag(&mut case, &[r], "lateral", &analyst()).unwrap()[0].done);
    assert!(tags(&case, 50, 0).unwrap().items.is_empty());
    assert_eq!(n(&case, "select count(*) from ox.tag"), 0);
    // Used again, it is defined again, by whoever used it.
    let other = Actor::parse("analyst:other").unwrap();
    assert!(add_tag(&mut case, &[r], "lateral", &other).unwrap()[0].done);
    let listed = tags(&case, 50, 0).unwrap().items;
    assert_eq!(
        (listed.len(), listed[0].rows, listed[0].created_by.as_str()),
        (1, 1, "analyst:other")
    );
    // A case written before this kept unused definitions; they are not listed.
    case.scalar_i64(
        "INSERT INTO ox.tag (name, created_by, created_at) \
         VALUES ('stale', 'analyst:tester', '2026-01-01T00:00:00Z') RETURNING 1",
    )
    .unwrap();
    let names: Vec<String> = tags(&case, 50, 0)
        .unwrap()
        .items
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["lateral"]);
}

#[test]
fn an_analyst_marks_and_clears() {
    let (_d, mut case) = loaded();
    let analyst = analyst();
    let r = logon(&case);
    assert!(mark_suspicious(&mut case, &[r], &analyst).unwrap()[0].done);
    let Listing {
        items: listed,
        truncated: more,
    } = marks(&case, None, 50, 0).unwrap();
    assert!(!more);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].marked_by, "analyst:tester");
    assert_eq!(listed[0].row_id, r);
    assert_eq!(listed[0].relation, "evtxtriage_events");
    assert_eq!(listed[0].host.as_deref(), Some("WS01"));
    assert_eq!(listed[0].mark, MarkState::Suspicious);
    assert!(listed[0].active);
    assert!(clear_suspicious(&mut case, &[r], &analyst).unwrap()[0].done);
    assert!(marks(&case, None, 50, 0).unwrap().items.is_empty());
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
        marks(&case, None, 50, 0).unwrap().items[0].marked_by,
        "agent:claude"
    );

    assert!(mark_suspicious(&mut case, &[r], &analyst).unwrap()[0].done);
    assert_eq!(
        marks(&case, None, 50, 0).unwrap().items[0].marked_by,
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
    let mine = marks(&case, Some(&agent), 50, 0).unwrap().items;
    assert_eq!(mine.iter().map(|m| m.row_id).collect::<Vec<_>>(), [r]);
    assert!(clear_suspicious(&mut case, &[r], &agent).unwrap()[0].done);
    assert!(marks(&case, Some(&agent), 50, 0).unwrap().items.is_empty());
    assert_eq!(marks(&case, None, 50, 0).unwrap().items.len(), 1);
}

#[test]
fn one_mark_per_row_under_every_path() {
    let (_d, mut case) = loaded();
    let (analyst, agent) = (analyst(), agent());
    let r = logon(&case);
    // The same row twice in one batch: one row, one outcome.
    let out = mark_suspicious(&mut case, &[r, r], &agent).unwrap();
    assert_eq!(out.iter().map(|o| o.done).collect::<Vec<_>>(), [true]);
    assert_eq!(mark_rows(&case, r), 1);
    // A take-over replaces the owner, not adds a mark.
    mark_suspicious(&mut case, &[r, r], &analyst).unwrap();
    assert_eq!(mark_rows(&case, r), 1);
    // Another analyst finds it marked already.
    let colleague = Actor::Analyst("colleague".into());
    assert!(mark_suspicious(&mut case, &[r], &colleague).unwrap()[0].done);
    assert_eq!(mark_rows(&case, r), 1);
    assert_eq!(
        marks(&case, None, 50, 0).unwrap().items[0].marked_by,
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
    // The agent's namespace, refused to an analyst as the analyst's is to an
    // agent: the same tag is fine from the other actor.
    assert_eq!(
        add_tag(&mut case, &[r], "ai:exfil", &analyst)
            .unwrap_err()
            .code(),
        "refused"
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
    let all = tags(&case, 50, 0).unwrap();
    assert!(!all.truncated);
    let listed = all.items;
    // The 64-character tag was removed from its one row, so it is gone.
    assert_eq!(
        listed.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["ai:rdp", "initial-access"]
    );
    let rdp = listed.iter().find(|t| t.name == "ai:rdp").unwrap();
    assert_eq!((rdp.rows, rdp.created_by.as_str()), (1, "agent:claude"));
    // Paged like every other list.
    let first = tags(&case, 1, 0).unwrap();
    assert_eq!((first.items.len(), first.truncated), (1, true));
    let rest = tags(&case, 1, 1).unwrap();
    assert_eq!(
        (rest.items[0].name.as_str(), rest.truncated),
        ("initial-access", false)
    );
    assert_eq!(tags(&case, 1001, 0).unwrap_err().code(), "invalid");

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
    let listed = notes(&case, r, 50, 0).unwrap();
    assert!(!listed.truncated);
    let listed = listed.items;
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].text, "first logon from 203.0.113.7");
    // Paged like every other list.
    let first = notes(&case, r, 1, 0).unwrap();
    assert_eq!((first.items[0].id, first.truncated), (mine.id, true));
    let rest = notes(&case, r, 1, 1).unwrap();
    assert_eq!((rest.items[0].id, rest.truncated), (theirs.id, false));
    assert_eq!(notes(&case, r, 1001, 0).unwrap_err().code(), "invalid");

    assert_eq!(
        delete_note(&mut case, mine.id, &agent).unwrap_err().code(),
        "refused"
    );
    delete_note(&mut case, mine.id, &analyst).unwrap();
    let listed = notes(&case, r, 50, 0).unwrap().items;
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
    assert!(notes(&case, r, 50, 0).unwrap().items.is_empty());
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
    assert_eq!(d.hash.as_deref().map(str::len), Some(64));
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

    let Listing {
        items: rows,
        truncated: more,
    } = history(&case, None, 3, 0).unwrap();
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
    let Listing {
        items: marks_only, ..
    } = history(&case, Some("row_mark"), 50, 0).unwrap();
    assert_eq!(marks_only.len(), 1);
    let Listing {
        items: all,
        truncated: more,
    } = history(&case, None, 50, 0).unwrap();
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

    let listed = marks(&case, None, 50, 0).unwrap().items;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].row_id, old);
    assert!(!listed[0].active);
    let detail = row_detail(&case, old).unwrap();
    assert!(!detail.active);
    assert_eq!(detail.host.as_deref(), Some("WS01"));
    assert!(detail.hash.is_some());
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
    // The row still opens: its data is gone, nothing else is.
    let d = row_detail(&case, r).unwrap();
    assert_eq!(
        (d.row_id, d.relation.as_str(), d.source_id, d.active),
        (r, "evtxtriage_events", 1, false)
    );
    assert!(d.fields.is_empty());
    assert_eq!(
        (d.host, d.hash, d.evidence_path, d.output_file),
        (None, None, None, None)
    );
    assert_eq!((d.mark, d.marked_by), (None, None));
    assert!(d.tags.is_empty() && d.notes.is_empty());
    assert_eq!(
        row_detail(&case, 999_999_999).unwrap_err().code(),
        "not_found"
    );
}

#[test]
fn an_orphaned_mark_is_listed_as_superseded() {
    let (_d, mut case) = loaded();
    let r = logon(&case);
    mark_suspicious(&mut case, &[r], &analyst()).unwrap();
    add_tag(&mut case, &[r], "lateral", &analyst()).unwrap();
    add_note(&mut case, r, "kept after removal", &agent()).unwrap();
    remove_source(&mut case, 1, true, &analyst()).unwrap();
    let listed = marks(&case, None, 50, 0).unwrap().items;
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].active);
    assert_eq!(listed[0].relation, "evtxtriage_events");
    assert_eq!(listed[0].host, None);

    // The row marks() lists opens, with what was left on it.
    let d = row_detail(&case, listed[0].row_id).unwrap();
    assert!(!d.active);
    assert!(d.fields.is_empty() && d.hash.is_none());
    assert_eq!((d.relation.as_str(), d.source_id), ("evtxtriage_events", 1));
    assert_eq!(d.mark, Some(MarkState::Suspicious));
    assert_eq!(d.marked_by.as_deref(), Some("analyst:tester"));
    assert_eq!(d.tags, ["lateral"]);
    assert_eq!(
        d.notes.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(),
        ["kept after removal"]
    );
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
        assert!(notes(&case, id, 50, 0).unwrap().items.is_empty());
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
    let Listing {
        items: first,
        truncated: more,
    } = marks(&case, None, 2, 0).unwrap();
    assert_eq!((first.len(), more), (2, true));
    let Listing {
        items: rest,
        truncated: more,
    } = marks(&case, None, 2, 2).unwrap();
    assert_eq!((rest.len(), more), (1, false));
    assert_eq!(marks(&case, None, 1001, 0).unwrap_err().code(), "invalid");
}

/// A row id named twice in one call is one row: one outcome, counted once
/// against the agent's cap.
#[test]
fn duplicate_row_ids_count_once() {
    let (_d, mut case) = loaded();
    let r = logon(&case);
    let out = mark_suspicious(&mut case, &[r, r], &agent()).unwrap();
    assert_eq!(
        out.iter().map(|o| (o.row_id, o.done)).collect::<Vec<_>>(),
        [(r, true)]
    );
    let out = add_tag(&mut case, &[r, r, r], "ai:seen", &agent()).unwrap();
    assert_eq!(out.len(), 1);

    // 1,000 distinct ids and 500 repeats of them are 1,000 rows, the cap.
    let distinct: Vec<i64> = (0..AGENT_ROWS_PER_CALL as i64).map(|i| -i - 1).collect();
    let mut rows = distinct.clone();
    rows.extend_from_slice(&distinct[..500]);
    assert_eq!(rows.len(), 1_500);
    let out = clear_suspicious(&mut case, &rows, &agent()).unwrap();
    // First appearances, in the order given.
    assert_eq!(out.iter().map(|o| o.row_id).collect::<Vec<_>>(), distinct);
    let out = add_tag(&mut case, &rows, "ai:many", &agent()).unwrap();
    assert_eq!(out.len(), AGENT_ROWS_PER_CALL);
}

#[test]
fn bare_agent_prefix_is_invalid() {
    let (_d, mut case) = loaded();
    let r = logon(&case);
    for bare in ["ai:", "AI:"] {
        let e = add_tag(&mut case, &[r], bare, &agent()).unwrap_err();
        assert_eq!(e.code(), "invalid", "{e}");
    }
    assert_eq!(n(&case, "select count(*) from ox.row_tag"), 0);
    assert!(add_tag(&mut case, &[r], "ai:x", &agent()).unwrap()[0].done);
    // Removing is not refused on the name's account, so a bare prefix an
    // earlier build let an agent add can still be taken off.
    assert!(!remove_tag(&mut case, &[r], "ai:", &agent()).unwrap()[0].done);
}

/// The refusal says who owns the mark or tag, and that its owner may clear
/// it: "only an analyst" was wrong when the owner is another agent.
#[test]
fn refusal_names_the_owner() {
    let (_d, mut case) = loaded();
    let r = logon(&case);
    let (a, b) = (Actor::Agent("a".into()), Actor::Agent("b".into()));
    assert!(mark_suspicious(&mut case, &[r], &a).unwrap()[0].done);
    let refused = &clear_suspicious(&mut case, &[r], &b).unwrap()[0];
    assert!(!refused.done);
    let reason = refused.reason.as_deref().unwrap();
    assert!(
        reason.contains("agent:a") && reason.contains("only agent:a or an analyst"),
        "{reason}"
    );

    assert!(add_tag(&mut case, &[r], "ai:x", &a).unwrap()[0].done);
    let refused = &remove_tag(&mut case, &[r], "ai:x", &b).unwrap()[0];
    assert!(!refused.done);
    let reason = refused.reason.as_deref().unwrap();
    assert!(
        reason.contains("agent:a") && reason.contains("only agent:a or an analyst"),
        "{reason}"
    );

    // An analyst's mark: only an analyst.
    let other = id(
        &case,
        "select _ox_row from evtxtriage_events where EventId = 4625",
    );
    mark_suspicious(&mut case, &[other], &analyst()).unwrap();
    let refused = &clear_suspicious(&mut case, &[other], &b).unwrap()[0];
    let reason = refused.reason.as_deref().unwrap();
    assert!(
        reason.contains("analyst:tester") && reason.contains("only an analyst"),
        "{reason}"
    );
}

/// A mark whose row no source range covers (the case was edited by hand, or
/// a range was lost) does not take the whole listing down with it.
#[test]
fn marks_listing_survives_an_unlocatable_mark() {
    let (d, mut case) = loaded();
    let r = logon(&case);
    mark_suspicious(&mut case, &[r], &analyst()).unwrap();
    let path = d.path().join("t.case.duckdb");
    drop(case);
    {
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO ox.row_mark VALUES (999999, 'suspicious', 'analyst:tester', \
             '2026-01-01T00:00:00.000000Z')",
        )
        .unwrap();
    }
    let case = Case::open(&path).unwrap();
    let listed = marks(&case, None, 50, 0).unwrap().items;
    assert_eq!(listed.len(), 2);
    let orphan = listed.iter().find(|m| m.row_id == 999_999).unwrap();
    assert!(!orphan.active);
    assert_eq!(
        (orphan.relation.as_str(), orphan.host.as_deref()),
        ("", None)
    );
    let real = listed.iter().find(|m| m.row_id == r).unwrap();
    assert!(real.active);
}
