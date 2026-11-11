//! A second run of a host: the newer rows replace the older ones per
//! dataset and per host, annotations follow identical rows, and a source can
//! be removed. Marks, tags and notes are written with plain SQL here; their
//! API arrives later.

mod common;

use common::{agent, analyst, n};
use oxl_engine::{
    load_file, load_run, relations, remove_source, sources, Case, FileOptions, LoadReport,
};
use std::path::{Path, PathBuf};

fn code<T>(result: oxl_engine::Result<T>) -> (&'static str, String) {
    match result {
        Ok(_) => ("ok", String::new()),
        Err(e) => (e.code(), e.to_string()),
    }
}

/// Copy a run tree, so a test can alter the copy and never the run the
/// fixture built.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
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

/// Run `sql` against the case file directly: the case is closed, written
/// and reopened, because the marks API does not exist yet.
fn with_sql(case: Case, sql: &str) -> Case {
    let path = case.path().to_path_buf();
    drop(case);
    {
        let conn = duckdb::Connection::open(&path).unwrap();
        conn.execute_batch(sql)
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    Case::open(&path).unwrap()
}

/// SQL marking the rows `select_rows` (a SELECT of `_ox_row`) suspicious.
fn mark(select_rows: &str) -> String {
    format!(
        "INSERT INTO ox.row_mark (row_id, mark, marked_by, marked_at) \
         SELECT _ox_row, 'suspicious', 'analyst:tester', '2026-11-11T00:00:00.000000Z' \
         FROM ({select_rows});"
    )
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
    let old_row = n(&case, SVCH0ST_WS01);
    let mut case = with_sql(
        case,
        &format!(
            "{} \
             INSERT INTO ox.tag VALUES ('lateral', 'analyst:tester', '2026-11-11T00:00:00.000000Z'); \
             INSERT INTO ox.row_tag VALUES ({old_row}, 'lateral', 'analyst:tester', '2026-11-11T00:00:00.000000Z'); \
             INSERT INTO ox.note VALUES (nextval('ox.note_seq'), {old_row}, 'look here', 'analyst:tester', \
                                         '2026-11-11T00:00:00.000000Z', false);",
            mark(SVCH0ST_WS01)
        ),
    );
    let r = load_run(
        &mut case,
        &oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b")),
        &analyst(),
    )
    .unwrap();
    assert_eq!((r.superseded[0].moved, r.superseded[0].left_behind), (1, 0));

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
         and json_extract(after_json, '$.moved')::BIGINT = 1 \
         and json_extract(after_json, '$.left_behind')::BIGINT = 0",
    );
    assert_eq!(after, 1);
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
    let old_row = n(&case, SVCH0ST_WS01);
    let mut case = with_sql(case, &mark(SVCH0ST_WS01));

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
    let mut case = with_sql(
        case,
        &mark(
            "SELECT _ox_row FROM evtxtriage_events \
             WHERE EventId = 1102 AND _triage_host = 'WS01' AND _ox_occ = 2",
        ),
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
    let mut case = with_sql(
        case,
        &mark("SELECT _ox_row FROM evtxtriage_events WHERE EventId = 4625"),
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
    // Source 1's WS01 prefetch rows stay superseded; its SRV01 row stays.
    assert_eq!(n(&case, "select count(*) from petriage_timeline"), 1);
    assert_eq!(n(&case, "select count(*) from evtxtriage_events"), 11);
    assert!(relations(&case)
        .unwrap()
        .iter()
        .any(|r| r.name == "petriage_timeline"));
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
fn each_arm_filters_on_its_own_host_column() {
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
    let r = load_run(&mut case, &renamed, &analyst()).unwrap();
    assert_eq!(r.superseded.len(), 5);
    assert_eq!(
        n(
            &case,
            "select count(*) from petriage_timeline where \"_triage_host_1\" is not null"
        ),
        5
    );
    assert_eq!(n(&case, "select count(*) from petriage_timeline"), 5);
}

#[test]
fn two_live_arms_with_different_host_columns_each_filter_on_their_own() {
    let (d, mut case) = setup();
    load_run(
        &mut case,
        &oxl_fixtures::two_hosts(&d.path().join("a")),
        &analyst(),
    )
    .unwrap();
    let rerun = oxl_fixtures::rerun_ws01_prefetch(&d.path().join("b"));
    let renamed = copy_run(&rerun, &d.path().join("c"), "20260102000000-bbbb", |json| {
        json["datasets"][0]["metadata_columns"]["host"] = "_triage_host_1".into();
    });
    let r = load_run(&mut case, &renamed, &analyst()).unwrap();
    assert_eq!(
        superseded_pairs(&r),
        [("petriage_timeline".to_string(), "WS01".to_string())]
    );
    // Source 1's arm keeps SRV01 under _triage_host; source 2's WS01 rows
    // carry their host under _triage_host_1.
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
            "select count(*) from petriage_timeline where \"_triage_host_1\" = 'WS01'"
        ),
        5
    );
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
