//! Loading a TriageSuite run: the tables and views it produces, the row
//! identity it assigns, what it records about the source, and the refusals
//! that leave the case untouched.

mod common;

use common::{analyst, loaded, long_dir, n};
use oxl_engine::{load_run, sources, Case};
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

/// A copy of the two-host run under `<dir>/copy`, with `edit` applied to its
/// parsed datasets.json.
fn edited_copy(dir: &Path, edit: impl FnOnce(&mut serde_json::Value)) -> PathBuf {
    let original = oxl_fixtures::two_hosts(&dir.join("original"));
    let copy = dir.join("copy");
    copy_tree(&original, &copy);
    let path = copy.join("duckdb").join("datasets.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut json);
    std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    copy
}

/// The set of distinct `_ox_hash` values of `evtxtriage_events`, as its
/// size and a fingerprint of the sorted set. `scalar_i64` reads integers
/// only, so the set is folded to one; the shift keeps the u64 in i64 range.
fn hash_set(case: &Case) -> (i64, i64) {
    (
        n(
            case,
            "select count(distinct _ox_hash) from evtxtriage_events",
        ),
        n(
            case,
            "select (hash(string_agg(h, ',' order by h)) >> 1)::BIGINT \
             from (select distinct _ox_hash h from evtxtriage_events)",
        ),
    )
}

fn assert_two_hosts_counts(case: &Case) {
    assert_eq!(n(case, "select count(*) from evtxtriage_events"), 11);
    assert_eq!(n(case, "select count(*) from petriage_timeline"), 5);
    assert_eq!(n(case, "select count(*) from ext_hayabusa_csv_timeline"), 5);
}

#[test]
fn loads_every_dataset_with_the_typed_view_shape() {
    let (_d, case) = loaded();
    assert_two_hosts_counts(&case);
    assert_eq!(
        n(
            &case,
            "select count(*) from evtxtriage_events \
             where TimeCreated is null and TimeCreated__text is not null"
        ),
        1
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from evtxtriage_events where EventId > 4000"
        ),
        6
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from raw_evtxtriage_events where TimeCreated = 'not-a-time'"
        ),
        1
    );
    assert_eq!(
        n(
            &case,
            "select count(distinct _triage_host) from evtxtriage_events"
        ),
        2
    );
}

#[test]
fn row_ids_are_unique_and_identical_rows_share_a_hash() {
    let (_d, case) = loaded();
    assert_eq!(
        n(
            &case,
            "select count(distinct _ox_row) from evtxtriage_events"
        ),
        11
    );
    assert_eq!(
        n(
            &case,
            "select count(distinct _ox_hash) from evtxtriage_events"
        ),
        10
    );
    assert_eq!(n(&case, "select max(_ox_occ) from evtxtriage_events"), 2);
}

#[test]
fn the_hash_ignores_run_id_and_output_path() {
    let d = tempfile::tempdir().unwrap();
    let dir1 = d.path().join("one");
    let root1 = oxl_fixtures::two_hosts(&dir1);
    let mut a = Case::create(&d.path().join("a.case.duckdb"), "A", &analyst()).unwrap();
    load_run(&mut a, &root1, &analyst()).unwrap();

    let root2 = edited_copy(&d.path().join("two"), |json| {
        json["run_id"] = "20990101000000-zzzz".into();
    });
    let mut b = Case::create(&d.path().join("b.case.duckdb"), "B", &analyst()).unwrap();
    load_run(&mut b, &root2, &analyst()).unwrap();

    // The two loads really did differ in run id and output path.
    assert_eq!(
        n(
            &b,
            "select count(*) from evtxtriage_events where _triage_run_id = '20990101000000-zzzz'"
        ),
        11
    );
    let ha = hash_set(&a);
    assert_eq!(ha.0, 10);
    assert_eq!(ha, hash_set(&b));
}

#[test]
fn records_the_source_and_the_inventory() {
    let (_d, case) = loaded();
    let s = sources(&case).unwrap();
    assert_eq!(
        (s.len(), s[0].kind.as_str(), s[0].run_id.as_deref()),
        (1, "run", Some("20260101000000-aaaa"))
    );
    assert_eq!(s[0].hosts, vec!["SRV01".to_string(), "WS01".to_string()]);
    assert_eq!(
        (s[0].loaded_by.as_str(), s[0].removed),
        ("analyst:tester", false)
    );
    assert_eq!(n(&case, "select count(*) from ox.inventory_dataset"), 3);
    assert_eq!(n(&case, "select count(*) from ox.inventory_file"), 5);
    assert_eq!(
        n(&case, "select count(*) from ox.source_dataset where active"),
        5
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history \
             where action = 'source_load' and target_kind = 'source' and \"by\" = 'analyst:tester'"
        ),
        1
    );
}

/// Found on the first real run: the per-channel event log files have long
/// names, and under an ordinary output root their paths passed the limit.
#[test]
fn a_run_whose_csv_paths_pass_the_windows_path_limit_loads() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(&long_dir(d.path()));
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &root, &analyst()).unwrap();
    assert_two_hosts_counts(&case);

    // What the analyst sees stays an ordinary path: the `\\?\` form is only
    // how the files are read.
    assert_eq!(
        n(
            &case,
            r"select count(*) from evtxtriage_events
              where starts_with(_triage_output_file, '\\?\')
                 or length(_triage_output_file) <= 260"
        ),
        0
    );
    let s = sources(&case).unwrap();
    assert!(
        s[0].path.len() > 260 && !s[0].path.starts_with(r"\\?\"),
        "{}",
        s[0].path
    );
}

#[test]
fn the_same_run_twice_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(d.path());
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &root, &analyst()).unwrap();
    let (c, message) = code(load_run(&mut case, &root, &analyst()));
    assert_eq!(c, "refused", "{message}");
    assert!(message.contains("already in this case"), "{message}");
    assert_eq!(sources(&case).unwrap().len(), 1);
    assert_two_hosts_counts(&case);
}

/// Load `fw.csv` under the relation name `name`.
fn load_fw_as(case: &mut Case, dir: &Path, name: &str) -> oxl_engine::Result<()> {
    let fw = oxl_fixtures::loose_fw_csv(dir);
    oxl_engine::load_file(
        case,
        &fw,
        oxl_engine::FileOptions {
            name: Some(name.into()),
            ..Default::default()
        },
        &analyst(),
    )
    .map(|_| ())
}

/// The case still holds only the file loaded as `name`, and its view works.
fn assert_only_the_file(case: &Case, name: &str) {
    assert_eq!(sources(case).unwrap().len(), 1);
    assert_eq!(n(case, &format!("select count(*) from \"{name}\"")), 3);
    assert_eq!(n(case, "select count(*) from ox.relation"), 1);
    assert_eq!(n(case, "select count(*) from ox.history"), 1);
    assert_eq!(
        n(
            case,
            "select count(*) from duckdb_schemas() where schema_name like 'src_%' \
             or schema_name = 'ox_stage'"
        ),
        1
    );
}

#[test]
fn a_dataset_whose_raw_view_a_file_has_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts(&d.path().join("run"));
    let mut case = oxl_fixtures::new_case(d.path());
    load_fw_as(&mut case, d.path(), "raw_evtxtriage_events").unwrap();
    let (c, message) = code(load_run(&mut case, &root, &analyst()));
    assert_eq!(c, "refused", "{message}");
    assert!(
        message.contains("evtxtriage_events") && message.contains("raw_evtxtriage_events"),
        "{message}"
    );
    assert_only_the_file(&case, "raw_evtxtriage_events");
}

#[test]
fn a_dataset_named_like_a_files_raw_view_is_refused_whatever_its_case() {
    let d = tempfile::tempdir().unwrap();
    let root = edited_copy(&d.path().join("run"), |json| {
        for dataset in json["datasets"].as_array_mut().unwrap() {
            if dataset["view"] == "petriage_timeline" {
                dataset["view"] = "RAW_FW".into();
            }
        }
    });
    let mut case = oxl_fixtures::new_case(d.path());
    load_fw_as(&mut case, d.path(), "fw").unwrap();
    let (c, message) = code(load_run(&mut case, &root, &analyst()));
    assert_eq!(c, "refused", "{message}");
    assert!(
        message.contains("RAW_FW") && message.contains("fw"),
        "{message}"
    );
    assert_only_the_file(&case, "fw");
}

#[test]
fn a_file_named_like_a_datasets_raw_view_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let (_run, mut case) = loaded();
    let (c, message) = code(load_fw_as(&mut case, d.path(), "raw_evtxtriage_events"));
    assert_eq!(c, "refused", "{message}");
    assert_eq!(sources(&case).unwrap().len(), 1);
    assert_two_hosts_counts(&case);
}

#[test]
fn a_status_other_than_ok_is_invalid_with_the_reason() {
    let d = tempfile::tempdir().unwrap();
    let root = edited_copy(d.path(), |json| json["status"] = "no-csv-output".into());
    let mut case = oxl_fixtures::new_case(d.path());
    let (c, message) = code(load_run(&mut case, &root, &analyst()));
    // The run named is what is wrong, not the actor or the case.
    assert_eq!(c, "invalid", "{message}");
    assert!(message.contains("no-csv-output"), "{message}");
    assert!(sources(&case).unwrap().is_empty());
}

#[test]
fn an_unknown_sql_type_in_the_inventory_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let root = edited_copy(d.path(), |json| {
        json["datasets"][0]["effective_types"]["TimeCreated"]["sql_type"] =
            "TIMESTAMP); DROP TABLE x; --".into();
    });
    let mut case = oxl_fixtures::new_case(d.path());
    let (c, message) = code(load_run(&mut case, &root, &analyst()));
    assert_eq!(c, "invalid", "{message}");
    assert!(sources(&case).unwrap().is_empty());
}

#[test]
fn a_missing_csv_fails_cleanly_and_leaves_nothing() {
    let d = tempfile::tempdir().unwrap();
    let root = edited_copy(d.path(), |_| {});
    std::fs::remove_file(
        root.join("Processed-SRV01-20260101000000")
            .join("Execution")
            .join("pf.csv"),
    )
    .unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let (c, message) = code(load_run(&mut case, &root, &analyst()));
    assert_ne!(c, "ok");
    assert!(message.contains("pf.csv"), "{message}");
    assert!(sources(&case).unwrap().is_empty());
    assert_eq!(
        n(
            &case,
            "select count(*) from duckdb_schemas() where schema_name like 'src_%'"
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
    assert_eq!(n(&case, "select count(*) from ox.column_catalog"), 0);
    assert_eq!(n(&case, "select count(*) from ox.relation"), 0);
    assert_eq!(n(&case, "select count(*) from ox.history"), 0);
    // The failed load did not leave the session pointed at the staging schema.
    assert_eq!(
        n(&case, "select count(*) from ox.meta where key = 'format'"),
        1
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from duckdb_views() where not internal and schema_name = 'main'"
        ),
        0
    );
}

#[test]
fn a_moved_collection_loads_without_regenerate() {
    let d = tempfile::tempdir().unwrap();
    let built = oxl_fixtures::two_hosts(&d.path().join("before"));
    let moved = d.path().join("after-the-move");
    std::fs::rename(&built, &moved).unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &moved, &analyst()).unwrap();
    assert_two_hosts_counts(&case);
}

#[test]
fn loads_a_real_run_when_one_is_provided() {
    // Opt-in: no real run is normally at hand, and an unset variable is not a
    // missing fixture. Once a path is named it must exist; a missing one goes
    // through the suite's gate and panics unless TRIAGE_ALLOW_COMPAT_SKIP=1.
    let Some(p) = std::env::var_os("OXIDELENS_REAL_RUN") else {
        eprintln!(
            "SKIP loads_a_real_run_when_one_is_provided: set OXIDELENS_REAL_RUN to a TriageSuite run output root to run it"
        );
        return;
    };
    let p = std::path::PathBuf::from(p);
    if triage_testkit::skip_if_missing(&p, "a real TriageSuite run for OxideLens") {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let report = load_run(&mut case, Path::new(&p), &analyst()).unwrap();
    let mut any_rows = false;
    for relation in &report.relations {
        let viewed = n(
            &case,
            &format!(
                "select count(*) from main.\"{}\"",
                relation.name.replace('"', "\"\"")
            ),
        );
        let recorded = n(
            &case,
            &format!(
                "select sum(rows)::BIGINT from ox.source_table where relation = '{}'",
                relation.name.replace('\'', "''")
            ),
        );
        assert_eq!(viewed, recorded, "{}", relation.name);
        assert_eq!(
            u64::try_from(viewed).unwrap(),
            relation.rows,
            "{}",
            relation.name
        );
        any_rows |= viewed > 0;
    }
    assert!(any_rows, "no relation of the real run has a row");
}
