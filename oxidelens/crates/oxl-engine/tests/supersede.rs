//! A second run of a host: the newer rows replace the older ones per
//! dataset and per host, annotations follow identical rows, and a source can
//! be removed. Marks, tags and notes are written through the annotation API.

mod common;

use common::{agent, analyst, copy_tree, n};
use oxl_engine::{
    add_note, add_tag, delete_note, load_file, load_run, mark_suspicious, relation, relations,
    remove_source, set_primary_time, sources, Case, FileOptions, LoadReport,
};
use std::path::{Path, PathBuf};

fn code<T>(result: oxl_engine::Result<T>) -> (&'static str, String) {
    match result {
        Ok(_) => ("ok", String::new()),
        Err(e) => (e.code(), e.to_string()),
    }
}

/// A copy of the run at `original` under `copy`, its run id set to `run_id`
/// and `edit` applied to its parsed datasets.json.
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
        .0;
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
        .0
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

#[test]
fn the_raw_view_shows_a_column_only_a_superseded_run_had_as_null() {
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
    assert_eq!(
        n(
            &case,
            "select count(*) from raw_petriage_timeline where ExecutableName is null"
        ),
        5
    );
    assert_eq!(n(&case, "select count(*) from raw_petriage_timeline"), 5);
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
