//! The record sheets' computed views: an IOC's sightings, which are Find
//! everywhere (exact, case ignored) for its value (ruling S5), and an
//! account's logon summary, counted from the event-log dataset's Security
//! logons with each count carrying the filter that opens its rows (S6).

mod common;

use common::analyst;
use oxl_engine::{
    count, delete_record, find, ioc_sightings, load_file, load_run, logon_summary, upsert_account,
    upsert_ioc, AccountSpec, Case, EngineError, FileOptions, FindMode, FindRequest, GridQuery,
    IocSpec, IocType, LogonOutcome, RecordKind,
};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

const EVENTS: &str = "evtxtriage_events";

/// A new case with `two_hosts_logons` loaded. Keep the directory alive for
/// as long as the case.
fn loaded() -> (tempfile::TempDir, Case) {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts_logons(&d.path().join("run"));
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &root, &analyst()).unwrap();
    (d, case)
}

fn ioc(case: &mut Case, t: IocType, value: &str) -> i64 {
    upsert_ioc(
        case,
        &IocSpec {
            id: None,
            ioc_type: t,
            value: value.into(),
            tool: None,
            attack_ids: Vec::new(),
            note: String::new(),
        },
        &analyst(),
    )
    .unwrap()
    .id
}

fn account(case: &mut Case, name: &str, domain: Option<&str>) -> i64 {
    upsert_account(
        case,
        &AccountSpec {
            name: name.into(),
            domain: domain.map(str::to_string),
            ..Default::default()
        },
        &analyst(),
    )
    .unwrap()
    .id
}

fn not_cancelled() -> AtomicBool {
    AtomicBool::new(false)
}

/// `svch0st.exe` is a whole cell in WS01's Sysmon process creation
/// (`PayloadData1`) and, upper-cased, in WS01's prefetch
/// (`ExecutableName`). It is only part of `svch0st.exe:443` and of
/// Hayabusa's `Proc: svch0st.exe ¦ …`, which an exact find leaves out. So
/// the sightings span two datasets, ignore case, and are not a contains
/// search; each total is checked against `find` itself.
#[test]
fn sightings_are_find_exact_for_the_value() {
    let (_d, mut case) = loaded();
    let id = ioc(&mut case, IocType::FileName, "svch0st.exe");
    let s = ioc_sightings(&case, id, &not_cancelled()).unwrap();

    assert_eq!(s.hits, 2);
    assert_eq!(
        s.relations,
        [
            ("evtxtriage_events".to_string(), 1),
            ("petriage_timeline".to_string(), 1)
        ]
    );
    assert_eq!(s.hosts, ["WS01"]);
    // The Sysmon event is at 02:15:44.380, the prefetch run at 02:15:44.412.
    assert_eq!(s.first.as_deref(), Some("2026-01-05T02:15:44.380000Z"));
    assert_eq!(s.last.as_deref(), Some("2026-01-05T02:15:44.412000Z"));

    let found = find(
        &case,
        &FindRequest {
            value: "svch0st.exe".into(),
            mode: FindMode::Exact,
            hosts: Vec::new(),
            from: None,
            to: None,
        },
        &mut |_, _| true,
    )
    .unwrap();
    assert_eq!(s.hits, found.hits.iter().map(|h| h.count).sum::<u64>());
    let mut by_relation: BTreeMap<String, u64> = BTreeMap::new();
    for h in &found.hits {
        *by_relation.entry(h.relation.clone()).or_default() += h.count;
    }
    assert_eq!(s.relations, by_relation.into_iter().collect::<Vec<_>>());
    assert_eq!(
        s.first,
        found.hits.iter().filter_map(|h| h.first.clone()).min()
    );
    assert_eq!(
        s.last,
        found.hits.iter().filter_map(|h| h.last.clone()).max()
    );
}

/// The address is a whole `RemoteHost` cell on four WS01 events and on
/// SRV01's none; Hayabusa's `SrcIP: 203.0.113.7` is only part of a cell.
#[test]
fn sightings_name_the_hosts_they_come_from() {
    let (_d, mut case) = loaded();
    let id = ioc(&mut case, IocType::Ip, "203.0.113.7");
    let s = ioc_sightings(&case, id, &not_cancelled()).unwrap();
    assert_eq!(s.hits, 4);
    assert_eq!(s.relations, [("evtxtriage_events".to_string(), 4)]);
    assert_eq!(s.hosts, ["WS01"]);
    assert_eq!(s.first.as_deref(), Some("2026-01-05T02:13:58.204000Z"));
    assert_eq!(s.last.as_deref(), Some("2026-01-05T02:40:00.000000Z"));
}

#[test]
fn an_ioc_seen_nowhere_says_not_found() {
    let (_d, mut case) = loaded();
    let id = ioc(&mut case, IocType::Domain, "nowhere.example");
    let s = ioc_sightings(&case, id, &not_cancelled()).unwrap();
    assert_eq!(s.hits, 0);
    assert!(s.relations.is_empty());
    assert!(s.hosts.is_empty());
    assert_eq!((s.first, s.last), (None, None));
}

#[test]
fn sightings_stop_when_cancelled() {
    let (_d, mut case) = loaded();
    let id = ioc(&mut case, IocType::Ip, "203.0.113.7");
    let e = ioc_sightings(&case, id, &AtomicBool::new(true)).unwrap_err();
    assert!(
        matches!(&e, EngineError::Invalid(m) if m == "cancelled"),
        "{e:?}"
    );
}

/// Only a live IOC has sightings: a deleted one, whose value the sheet no
/// longer lists, is `NotFound` like an id the case never had.
#[test]
fn sightings_of_a_deleted_or_unknown_ioc_are_not_found() {
    let (_d, mut case) = loaded();
    let id = ioc(&mut case, IocType::Ip, "203.0.113.7");
    delete_record(&mut case, RecordKind::Ioc, id, &analyst()).unwrap();
    for id in [id, 999] {
        let e = ioc_sightings(&case, id, &not_cancelled()).unwrap_err();
        assert!(matches!(e, EngineError::NotFound(_)), "{id}: {e:?}");
    }
}

/// `(outcome, logon type, host, count)` of each row of the summary.
type Counted = (LogonOutcome, u32, Option<String>, u64);

fn counted(case: &Case, account_id: i64) -> (Vec<Counted>, u64) {
    let s = logon_summary(case, account_id).unwrap();
    assert_eq!(s.source.as_deref(), Some(EVENTS));
    let rows = s
        .rows
        .iter()
        .map(|r| (r.outcome, r.logon_type, r.host.clone(), r.count))
        .collect();
    (rows, s.unparsed)
}

/// `two_hosts_logons` adds WS01's 4624 (`LogonType 10`) and 4625
/// (`LogonType 3`) for `Target: ACME\jdoe`, and a 4624 for `Target: -`.
/// The unparsed rows are that one (ruling: `-` is no account) and the four
/// Security 4624/4625 rows `two_hosts` already has, written in the older
/// form with `LogonType N` in `PayloadData1` and no `Target: ` (WS01's
/// 4625 and 4624, SRV01's two 4624s): S6 counts every row that does not
/// parse, never only those that start the right way.
#[test]
fn logon_summary_counts_by_type_and_host() {
    let (_d, mut case) = loaded();
    let jdoe = account(&mut case, "jdoe", Some("ACME"));
    let (rows, unparsed) = counted(&case, jdoe);
    let ws01 = Some("WS01".to_string());
    assert_eq!(
        rows,
        [
            (LogonOutcome::Failure, 3, ws01.clone(), 1),
            (LogonOutcome::Success, 10, ws01, 1),
        ]
    );
    assert_eq!(unparsed, 5);
}

/// S6: name and domain both match, ASCII case ignored; a record without a
/// domain matches the name in any domain.
#[test]
fn accounts_match_by_name_and_domain_with_case_ignored() {
    let (_d, mut case) = loaded();
    let other_case = account(&mut case, "JDOE", Some("acme"));
    let domainless = account(&mut case, "jdoe", None);
    let elsewhere = account(&mut case, "jdoe", Some("OTHER"));
    let someone = account(&mut case, "asmith", Some("ACME"));
    for id in [other_case, domainless] {
        let (rows, _) = counted(&case, id);
        assert_eq!(rows.iter().map(|r| r.3).sum::<u64>(), 2, "account {id}");
    }
    for id in [elsewhere, someone] {
        let (rows, unparsed) = counted(&case, id);
        assert!(rows.is_empty(), "account {id}: {rows:?}");
        assert_eq!(unparsed, 5);
    }
}

/// `two_hosts_logons` with more Security logons, each a near miss for
/// one leaf of a count's filter: the same account and type as the 4624
/// type 10 but a 4625, on SRV01, in another channel, for another account,
/// with another type; one spelled in other case, which is the same account
/// and is counted with the first; and one whose `target: ` is in other
/// case, which S6 does not read, so it is unparsed and no count opens it.
fn near_misses() -> (tempfile::TempDir, Case) {
    let d = tempfile::tempdir().unwrap();
    let root = oxl_fixtures::two_hosts_logons(&d.path().join("run"));
    let log = r"C:\Windows\System32\winevt\Logs\Security.evtx";
    let row = |t: &str, id: u32, channel: &str, host: &str, account: &str, ty: u32| {
        format!(
            "2026-01-05T03:{t}.0000000Z,{id},{channel},{host},jdoe,,Logon,{account},{log},LogonType {ty}\n"
        )
    };
    let append = |host: &str, rows: &[String]| {
        let path = root
            .join(format!("Processed-{host}-20260101000000"))
            .join("EventLogs")
            .join("evtx.csv");
        let mut text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with('\n'));
        text.extend(rows.iter().map(String::as_str));
        std::fs::write(&path, text).unwrap();
    };
    append(
        "WS01",
        &[
            row("00:00", 4625, "Security", "WS01", r"Target: ACME\jdoe", 10),
            row(
                "01:00",
                4624,
                "Application",
                "WS01",
                r"Target: ACME\jdoe",
                10,
            ),
            row(
                "02:00",
                4624,
                "Security",
                "WS01",
                r"Target: ACME\asmith",
                10,
            ),
            row("03:00", 4624, "Security", "WS01", r"Target: ACME\jdoe", 2),
            row("04:00", 4624, "Security", "WS01", r"Target: acme\JDOE", 10),
            row("06:00", 4624, "Security", "WS01", r"target: ACME\jdoe", 10),
        ],
    );
    append(
        "SRV01",
        &[row(
            "05:00",
            4624,
            "Security",
            "SRV01",
            r"Target: ACME\jdoe",
            10,
        )],
    );
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &root, &analyst()).unwrap();
    (d, case)
}

/// Each count's filter, applied to the event-log dataset, selects exactly
/// the rows it counts: the outcome's event id, the channel, the logon type
/// and the host as well as the account, every spelling of it counted.
#[test]
fn each_logon_count_opens_its_rows() {
    let (_d, mut case) = near_misses();
    let jdoe = account(&mut case, "jdoe", Some("ACME"));
    let (rows, unparsed) = counted(&case, jdoe);
    let (ws01, srv01) = (Some("WS01".to_string()), Some("SRV01".to_string()));
    assert_eq!(
        rows,
        [
            (LogonOutcome::Success, 10, srv01, 1),
            (LogonOutcome::Success, 2, ws01.clone(), 1),
            (LogonOutcome::Failure, 3, ws01.clone(), 1),
            (LogonOutcome::Success, 10, ws01.clone(), 2),
            (LogonOutcome::Failure, 10, ws01, 1),
        ]
    );
    assert_eq!(unparsed, 6);
    let s = logon_summary(&case, jdoe).unwrap();
    for row in &s.rows {
        let opened = count(
            &case,
            &GridQuery {
                relation: EVENTS.into(),
                filter: Some(row.filter.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(opened, row.count, "{row:?}");
    }
}

#[test]
fn no_event_log_dataset_says_so() {
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let fw = oxl_fixtures::loose_fw_csv(d.path());
    load_file(&mut case, &fw, FileOptions::default(), &analyst()).unwrap();
    let jdoe = account(&mut case, "jdoe", Some("ACME"));
    let s = logon_summary(&case, jdoe).unwrap();
    assert_eq!(s.source, None);
    assert!(s.rows.is_empty());
    assert_eq!(s.unparsed, 0);
}

/// Only a live account has a summary, as only a live IOC has sightings.
#[test]
fn logon_summary_of_a_deleted_or_unknown_account_is_not_found() {
    let (_d, mut case) = loaded();
    let jdoe = account(&mut case, "jdoe", Some("ACME"));
    delete_record(&mut case, RecordKind::Account, jdoe, &analyst()).unwrap();
    for id in [jdoe, 999] {
        let e = logon_summary(&case, id).unwrap_err();
        assert!(matches!(e, EngineError::NotFound(_)), "{id}: {e:?}");
    }
}
