//! The column catalog: what the suite typed, what the app types for it,
//! what an analyst may declare, and which column is a relation's time.

mod common;

use common::{agent, analyst, loaded, n};
use oxl_engine::catalog::appmap::{APP_TYPES, PRIMARY_TIME};
use oxl_engine::catalog::{
    columns, declare_type, relation, relations, set_primary_time, ColumnRole, RelationKind,
    SqlType, TimeSemantics, TypeOrigin,
};
use oxl_engine::{load_file, FileOptions};
use std::collections::BTreeSet;

#[test]
fn suite_types_are_catalogued_as_declared() {
    let (_d, case) = loaded();
    let c = columns(&case, "evtxtriage_events").unwrap();
    let t = c.iter().find(|c| c.name == "TimeCreated").unwrap();
    assert_eq!(
        (
            t.text_column.as_str(),
            t.typed_column.as_deref(),
            t.sql_type,
            t.time_semantics,
            t.origin
        ),
        (
            "TimeCreated__text",
            Some("TimeCreated"),
            Some(SqlType::Timestamp),
            Some(TimeSemantics::Utc),
            Some(TypeOrigin::Suite)
        )
    );
    assert!(c
        .iter()
        .all(|c| !c.name.starts_with("_ox_") && !c.name.ends_with("__text")));
    assert_eq!(
        c.iter()
            .map(|c| c.name.as_str())
            .take(3)
            .collect::<Vec<_>>(),
        ["_triage_run_id", "_triage_host", "_triage_identity"]
    );
}

#[test]
fn hayabusa_timestamp_is_typed_by_the_app_without_touching_the_original() {
    let (_d, case) = loaded();
    let t = columns(&case, "ext_hayabusa_csv_timeline")
        .unwrap()
        .into_iter()
        .find(|c| c.name == "Timestamp")
        .unwrap();
    assert_eq!(
        (
            t.text_column.as_str(),
            t.typed_column.as_deref(),
            t.origin,
            t.time_semantics
        ),
        (
            "Timestamp",
            Some("Timestamp__typed"),
            Some(TypeOrigin::AppMap),
            Some(TimeSemantics::Utc)
        )
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ext_hayabusa_csv_timeline where typeof(\"Timestamp\") = 'VARCHAR'"
        ),
        5
    );
    // The row without Z stays untyped.
    assert_eq!(
        n(
            &case,
            "select count(*) from ext_hayabusa_csv_timeline where \"Timestamp__typed\" is not null"
        ),
        4
    );
    // App typing is not a change anyone made: only the load's own row.
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action <> 'source_load'"
        ),
        0
    );
}

#[test]
fn primary_time_defaults() {
    let (_d, case) = loaded();
    assert_eq!(
        relation(&case, "evtxtriage_events")
            .unwrap()
            .primary_time
            .as_deref(),
        Some("TimeCreated")
    );
    assert_eq!(
        relation(&case, "ext_hayabusa_csv_timeline")
            .unwrap()
            .primary_time
            .as_deref(),
        Some("Timestamp")
    );
}

/// Found on the first real run: declaring a column a timestamp on a
/// relation with no primary time left it with none, though a later load of
/// the same relation then picked that column. The declaration applies the
/// load's own default, which never overrides an analyst's choice.
#[test]
fn declaring_a_timestamp_gives_a_relation_without_one_its_primary_time() {
    let (d, mut case) = loaded();
    // One value that is no time keeps the column from being typed on load.
    let csv = d.path().join("logins.csv");
    std::fs::write(
        &csv,
        "user,stamp\nalice,2026-01-05T02:14:00Z\nbob,not recorded\n",
    )
    .unwrap();
    let name = oxl_engine::load_file(
        &mut case,
        &csv,
        oxl_engine::FileOptions::default(),
        &analyst(),
    )
    .unwrap()
    .relations[0]
        .name
        .clone();
    assert_eq!(relation(&case, &name).unwrap().primary_time, None);
    let declare = |case: &mut oxl_engine::Case| {
        declare_type(
            case,
            &name,
            "stamp",
            SqlType::Timestamp,
            Some(oxl_engine::TimeSemantics::Utc),
            &analyst(),
        )
        .unwrap()
    };
    declare(&mut case);
    assert_eq!(
        relation(&case, &name).unwrap().primary_time.as_deref(),
        Some("stamp")
    );
    // The declaration's history row says what it did to the primary time.
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'declare_type' \
             and json_extract(before_json, '$.primary_time') = 'null' \
             and json_extract_string(after_json, '$.primary_time') = 'stamp'"
        ),
        1
    );

    // An analyst who chose no primary time keeps that choice.
    set_primary_time(&mut case, &name, None, &analyst()).unwrap();
    declare(&mut case);
    assert_eq!(relation(&case, &name).unwrap().primary_time, None);
}

/// Found on the first real run: 16 relations had no primary time, so
/// `around` and a time-bounded `find` passed them by, and `petriage_main`'s
/// timeline ran on `PreviousRun0` rather than its last run.
#[test]
fn the_app_gives_registry_and_prefetch_relations_their_timeline() {
    let d = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::app_timed(&d.path().join("run"));
    let mut case = oxl_fixtures::new_case(d.path());
    oxl_engine::load_run(&mut case, &run, &analyst()).unwrap();
    for (rel, time) in [
        ("petriage_main", "LastRun"),
        ("retriage_userassist_ntuser_dat", "LastExecuted"),
        ("appcompattriage_appcompat", "LastModifiedTimeUTC"),
        ("retriage_taskcache_software", "CreatedOn"),
        ("sbetriage_shellbags", "LastWriteTime"),
    ] {
        assert_eq!(
            relation(&case, rel).unwrap().primary_time.as_deref(),
            Some(time),
            "{rel}"
        );
        assert_eq!(
            n(
                &case,
                &format!("select count(*) from {rel} where \"{time}__typed\" is not null")
            ),
            if rel == "petriage_main" { 2 } else { 1 },
            "{rel}.{time} did not cast"
        );
        let column = columns(&case, rel)
            .unwrap()
            .into_iter()
            .find(|c| c.name == time)
            .unwrap();
        assert_eq!(
            (column.sql_type, column.time_semantics, column.origin),
            (
                Some(SqlType::Timestamp),
                Some(oxl_engine::TimeSemantics::Utc),
                Some(TypeOrigin::AppMap)
            ),
            "{rel}.{time}"
        );
    }
    // A value without the Z names no zone and stays untyped.
    assert_eq!(
        n(
            &case,
            "select count(*) from retriage_userassist_ntuser_dat where \"LastExecuted__typed\" is not null"
        ),
        1
    );
    // Every app-chosen primary time the app types is one it types as a time.
    for (rel, column) in PRIMARY_TIME {
        if let Some(app) = APP_TYPES
            .iter()
            .find(|t| t.relation == *rel && t.column == *column)
        {
            assert_eq!(app.sql_type, SqlType::Timestamp, "{rel}.{column}");
        }
    }
}

/// Found on the second real run, a domain controller: 37 relations had no
/// timeline, every browser and SUM dataset among them, though their times
/// are text in the Z form, and RETriage's RegBack copies missed what the app
/// gives the datasets they copy.
#[test]
fn browser_sum_mru_and_regback_relations_get_a_timeline() {
    let d = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::app_timed_more(&d.path().join("run"));
    let mut case = oxl_fixtures::new_case(d.path());
    oxl_engine::load_run(&mut case, &run, &analyst()).unwrap();
    for (rel, time) in [
        ("browsertriage_history", "Visit Time"),
        ("browsertriage_timeline", "Timestamp"),
        ("sumetriage_clients", "InsertDate"),
        ("retriage_recent_documents_ntuser_dat", "OpenedOn"),
        // Found on the third real run, a client: neither relation was on
        // the earlier captures.
        ("retriage_office_mru_ntuser_dat", "LastOpened"),
        ("retriage_first_folder_ntuser_dat", "OpenedOn"),
        ("retriage_taskcache_software_regback", "CreatedOn"),
    ] {
        assert_eq!(
            relation(&case, rel).unwrap().primary_time.as_deref(),
            Some(time),
            "{rel}"
        );
        assert_eq!(
            n(
                &case,
                &format!("select count(*) from {rel} where \"{time}__typed\" is not null")
            ),
            1,
            "{rel}.{time} did not cast"
        );
        let column = columns(&case, rel)
            .unwrap()
            .into_iter()
            .find(|c| c.name == time)
            .unwrap();
        assert_eq!(
            (column.sql_type, column.time_semantics, column.origin),
            (
                Some(SqlType::Timestamp),
                Some(TimeSemantics::Utc),
                Some(TypeOrigin::AppMap)
            ),
            "{rel}.{time}"
        );
    }
    // The other times of a relation are typed too, the primary one aside.
    for (rel, other) in [
        ("browsertriage_history", "Last Visit Time"),
        ("sumetriage_clients", "LastAccess"),
    ] {
        assert_eq!(
            n(
                &case,
                &format!("select count(*) from {rel} where \"{other}__typed\" is not null")
            ),
            1,
            "{rel}.{other}"
        );
    }
}

/// Reviewed: a case loaded before an entry existed chose its primary time
/// automatically (`PreviousRun0`); a later load moves an automatic choice
/// to the listed column, and leaves an analyst's choice as it is.
#[test]
fn a_later_load_moves_an_automatic_primary_time_to_the_listed_one() {
    for analyst_chose in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let run = oxl_fixtures::app_timed(&d.path().join("run"));
        let mut case = oxl_fixtures::new_case(d.path());
        oxl_engine::load_run(&mut case, &run, &analyst()).unwrap();
        if analyst_chose {
            set_primary_time(&mut case, "petriage_main", Some("PreviousRun0"), &analyst()).unwrap();
        } else {
            // What a build without the entry left: its own pick, not chosen.
            case.scalar_i64(
                "UPDATE ox.relation SET primary_time = 'PreviousRun0' \
                 WHERE name = 'petriage_main' RETURNING 1",
            )
            .unwrap();
        }
        // A later run of the same host.
        let later = d.path().join("later");
        oxl_fixtures_copy(&run, &later, "20260111000000-jjjj");
        oxl_engine::load_run(&mut case, &later, &analyst()).unwrap();
        let expected = if analyst_chose {
            "PreviousRun0"
        } else {
            "LastRun"
        };
        assert_eq!(
            relation(&case, "petriage_main")
                .unwrap()
                .primary_time
                .as_deref(),
            Some(expected),
            "analyst chose: {analyst_chose}"
        );
    }
}

/// A copy of the run at `from` under `to` with run id `run_id`, and a
/// generation of that run.
fn oxl_fixtures_copy(from: &std::path::Path, to: &std::path::Path, run_id: &str) {
    common::copy_tree(from, to);
    let path = to.join("duckdb").join("datasets.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    json["run_id"] = run_id.into();
    json["generation"] = format!("{run_id}-g1").into();
    std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
}

#[test]
fn every_summary_entry_names_a_distinct_relation_and_some_columns() {
    use oxl_engine::catalog::appmap::SUMMARY;
    let distinct: BTreeSet<&str> = SUMMARY.iter().map(|(relation, _)| *relation).collect();
    assert_eq!(distinct.len(), SUMMARY.len());
    for (relation, columns) in SUMMARY {
        assert!(!columns.is_empty(), "{relation}");
        let unique: BTreeSet<&&str> = columns.iter().collect();
        assert_eq!(unique.len(), columns.len(), "{relation}");
    }
}

#[test]
fn every_primary_time_entry_names_a_distinct_relation() {
    let distinct: BTreeSet<&str> = PRIMARY_TIME.iter().map(|(relation, _)| *relation).collect();
    assert_eq!(distinct.len(), PRIMARY_TIME.len(), "{PRIMARY_TIME:?}");
}

#[test]
fn an_analyst_can_declare_a_type_and_an_agent_cannot() {
    let (_d, mut case) = loaded();
    declare_type(
        &mut case,
        "ext_hayabusa_csv_timeline",
        "EventID",
        SqlType::BigInt,
        None,
        &analyst(),
    )
    .unwrap();
    assert_eq!(
        n(
            &case,
            "select count(*) from ext_hayabusa_csv_timeline where \"EventID__typed\" = 1102"
        ),
        1
    );
    let declared = columns(&case, "ext_hayabusa_csv_timeline")
        .unwrap()
        .into_iter()
        .find(|c| c.name == "EventID")
        .unwrap();
    assert_eq!(
        (
            declared.text_column.as_str(),
            declared.typed_column.as_deref(),
            declared.origin
        ),
        ("EventID", Some("EventID__typed"), Some(TypeOrigin::Analyst))
    );
    assert_eq!(
        declare_type(
            &mut case,
            "ext_hayabusa_csv_timeline",
            "Level",
            SqlType::BigInt,
            None,
            &agent()
        )
        .unwrap_err()
        .code(),
        "refused"
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'declare_type'"
        ),
        1
    );
}

#[test]
fn declaring_again_replaces_the_companion_in_place() {
    let (_d, mut case) = loaded();
    for sql_type in [SqlType::BigInt, SqlType::UBigInt] {
        declare_type(
            &mut case,
            "ext_hayabusa_csv_timeline",
            "EventID",
            sql_type,
            None,
            &analyst(),
        )
        .unwrap();
    }
    assert_eq!(
        n(
            &case,
            "select count(*) from ext_hayabusa_csv_timeline \
             where typeof(\"EventID__typed\") = 'UBIGINT'"
        ),
        5
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from (describe ext_hayabusa_csv_timeline) \
             where column_name like 'EventID%'"
        ),
        2
    );
}

#[test]
fn a_suite_typed_column_cannot_be_redeclared() {
    let (_d, mut case) = loaded();
    let before = columns(&case, "evtxtriage_events")
        .unwrap()
        .into_iter()
        .find(|c| c.name == "TimeCreated")
        .unwrap();
    let err = declare_type(
        &mut case,
        "evtxtriage_events",
        "TimeCreated",
        SqlType::BigInt,
        None,
        &analyst(),
    )
    .unwrap_err();
    assert_eq!(err.code(), "invalid", "{err}");
    let after = columns(&case, "evtxtriage_events")
        .unwrap()
        .into_iter()
        .find(|c| c.name == "TimeCreated")
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(
        (
            after.typed_column.as_deref(),
            after.text_column.as_str(),
            after.origin
        ),
        (
            Some("TimeCreated"),
            "TimeCreated__text",
            Some(TypeOrigin::Suite)
        )
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from (describe evtxtriage_events) \
             where lower(column_name) = 'timecreated__typed'"
        ),
        0
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'declare_type'"
        ),
        0
    );
}

#[test]
fn a_local_or_missing_timestamp_semantics_is_invalid() {
    let (_d, mut case) = loaded();
    for semantics in [Some(TimeSemantics::Local), None] {
        let err = declare_type(
            &mut case,
            "ext_hayabusa_csv_timeline",
            "Details",
            SqlType::Timestamp,
            semantics,
            &analyst(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "invalid", "{semantics:?}");
    }
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'declare_type'"
        ),
        0
    );
}

#[test]
fn set_primary_time_requires_a_catalogued_timestamp_and_is_recorded() {
    let (_d, mut case) = loaded();
    let err =
        set_primary_time(&mut case, "evtxtriage_events", Some("Channel"), &analyst()).unwrap_err();
    assert_eq!(err.code(), "invalid");
    set_primary_time(&mut case, "evtxtriage_events", None, &analyst()).unwrap();
    assert_eq!(
        relation(&case, "evtxtriage_events").unwrap().primary_time,
        None
    );
    set_primary_time(
        &mut case,
        "evtxtriage_events",
        Some("TimeCreated"),
        &analyst(),
    )
    .unwrap();
    assert_eq!(
        relation(&case, "evtxtriage_events")
            .unwrap()
            .primary_time
            .as_deref(),
        Some("TimeCreated")
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history \
             where action = 'set_primary_time' and target_kind = 'relation'"
        ),
        2
    );
}

#[test]
fn retyping_the_primary_time_as_a_non_timestamp_is_invalid() {
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let fw = oxl_fixtures::loose_fw_csv(d.path());
    load_file(&mut case, &fw, FileOptions::default(), &analyst()).unwrap();
    let err = declare_type(
        &mut case,
        "fw_csv",
        "time",
        SqlType::BigInt,
        None,
        &analyst(),
    )
    .unwrap_err();
    assert_eq!(err.code(), "invalid", "{err}");
    assert_eq!(
        columns(&case, "fw_csv")
            .unwrap()
            .into_iter()
            .find(|c| c.name == "time")
            .unwrap()
            .sql_type,
        Some(SqlType::Timestamp)
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'declare_type'"
        ),
        0
    );
}

#[test]
fn an_agent_cannot_set_a_primary_time() {
    let (_d, mut case) = loaded();
    for column in [None, Some("TimeCreated")] {
        let err = set_primary_time(&mut case, "evtxtriage_events", column, &agent()).unwrap_err();
        assert_eq!(err.code(), "refused", "{column:?}");
    }
    assert_eq!(
        relation(&case, "evtxtriage_events")
            .unwrap()
            .primary_time
            .as_deref(),
        Some("TimeCreated")
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.relation where primary_time_chosen"
        ),
        0
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where action = 'set_primary_time'"
        ),
        0
    );
}

#[test]
fn unknown_relation_or_column_is_not_found() {
    let (_d, mut case) = loaded();
    assert_eq!(relation(&case, "nope").unwrap_err().code(), "not_found");
    assert_eq!(columns(&case, "nope").unwrap_err().code(), "not_found");
    assert_eq!(
        declare_type(&mut case, "nope", "x", SqlType::BigInt, None, &analyst())
            .unwrap_err()
            .code(),
        "not_found"
    );
    assert_eq!(
        declare_type(
            &mut case,
            "ext_hayabusa_csv_timeline",
            "nope",
            SqlType::BigInt,
            None,
            &analyst()
        )
        .unwrap_err()
        .code(),
        "not_found"
    );
    assert_eq!(
        set_primary_time(&mut case, "evtxtriage_events", Some("nope"), &analyst())
            .unwrap_err()
            .code(),
        "not_found"
    );
}

#[test]
fn relations_report_rows_and_hosts() {
    let (_d, case) = loaded();
    let r = relation(&case, "petriage_timeline").unwrap();
    assert_eq!(
        (r.rows, r.hosts.clone(), r.kind),
        (
            5,
            vec!["SRV01".to_string(), "WS01".to_string()],
            RelationKind::Dataset
        )
    );
    assert_eq!(
        relations(&case)
            .unwrap()
            .iter()
            .map(|r| r.name.as_str())
            .collect::<Vec<_>>(),
        [
            "evtxtriage_events",
            "ext_hayabusa_csv_timeline",
            "petriage_timeline"
        ]
    );
}

#[test]
fn columns_carry_their_role() {
    let (d, mut case) = loaded();
    let c = columns(&case, "evtxtriage_events").unwrap();
    let role = |name: &str| c.iter().find(|c| c.name == name).unwrap().role;
    assert_eq!(
        [
            role("_triage_run_id"),
            role("_triage_host"),
            role("_triage_identity"),
            role("_triage_output_file"),
            role("EventId"),
        ],
        [
            ColumnRole::RunId,
            ColumnRole::Host,
            ColumnRole::Identity,
            ColumnRole::OutputFile,
            ColumnRole::Data,
        ]
    );

    let report = load_file(
        &mut case,
        &oxl_fixtures::loose_fw_csv(d.path()),
        FileOptions {
            host: Some("WS01".into()),
            ..Default::default()
        },
        &analyst(),
    )
    .unwrap();
    let fw = columns(&case, &report.relations[0].name).unwrap();
    let hosts: Vec<&str> = fw
        .iter()
        .filter(|c| c.role == ColumnRole::Host)
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(hosts, ["_triage_host"]);
    assert!(fw
        .iter()
        .all(|c| c.role == ColumnRole::Host || c.role == ColumnRole::Data));
}

#[test]
fn the_raw_view_comes_from_the_catalog() {
    let (_d, mut case) = loaded();
    declare_type(
        &mut case,
        "ext_hayabusa_csv_timeline",
        "EventID",
        SqlType::BigInt,
        None,
        &analyst(),
    )
    .unwrap();
    // The raw view shows text only: no companion, the original under its name.
    assert_eq!(
        n(
            &case,
            "select count(*) from (describe raw_ext_hayabusa_csv_timeline) \
             where column_name like '%\\_\\_typed' escape '\\'"
        ),
        0
    );
    assert_eq!(
        n(
            &case,
            "select count(*) from raw_ext_hayabusa_csv_timeline where \"EventID\" = '1102'"
        ),
        1
    );
}
