//! The column catalog: what the suite typed, what the app types for it,
//! what an analyst may declare, and which column is a relation's time.

mod common;

use common::{agent, analyst, loaded, n};
use oxl_engine::catalog::appmap::PRIMARY_TIME;
use oxl_engine::catalog::{
    columns, declare_type, relation, relations, set_primary_time, RelationKind, SqlType,
    TimeSemantics, TypeOrigin,
};
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
