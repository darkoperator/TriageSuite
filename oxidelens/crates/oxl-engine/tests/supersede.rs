//! A second run of a host: the newer rows replace the older ones per
//! dataset and per host, annotations follow identical rows, and a source can
//! be removed. Marks, tags and notes are written through the annotation API.

mod common;

use common::{agent, analyst, copy_tree, n};
use oxl_engine::{
    add_note, add_tag, around, delete_note, find, load_file, load_run, mark_suspicious,
    merged_rows, relation, relations, remove_source, set_primary_time, sources, Case, FileOptions,
    FindMode, FindRequest, LoadReport, MergedSource,
};
use std::path::{Path, PathBuf};

fn code<T>(result: oxl_engine::Result<T>) -> (&'static str, String) {
    match result {
        Ok(_) => ("ok", String::new()),
        Err(e) => (e.code(), e.to_string()),
    }
}

/// A copy of the run at `original` under `copy`, its run id set to `run_id`
/// (and its generation to one of that run, as the suite derives it) and
/// `edit` applied to its parsed datasets.json.
fn copy_run(
    original: &Path,
    copy: &Path,
    run_id: &str,
    edit: impl FnOnce(&mut serde_json::Value),
) -> PathBuf {
    copy_tree(original, copy);
    let path = copy.join("duckdb").join("datasets.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    json["run_id"] = run_id.into();
    json["generation"] = format!("{run_id}-g1").into();
    edit(&mut json);
    std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    copy.to_path_buf()
}

/// A copy of the two-host run under a new run id, nothing else changed.
fn two_hosts_again(dir: &Path) -> PathBuf {
    let original = oxl_fixtures::two_hosts(&dir.join("original"));
    copy_run(&original, &dir.join("again"), "20260104000000-dddd", |_| {})
}

/// Mark the one row `select_row` (a SELECT of `_ox_row`) selects
/// suspicious, as the analyst; returns its id.
fn mark(case: &mut Case, select_row: &str) -> i64 {
    let row = n(case, select_row);
    assert!(mark_suspicious(case, &[row], &analyst()).unwrap()[0].done);
    row
}

const SVCH0ST_WS01: &str = "SELECT _ox_row FROM petriage_timeline \
                            WHERE ExecutableName = 'SVCH0ST.EXE' AND _triage_host = 'WS01'";

fn setup() -> (tempfile::TempDir, Case) {
    let d = tempfile::tempdir().unwrap();
    let case = oxl_fixtures::new_case(d.path());
    (d, case)
}

fn superseded_pairs(report: &LoadReport) -> Vec<(String, String)> {
    report
        .superseded
        .iter()
        .map(|s| (s.relation.clone(), s.host.clone()))
        .collect()
}

/// Found on the second real run: a superseded row's id listed no notes and
/// a removed source's row opened with no fields, and neither said why. A
/// row now says where it stands, and a superseded one which row in the view
/// has its content: here the one its note moved to.
#[test]
fn a_row_says_where_it_stands_and_which_row_replaced_it() {
    use oxl_engine::{notes, row_detail, row_place, RowStanding};
    let (d, mut case) = setup();
    let first = load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap()
    .source_id;
    let old = n(&case, SVCH0ST_WS01);
    add_note(&mut case, old, "seen first here", &analyst()).unwrap();
    let place = row_place(&case, old).unwrap().unwrap();
    assert_eq!(
        (place.standing, place.current_row),
        (RowStanding::Active, None)
    );

    let later = load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b")),
        &analyst(),
    )
    .unwrap()
    .source_id;
    let new = n(&case, SVCH0ST_WS01);
    assert_ne!(old, new);
    let place = row_place(&case, old).unwrap().unwrap();
    assert_eq!(
        (place.standing, place.current_row),
        (RowStanding::Superseded, Some(new))
    );
    // The note went with the row, and the old id says where.
    assert!(notes(&case, old, 50, 0).unwrap().items.is_empty());
    assert_eq!(notes(&case, new, 50, 0).unwrap().items.len(), 1);
    let detail = row_detail(&case, old).unwrap();
    assert_eq!(
        (detail.standing, detail.current_row, detail.active),
        (Some(RowStanding::Superseded), Some(new), false)
    );

    // Removing the later run reinstates the first; then removing the first
    // leaves its rows with no data but their place.
    remove_source(&mut case, later, true, &analyst()).unwrap();
    assert_eq!(
        row_place(&case, old).unwrap().unwrap().standing,
        RowStanding::Active
    );
    let removed = row_place(&case, new).unwrap().unwrap();
    assert_eq!(
        (removed.standing, removed.current_row),
        (RowStanding::Removed, None)
    );
    remove_source(&mut case, first, true, &analyst()).unwrap();
    let detail = row_detail(&case, old).unwrap();
    assert_eq!(
        (detail.standing, detail.current_row, detail.fields.len()),
        (Some(RowStanding::Removed), None, 0)
    );
    // An id no source loaded has no place.
    assert!(row_place(&case, 999_999_999).unwrap().is_none());
}

/// Reviewed: `current_row` is the row with the same content now in the
/// view, not necessarily where the annotations went. A run in between that
/// lacked the row left them on it, and a later run that has the row again
/// supersedes only that run, so they stay; the old row still lists them.
#[test]
fn current_row_is_the_same_content_now_in_view_and_left_behind_annotations_stay_listed() {
    use oxl_engine::{row_detail, row_place, RowStanding};
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    let s1_x = mark(&mut case, SVCH0ST_WS01);
    let rerun = oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b"));
    load_run(
        &mut case,
        &without_svch0st(&rerun, &d.path().join("s2"), "20260102000000-bbbb"),
        &analyst(),
    )
    .unwrap();
    // No run in the view has the row: no current row, the mark stays.
    let place = row_place(&case, s1_x).unwrap().unwrap();
    assert_eq!(
        (place.standing, place.current_row),
        (RowStanding::Superseded, None)
    );
    load_run(
        &mut case,
        &copy_run(&rerun, &d.path().join("s3"), "20260105000000-eeee", |_| {}),
        &analyst(),
    )
    .unwrap();
    let s3_x = n(&case, SVCH0ST_WS01);
    let detail = row_detail(&case, s1_x).unwrap();
    assert_eq!(
        (detail.standing, detail.current_row),
        (Some(RowStanding::Superseded), Some(s3_x))
    );
    // The mark was left on the old row, and its detail says so.
    assert!(detail.mark.is_some());
    assert!(row_detail(&case, s3_x).unwrap().mark.is_none());
}

/// Reviewed: over four runs a note can move from the first copy to the
/// second, stay there when the third lacks the row, and the fourth has it
/// again. The first copy's `current_row` is then the fourth's, neither
/// lists the note, and the history's supersede entry says where it stayed.
#[test]
fn a_note_left_on_an_intermediate_copy_is_traced_through_the_history() {
    use oxl_engine::{notes, row_place};
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    let a_x = n(&case, SVCH0ST_WS01);
    add_note(&mut case, a_x, "first seen", &analyst()).unwrap();
    let rerun = oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b"));
    load_run(&mut case, &rerun, &analyst()).unwrap();
    let b_x = n(&case, SVCH0ST_WS01);
    assert_eq!(notes(&case, b_x, 50, 0).unwrap().items.len(), 1);
    load_run(
        &mut case,
        &without_svch0st(&rerun, &d.path().join("c"), "20260103000000-cccc"),
        &analyst(),
    )
    .unwrap();
    load_run(
        &mut case,
        &copy_run(&rerun, &d.path().join("d"), "20260105000000-eeee", |_| {}),
        &analyst(),
    )
    .unwrap();
    let d_x = n(&case, SVCH0ST_WS01);
    for old in [a_x, b_x] {
        assert_eq!(
            row_place(&case, old).unwrap().unwrap().current_row,
            Some(d_x),
            "{old}"
        );
    }
    assert!(notes(&case, a_x, 50, 0).unwrap().items.is_empty());
    assert!(notes(&case, d_x, 50, 0).unwrap().items.is_empty());
    assert_eq!(notes(&case, b_x, 50, 0).unwrap().items.len(), 1);
    // The supersession that lacked the row recorded the note left on b.
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.history, \
                 unnest(from_json(json_extract(after_json, '$.left_behind'), '[\"JSON\"]')) AS e(entry) \
                 where action = 'supersede' \
                   and json_extract_string(entry, '$.kind') = 'note' \
                   and cast(json_extract(entry, '$.from_row') as bigint) = {b_x}"
            )
        ),
        1
    );
}

#[test]
fn a_retry_of_one_tool_replaces_only_that_dataset_for_that_host() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    let r = load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b")),
        &analyst(),
    )
    .unwrap();
    assert_eq!(r.superseded.len(), 1);
    assert_eq!(
        (
            r.superseded[0].relation.as_str(),
            r.superseded[0].host.as_str()
        ),
        ("petriage_timeline", "WS01")
    );
    assert_eq!(r.superseded[0].old_source, 1);
    // 5 new WS01 rows + 1 SRV01 row.
    assert_eq!(n(&case, "select count(*) from petriage_timeline"), 6);
    assert_eq!(
        n(
            &case,
            "select count(*) from petriage_timeline where _triage_host = 'SRV01'"
        ),
        1
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from petriage_timeline \
             where _triage_host = 'WS01' and _triage_run_id = '20260102000000-bbbb'"
        ),
        5
    );
    assert_eq!(n(&case, "select count(*) from evtxtriage_events"), 11);
    assert_eq!(
        n(&case, "select count(*) from ext_hayabusa_csv_timeline"),
        5
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.source_dataset \
             where source_id = 1 and relation = 'petriage_timeline' and host = 'WS01' and not active"
        ),
        1
    );
    // R2: one supersede row per superseded relation and host.
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'supersede' \
             and target_kind = 'source' and target_id = '1' and \"by\" = 'analyst:tester'"
        ),
        1
    );
}

#[test]
fn marks_follow_identical_rows_and_report_the_rest() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    let old_row = mark(&mut case, SVCH0ST_WS01);
    assert!(add_tag(&mut case, &[old_row], "lateral", &analyst()).unwrap()[0].done);
    let note = add_note(&mut case, old_row, "look here", &analyst()).unwrap();
    let r = load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b")),
        &analyst(),
    )
    .unwrap();
    // One mark, one tag and one note, each counted.
    assert_eq!((r.superseded[0].moved, r.superseded[0].left_behind), (3, 0));

    let new_row = n(&case, SVCH0ST_WS01);
    assert_ne!(new_row, old_row);
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.row_mark m join petriage_timeline p on p._ox_row = m.row_id \
             where p._triage_run_id = '20260102000000-bbbb'"
        ),
        1
    );
    assert_eq!(n(&case, "select row_id from ox.row_mark"), new_row);
    assert_eq!(n(&case, "select row_id from ox.row_tag"), new_row);
    assert_eq!(n(&case, "select row_id from ox.note"), new_row);
    let after = n(
        &case,
        "select count(*) from ox.history where action = 'supersede' \
         and json_extract(after_json, '$.moved_count')::BIGINT = 3 \
         and json_extract(after_json, '$.left_behind_count')::BIGINT = 0",
    );
    assert_eq!(after, 1);

    // The history row says which annotation went from which row to which.
    let after = supersede_after(&case);
    let mut moved = after["moved"].as_array().unwrap().clone();
    moved.sort_by_key(|m| m["kind"].as_str().unwrap().to_string());
    assert_eq!(
        moved,
        vec![
            serde_json::json!({"kind": "note", "from_row": old_row, "to_row": new_row,
                               "note_id": note.id, "deleted": false}),
            serde_json::json!({"kind": "row_mark", "from_row": old_row, "to_row": new_row}),
            serde_json::json!({"kind": "row_tag", "from_row": old_row, "to_row": new_row,
                               "tag": "lateral"}),
        ]
    );
    assert_eq!(after["left_behind"], serde_json::json!([]));
}

/// The `after_json` of the case's one `supersede` history row.
fn supersede_after(case: &Case) -> serde_json::Value {
    let entries = oxl_engine::history(case, Some("source"), 1000, 0)
        .unwrap()
        .items;
    let mut supersede = entries.into_iter().filter(|e| e.action == "supersede");
    let one = supersede.next().expect("a supersede row");
    assert!(supersede.next().is_none(), "more than one supersede row");
    one.after.unwrap()
}

/// A copy of the run at `rerun` (a `rerun_ws01_prefetch` tree) under `to`,
/// with run id `run_id` and without its SVCH0ST.EXE row.
fn without_svch0st(rerun: &Path, to: &Path, run_id: &str) -> PathBuf {
    let copy = copy_run(rerun, to, run_id, |_| {});
    let csv = copy
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
    copy
}

#[test]
fn reinstating_never_doubles_a_mark_or_a_tag() {
    let (d, mut case) = setup();
    // s1: X (SVCH0ST.EXE on WS01) gets mark M1 (the analyst's) and tag
    // `lateral`.
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    let s1_x = mark(&mut case, SVCH0ST_WS01);
    assert!(add_tag(&mut case, &[s1_x], "lateral", &analyst()).unwrap()[0].done);
    // s2 lacks X: M1 and the tag stay on s1's X.
    let rerun = oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b"));
    let s2 = load_run(
        &mut case,
        &without_svch0st(&rerun, &d.path().join("s2"), "20260102000000-bbbb"),
        &analyst(),
    )
    .unwrap();
    assert_eq!(
        (s2.superseded[0].moved, s2.superseded[0].left_behind),
        (0, 2)
    );
    // s3 has X again; s2 had nothing to move. An agent marks s3's X M2 (its
    // owner tells it from M1) and the analyst tags it `lateral` too.
    let s3 = load_run(
        &mut case,
        &copy_run(&rerun, &d.path().join("s3"), "20260105000000-eeee", |_| {}),
        &analyst(),
    )
    .unwrap();
    assert_eq!(s3.superseded[0].old_source, s2.source_id);
    let s3_x = n(&case, SVCH0ST_WS01);
    assert!(mark_suspicious(&mut case, &[s3_x], &agent()).unwrap()[0].done);
    assert!(add_tag(&mut case, &[s3_x], "lateral", &analyst()).unwrap()[0].done);
    // With s2 gone, removing s3 reinstates s1, whose X already carries M1
    // and `lateral`.
    remove_source(&mut case, s2.source_id, false, &analyst()).unwrap();
    let report = remove_source(&mut case, s3.source_id, true, &analyst()).unwrap();
    assert_eq!((report.orphaned_marks, report.orphaned_tags), (1, 1));

    assert_eq!(n(&case, SVCH0ST_WS01), s1_x);
    assert_eq!(
        n(
            &case,
            &format!("select count(*) from ox.row_mark where row_id = {s1_x}")
        ),
        1
    );
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.row_mark where row_id = {s1_x} \
                 and marked_by = 'analyst:tester'"
            )
        ),
        1
    );
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.row_mark where row_id = {s3_x} \
                 and marked_by = 'agent:claude'"
            )
        ),
        1
    );
    assert_eq!(
        n(
            &case,
            &format!("select count(*) from ox.row_tag where row_id = {s1_x}")
        ),
        1
    );
    assert_eq!(
        n(
            &case,
            &format!("select count(*) from ox.row_tag where row_id = {s3_x} and tag = 'lateral'")
        ),
        1
    );
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.history where action = 'source_remove' \
                 and target_id = '{}' \
                 and json_extract(after_json, '$.reinstated[0].source_id')::BIGINT = 1 \
                 and json_extract(after_json, '$.reinstated[0].moved_count')::BIGINT = 0 \
                 and json_extract(after_json, '$.reinstated[0].left_behind_count')::BIGINT = 2 \
                 and json_array_length(json_extract(after_json, '$.reinstated[0].moved')) = 0 \
                 and json_array_length(json_extract(after_json, '$.reinstated[0].left_behind')) = 2",
                s3.source_id
            )
        ),
        1
    );
}

#[test]
fn a_deleted_note_moves_with_its_row_but_is_not_counted() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    let old_row = n(&case, SVCH0ST_WS01);
    let note = add_note(&mut case, old_row, "gone", &analyst()).unwrap();
    delete_note(&mut case, note.id, &analyst()).unwrap();
    let r = load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b")),
        &analyst(),
    )
    .unwrap();
    assert_eq!((r.superseded[0].moved, r.superseded[0].left_behind), (0, 0));
    let new_row = n(&case, SVCH0ST_WS01);
    assert_eq!(n(&case, "select row_id from ox.note"), new_row);
    // Listed as moved, flagged deleted, though not counted.
    assert_eq!(
        supersede_after(&case)["moved"],
        serde_json::json!([{"kind": "note", "from_row": old_row, "to_row": new_row,
                            "note_id": note.id, "deleted": true}])
    );
}

#[test]
fn reloading_after_removal_restores_the_default_primary_time() {
    let (d, mut case) = setup();
    let root = oxl_fixtures::two_hosts(d.path());
    load_run(&mut case, &root, &analyst()).unwrap();
    set_primary_time(&mut case, "petriage_timeline", None, &analyst()).unwrap();
    remove_source(&mut case, 1, true, &analyst()).unwrap();
    load_run(&mut case, &root, &analyst()).unwrap();
    assert_eq!(
        relation(&case, "petriage_timeline").unwrap().primary_time,
        Some("RunTime".to_string())
    );
}

#[test]
fn an_analysts_primary_time_survives_a_later_load() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    set_primary_time(&mut case, "petriage_timeline", None, &analyst()).unwrap();
    load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b")),
        &analyst(),
    )
    .unwrap();
    assert_eq!(
        relation(&case, "petriage_timeline").unwrap().primary_time,
        None
    );
}

#[test]
fn a_mark_whose_row_the_rerun_lacks_stays_behind() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    let old_row = mark(&mut case, SVCH0ST_WS01);

    let rerun = oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b"));
    let lacking = d.path().join("c");
    copy_tree(&rerun, &lacking);
    let csv = lacking
        .join("Processed-WS01-20260102000000")
        .join("Execution")
        .join("pf.csv");
    let text = std::fs::read_to_string(&csv).unwrap();
    let kept: String = text
        .lines()
        .filter(|l| !l.contains("SVCH0ST.EXE"))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_eq!(kept.lines().count(), text.lines().count() - 1);
    std::fs::write(&csv, kept).unwrap();

    let r = load_run(&mut case, &lacking, &analyst()).unwrap();
    assert_eq!(r.superseded.len(), 1);
    assert_eq!((r.superseded[0].moved, r.superseded[0].left_behind), (0, 1));
    assert_eq!(n(&case, "select count(*) from ox.row_mark"), 1);
    assert_eq!(n(&case, "select row_id from ox.row_mark"), old_row);
    // The old row left the view with its host.
    assert_eq!(
        n(
            &case,
            "select count(*) from petriage_timeline where ExecutableName = 'SVCH0ST.EXE'"
        ),
        0
    );
}

#[test]
fn duplicate_rows_rebind_by_occurrence() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    mark(
        &mut case,
        "SELECT _ox_row FROM evtxtriage_events \
         WHERE EventId = 1102 AND _triage_host = 'WS01' AND _ox_occ = 2",
    );
    let r = load_run(&mut case, &two_hosts_again(&d.path().join("b")), &analyst()).unwrap();
    let mut pairs = superseded_pairs(&r);
    pairs.sort();
    assert_eq!(
        pairs,
        [
            ("evtxtriage_events", "SRV01"),
            ("evtxtriage_events", "WS01"),
            ("ext_hayabusa_csv_timeline", "WS01"),
            ("petriage_timeline", "SRV01"),
            ("petriage_timeline", "WS01"),
        ]
        .map(|(r, h)| (r.to_string(), h.to_string()))
    );
    let evtx_ws01 = r
        .superseded
        .iter()
        .find(|s| s.relation == "evtxtriage_events" && s.host == "WS01")
        .unwrap();
    assert_eq!((evtx_ws01.moved, evtx_ws01.left_behind), (1, 0));
    assert_eq!(
        n(
            &case,
            "select e._ox_occ from ox.row_mark m join evtxtriage_events e on e._ox_row = m.row_id \
             where e.EventId = 1102 and e._triage_run_id = '20260104000000-dddd'"
        ),
        2
    );
    assert_eq!(n(&case, "select count(*) from evtxtriage_events"), 11);
}

#[test]
fn removing_a_source_with_annotations_needs_confirmation() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    mark(
        &mut case,
        "SELECT _ox_row FROM evtxtriage_events WHERE EventId = 4625",
    );

    let (c, message) = code(remove_source(&mut case, 1, false, &analyst()));
    assert_eq!(c, "refused");
    assert!(message.contains("1 mark"), "{message}");
    assert!(!sources(&case).unwrap()[0].removed);
    assert_eq!(n(&case, "select count(*) from evtxtriage_events"), 11);

    let report = remove_source(&mut case, 1, true, &analyst()).unwrap();
    assert_eq!(
        (
            report.orphaned_marks,
            report.orphaned_tags,
            report.orphaned_notes
        ),
        (1, 0, 0)
    );
    assert!(case
        .scalar_i64("select count(*) from evtxtriage_events")
        .is_err());
    assert!(relations(&case)
        .unwrap()
        .iter()
        .all(|r| r.name != "evtxtriage_events"));
    assert_eq!(n(&case, "select count(*) from ox.row_mark"), 1);
    assert!(sources(&case).unwrap()[0].removed);
    assert_eq!(
        n(
            &case,
            "select count(*) from duckdb_schemas() where schema_name = 'src_1'"
        ),
        0
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.source_dataset where source_id = 1 and active"
        ),
        0
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'source_remove' \
             and target_kind = 'source' and target_id = '1' and \"by\" = 'analyst:tester'"
        ),
        1
    );
}

#[test]
fn removing_one_source_keeps_the_others_rows() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b")),
        &analyst(),
    )
    .unwrap();
    let report = remove_source(&mut case, 2, false, &analyst()).unwrap();
    assert_eq!(
        (
            report.orphaned_marks,
            report.orphaned_tags,
            report.orphaned_notes
        ),
        (0, 0, 0)
    );
    // R12: source 1's WS01 prefetch rows come back beside its SRV01 row.
    assert_eq!(n(&case, "select count(*) from petriage_timeline"), 5);
    assert_eq!(
        n(
            &case,
            "select count(*) from petriage_timeline \
             where _triage_host = 'WS01' and _triage_run_id = '20260101000000-aaaa'"
        ),
        4
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from petriage_timeline where ExecutableName = 'PSEXESVC.EXE'"
        ),
        0
    );
    assert_eq!(n(&case, "select count(*) from evtxtriage_events"), 11);
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.source_dataset \
             where source_id = 1 and relation = 'petriage_timeline' and host = 'WS01' and active"
        ),
        1
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'source_remove' and target_id = '2' \
             and json_array_length(json_extract(after_json, '$.reinstated')) = 1 \
             and json_extract_string(after_json, '$.reinstated[0].host') = 'WS01' \
             and json_extract(after_json, '$.reinstated[0].source_id')::BIGINT = 1"
        ),
        1
    );
}

#[test]
fn removing_a_rerun_moves_marks_back_and_orphans_the_rest() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b")),
        &analyst(),
    )
    .unwrap();
    let rerun_svch0st = mark(&mut case, SVCH0ST_WS01);
    let psexesvc = mark(
        &mut case,
        "select _ox_row from petriage_timeline where ExecutableName = 'PSEXESVC.EXE'",
    );

    // Both marks count toward the confirmation, though one will move back.
    let (c, message) = code(remove_source(&mut case, 2, false, &analyst()));
    assert_eq!(c, "refused");
    assert!(message.contains("2 marks"), "{message}");

    let report = remove_source(&mut case, 2, true, &analyst()).unwrap();
    assert_eq!(
        (
            report.orphaned_marks,
            report.orphaned_tags,
            report.orphaned_notes
        ),
        (1, 0, 0)
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.row_mark m join petriage_timeline p on p._ox_row = m.row_id \
             where p.ExecutableName = 'SVCH0ST.EXE' and p._triage_run_id = '20260101000000-aaaa'"
        ),
        1
    );
    assert_eq!(
        n(
            &case,
            &format!("select count(*) from ox.row_mark where row_id = {psexesvc}")
        ),
        1
    );
    assert_eq!(n(&case, "select count(*) from ox.row_mark"), 2);
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'source_remove' \
             and json_extract(after_json, '$.reinstated[0].moved_count')::BIGINT = 1 \
             and json_extract(after_json, '$.reinstated[0].left_behind_count')::BIGINT = 1 \
             and json_extract(after_json, '$.orphaned_marks')::BIGINT = 1"
        ),
        1
    );
    let removal = oxl_engine::history(&case, Some("source"), 1000, 0)
        .unwrap()
        .items
        .into_iter()
        .find(|e| e.action == "source_remove")
        .unwrap();
    let entry = &removal.after.unwrap()["reinstated"][0];
    let original_svch0st = n(&case, SVCH0ST_WS01);
    assert_eq!(
        entry["moved"],
        serde_json::json!([{"kind": "row_mark", "from_row": rerun_svch0st,
                            "to_row": original_svch0st}])
    );
    assert_eq!(
        entry["left_behind"],
        serde_json::json!([{"kind": "row_mark", "from_row": psexesvc}])
    );
}

#[test]
fn removing_a_source_that_superseded_nothing_reinstates_nothing() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b")),
        &analyst(),
    )
    .unwrap();
    load_run(
        &mut case,
        &oxl_fixtures::empty_dataset(&d.path().join("c")),
        &analyst(),
    )
    .unwrap();
    remove_source(&mut case, 1, false, &analyst()).unwrap();
    assert!(case
        .scalar_i64("select count(*) from petriage_timeline")
        .is_err());
    assert_eq!(
        n(&case, "select count(*) from ox.source_dataset where active"),
        1
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'source_remove' \
             and json_array_length(json_extract(after_json, '$.reinstated')) = 0"
        ),
        1
    );
}

#[test]
fn an_agent_cannot_remove_a_source() {
    let (_d, mut case) = common::loaded();
    let (c, _) = code(remove_source(&mut case, 1, true, &agent()));
    assert_eq!(c, "refused");
    assert!(!sources(&case).unwrap()[0].removed);
    assert_eq!(n(&case, "select count(*) from evtxtriage_events"), 11);
}

#[test]
fn removing_an_unknown_or_removed_source_is_refused() {
    let (_d, mut case) = common::loaded();
    assert_eq!(
        code(remove_source(&mut case, 99, true, &analyst())).0,
        "not_found"
    );
    remove_source(&mut case, 1, true, &analyst()).unwrap();
    assert_eq!(
        code(remove_source(&mut case, 1, true, &analyst())).0,
        "refused"
    );
}

#[test]
fn a_removed_run_may_be_loaded_again() {
    let (d, mut case) = setup();
    let root = oxl_fixtures::two_hosts(d.path());
    load_run(&mut case, &root, &analyst()).unwrap();
    remove_source(&mut case, 1, true, &analyst()).unwrap();
    let r = load_run(&mut case, &root, &analyst()).unwrap();
    assert!(r.superseded.is_empty());
    assert_eq!(n(&case, "select count(*) from evtxtriage_events"), 11);
    assert_eq!(n(&case, "select count(*) from petriage_timeline"), 5);
    assert_eq!(
        n(
            &case,
            "select count(*) from ext_hayabusa_csv_timeline where \"Timestamp__typed\" is not null"
        ),
        4
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.column_catalog where relation = 'evtxtriage_events' and name = 'EventId'"
        ),
        1
    );
}

#[test]
fn loose_files_never_supersede_and_can_be_removed() {
    let (d, mut case) = setup();
    let fw = oxl_fixtures::loose_fw_csv(d.path());
    let hosted = load_file(
        &mut case,
        &fw,
        FileOptions {
            name: Some("fw_ws01".into()),
            host: Some("WS01".into()),
        },
        &analyst(),
    )
    .unwrap();
    let other = d.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let hostless = load_file(
        &mut case,
        &oxl_fixtures::loose_fw_csv(&other),
        FileOptions::default(),
        &analyst(),
    )
    .unwrap();
    assert!(hosted.superseded.is_empty() && hostless.superseded.is_empty());

    let r = load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    assert!(r.superseded.is_empty());
    assert_eq!(n(&case, "select count(*) from fw_ws01"), 3);
    assert_eq!(
        n(&case, "select count(*) from ox.source_dataset where active"),
        2 + 5
    );

    // A file with no host is removable like any source.
    let relation = hostless.relations[0].name.clone();
    remove_source(&mut case, hostless.source_id, false, &analyst()).unwrap();
    assert!(case
        .scalar_i64(&format!("select count(*) from {relation}"))
        .is_err());
    assert!(relations(&case).unwrap().iter().all(|r| r.name != relation));
    assert_eq!(n(&case, "select count(*) from fw_ws01"), 3);
}

#[test]
fn a_run_that_names_a_metadata_column_differently_is_refused() {
    let (d, mut case) = setup();
    let original = oxl_fixtures::two_hosts(&d.path().join("a"));
    load_run(&mut case, &original, &analyst()).unwrap();
    let renamed = copy_run(
        &original,
        &d.path().join("b"),
        "20260104000000-dddd",
        |json| {
            let datasets = json["datasets"].as_array_mut().unwrap();
            let pf = datasets
                .iter_mut()
                .find(|ds| ds["view"] == "petriage_timeline")
                .unwrap();
            pf["metadata_columns"]["host"] = "_triage_host_1".into();
        },
    );
    let (c, message) = code(load_run(&mut case, &renamed, &analyst()));
    assert_eq!(c, "invalid");
    assert!(
        message.contains("petriage_timeline") && message.contains("_triage_host_1"),
        "{message}"
    );
    assert_eq!(sources(&case).unwrap().len(), 1);
    assert_eq!(
        n(
            &case,
            "select count(*) from duckdb_schemas() where schema_name = 'src_2'"
        ),
        0
    );
    assert_eq!(n(&case, "select count(*) from petriage_timeline"), 5);
}

/// The two-host run, then the same run again with its prefetch files lacking
/// `ExecutableName`: the catalog still has the column, and no active source.
fn rerun_without_executable_name() -> (tempfile::TempDir, Case) {
    let (d, mut case) = setup();
    let original = oxl_fixtures::two_hosts(&d.path().join("a"));
    load_run(&mut case, &original, &analyst()).unwrap();
    // The same run again, its prefetch files without ExecutableName.
    let copy = copy_run(
        &original,
        &d.path().join("b"),
        "20260104000000-dddd",
        |json| {
            let datasets = json["datasets"].as_array_mut().unwrap();
            let pf = datasets
                .iter_mut()
                .find(|ds| ds["view"] == "petriage_timeline")
                .unwrap();
            let without = |list: &mut serde_json::Value| {
                list.as_array_mut()
                    .unwrap()
                    .retain(|c| c != "ExecutableName");
            };
            without(&mut pf["columns"]);
            for file in pf["files"].as_array_mut().unwrap() {
                without(&mut file["header"]);
            }
        },
    );
    for host in ["WS01", "SRV01"] {
        let csv = copy
            .join(format!("Processed-{host}-20260101000000"))
            .join("Execution")
            .join("pf.csv");
        let text: String = std::fs::read_to_string(&csv)
            .unwrap()
            .lines()
            .map(|l| {
                let fields: Vec<&str> = l.split(',').collect();
                format!("{},{}\n", fields[0], fields[2])
            })
            .collect();
        std::fs::write(&csv, text).unwrap();
    }
    load_run(&mut case, &copy, &analyst()).unwrap();
    (d, case)
}

#[test]
fn the_raw_view_shows_a_column_only_a_superseded_run_had_as_null() {
    let (_d, case) = rerun_without_executable_name();
    assert_eq!(
        n(
            &case,
            "select count(*) from raw_petriage_timeline where ExecutableName is null"
        ),
        5
    );
    assert_eq!(n(&case, "select count(*) from raw_petriage_timeline"), 5);
}

/// A pivot reads the columns of every view at once instead of describing
/// each view, and must come to the same answer: a column no active source
/// has reads as empty, where naming it in the view would fail to bind.
#[test]
fn a_pivot_reads_a_column_only_a_superseded_run_had_as_empty() {
    let (_d, case) = rerun_without_executable_name();
    let request = FindRequest {
        value: "SVCH0ST.EXE".into(),
        mode: FindMode::Exact,
        hosts: vec![],
        from: None,
        to: None,
    };
    let found = find(&case, &request, &mut |_, _| true).unwrap();
    // The prefetch cell that held it is gone; the event log's is not.
    assert_eq!(
        found
            .hits
            .iter()
            .map(|h| (h.relation.as_str(), h.column.as_str(), h.count))
            .collect::<Vec<_>>(),
        [("evtxtriage_events", "PayloadData1", 1)]
    );
    assert!(found.searched.contains(&"petriage_timeline".to_string()));

    let (host, at) = ("WS01", "2026-01-05T02:15:44.380000Z");
    let counts = around(&case, host, at, 60).unwrap();
    let prefetch = counts
        .iter()
        .find(|c| c.relation == "petriage_timeline")
        .unwrap();
    assert_eq!(prefetch.count, 1);
    let source = MergedSource::Around {
        host: host.into(),
        at: at.into(),
        window_seconds: 60,
    };
    let rows = merged_rows(&case, &source, 50, 0).unwrap().items;
    let row = rows
        .iter()
        .find(|r| r.relation == "petriage_timeline")
        .unwrap();
    // With the name it was "SVCH0ST.EXE · 1".
    assert_eq!(row.summary, "1");
}

/// Found on the first real run: `around --row` on a superseded row said
/// "no row in the case" while `row` showed the same row, inactive. A
/// superseded row still has a host and a time, so it anchors a pivot: what
/// happened on that host around that moment, in the rows active now.
#[test]
fn a_superseded_row_anchors_a_pivot() {
    let (d, mut case) = setup();
    let first = oxl_fixtures::two_hosts(&d.path().join("a"));
    load_run(&mut case, &first, &analyst()).unwrap();
    let old = n(&case, SVCH0ST_WS01);
    let rerun = oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b"));
    let report = load_run(&mut case, &rerun, &analyst()).unwrap();
    assert!(!report.superseded.is_empty());
    assert_ne!(
        n(&case, SVCH0ST_WS01),
        old,
        "the old row is no longer in the view"
    );

    let (host, at) = oxl_engine::anchor_of(&case, old).unwrap();
    assert_eq!(
        (host.as_str(), at.as_str()),
        ("WS01", "2026-01-05T02:15:44.412000Z")
    );
    let counts = around(&case, &host, &at, 60).unwrap();
    let prefetch = counts
        .iter()
        .find(|c| c.relation == "petriage_timeline")
        .unwrap();
    // The rerun's identical row, the one active now.
    assert_eq!(prefetch.count, 1);

    // A row whose source was removed has nothing left to read.
    let source = sources(&case)
        .unwrap()
        .into_iter()
        .find(|s| s.run_id.as_deref() == Some("20260101000000-aaaa"))
        .unwrap()
        .id;
    remove_source(&mut case, source, true, &analyst()).unwrap();
    let e = oxl_engine::anchor_of(&case, old).unwrap_err();
    assert_eq!(e.code(), "not_found", "{e}");
}

#[test]
fn a_run_that_types_a_column_differently_is_refused() {
    let (d, mut case) = setup();
    let original = oxl_fixtures::two_hosts(&d.path().join("a"));
    load_run(&mut case, &original, &analyst()).unwrap();
    let retyped = copy_run(
        &original,
        &d.path().join("b"),
        "20260104000000-dddd",
        |json| {
            let datasets = json["datasets"].as_array_mut().unwrap();
            let evtx = datasets
                .iter_mut()
                .find(|ds| ds["view"] == "evtxtriage_events")
                .unwrap();
            evtx["effective_types"]
                .as_object_mut()
                .unwrap()
                .remove("EventId")
                .unwrap();
        },
    );
    let (c, message) = code(load_run(&mut case, &retyped, &analyst()));
    assert_eq!(c, "invalid");
    assert!(
        message.contains("evtxtriage_events") && message.contains("EventId"),
        "{message}"
    );
    assert_eq!(sources(&case).unwrap().len(), 1);
    assert_eq!(
        n(
            &case,
            "select count(*) from duckdb_schemas() where schema_name = 'src_2'"
        ),
        0
    );
    assert_eq!(n(&case, "select count(*) from evtxtriage_events"), 11);
}

#[test]
fn companions_reach_a_second_source() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    load_run(&mut case, &two_hosts_again(&d.path().join("b")), &analyst()).unwrap();
    assert_eq!(
        n(
            &case,
            "select count(*) from ext_hayabusa_csv_timeline where \"Timestamp__typed\" is not null"
        ),
        4
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ext_hayabusa_csv_timeline \
             where _triage_run_id = '20260104000000-dddd'"
        ),
        5
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.column_catalog \
             where relation = 'ext_hayabusa_csv_timeline' and name = 'Timestamp'"
        ),
        1
    );
}

/// DuckDB resolves `eventid` and `EventId` to one column, so a run that
/// spells a column in another case is the same column, and its typing must
/// agree (ruling R10) just as if it were spelled alike.
#[test]
fn typing_check_ignores_name_case() {
    let (d, mut case) = setup();
    let original = oxl_fixtures::two_hosts(&d.path().join("a"));
    load_run(&mut case, &original, &analyst()).unwrap();
    let respelt = copy_run(
        &original,
        &d.path().join("b"),
        "20260104000000-dddd",
        |json| {
            let datasets = json["datasets"].as_array_mut().unwrap();
            let evtx = datasets
                .iter_mut()
                .find(|ds| ds["view"] == "evtxtriage_events")
                .unwrap();
            let respell = |list: &mut serde_json::Value| {
                for name in list.as_array_mut().unwrap() {
                    if name == "EventId" {
                        *name = "eventid".into();
                    }
                }
            };
            respell(&mut evtx["columns"]);
            for file in evtx["files"].as_array_mut().unwrap() {
                respell(&mut file["header"]);
            }
            // Left as text by this run.
            evtx["effective_types"]
                .as_object_mut()
                .unwrap()
                .remove("EventId")
                .unwrap();
        },
    );
    for host in ["WS01", "SRV01"] {
        let csv = respelt
            .join(format!("Processed-{host}-20260101000000"))
            .join("EventLogs")
            .join("evtx.csv");
        let text = std::fs::read_to_string(&csv)
            .unwrap()
            .replacen(",EventId,", ",eventid,", 1);
        std::fs::write(&csv, text).unwrap();
    }
    let (c, message) = code(load_run(&mut case, &respelt, &analyst()));
    assert_eq!(c, "invalid", "{message}");
    assert!(
        message.contains("evtxtriage_events") && message.contains("EventId"),
        "{message}"
    );
    assert_eq!(sources(&case).unwrap().len(), 1);
    assert_eq!(n(&case, "select count(*) from evtxtriage_events"), 11);
}

/// A later run that spells a column in another case but agrees on its
/// typing loads into the same column, and its rows are the same rows: they
/// hash under the catalog's spelling, so annotations follow them.
#[test]
fn a_respelt_column_still_matches_rows() {
    let (d, mut case) = setup();
    let original = oxl_fixtures::two_hosts(&d.path().join("a"));
    load_run(&mut case, &original, &analyst()).unwrap();
    let row = n(&case, SVCH0ST_WS01);
    assert!(mark_suspicious(&mut case, &[row], &analyst()).unwrap()[0].done);
    assert!(add_tag(&mut case, &[row], "seen", &analyst()).unwrap()[0].done);
    add_note(&mut case, row, "looked at", &analyst()).unwrap();

    let respelt = copy_run(
        &original,
        &d.path().join("b"),
        "20260104000000-dddd",
        |json| {
            let pf = json["datasets"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|ds| ds["view"] == "petriage_timeline")
                .unwrap();
            let respell = |list: &mut serde_json::Value| {
                for name in list.as_array_mut().unwrap() {
                    if name == "ExecutableName" {
                        *name = "executablename".into();
                    }
                }
            };
            respell(&mut pf["columns"]);
            for file in pf["files"].as_array_mut().unwrap() {
                respell(&mut file["header"]);
            }
        },
    );
    for host in ["WS01", "SRV01"] {
        let csv = respelt
            .join(format!("Processed-{host}-20260101000000"))
            .join("Execution")
            .join("pf.csv");
        let text =
            std::fs::read_to_string(&csv)
                .unwrap()
                .replacen("ExecutableName", "executablename", 1);
        std::fs::write(&csv, text).unwrap();
    }
    let report = load_run(&mut case, &respelt, &analyst()).unwrap();
    let pf = report
        .superseded
        .iter()
        .find(|s| s.relation == "petriage_timeline" && s.host == "WS01")
        .unwrap();
    assert_eq!((pf.moved, pf.left_behind), (3, 0));
    let now = n(&case, SVCH0ST_WS01);
    assert_ne!(now, row);
    for table in ["ox.row_mark", "ox.row_tag", "ox.note"] {
        assert_eq!(
            n(
                &case,
                &format!("select count(*) from {table} where row_id = {now}")
            ),
            1,
            "{table}"
        );
    }
    // One catalogued column, under its first spelling.
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.column_catalog \
             where relation = 'petriage_timeline' and lower(name) = 'executablename'"
        ),
        1
    );
}

/// A dataset whose files spell one column two ways (`ExecutableName` in
/// WS01's file, `executablename` in SRV01's) has both names, catalogued
/// apart. Each name the catalog has exactly keeps its own spelling in the
/// hash, so the same rows in a later run still match.
#[test]
fn two_spellings_in_one_dataset_keep_their_own_hash_names() {
    let (d, mut case) = setup();
    let original = oxl_fixtures::two_hosts(&d.path().join("a"));
    let both = copy_run(
        &original,
        &d.path().join("b"),
        "20260101000000-aaaa",
        |json| {
            let pf = json["datasets"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|ds| ds["view"] == "petriage_timeline")
                .unwrap();
            pf["columns"]
                .as_array_mut()
                .unwrap()
                .push("executablename".into());
            for file in pf["files"].as_array_mut().unwrap() {
                if file["host"] == "SRV01" {
                    for name in file["header"].as_array_mut().unwrap() {
                        if name == "ExecutableName" {
                            *name = "executablename".into();
                        }
                    }
                }
            }
        },
    );
    let srv = both
        .join("Processed-SRV01-20260101000000")
        .join("Execution")
        .join("pf.csv");
    let text =
        std::fs::read_to_string(&srv)
            .unwrap()
            .replacen("ExecutableName", "executablename", 1);
    std::fs::write(&srv, text).unwrap();
    load_run(&mut case, &both, &analyst()).unwrap();
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.column_catalog \
             where relation = 'petriage_timeline' and lower(name) = 'executablename'"
        ),
        2
    );
    let row = n(&case, SVCH0ST_WS01);
    assert!(mark_suspicious(&mut case, &[row], &analyst()).unwrap()[0].done);

    let again = copy_run(&both, &d.path().join("c"), "20260104000000-dddd", |_| {});
    let report = load_run(&mut case, &again, &analyst()).unwrap();
    let pf = report
        .superseded
        .iter()
        .find(|s| s.relation == "petriage_timeline" && s.host == "WS01")
        .unwrap();
    assert_eq!((pf.moved, pf.left_behind), (1, 0));
    let now = n(&case, SVCH0ST_WS01);
    assert_ne!(now, row);
    assert_eq!(
        n(
            &case,
            &format!("select count(*) from ox.row_mark where row_id = {now}")
        ),
        1
    );
}

/// A run whose dataset takes a loose file's relation name is refused for the
/// name (`refused`), before its columns are compared with the file's, which
/// would call the same run `invalid` for a typing it never shared.
#[test]
fn relation_kind_refusal_comes_first() {
    let (d, mut case) = setup();
    let p = d.path().join("pf.csv");
    std::fs::write(
        &p,
        "RunTime,ExecutableName,RunCount\n2026-01-05 02:15:44,X.EXE,1\n",
    )
    .unwrap();
    let options = FileOptions {
        name: Some("petriage_timeline".into()),
        host: None,
    };
    load_file(&mut case, &p, options, &analyst()).unwrap();
    let run = oxl_fixtures::two_hosts(&d.path().join("a"));
    let (c, message) = code(load_run(&mut case, &run, &analyst()));
    assert_eq!(c, "refused", "{message}");
    assert!(message.contains("petriage_timeline"), "{message}");
    assert_eq!(sources(&case).unwrap().len(), 1);
}

/// A run of WS01 whose prefetch file has `extra` more rows than the
/// two-host run's, each distinct.
fn two_hosts_with_more_prefetch(dir: &Path, extra: usize) -> PathBuf {
    let root = oxl_fixtures::two_hosts(dir);
    let csv = root
        .join("Processed-WS01-20260101000000")
        .join("Execution")
        .join("pf.csv");
    let mut text = std::fs::read_to_string(&csv).unwrap();
    for i in 0..extra {
        text.push_str(&format!(
            "2026-01-06T{:02}:{:02}:00.0000000Z,TOOL{i}.EXE,{i}\n",
            i / 60,
            i % 60
        ));
    }
    std::fs::write(&csv, text).unwrap();
    root
}

/// Found on the second real run's review: rebinding asked about each
/// annotated row with statements of its own. It reads and writes the
/// annotations a few statements at a time, so a supersession costs the same
/// statements whether one row is annotated or two hundred.
#[test]
fn rebind_cost_does_not_grow_with_annotations() {
    let cost = |annotated: usize| -> usize {
        let (d, mut case) = setup();
        let first = two_hosts_with_more_prefetch(&d.path().join("a"), 210);
        load_run(&mut case, &first, &analyst()).unwrap();
        let rows: Vec<i64> = (0..annotated)
            .map(|i| {
                n(
                    &case,
                    &format!(
                        "select _ox_row from petriage_timeline where _triage_host = 'WS01' \
                         order by _ox_row limit 1 offset {i}"
                    ),
                )
            })
            .collect();
        assert!(mark_suspicious(&mut case, &rows, &analyst())
            .unwrap()
            .iter()
            .all(|o| o.done));
        assert!(add_tag(&mut case, &rows, "seen", &analyst())
            .unwrap()
            .iter()
            .all(|o| o.done));
        for row in &rows {
            add_note(&mut case, *row, "looked at", &analyst()).unwrap();
        }
        let rerun = copy_run(&first, &d.path().join("b"), "20260104000000-dddd", |_| {});
        let (report, ran) = case
            .statements_during_mut(|c| load_run(c, &rerun, &analyst()).unwrap())
            .unwrap();
        let prefetch = report
            .superseded
            .iter()
            .find(|s| s.relation == "petriage_timeline" && s.host == "WS01")
            .unwrap();
        assert_eq!(
            (prefetch.moved, prefetch.left_behind),
            (3 * annotated as u64, 0)
        );
        // Every annotation is on the rerun's rows now.
        assert_eq!(
            n(
                &case,
                "select count(*) from ox.row_mark m join petriage_timeline v \
                 on v._ox_row = m.row_id"
            ),
            annotated as i64
        );
        ran.len()
    };
    assert_eq!(cost(1), cost(200));
}

/// A run that types a column and a run that leaves it as text cannot feed
/// one relation, whichever came first (ruling R10): the later one is
/// refused naming the relation and the column, and the case is as it was.
#[test]
fn typing_conflict_is_refused_both_ways() {
    let untype_event_id = |json: &mut serde_json::Value| {
        let datasets = json["datasets"].as_array_mut().unwrap();
        let evtx = datasets
            .iter_mut()
            .find(|ds| ds["view"] == "evtxtriage_events")
            .unwrap();
        evtx["effective_types"]
            .as_object_mut()
            .unwrap()
            .remove("EventId")
            .unwrap();
    };
    for typed_first in [true, false] {
        let (d, mut case) = setup();
        let typed = oxl_fixtures::two_hosts(&d.path().join("a"));
        let untyped = copy_run(&typed, &d.path().join("b"), "20260104000000-dddd", |j| {
            untype_event_id(j)
        });
        let (first, second) = if typed_first {
            (&typed, &untyped)
        } else {
            (&untyped, &typed)
        };
        load_run(&mut case, first, &analyst()).unwrap();
        let (c, message) = code(load_run(&mut case, second, &analyst()));
        assert_eq!(c, "invalid", "typed_first={typed_first}: {message}");
        assert!(
            message.contains("evtxtriage_events") && message.contains("EventId"),
            "{message}"
        );
        assert_eq!(
            sources(&case).unwrap().len(),
            1,
            "typed_first={typed_first}"
        );
        assert_eq!(
            n(
                &case,
                "select count(*) from duckdb_schemas() where schema_name = 'src_2'"
            ),
            0
        );
        assert_eq!(n(&case, "select count(*) from evtxtriage_events"), 11);
        // The column is still typed (or still text) exactly as the first
        // run left it.
        let typed_now = n(
            &case,
            "select count(*) from ox.column_catalog where relation = 'evtxtriage_events' \
             and name = 'EventId' and sql_type is not null",
        );
        assert_eq!(typed_now, i64::from(typed_first));
    }
}

/// A load that fails partway, after it has staged datasets of its own (it
/// fails before `supersede` runs; the failure after it is the unit test
/// `a_load_that_fails_after_supersede_rolls_back` in `load/run.rs`), leaves
/// no trace: the earlier run is still the one in view with every annotation
/// on its rows, and the same run loads once its file is back.
#[test]
fn a_load_that_fails_partway_rolls_back_whole() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    let svch0st = mark(&mut case, SVCH0ST_WS01);
    assert!(add_tag(&mut case, &[svch0st], "lateral", &analyst()).unwrap()[0].done);
    let note = add_note(&mut case, svch0st, "look here", &analyst()).unwrap();
    let history_before = n(&case, "select count(*) from ox.history");
    let base = oxl_fixtures::two_hosts(&d.path().join("base"));
    let again = copy_run(&base, &d.path().join("b"), "20260104000000-dddd", |_| {});
    // One host's Windows event log is missing, so the load fails once it
    // is under way.
    let evtx = again
        .join("Processed-SRV01-20260101000000")
        .join("EventLogs")
        .join("evtx.csv");
    let kept = std::fs::read(&evtx).unwrap();
    std::fs::remove_file(&evtx).unwrap();
    let (c, message) = code(load_run(&mut case, &again, &analyst()));
    assert_ne!(c, "ok");
    assert!(message.contains("evtx.csv"), "{message}");

    assert_eq!(sources(&case).unwrap().len(), 1);
    assert_eq!(n(&case, "select count(*) from ox.history"), history_before);
    assert_eq!(
        n(
            &case,
            "select count(*) from duckdb_schemas() \
             where schema_name like 'src_%' and schema_name <> 'src_1'"
        ),
        0
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from duckdb_schemas() where schema_name = 'ox_stage'"
        ),
        0
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.source_dataset where NOT active"
        ),
        0
    );
    assert_eq!(n(&case, SVCH0ST_WS01), svch0st);
    assert_eq!(n(&case, "select count(*) from petriage_timeline"), 5);
    assert_eq!(
        n(
            &case,
            "select count(*) from petriage_timeline where _triage_run_id = '20260104000000-dddd'"
        ),
        0
    );
    for table in ["row_mark", "row_tag", "note"] {
        assert_eq!(
            n(
                &case,
                &format!("select count(*) from ox.{table} where row_id = {svch0st}")
            ),
            1,
            "{table}"
        );
    }
    assert_eq!(
        n(
            &case,
            &format!("select id from ox.note where row_id = {svch0st}")
        ),
        note.id
    );

    // With the file back the same run loads and the annotations follow.
    std::fs::write(&evtx, kept).unwrap();
    let report = load_run(&mut case, &again, &analyst()).unwrap();
    assert_eq!(report.source_id, 2);
    assert_ne!(n(&case, SVCH0ST_WS01), svch0st);
    assert_eq!(n(&case, "select count(*) from ox.row_mark"), 1);
}

/// Taking back a rerun puts the tags and notes its rows took over on the
/// earlier run's identical rows, as it does a mark; those on a row only the
/// rerun had are orphaned, counted in the report, and still listed.
#[test]
fn tags_and_notes_return_on_reinstatement() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    let original = n(&case, SVCH0ST_WS01);
    assert!(add_tag(&mut case, &[original], "lateral", &analyst()).unwrap()[0].done);
    let note = add_note(&mut case, original, "look here", &analyst()).unwrap();
    let rerun = load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b")),
        &analyst(),
    )
    .unwrap();
    let taken = n(&case, SVCH0ST_WS01);
    assert_ne!(taken, original);
    assert_eq!(n(&case, "select row_id from ox.row_tag"), taken);
    // And one pair on a row the earlier run never had.
    let only_rerun = n(
        &case,
        "select _ox_row from petriage_timeline where ExecutableName = 'PSEXESVC.EXE'",
    );
    assert!(add_tag(&mut case, &[only_rerun], "psexec", &analyst()).unwrap()[0].done);
    let stranded = add_note(&mut case, only_rerun, "only here", &analyst()).unwrap();

    let report = remove_source(&mut case, rerun.source_id, true, &analyst()).unwrap();
    assert_eq!(
        (
            report.orphaned_marks,
            report.orphaned_tags,
            report.orphaned_notes
        ),
        (0, 1, 1)
    );
    assert_eq!(n(&case, SVCH0ST_WS01), original);
    assert_eq!(
        n(&case, "select row_id from ox.row_tag where tag = 'lateral'"),
        original
    );
    assert_eq!(
        n(
            &case,
            &format!("select row_id from ox.note where id = {}", note.id)
        ),
        original
    );
    // The tag and the note are on the row in view, where the listings find them.
    assert_eq!(
        oxl_engine::notes(&case, original, 50, 0)
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        oxl_engine::row_detail(&case, original).unwrap().tags,
        ["lateral"]
    );
    // The stranded pair stayed on the rerun's row, flagged and not in view.
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.note where id = {} and row_id = {only_rerun}",
                stranded.id
            )
        ),
        1
    );
    let detail = oxl_engine::row_detail(&case, only_rerun).unwrap();
    assert!(!detail.active);

    let removal = oxl_engine::history(&case, Some("source"), 1000, 0)
        .unwrap()
        .items
        .into_iter()
        .find(|e| e.action == "source_remove")
        .unwrap();
    let entry = &removal.after.unwrap()["reinstated"][0];
    let mut moved = entry["moved"].as_array().unwrap().clone();
    moved.sort_by_key(|m| m["kind"].as_str().unwrap().to_string());
    assert_eq!(
        moved,
        vec![
            serde_json::json!({"kind": "note", "from_row": taken, "to_row": original,
                               "note_id": note.id, "deleted": false}),
            serde_json::json!({"kind": "row_tag", "from_row": taken, "to_row": original,
                               "tag": "lateral"}),
        ]
    );
    let mut left = entry["left_behind"].as_array().unwrap().clone();
    left.sort_by_key(|m| m["kind"].as_str().unwrap().to_string());
    assert_eq!(
        left,
        vec![
            serde_json::json!({"kind": "note", "from_row": only_rerun, "note_id": stranded.id,
                               "deleted": false}),
            serde_json::json!({"kind": "row_tag", "from_row": only_rerun, "tag": "psexec"}),
        ]
    );
}

const FAILED_LOGON_WS01: &str =
    "SELECT _ox_row FROM evtxtriage_events WHERE MapDescription = 'Failed logon'";
/// `rerun_ws01_events` edits this row's `MapDescription`, so it has no
/// identical row in that run.
const RCLONE_WS01: &str = "SELECT _ox_row FROM evtxtriage_events \
                           WHERE PayloadData1 = 'rclone.exe' AND _triage_host = 'WS01'";

/// The two-host run loaded, the failed logon and the `rclone.exe` row
/// promoted into one entry; returns the entry's id and the two rows.
fn evidence_case(d: &Path, case: &mut Case) -> (i64, i64, i64) {
    use oxl_engine::{attach_rows, promote_rows};
    load_run(case, &oxl_fixtures::two_hosts(&d.join("a")), &analyst()).unwrap();
    let failed = n(case, FAILED_LOGON_WS01);
    let rclone = n(case, RCLONE_WS01);
    let entry = promote_rows(case, &[failed], &analyst()).unwrap()[0]
        .entry_id
        .unwrap();
    assert!(attach_rows(case, entry, &[rclone], &analyst()).unwrap()[0].done);
    (entry, failed, rclone)
}

/// The entry's evidence as `(row_id, standing)`, by row id.
fn standings(case: &Case, entry: i64) -> Vec<(i64, oxl_engine::EvidenceStanding)> {
    let mut out: Vec<_> = oxl_engine::evidence(case, entry)
        .unwrap()
        .into_iter()
        .map(|e| (e.row_id, e.standing))
        .collect();
    out.sort();
    out
}

#[test]
fn evidence_moves_by_hash() {
    use oxl_engine::EvidenceStanding::Unchanged;
    let (d, mut case) = setup();
    let (entry, failed, _) = evidence_case(d.path(), &mut case);
    let snapshot = snapshot_of(&case, entry, failed);
    let r = load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_events(&d.path().join("b")),
        &analyst(),
    )
    .unwrap();
    let ws01 = r
        .superseded
        .iter()
        .find(|s| s.relation == "evtxtriage_events" && s.host == "WS01")
        .unwrap();
    // The failed logon's evidence moved; the edited row's stayed.
    assert_eq!((ws01.moved, ws01.left_behind), (1, 1));
    let moved_to = n(&case, FAILED_LOGON_WS01);
    assert_ne!(moved_to, failed);
    let rows = standings(&case, entry);
    assert!(rows.contains(&(moved_to, Unchanged)), "{rows:?}");
    // The snapshot went with it byte for byte, and the hash with it.
    assert_eq!(snapshot_of(&case, entry, moved_to), snapshot);
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.evidence v join evtxtriage_events r \
                 on r._ox_row = v.row_id and r._ox_hash = v.row_hash where v.row_id = {moved_to}"
            )
        ),
        1
    );
    // The supersession's history lists the move.
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.history where action = 'supersede' \
                 and after_json like '%\"kind\":\"evidence\"%\"to_row\":{moved_to}%'"
            )
        ),
        1
    );
    // The row is still malicious where it now is, and its entry still lists it.
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.row_state where row_id = {moved_to} and mark = 'malicious'"
            )
        ),
        1
    );
}

#[test]
fn unmatched_evidence_stays_from_a_superseded_run() {
    use oxl_engine::EvidenceStanding::{Superseded, Unchanged};
    let (d, mut case) = setup();
    let (entry, failed, rclone) = evidence_case(d.path(), &mut case);
    let later = load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_events(&d.path().join("b")),
        &analyst(),
    )
    .unwrap()
    .source_id;
    let moved_to = n(&case, FAILED_LOGON_WS01);
    assert_eq!(
        standings(&case, entry),
        [(rclone, Superseded), (moved_to, Unchanged)]
    );
    // Removing the later run moves the evidence back to the first run's
    // row, so none is orphaned; the row that stayed behind is in the view
    // again.
    let report = remove_source(&mut case, later, true, &analyst()).unwrap();
    assert_eq!(report.orphaned_evidence, 0);
    assert_eq!(
        standings(&case, entry),
        [(failed, Unchanged), (rclone, Unchanged)]
    );
}

#[test]
fn removing_a_source_with_evidence_needs_confirmation() {
    use oxl_engine::EvidenceStanding::SourceRemoved;
    let (d, mut case) = setup();
    let (entry, failed, rclone) = evidence_case(d.path(), &mut case);
    let before = [failed, rclone].map(|row| snapshot_of(&case, entry, row));

    let (c, message) = code(remove_source(&mut case, 1, false, &analyst()));
    assert_eq!(c, "refused");
    assert!(message.contains("2 evidence rows"), "{message}");
    assert!(!sources(&case).unwrap()[0].removed);

    let report = remove_source(&mut case, 1, true, &analyst()).unwrap();
    assert_eq!(
        (
            report.orphaned_marks,
            report.orphaned_tags,
            report.orphaned_notes,
            report.orphaned_evidence
        ),
        (0, 0, 0, 2)
    );
    assert_eq!(
        standings(&case, entry),
        [(failed, SourceRemoved), (rclone, SourceRemoved)]
    );
    // Every snapshot is kept byte for byte, and the evidence still reads
    // from it.
    assert_eq!(
        [failed, rclone].map(|row| snapshot_of(&case, entry, row)),
        before
    );
    let listed = oxl_engine::evidence(&case, entry).unwrap();
    let summary = &listed.iter().find(|e| e.row_id == failed).unwrap().summary;
    assert!(summary.contains("Failed logon"), "{summary}");
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'source_remove' \
             and after_json like '%\"orphaned_evidence\":2%'"
        ),
        1
    );
}

/// The stored snapshot of row `row` as evidence on `entry`, exactly.
fn snapshot_of(case: &Case, entry: i64, row: i64) -> String {
    case.scalar_text(&format!(
        "select snapshot_json from ox.evidence where entry_id = {entry} and row_id = {row}"
    ))
    .unwrap()
    .unwrap()
}

/// The `after_json` of the newest history row with `action`, parsed.
fn last_after(case: &Case, action: &str) -> serde_json::Value {
    let text = case
        .scalar_text(&format!(
            "select after_json from ox.history where action = '{action}' order by id desc limit 1"
        ))
        .unwrap()
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The evidence entries of a history list, as `(from_row, to_row, entry_id)`.
fn evidence_moves(list: &serde_json::Value) -> Vec<(i64, Option<i64>, i64)> {
    list.as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "evidence")
        .map(|e| {
            (
                e["from_row"].as_i64().unwrap(),
                e["to_row"].as_i64(),
                e["entry_id"].as_i64().unwrap(),
            )
        })
        .collect()
}

/// Evidence moves are not history rows of their own (step 1's rule for
/// annotations): the supersession's history row lists each evidence row it
/// moved, with its row before and after, and each it left; the removal
/// that reinstates the older run lists each it moved back.
#[test]
fn evidence_moves_are_listed_in_the_history() {
    let (d, mut case) = setup();
    let (entry, failed, rclone) = evidence_case(d.path(), &mut case);
    let later = load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_events(&d.path().join("b")),
        &analyst(),
    )
    .unwrap()
    .source_id;
    let moved_to = n(&case, FAILED_LOGON_WS01);
    // The rerun holds only WS01's events: one supersession.
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'supersede'"
        ),
        1
    );
    let supersede = last_after(&case, "supersede");
    assert_eq!(
        evidence_moves(&supersede["moved"]),
        [(failed, Some(moved_to), entry)]
    );
    assert_eq!(
        evidence_moves(&supersede["left_behind"]),
        [(rclone, None, entry)]
    );

    remove_source(&mut case, later, true, &analyst()).unwrap();
    let removal = last_after(&case, "source_remove");
    let reinstated = removal["reinstated"].as_array().unwrap();
    let ws01 = reinstated
        .iter()
        .find(|r| r["relation"] == "evtxtriage_events" && r["host"] == "WS01")
        .unwrap();
    assert_eq!(
        evidence_moves(&ws01["moved"]),
        [(moved_to, Some(failed), entry)]
    );
}

/// Deleting an entry keeps its evidence, so a source holding only a deleted
/// entry's evidence still needs confirmation to remove.
#[test]
fn removing_a_source_with_a_deleted_entrys_evidence_needs_confirmation() {
    let (d, mut case) = setup();
    let (entry, failed, rclone) = evidence_case(d.path(), &mut case);
    oxl_engine::delete_entry(&mut case, entry, &analyst()).unwrap();
    let before = [failed, rclone].map(|row| snapshot_of(&case, entry, row));

    let (c, message) = code(remove_source(&mut case, 1, false, &analyst()));
    assert_eq!(c, "refused", "{message}");
    assert!(message.contains("2 evidence rows"), "{message}");
    assert!(!sources(&case).unwrap()[0].removed);

    let report = remove_source(&mut case, 1, true, &analyst()).unwrap();
    assert_eq!(report.orphaned_evidence, 2);
    assert_eq!(
        [failed, rclone].map(|row| snapshot_of(&case, entry, row)),
        before
    );
}
