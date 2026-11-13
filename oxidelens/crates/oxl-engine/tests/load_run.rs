//! Loading a TriageSuite run: the tables and views it produces, the row
//! identity it assigns, what it records about the source, and the refusals
//! that leave the case untouched.

mod common;

use common::{analyst, loaded, long_dir, n};
use oxl_engine::{load_run, load_run_with, sources, Case, RunOptions};
use std::path::{Path, PathBuf};

fn code<T>(result: oxl_engine::Result<T>) -> (&'static str, String) {
    match result {
        Ok(_) => ("ok", String::new()),
        Err(e) => (e.code(), e.to_string()),
    }
}

/// The relations of the case, by name.
fn relation_names(case: &Case) -> Vec<String> {
    oxl_engine::relations(case)
        .unwrap()
        .into_iter()
        .map(|r| r.name)
        .collect()
}

/// Found on the first real run: EvtxTriage writes every event twice, once
/// in its aggregate and once in a per-channel copy under `Individual/`, so
/// 106 relations held nothing the aggregate lacked, took two thirds of a
/// 57 s load, and doubled every find and pivot over the event log.
/// Found on the second real run: a load on all 32 threads of the machine
/// held 7.2 GB, each thread with its own buffers; on 8 it held 4.7 GB and
/// was faster. Both loads run on at most 8, and leave the setting as it was.
#[test]
fn a_load_runs_on_at_most_eight_threads_and_leaves_the_setting_as_it_was() {
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let threads = |c: &Case| c.scalar_i64("SELECT current_setting('threads')").unwrap();
    let before = threads(&case);
    let run = oxl_fixtures::two_hosts(&d.path().join("run"));
    let fw = oxl_fixtures::loose_fw_csv(d.path());
    let (_, ran_run) = case
        .statements_during_mut(|c| load_run(c, &run, &analyst()).unwrap())
        .unwrap();
    let (_, ran_file) = case
        .statements_during_mut(|c| {
            oxl_engine::load_file(c, &fw, oxl_engine::FileOptions::default(), &analyst()).unwrap()
        })
        .unwrap();
    assert_eq!(threads(&case), before);
    // A load that fails (the run is already in the case) puts it back too.
    assert!(load_run(&mut case, &run, &analyst()).is_err());
    assert_eq!(threads(&case), before);
    for ran in [&ran_run, &ran_file] {
        let bounded = ran.iter().any(|s| s.contains("SET threads = 8"));
        // A machine with 8 threads or fewer keeps its own.
        assert_eq!(bounded, before > 8, "{before} threads: {ran:#?}");
    }
}

#[test]
fn per_channel_copies_of_the_event_log_are_left_out_unless_asked_for() {
    let d = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::with_channels(&d.path().join("run"), true);
    let mut case = oxl_fixtures::new_case(d.path());
    let report = load_run(&mut case, &run, &analyst()).unwrap();
    assert_eq!(relation_names(&case), ["evtxtriage_events"]);
    assert_eq!(
        report
            .skipped
            .iter()
            .map(|s| (s.relation.as_str(), s.dataset_id.as_str()))
            .collect::<Vec<_>>(),
        [
            ("evtxtriage_individual_security", "Individual/Security"),
            ("evtxtriage_individual_sysmon", "Individual/Sysmon"),
        ]
    );
    assert_eq!(n(&case, "select count(*) from evtxtriage_events"), 8);
    // The case still says what the run held, and what was left out.
    assert_eq!(
        n(&case, "select count(*) from ox.inventory_dataset"),
        3,
        "every dataset of the run is in the inventory"
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history \
             where action = 'source_load' and after_json like '%Individual/Security%'"
        ),
        1
    );

    // Asked for, they load.
    let d = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::with_channels(&d.path().join("run"), true);
    let mut case = oxl_fixtures::new_case(d.path());
    let report = load_run_with(
        &mut case,
        &run,
        RunOptions { per_channel: true },
        &analyst(),
    )
    .unwrap();
    assert!(report.skipped.is_empty());
    assert_eq!(
        relation_names(&case),
        [
            "evtxtriage_events",
            "evtxtriage_individual_security",
            "evtxtriage_individual_sysmon"
        ]
    );
    assert_eq!(
        n(&case, "select count(*) from evtxtriage_individual_security"),
        5
    );
}

/// Reviewed: a copy is left out host by host. A host the aggregate holds no
/// included file for keeps its per-channel rows, the only ones it has.
#[test]
fn a_per_channel_file_loads_for_a_host_the_aggregate_lacks() {
    let d = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::with_channels(&d.path().join("run"), true);
    // SRV01 has a Security copy (the same CSV, as SRV01's) and no aggregate.
    let path = run.join("duckdb").join("datasets.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let security = json["datasets"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|ds| ds["dataset_id"] == "Individual/Security")
        .unwrap();
    let mut srv = security["files"][0].clone();
    srv["host"] = "SRV01".into();
    security["files"].as_array_mut().unwrap().push(srv);
    std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

    let mut case = oxl_fixtures::new_case(d.path());
    let report = load_run(&mut case, &run, &analyst()).unwrap();
    assert_eq!(
        relation_names(&case),
        ["evtxtriage_events", "evtxtriage_individual_security"]
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from evtxtriage_individual_security where _triage_host = 'SRV01'"
        ),
        5
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from evtxtriage_individual_security where _triage_host = 'WS01'"
        ),
        0
    );
    assert_eq!(
        report
            .skipped
            .iter()
            .map(|s| (s.dataset_id.as_str(), s.hosts.clone()))
            .collect::<Vec<_>>(),
        [
            ("Individual/Security", vec!["WS01".to_string()]),
            ("Individual/Sysmon", vec!["WS01".to_string()]),
        ]
    );
}

/// Reviewed, and kept: a per-channel relation an earlier load brought (an
/// older build, or `--per-channel`) is not superseded by a later run that
/// leaves the copies out, since that run has no rows for it. Removing the
/// earlier source takes it out.
#[test]
fn an_earlier_per_channel_load_stays_until_its_source_is_removed() {
    let d = tempfile::tempdir().unwrap();
    let first = oxl_fixtures::with_channels(&d.path().join("first"), true);
    let mut case = oxl_fixtures::new_case(d.path());
    let earlier = load_run_with(
        &mut case,
        &first,
        RunOptions { per_channel: true },
        &analyst(),
    )
    .unwrap();
    // The same run under another id: a later run of WS01.
    let later = d.path().join("later");
    copy_tree(&first, &later);
    let path = later.join("duckdb").join("datasets.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    json["run_id"] = "20260108000000-gggg".into();
    json["generation"] = "20260108000000-gggg-g1".into();
    std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    load_run(&mut case, &later, &analyst()).unwrap();
    assert_eq!(n(&case, "select count(*) from evtxtriage_events"), 8);
    assert_eq!(
        n(&case, "select count(*) from evtxtriage_individual_security"),
        5
    );
    oxl_engine::remove_source(&mut case, earlier.source_id, false, &analyst()).unwrap();
    assert_eq!(relation_names(&case), ["evtxtriage_events"]);
    assert_eq!(n(&case, "select count(*) from evtxtriage_events"), 8);
}

/// Reviewed: a left-out copy is in the inventory but feeds no relation, so
/// it does not bind how a later source names the relation's metadata
/// columns.
#[test]
fn a_left_out_copy_does_not_bind_a_later_load() {
    let d = tempfile::tempdir().unwrap();
    let first = oxl_fixtures::with_channels(&d.path().join("first"), true);
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &first, &analyst()).unwrap();
    // A later run whose Security copy names its host column differently.
    let later = d.path().join("later");
    copy_tree(&first, &later);
    let path = later.join("duckdb").join("datasets.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    json["run_id"] = "20260109000000-hhhh".into();
    json["generation"] = "20260109000000-hhhh-g1".into();
    for ds in json["datasets"].as_array_mut().unwrap() {
        if ds["dataset_id"] == "Individual/Security" {
            ds["metadata_columns"]["host"] = "_triage_host_1".into();
        }
    }
    std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    load_run_with(
        &mut case,
        &later,
        RunOptions { per_channel: true },
        &analyst(),
    )
    .unwrap();
    assert_eq!(
        n(
            &case,
            "select count(*) from evtxtriage_individual_security where _triage_host_1 = 'WS01'"
        ),
        5
    );
}

/// Without the aggregate a per-channel dataset is no copy of anything.
#[test]
fn per_channel_datasets_load_when_the_run_has_no_aggregate() {
    let d = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::with_channels(&d.path().join("run"), false);
    let mut case = oxl_fixtures::new_case(d.path());
    let report = load_run(&mut case, &run, &analyst()).unwrap();
    assert!(report.skipped.is_empty());
    assert_eq!(
        relation_names(&case),
        [
            "evtxtriage_individual_security",
            "evtxtriage_individual_sysmon"
        ]
    );
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

/// A run id is a stamp with no host in it, so two collections processed in
/// the same millisecond share one; the generation is what names one
/// writing of a run's output. The same generation twice is the same run.
#[test]
fn same_run_id_new_generation_loads() {
    let d = tempfile::tempdir().unwrap();
    let first = oxl_fixtures::two_hosts(&d.path().join("a"));
    let second = edited_copy(&d.path().join("b"), |json| {
        assert_eq!(json["run_id"], "20260101000000-aaaa");
        json["generation"] = "20260101000000-aaaa-g2".into();
    });
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &first, &analyst()).unwrap();
    load_run(&mut case, &second, &analyst()).unwrap();
    assert_eq!(sources(&case).unwrap().len(), 2);
    // The second superseded the first, host by host.
    assert_two_hosts_counts(&case);

    let (c, message) = code(load_run(&mut case, &second, &analyst()));
    assert_eq!(c, "refused", "{message}");
    assert!(message.contains("already in this case"), "{message}");
    assert_eq!(sources(&case).unwrap().len(), 2);
}

/// The first WS01 row of `evtxtriage_events`' hash, pinned. Supersession
/// matches rows by this hash across runs and across builds, so if this
/// changes (a DuckDB upgrade that writes `to_json` differently, a change to
/// `row_hash_sql` or to the columns hashed), every existing case's rows stop
/// matching the next run's, and annotations no longer follow them.
#[test]
fn row_hash_is_pinned() {
    let (_d, case) = loaded();
    // The fixture's first WS01 event: the failed logon at 02:13:58.
    let row = n(
        &case,
        "SELECT _ox_row FROM evtxtriage_events WHERE _triage_host = 'WS01' \
         AND TimeCreated__text = '2026-01-05T02:13:58.2040000Z'",
    );
    let hash = oxl_engine::row_detail(&case, row).unwrap().hash.unwrap();
    // If this changes, every case's supersession matching changes with it:
    // rows hashed by an earlier build no longer match a later run's rows.
    assert_eq!(
        hash,
        "b9cceffeb2ed30795c0dcba551add768b9f0bf06b3ab9b5e24ab3c3fb1eb689c"
    );
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
