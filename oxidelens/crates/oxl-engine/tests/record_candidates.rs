//! The account and IOC candidates a promoted row offers: one test per
//! entry of the column map, each on a relation loaded with the header its
//! tool really writes; values that do not validate are left out; a record
//! the case already has is named; and nothing is ever added.

mod common;

use common::{analyst, n};
use duckdb::types::Value as SqlValue;
use oxl_engine::{
    candidates, delete_record, load_run, upsert_account, upsert_ioc, AccountSpec, Candidate, Case,
    IocSpec, IocType, RecordKind,
};

/// A new case with `two_hosts_logons` and `candidate_sources_run` loaded.
/// Keep the directory alive for as long as the case.
fn loaded() -> (tempfile::TempDir, Case) {
    let d = tempfile::tempdir().unwrap();
    let logons = oxl_fixtures::two_hosts_logons(&d.path().join("logons"));
    let sources = oxl_fixtures::candidate_sources_run(&d.path().join("sources"));
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &logons, &analyst()).unwrap();
    load_run(&mut case, &sources, &analyst()).unwrap();
    (d, case)
}

/// The one row `select` selects, and its candidates.
fn of(case: &Case, select: &str) -> Vec<Candidate> {
    candidates(case, n(case, select)).unwrap()
}

fn account(name: &str, domain: Option<&str>, sid: Option<&str>, column: &str) -> Candidate {
    Candidate {
        kind: RecordKind::Account,
        ioc_type: None,
        value: name.to_string(),
        domain: domain.map(str::to_string),
        sid: sid.map(str::to_string),
        column: column.to_string(),
        existing_id: None,
    }
}

fn ioc(t: IocType, value: &str, column: &str) -> Candidate {
    Candidate {
        kind: RecordKind::Ioc,
        ioc_type: Some(t),
        value: value.to_string(),
        domain: None,
        sid: None,
        column: column.to_string(),
        existing_id: None,
    }
}

const SECURITY_LOG: &str = r"C:\Windows\System32\winevt\Logs\Security.evtx";
const JDOE_LOGON: &str = r"select _ox_row from evtxtriage_events
     where PayloadData1 = 'Target: ACME\jdoe' and MapDescription = 'Successful logon'";

/// The candidates of `rows` that come from `column`.
fn from(rows: &[Candidate], column: &str) -> Vec<Candidate> {
    rows.iter()
        .filter(|c| c.column == column)
        .cloned()
        .collect()
}

#[test]
fn an_event_user_name_is_an_account() {
    let (_d, case) = loaded();
    let rows = of(&case, JDOE_LOGON);
    assert_eq!(
        from(&rows, "UserName"),
        vec![account("jdoe", None, None, "UserName")]
    );
}

#[test]
fn an_event_logon_target_is_an_account_with_its_domain() {
    let (_d, case) = loaded();
    let rows = of(&case, JDOE_LOGON);
    assert_eq!(
        from(&rows, "PayloadData1"),
        vec![account("jdoe", Some("ACME"), None, "PayloadData1")]
    );
    // `Target: -` names no account (S6), and the row's blank UserName none.
    let rows = of(
        &case,
        "select _ox_row from evtxtriage_events where PayloadData1 = 'Target: -'",
    );
    assert!(from(&rows, "PayloadData1").is_empty(), "{rows:?}");
    assert!(from(&rows, "UserName").is_empty(), "{rows:?}");
    // A PayloadData1 that is not a logon target is not read as one.
    let rows = of(
        &case,
        "select _ox_row from evtxtriage_events where PayloadData1 = 'svch0st.exe'",
    );
    assert!(from(&rows, "PayloadData1").is_empty(), "{rows:?}");
}

#[test]
fn an_event_remote_host_is_an_ip_when_it_is_one() {
    let (_d, case) = loaded();
    let rows = of(&case, JDOE_LOGON);
    assert_eq!(
        from(&rows, "RemoteHost"),
        vec![ioc(IocType::Ip, "203.0.113.7", "RemoteHost")]
    );
    // SRV01's logon from `WS01`: a host name, not an address.
    let rows = of(
        &case,
        "select _ox_row from evtxtriage_events
         where Computer = 'SRV01' and RemoteHost = 'WS01' and TimeCreated is not null",
    );
    assert!(from(&rows, "RemoteHost").is_empty(), "{rows:?}");
}

#[test]
fn an_event_remote_host_offers_the_ip_in_its_parentheses() {
    let d = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::remote_hosts_run(&d.path().join("run"));
    let mut case = oxl_fixtures::new_case(d.path());
    load_run(&mut case, &run, &analyst()).unwrap();
    let remote = |i: u32| -> Vec<Candidate> {
        let row = case
            .scalar_i64_with(
                "select _ox_row from evtxtriage_events where PayloadData1 = ?",
                &[SqlValue::Text(format!(r"Target: ACME\svc{i}"))],
            )
            .unwrap();
        from(&candidates(&case, row).unwrap(), "RemoteHost")
    };
    // `NAME (IP)`, as EvtxECmd's logon maps write it: the address.
    assert_eq!(
        remote(1),
        vec![ioc(IocType::Ip, "203.0.113.7", "RemoteHost")]
    );
    assert_eq!(
        remote(2),
        vec![ioc(IocType::Ip, "2001:db8::5", "RemoteHost")]
    );
    // A bare address offers itself.
    assert_eq!(
        remote(3),
        vec![ioc(IocType::Ip, "198.51.100.9", "RemoteHost")]
    );
    // `- (-)`, a name with no address in its parentheses, and a bare name:
    // nothing.
    for i in [4, 5, 6] {
        assert!(remote(i).is_empty(), "r{i}: {:?}", remote(i));
    }
}

#[test]
fn an_amcache_file_entry_sha1_is_a_sha1() {
    let (_d, case) = loaded();
    for (select, sha1) in [
        (
            "select _ox_row from amcachetriage_unassociated_file_entries",
            "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3",
        ),
        (
            "select _ox_row from amcachetriage_associated_file_entries",
            "3c363836cf4e16666669a25da280a1865c2d2874",
        ),
    ] {
        let rows = of(&case, select);
        assert_eq!(rows, vec![ioc(IocType::Sha1, sha1, "SHA1")], "{select}");
    }
}

#[test]
fn a_prefetch_hash_is_a_sha1_only_at_40_digits() {
    let (_d, case) = loaded();
    let rows = of(
        &case,
        "select _ox_row from petriage_main where ExecutableName = 'SVCH0ST.EXE'",
    );
    assert_eq!(
        rows,
        vec![ioc(
            IocType::Sha1,
            "da39a3ee5e6b4b0d3255bfef95601890afd80709",
            "Hash"
        )]
    );
    // The prefetch header hash as PETriage writes it: eight digits.
    let rows = of(
        &case,
        "select _ox_row from petriage_main where ExecutableName = 'RCLONE.EXE'",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

#[test]
fn a_download_hash_is_a_sha256() {
    let (_d, case) = loaded();
    let rows = of(
        &case,
        "select _ox_row from browsertriage_downloads where State = 'Complete'",
    );
    assert_eq!(
        from(&rows, "Hash (SHA-256)"),
        vec![ioc(
            IocType::Sha256,
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "Hash (SHA-256)"
        )]
    );
}

#[test]
fn a_download_url_is_a_url() {
    let (_d, case) = loaded();
    let rows = of(
        &case,
        "select _ox_row from browsertriage_downloads where State = 'Complete'",
    );
    assert_eq!(
        from(&rows, "Download URL"),
        vec![ioc(
            IocType::Url,
            "https://files.example.com/rclone.zip",
            "Download URL"
        )]
    );
}

#[test]
fn a_history_url_is_a_url() {
    let (_d, case) = loaded();
    let rows = of(
        &case,
        "select _ox_row from browsertriage_history where Title = 'folder'",
    );
    assert_eq!(
        rows,
        vec![ioc(
            IocType::Url,
            "https://mega.example.org/folder/abc",
            "URL"
        )]
    );
}

#[test]
fn a_typed_url_is_a_url() {
    let (_d, case) = loaded();
    let rows = of(&case, "select _ox_row from retriage_typedurls_ntuser_dat");
    assert_eq!(
        rows,
        vec![ioc(IocType::Url, "http://198.51.100.9/stage/", "Url")]
    );
}

#[test]
fn a_srum_user_name_is_an_account() {
    let (_d, case) = loaded();
    let rows = of(
        &case,
        "select _ox_row from srumetriage_network_usage where UserName = 'jdoe'",
    );
    // Its SID text is not a SID: the account is offered without one.
    assert_eq!(rows, vec![account("jdoe", None, None, "UserName")]);
    // A SID no user name was resolved for names no account.
    let rows = of(
        &case,
        "select _ox_row from srumetriage_network_usage where Sid = 'S-1-5-18'",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

#[test]
fn a_srum_sid_is_the_accounts_sid() {
    let (_d, case) = loaded();
    let rows = of(
        &case,
        "select _ox_row from srumetriage_network_usage where UserName = 'svc_backup'",
    );
    assert_eq!(
        rows,
        vec![account(
            "svc_backup",
            None,
            Some("S-1-5-21-1004336348-1177238915-682003330-1001"),
            "UserName"
        )]
    );
}

#[test]
fn the_evidence_path_is_a_file_path() {
    let (_d, case) = loaded();
    let rows = of(&case, JDOE_LOGON);
    assert_eq!(
        from(&rows, "SourceFile"),
        vec![ioc(IocType::FilePath, SECURITY_LOG, "SourceFile")]
    );
    // The whole map on one row, in its order, the evidence path last.
    assert_eq!(
        rows,
        vec![
            account("jdoe", None, None, "UserName"),
            account("jdoe", Some("ACME"), None, "PayloadData1"),
            ioc(IocType::Ip, "203.0.113.7", "RemoteHost"),
            ioc(IocType::FilePath, SECURITY_LOG, "SourceFile"),
        ]
    );
    // A relation with no evidence path column offers none.
    let rows = of(&case, "select _ox_row from retriage_typedurls_ntuser_dat");
    assert!(
        rows.iter().all(|c| c.ioc_type != Some(IocType::FilePath)),
        "{rows:?}"
    );
}

#[test]
fn a_value_that_does_not_validate_is_not_offered() {
    let (_d, case) = loaded();
    // `javascript:void(0)` is no http, https or ftp URL; `not-a-hash` no
    // SHA-256: the row offers nothing at all.
    let rows = of(
        &case,
        "select _ox_row from browsertriage_downloads where State = 'Cancelled'",
    );
    assert!(rows.is_empty(), "{rows:?}");
    let rows = of(
        &case,
        "select _ox_row from browsertriage_history where Title = 'Settings'",
    );
    assert!(rows.is_empty(), "{rows:?}");
}

#[test]
fn an_existing_record_is_named() {
    let (_d, mut case) = loaded();
    let me = analyst();
    let acme_jdoe = upsert_account(
        &mut case,
        &AccountSpec {
            name: "JDOE".into(),
            domain: Some("acme".into()),
            ..Default::default()
        },
        &me,
    )
    .unwrap()
    .id;
    let ip = upsert_ioc(
        &mut case,
        &IocSpec {
            id: None,
            ioc_type: IocType::Ip,
            value: "203.0.113.7".into(),
            tool: None,
            attack_ids: Vec::new(),
            note: String::new(),
        },
        &me,
    )
    .unwrap()
    .id;
    let rows = of(&case, JDOE_LOGON);
    let named = |column: &str| from(&rows, column)[0].existing_id;
    // Name and domain match with ASCII case ignored; the domainless
    // `jdoe` is another account, which the case does not have.
    assert_eq!(named("PayloadData1"), Some(acme_jdoe));
    assert_eq!(named("UserName"), None);
    assert_eq!(named("RemoteHost"), Some(ip));
    assert_eq!(named("SourceFile"), None);

    // A deleted record is not named.
    delete_record(&mut case, RecordKind::Ioc, ip, &me).unwrap();
    let rows = of(&case, JDOE_LOGON);
    assert_eq!(from(&rows, "RemoteHost")[0].existing_id, None);
}

#[test]
fn an_account_without_a_domain_matches_any_domain() {
    let (_d, mut case) = loaded();
    let me = analyst();
    let spec = |name: &str, domain: Option<&str>| AccountSpec {
        name: name.into(),
        domain: domain.map(str::to_string),
        ..Default::default()
    };
    // No match: neither `ACME\jdoe` nor a domainless `jdoe` is recorded.
    upsert_account(&mut case, &spec("someone", None), &me).unwrap();
    let rows = of(&case, JDOE_LOGON);
    assert_eq!(from(&rows, "PayloadData1")[0].existing_id, None);
    assert_eq!(from(&rows, "UserName")[0].existing_id, None);

    // Only a domainless `Jdoe`: S6 matches it for `ACME\jdoe` too, ASCII
    // case ignored.
    let bare = upsert_account(&mut case, &spec("Jdoe", None), &me)
        .unwrap()
        .id;
    let rows = of(&case, JDOE_LOGON);
    assert_eq!(from(&rows, "PayloadData1")[0].existing_id, Some(bare));
    assert_eq!(from(&rows, "UserName")[0].existing_id, Some(bare));

    // Both: the exact name and domain comes first; the domainless
    // candidate still names the domainless record.
    let exact = upsert_account(&mut case, &spec("jdoe", Some("Acme")), &me)
        .unwrap()
        .id;
    let rows = of(&case, JDOE_LOGON);
    assert_eq!(from(&rows, "PayloadData1")[0].existing_id, Some(exact));
    assert_eq!(from(&rows, "UserName")[0].existing_id, Some(bare));

    // A record in another domain is not this account.
    delete_record(&mut case, RecordKind::Account, exact, &me).unwrap();
    delete_record(&mut case, RecordKind::Account, bare, &me).unwrap();
    upsert_account(&mut case, &spec("jdoe", Some("OTHER")), &me).unwrap();
    let rows = of(&case, JDOE_LOGON);
    assert_eq!(from(&rows, "PayloadData1")[0].existing_id, None);
    assert_eq!(from(&rows, "UserName")[0].existing_id, None);
}

#[test]
fn candidates_add_nothing() {
    let (_d, case) = loaded();
    let counts = |case: &Case| -> Vec<i64> {
        [
            "select count(*) from ox.account",
            "select count(*) from ox.ioc",
            "select count(*) from ox.host",
            "select count(*) from ox.entry",
            "select count(*) from ox.entry_account",
            "select count(*) from ox.entry_ioc",
            "select count(*) from ox.history",
        ]
        .iter()
        .map(|sql| n(case, sql))
        .collect()
    };
    let before = counts(&case);
    let row = n(&case, JDOE_LOGON);
    let (offered, statements) = case.statements_during(|| candidates(&case, row)).unwrap();
    assert_eq!(offered.unwrap().len(), 4);
    assert_eq!(counts(&case), before);
    let writes: Vec<&String> = statements
        .iter()
        .filter(|s| {
            let s = s.trim_start().to_ascii_uppercase();
            ["INSERT", "UPDATE", "DELETE", "CREATE", "ALTER", "DROP"]
                .iter()
                .any(|w| s.starts_with(w))
        })
        .collect();
    assert!(writes.is_empty(), "{writes:?}");
}

#[test]
fn an_unknown_row_is_not_found() {
    let (_d, case) = loaded();
    let e = candidates(&case, 9_999_999).unwrap_err();
    assert_eq!(e.code(), "not_found");
}

#[test]
fn below_format_3_candidates_name_no_record() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("old.case.duckdb");
    let logons = oxl_fixtures::two_hosts_logons(&d.path().join("logons"));
    {
        let mut case = Case::create_at_format_2(&path, "Old", &analyst()).unwrap();
        load_run(&mut case, &logons, &analyst()).unwrap();
    }
    let case = Case::open_read_only(&path).unwrap();
    assert!(case.format() < 3);
    let rows = of(&case, JDOE_LOGON);
    assert_eq!(rows.len(), 4, "{rows:?}");
    assert!(rows.iter().all(|c| c.existing_id.is_none()), "{rows:?}");
}
