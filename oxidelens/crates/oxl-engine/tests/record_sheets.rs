//! Host, account and IOC records: validation, uniqueness, links to
//! timeline entries, delete, merge, status, the columns computed from the
//! linked entries, and who may do which (ruling S3).

mod common;

use common::{agent, analyst, loaded, n};
use oxl_engine::{
    account_record, accounts, attach_rows, delete_entry, delete_record, detach_row, edit_entry,
    guess_ioc_type, history, host_record, hosts, ioc_record, iocs, link, merge_records, new_entry,
    promote_rows, record_entries, set_record_status, unlink, upsert_account, upsert_host,
    upsert_ioc, validate_attack_id, validate_ioc, validate_sid, AccountSpec, Actor, Case,
    EntryEdit, HostSpec, IocQuery, IocSpec, IocType, Privilege, RecordKind, RecordStatus,
};
use std::cell::RefCell;
use triage_testkit::boundary::assert_total;

fn code<T>(result: oxl_engine::Result<T>) -> (&'static str, String) {
    match result {
        Ok(_) => ("ok", String::new()),
        Err(e) => (e.code(), e.to_string()),
    }
}

const FAILED_LOGON_WS01: &str =
    "select _ox_row from evtxtriage_events where MapDescription = 'Failed logon'";
const SHARE_SRV01: &str =
    "select _ox_row from evtxtriage_events where MapDescription = 'Network share accessed'";

/// Promote the one row `select` selects as `actor`; the new entry's id.
fn promote_at(case: &mut Case, select: &str, actor: &Actor) -> i64 {
    let row = n(case, select);
    let out = promote_rows(case, &[row], actor).unwrap();
    assert!(out[0].created, "{out:?}");
    out[0].entry_id.unwrap()
}

fn entry(case: &mut Case, title: &str, time: Option<&str>, actor: &Actor) -> i64 {
    let edit = EntryEdit {
        title: Some(title.to_string()),
        time: time.map(|t| Some(t.to_string())),
        ..Default::default()
    };
    new_entry(case, &edit, actor).unwrap().id
}

fn host_spec(name: &str) -> HostSpec {
    HostSpec {
        name: name.to_string(),
        ..Default::default()
    }
}

fn account_spec(name: &str, domain: Option<&str>) -> AccountSpec {
    AccountSpec {
        name: name.to_string(),
        domain: domain.map(str::to_string),
        ..Default::default()
    }
}

fn ioc_spec(ioc_type: IocType, value: &str) -> IocSpec {
    IocSpec {
        id: None,
        ioc_type,
        value: value.to_string(),
        tool: None,
        attack_ids: Vec::new(),
        note: String::new(),
    }
}

/// The hostile strings every validator here meets: blank, control
/// characters, bidi overrides, formulas, markup, non-ASCII digits, a
/// character that is several bytes at a cut, and lengths at and past 4,096.
fn hostile() -> Vec<String> {
    let mut out: Vec<String> = [
        "",
        " ",
        "\t",
        "\u{0}",
        "a\nb",
        "a\rb",
        "a\u{7f}",
        "a\u{85}b",
        "\u{202E}exe.txt",
        "=HYPERLINK(\"http://x\")",
        "<img src=x onerror=1>",
        "|`|",
        "１２７.０.０.１",
        "T\u{0661}\u{0660}\u{0665}\u{0669}",
        "S-1-\u{0665}-18",
        "S-1-5-",
        "S-1-",
        "S-1-5--18",
        "S-1-99999999999999999999999-1",
        "S-1-5-99999999999999999999999",
        "S-1-281474976710656-1",
        "S-1-5-4294967296",
        "s-1-5-18",
        "::",
        "[::1]",
        "http://",
        "http://[::1",
        "https://user@",
        "ftp://:21/",
        "@",
        "a@",
        "@b",
        "a@@b",
        "..",
        ".",
        "a.",
        "-",
        "é",
        "😀",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    out.push("x".repeat(4_096));
    out.push("x".repeat(4_097));
    out.push("é".repeat(4_096));
    out.push("é".repeat(4_097));
    out.push("a".repeat(63) + ".example");
    out.push("a".repeat(64) + ".example");
    out.push(["a"; 127].join("."));
    out.push(["a"; 128].join("."));
    out.push(format!("http://{}", "a.".repeat(3_000)));
    out
}

#[test]
fn ioc_values_are_validated_and_canonical() {
    use IocType::*;
    let ok = |t: IocType, raw: &str, stored: &str| {
        assert_eq!(
            validate_ioc(t, raw).unwrap_or_else(|e| panic!("{t:?} {raw:?}: {e}")),
            stored,
            "{t:?} {raw:?}"
        );
    };
    let bad = |t: IocType, raw: &str| {
        assert_eq!(code(validate_ioc(t, raw)).0, "invalid", "{t:?} {raw:?}");
    };
    // ip: IPv4 or IPv6, stored canonical.
    ok(Ip, "203.0.113.7", "203.0.113.7");
    ok(Ip, "::1", "::1");
    ok(Ip, "0:0:0:0:0:0:0:1", "::1");
    ok(Ip, "2001:DB8:0:0:0:0:0:1", "2001:db8::1");
    for raw in [
        "256.0.0.1",
        "203.0.113",
        "203.0.113.7 ",
        "[::1]",
        "acme.local",
        "",
    ] {
        bad(Ip, raw);
    }
    // domain and hostname: dot-joined labels of [A-Za-z0-9-]{1,63}, 1-253
    // characters, stored lower-case.
    ok(Domain, "Files.Example.COM", "files.example.com");
    ok(Hostname, "WS01", "ws01");
    ok(
        Domain,
        &("a".repeat(63) + ".example"),
        &("a".repeat(63) + ".example"),
    );
    let longest = ["a"; 127].join(".");
    assert_eq!(longest.len(), 253);
    ok(Hostname, &longest, &longest);
    for raw in [
        "acme_local",
        "a..b",
        ".acme",
        "acme.",
        "acme local",
        "ácme.local",
        &("a".repeat(64) + ".example"),
        &["a"; 128].join("."),
    ] {
        bad(Domain, raw);
        bad(Hostname, raw);
    }
    // url: http, https or ftp, and a host.
    ok(
        Url,
        "https://files.example.com/rclone.zip?x=1#y",
        "https://files.example.com/rclone.zip?x=1#y",
    );
    ok(Url, "HTTP://203.0.113.7:8080/", "HTTP://203.0.113.7:8080/");
    ok(
        Url,
        "ftp://user@[2001:db8::1]/x",
        "ftp://user@[2001:db8::1]/x",
    );
    for raw in [
        "files.example.com/x",
        "file:///C:/x",
        "javascript:alert(1)",
        "https://",
        "https:///x",
        "https://user@/x",
        "https://bad_host/x",
        "https://[::1/x",
        "https:files.example.com",
    ] {
        bad(Url, raw);
    }
    // email: local@domain, the domain valid.
    ok(Email, "jdoe@acme.local", "jdoe@acme.local");
    for raw in [
        "jdoe",
        "jdoe@",
        "@acme.local",
        "jdoe@acme_local",
        "j doe@acme.local",
    ] {
        bad(Email, raw);
    }
    // Hashes: hex of 32, 40 or 64, stored lower-case.
    let md5 = "D41D8CD98F00B204E9800998ECF8427E";
    let sha1 = "DA39A3EE5E6B4B0D3255BFEF95601890AFD80709";
    let sha256 = "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855";
    ok(Md5, md5, &md5.to_lowercase());
    ok(Sha1, sha1, &sha1.to_lowercase());
    ok(Sha256, sha256, &sha256.to_lowercase());
    bad(Md5, &md5[1..]);
    bad(Md5, sha1);
    bad(Sha1, md5);
    bad(Sha256, sha1);
    bad(Md5, &format!("{}g", &md5[1..]));
    // file_name: no separator.
    ok(FileName, "rclone.exe", "rclone.exe");
    bad(FileName, r"C:\Tools\rclone.exe");
    bad(FileName, "stage/rclone.exe");
    // The free types keep the value as written.
    for t in [FilePath, RegistryKey, Service, ScheduledTask, Other] {
        ok(t, r"C:\Tools\rclone.exe", r"C:\Tools\rclone.exe");
        ok(t, "Mixed Case\tTab", "Mixed Case\tTab");
    }
    // Every value: 1 to 4,096 characters, no control character but tab.
    ok(Other, &"é".repeat(4_096), &"é".repeat(4_096));
    for raw in ["", "   ", "a\nb", "a\u{0}b", "a\u{7f}", "a\u{9b}b"] {
        bad(Other, raw);
    }
    bad(Other, &"x".repeat(4_097));
    bad(FilePath, &"é".repeat(4_097));

    // The I key's guess: the first type the value validates as, in the
    // order sha256, sha1, md5, ip, url, email, domain.
    for (raw, guess) in [
        (sha256, Some(Sha256)),
        (sha1, Some(Sha1)),
        (md5, Some(Md5)),
        ("203.0.113.7", Some(Ip)),
        ("::1", Some(Ip)),
        ("https://files.example.com/x", Some(Url)),
        ("jdoe@acme.local", Some(Email)),
        ("files.example.com", Some(Domain)),
        ("WS01", Some(Domain)),
        (r"C:\Tools\rclone.exe", None),
        ("", None),
    ] {
        assert_eq!(guess_ioc_type(raw), guess, "{raw:?}");
    }

    // What upsert stores is the canonical form.
    let (_d, mut case) = loaded();
    let i = upsert_ioc(&mut case, &ioc_spec(Sha1, sha1), &analyst()).unwrap();
    assert_eq!(i.value, sha1.to_lowercase());
    assert_eq!(i.ioc_type, Sha1);
    let i = upsert_ioc(&mut case, &ioc_spec(Ip, "0:0:0:0:0:0:0:1"), &analyst()).unwrap();
    assert_eq!(i.value, "::1");
    assert_eq!(
        code(upsert_ioc(
            &mut case,
            &ioc_spec(Ip, "localhost"),
            &analyst()
        ))
        .0,
        "invalid"
    );
    assert_eq!(n(&case, "select count(*) from ox.ioc"), 2);
}

#[test]
fn ioc_validation_is_total() {
    let inputs = hostile();
    let inputs: Vec<&str> = inputs.iter().map(String::as_str).collect();
    for t in IocType::ALL {
        assert_total(&format!("record::validate_ioc({t:?})"), &inputs, |raw| {
            validate_ioc(t, raw)
        });
        // Whatever is accepted is within the rules every value keeps.
        for raw in &inputs {
            if let Ok(stored) = validate_ioc(t, raw) {
                let chars = stored.chars().count();
                assert!((1..=4_096).contains(&chars), "{t:?} {raw:?}");
                assert!(
                    !stored.chars().any(|c| c.is_control() && c != '\t'),
                    "{t:?} {raw:?}"
                );
            }
        }
    }
    assert_total("record::guess_ioc_type", &inputs, guess_ioc_type);
}

#[test]
fn sids_are_validated_and_canonical() {
    for (raw, stored) in [
        ("S-1-5-18", "S-1-5-18"),
        (
            "S-1-5-21-3623811015-3361044348-30300820-1013",
            "S-1-5-21-3623811015-3361044348-30300820-1013",
        ),
        ("s-1-5-18", "S-1-5-18"),
        ("S-1-05-018", "S-1-5-18"),
        (
            "S-1-281474976710655-4294967295",
            "S-1-281474976710655-4294967295",
        ),
    ] {
        assert_eq!(validate_sid(raw).unwrap(), stored, "{raw:?}");
    }
    for raw in [
        "",
        "S-1-5",
        "S-1",
        "S-2-5-18",
        "S-1-5-",
        "S-1--18",
        "S-1-5-18 ",
        " S-1-5-18",
        "S-1-281474976710656-1",
        "S-1-5-4294967296",
        "S-1-5-1-2-3-4-5-6-7-8-9-10-11-12-13-14-15-16",
        "S-1-\u{0665}-18",
        "S-1-+5-18",
        "S-1-5-18\u{0}",
    ] {
        assert_eq!(code(validate_sid(raw)).0, "invalid", "{raw:?}");
    }
    let inputs = hostile();
    let inputs: Vec<&str> = inputs.iter().map(String::as_str).collect();
    assert_total("record::validate_sid", &inputs, validate_sid);

    // An account's SID goes through it.
    let (_d, mut case) = loaded();
    let mut spec = account_spec("jdoe", Some("ACME"));
    spec.sid = Some("s-1-5-21-1-2-3-1013".into());
    let a = upsert_account(&mut case, &spec, &analyst()).unwrap();
    assert_eq!(a.sid.as_deref(), Some("S-1-5-21-1-2-3-1013"));
    spec.id = Some(a.id);
    spec.sid = Some("S-1-5".into());
    assert_eq!(
        code(upsert_account(&mut case, &spec, &analyst())).0,
        "invalid"
    );
}

#[test]
fn attack_ids_are_validated() {
    for good in ["T1059", "T1059.001"] {
        assert_eq!(validate_attack_id(good).unwrap(), good);
    }
    let inputs = hostile();
    let mut inputs: Vec<&str> = inputs.iter().map(String::as_str).collect();
    inputs.extend(["t1059", "T105", "T1059.01", "TA0001", "T1059 ", "T１０５９"]);
    assert_total("record::validate_attack_id", &inputs, validate_attack_id);
    for raw in &inputs {
        assert_eq!(code(validate_attack_id(raw)).0, "invalid", "{raw:?}");
    }

    // An IOC's ids go through it; a repeated id is kept once, and a refused
    // one writes nothing.
    let (_d, case) = loaded();
    let case = RefCell::new(case);
    let mut spec = ioc_spec(IocType::FileName, "rclone.exe");
    spec.attack_ids = vec!["T1567.002".into(), "T1048".into(), "T1567.002".into()];
    let i = upsert_ioc(&mut case.borrow_mut(), &spec, &analyst()).unwrap();
    assert_eq!(i.attack_ids, ["T1567.002", "T1048"]);
    spec.id = Some(i.id);
    let attempt = |raw: &str| {
        let mut spec = spec.clone();
        spec.attack_ids = vec!["T1048".into(), raw.to_string()];
        code(upsert_ioc(&mut case.borrow_mut(), &spec, &analyst())).0
    };
    assert_total("record::ioc attack_ids", &inputs, attempt);
    for raw in &inputs {
        assert_eq!(attempt(raw), "invalid", "{raw:?}");
    }
    assert_eq!(
        ioc_record(&case.borrow(), i.id).unwrap().attack_ids,
        ["T1567.002", "T1048"]
    );
    // A privilege is one of five.
    for raw in [
        "standard",
        "local_admin",
        "domain_admin",
        "service",
        "unknown",
    ] {
        assert_eq!(Privilege::parse(raw).unwrap().as_str(), raw);
    }
    assert_total("record::Privilege::parse", &inputs, Privilege::parse);
    assert_eq!(code(Privilege::parse("Domain_Admin")).0, "invalid");
}

#[test]
fn record_text_is_bounded() {
    let (_d, mut case) = loaded();
    for name in ["", "  ", "WS\n01", "WS\u{0}01", &"x".repeat(257)] {
        assert_eq!(
            code(upsert_host(&mut case, &host_spec(name), &analyst())).0,
            "invalid",
            "{name:?}"
        );
        assert_eq!(
            code(upsert_account(
                &mut case,
                &account_spec(name, None),
                &analyst()
            ))
            .0,
            "invalid",
            "{name:?}"
        );
    }
    let mut spec = host_spec("WS01");
    spec.note = "x".repeat(10_001);
    assert_eq!(code(upsert_host(&mut case, &spec, &analyst())).0, "invalid");
    spec.note = "two\nlines".into();
    spec.role = "x".repeat(201);
    assert_eq!(code(upsert_host(&mut case, &spec, &analyst())).0, "invalid");
    let mut spec = account_spec("jdoe", Some("ACME\nX"));
    assert_eq!(
        code(upsert_account(&mut case, &spec, &analyst())).0,
        "invalid"
    );
    spec.domain = Some(String::new());
    // An empty domain is no domain.
    let a = upsert_account(&mut case, &spec, &analyst()).unwrap();
    assert_eq!(a.domain, None);
    let mut spec = ioc_spec(IocType::FileName, "rclone.exe");
    spec.tool = Some("x".repeat(201));
    assert_eq!(code(upsert_ioc(&mut case, &spec, &analyst())).0, "invalid");
    spec.tool = Some(String::new());
    assert_eq!(upsert_ioc(&mut case, &spec, &analyst()).unwrap().tool, None);
    assert_eq!(n(&case, "select count(*) from ox.host"), 0);
    // Hostile text that passes the rules is kept as written.
    let mut spec = host_spec("=HYPERLINK(\"http://x\")");
    spec.note = "<img src=x onerror=1> \u{202E} | `x`".into();
    let h = upsert_host(&mut case, &spec, &analyst()).unwrap();
    assert_eq!(h.name, spec.name);
    assert_eq!(h.note, spec.note);
}

#[test]
fn duplicate_names_are_refused_ignoring_case() {
    let (_d, mut case) = loaded();
    let ws01 = upsert_host(&mut case, &host_spec("WS01"), &analyst()).unwrap();
    let (c, why) = code(upsert_host(&mut case, &host_spec("ws01"), &analyst()));
    assert_eq!(c, "refused", "{why}");
    assert!(why.contains(&format!("H-{}", ws01.id)), "{why}");
    let other = upsert_host(&mut case, &host_spec("SRV01"), &analyst()).unwrap();
    let mut rename = host_spec("Ws01");
    rename.id = Some(other.id);
    assert_eq!(
        code(upsert_host(&mut case, &rename, &analyst())).0,
        "refused"
    );
    // A record may change its own name's case.
    rename.id = Some(ws01.id);
    assert_eq!(
        upsert_host(&mut case, &rename, &analyst()).unwrap().name,
        "Ws01"
    );
    // Only ASCII case is ignored.
    upsert_host(&mut case, &host_spec("HÉ01"), &analyst()).unwrap();
    upsert_host(&mut case, &host_spec("hé01"), &analyst()).unwrap();
    // Attaching a WS01 row links the record, whatever its case.
    let e = promote_at(&mut case, FAILED_LOGON_WS01, &analyst());
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.host where lower(name) = 'ws01'"
        ),
        1
    );
    assert_eq!(
        record_entries(&case, RecordKind::Host, ws01.id, 10, 0)
            .unwrap()
            .items
            .iter()
            .map(|e| e.id)
            .collect::<Vec<_>>(),
        [e]
    );

    // Accounts: name and domain together.
    let jdoe = upsert_account(&mut case, &account_spec("jdoe", Some("ACME")), &analyst()).unwrap();
    let (c, why) = code(upsert_account(
        &mut case,
        &account_spec("JDOE", Some("acme")),
        &analyst(),
    ));
    assert_eq!(c, "refused", "{why}");
    assert!(why.contains(&format!("A-{}", jdoe.id)), "{why}");
    upsert_account(&mut case, &account_spec("jdoe", None), &analyst()).unwrap();
    assert_eq!(
        code(upsert_account(
            &mut case,
            &account_spec("JDoe", None),
            &analyst()
        ))
        .0,
        "refused"
    );
    upsert_account(&mut case, &account_spec("jdoe", Some("ACME2")), &analyst()).unwrap();

    // IOCs: type and value, after validation.
    let evil = upsert_ioc(
        &mut case,
        &ioc_spec(IocType::Domain, "evil.example"),
        &analyst(),
    )
    .unwrap();
    let (c, why) = code(upsert_ioc(
        &mut case,
        &ioc_spec(IocType::Domain, "EVIL.Example"),
        &analyst(),
    ));
    assert_eq!(c, "refused", "{why}");
    assert!(why.contains(&format!("I-{}", evil.id)), "{why}");
    upsert_ioc(
        &mut case,
        &ioc_spec(IocType::Hostname, "evil.example"),
        &analyst(),
    )
    .unwrap();
    upsert_ioc(&mut case, &ioc_spec(IocType::Ip, "::1"), &analyst()).unwrap();
    assert_eq!(
        code(upsert_ioc(
            &mut case,
            &ioc_spec(IocType::Ip, "0:0::1"),
            &analyst()
        ))
        .0,
        "refused"
    );
    upsert_ioc(&mut case, &ioc_spec(IocType::FilePath, r"C:\x"), &analyst()).unwrap();
    assert_eq!(
        code(upsert_ioc(
            &mut case,
            &ioc_spec(IocType::FilePath, r"c:\X"),
            &analyst()
        ))
        .0,
        "refused"
    );

    // Unique among live records: a deleted name is free again.
    delete_record(&mut case, RecordKind::Ioc, evil.id, &analyst()).unwrap();
    upsert_ioc(
        &mut case,
        &ioc_spec(IocType::Domain, "EVIL.example"),
        &analyst(),
    )
    .unwrap();
}

#[test]
fn computed_seen_comes_from_entries() {
    let (_d, mut case) = loaded();
    let ws = promote_at(&mut case, FAILED_LOGON_WS01, &analyst());
    let srv = promote_at(&mut case, SHARE_SRV01, &analyst());
    let undated = entry(&mut case, "Undated", None, &analyst());
    let gone = entry(
        &mut case,
        "Deleted",
        Some("2020-01-01T00:00:00Z"),
        &analyst(),
    );
    let elsewhere = upsert_host(&mut case, &host_spec("SRV99"), &analyst()).unwrap();
    link(&mut case, gone, RecordKind::Host, elsewhere.id, &analyst()).unwrap();
    let jdoe = upsert_account(&mut case, &account_spec("jdoe", Some("ACME")), &analyst()).unwrap();
    let ioc = upsert_ioc(
        &mut case,
        &ioc_spec(IocType::FileName, "rclone.exe"),
        &analyst(),
    )
    .unwrap();
    for e in [ws, srv, undated, gone] {
        link(&mut case, e, RecordKind::Account, jdoe.id, &analyst()).unwrap();
        link(&mut case, e, RecordKind::Ioc, ioc.id, &analyst()).unwrap();
    }
    let a = account_record(&case, jdoe.id).unwrap();
    assert_eq!(a.entries, 4);
    assert_eq!(a.first_seen.as_deref(), Some("2020-01-01T00:00:00.000000Z"));
    delete_entry(&mut case, gone, &analyst()).unwrap();

    let a = account_record(&case, jdoe.id).unwrap();
    assert_eq!(a.entries, 3, "the deleted entry is not counted");
    assert_eq!(a.first_seen.as_deref(), Some("2026-01-05T02:13:58.204000Z"));
    assert_eq!(a.last_seen.as_deref(), Some("2026-01-05T02:22:04.551000Z"));
    assert_eq!(a.hosts_seen, ["SRV01", "WS01"]);
    let i = ioc_record(&case, ioc.id).unwrap();
    assert_eq!(
        (i.entries, i.first_seen, i.last_seen, i.hosts_seen),
        (a.entries, a.first_seen, a.last_seen, a.hosts_seen)
    );
    let ws01 = hosts(&case, 10, 0)
        .unwrap()
        .items
        .into_iter()
        .find(|h| h.name == "WS01")
        .unwrap();
    assert_eq!(ws01.entries, 1);
    assert_eq!(
        ws01.first_seen.as_deref(),
        Some("2026-01-05T02:13:58.204000Z")
    );
    assert_eq!(ws01.first_seen, ws01.last_seen);
    let srv99 = host_record(&case, elsewhere.id).unwrap();
    assert_eq!((srv99.entries, srv99.first_seen), (0, None));

    // Only undated entries: counted, and nothing seen.
    let solo = upsert_account(&mut case, &account_spec("solo", None), &analyst()).unwrap();
    link(&mut case, undated, RecordKind::Account, solo.id, &analyst()).unwrap();
    let s = account_record(&case, solo.id).unwrap();
    assert_eq!(
        (s.entries, s.first_seen, s.last_seen, s.hosts_seen.len()),
        (1, None, None, 0)
    );
    // An entry's time moving moves the record's.
    edit_entry(
        &mut case,
        undated,
        &EntryEdit {
            time: Some(Some("2026-01-06T00:00:00Z".into())),
            ..Default::default()
        },
        &analyst(),
    )
    .unwrap();
    assert_eq!(
        account_record(&case, jdoe.id).unwrap().last_seen.as_deref(),
        Some("2026-01-06T00:00:00.000000Z")
    );
    // The record's entries, in the Timeline's order.
    assert_eq!(
        record_entries(&case, RecordKind::Account, jdoe.id, 10, 0)
            .unwrap()
            .items
            .iter()
            .map(|e| e.id)
            .collect::<Vec<_>>(),
        [ws, srv, undated]
    );
    let page = record_entries(&case, RecordKind::Account, jdoe.id, 2, 0).unwrap();
    assert_eq!((page.items.len(), page.truncated), (2, true));
    assert_eq!(
        code(record_entries(&case, RecordKind::Account, 999_999, 10, 0)).0,
        "not_found"
    );
}

#[test]
fn listings_page_and_filter() {
    let (_d, mut case) = loaded();
    for name in ["SRV01", "WS02", "ws01"] {
        upsert_host(&mut case, &host_spec(name), &analyst()).unwrap();
    }
    let gone = upsert_host(&mut case, &host_spec("OLD"), &analyst()).unwrap();
    delete_record(&mut case, RecordKind::Host, gone.id, &analyst()).unwrap();
    let names = |case: &Case, limit, offset| -> (Vec<String>, bool) {
        let l = hosts(case, limit, offset).unwrap();
        (l.items.into_iter().map(|h| h.name).collect(), l.truncated)
    };
    assert_eq!(
        names(&case, 10, 0),
        (vec!["SRV01".into(), "ws01".into(), "WS02".into()], false)
    );
    assert_eq!(names(&case, 1, 1), (vec!["ws01".into()], true));
    assert_eq!(code(hosts(&case, 0, 0)).0, "invalid");

    upsert_account(&mut case, &account_spec("svc_backup", None), &analyst()).unwrap();
    upsert_account(
        &mut case,
        &account_spec("Administrator", Some("ACME")),
        &analyst(),
    )
    .unwrap();
    assert_eq!(
        accounts(&case, 10, 0)
            .unwrap()
            .items
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["Administrator", "svc_backup"]
    );

    let mut rclone = ioc_spec(IocType::FileName, "rclone.exe");
    rclone.tool = Some("Rclone".into());
    upsert_ioc(&mut case, &rclone, &analyst()).unwrap();
    let mut url = ioc_spec(IocType::Url, "https://files.example.com/rclone.zip");
    url.tool = Some("rclone".into());
    upsert_ioc(&mut case, &url, &analyst()).unwrap();
    let mut ip = ioc_spec(IocType::Ip, "203.0.113.7");
    ip.note = "staging server".into();
    upsert_ioc(&mut case, &ip, &analyst()).unwrap();
    let values = |q: &IocQuery| -> Vec<String> {
        iocs(&case, q, 10, 0)
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.value)
            .collect()
    };
    assert_eq!(values(&IocQuery::default()).len(), 3);
    assert_eq!(
        values(&IocQuery {
            ioc_type: Some(IocType::Ip),
            ..Default::default()
        }),
        ["203.0.113.7"]
    );
    assert_eq!(
        values(&IocQuery {
            tool: Some("RCLONE".into()),
            ..Default::default()
        }),
        ["rclone.exe", "https://files.example.com/rclone.zip"]
    );
    assert_eq!(
        values(&IocQuery {
            text: Some("STAGING".into()),
            ..Default::default()
        }),
        ["203.0.113.7"]
    );
    assert_eq!(
        values(&IocQuery {
            text: Some("ZIP".into()),
            ..Default::default()
        }),
        ["https://files.example.com/rclone.zip"]
    );
    assert_eq!(
        code(iocs(
            &case,
            &IocQuery {
                text: Some("x".repeat(1_001)),
                ..Default::default()
            },
            10,
            0
        ))
        .0,
        "invalid"
    );
}

#[test]
fn a_case_below_format_3_lists_no_records() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("old.case.duckdb");
    drop(Case::create_at_format_2(&path, "Old", &analyst()).unwrap());
    let case = Case::open_read_only(&path).unwrap();
    assert!(case.format() < 3);
    assert!(hosts(&case, 10, 0).unwrap().items.is_empty());
    assert!(accounts(&case, 10, 0).unwrap().items.is_empty());
    assert!(iocs(&case, &IocQuery::default(), 10, 0)
        .unwrap()
        .items
        .is_empty());
    assert_eq!(code(host_record(&case, 1)).0, "not_found");
    assert!(record_entries(&case, RecordKind::Ioc, 1, 10, 0)
        .unwrap()
        .items
        .is_empty());
}

#[test]
fn merge_moves_links_and_tombstones() {
    let (_d, mut case) = loaded();
    let e1 = entry(&mut case, "one", Some("2026-01-05T01:00:00Z"), &analyst());
    let e2 = entry(&mut case, "two", Some("2026-01-05T02:00:00Z"), &analyst());
    let from = upsert_account(&mut case, &account_spec("jdoe", Some("ACME")), &analyst()).unwrap();
    let into = upsert_account(
        &mut case,
        &account_spec("john.doe", Some("ACME")),
        &analyst(),
    )
    .unwrap();
    link(&mut case, e1, RecordKind::Account, from.id, &analyst()).unwrap();
    link(&mut case, e2, RecordKind::Account, from.id, &analyst()).unwrap();
    link(&mut case, e2, RecordKind::Account, into.id, &analyst()).unwrap();

    merge_records(&mut case, RecordKind::Account, from.id, into.id, &analyst()).unwrap();
    // Each entry linked to `into` once; nothing linked to `from`.
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.entry_account where record_id = {}",
                into.id
            )
        ),
        2
    );
    assert_eq!(
        n(
            &case,
            &format!(
                "select count(*) from ox.entry_account where record_id = {}",
                from.id
            )
        ),
        0
    );
    assert_eq!(account_record(&case, into.id).unwrap().entries, 2);
    let gone = account_record(&case, from.id).unwrap();
    assert!(gone.deleted);
    assert_eq!(gone.entries, 0);
    assert_eq!(
        accounts(&case, 10, 0)
            .unwrap()
            .items
            .iter()
            .map(|a| a.id)
            .collect::<Vec<_>>(),
        [into.id]
    );
    let merge = history(&case, Some("account"), None, 1, 0)
        .unwrap()
        .items
        .remove(0);
    assert_eq!(merge.action, "account_merge");
    assert_eq!(merge.target_id, from.id.to_string());
    assert_eq!(merge.after.unwrap()["merged_into"], into.id);
    let mut links: Vec<(String, String)> = history(&case, Some("link"), None, 100, 0)
        .unwrap()
        .items
        .into_iter()
        .take(3)
        .map(|h| (h.action, h.target_id))
        .collect();
    links.sort();
    // e2's link to `from` went (it already had `into`); e1's moved.
    assert_eq!(
        links,
        [
            ("link_add".into(), format!("entry_account:{e1}:{}", into.id)),
            (
                "link_remove".into(),
                format!("entry_account:{e1}:{}", from.id)
            ),
            (
                "link_remove".into(),
                format!("entry_account:{e2}:{}", from.id)
            ),
        ]
    );
    // Three links made, then three link rows from the merge, no more.
    assert_eq!(
        n(
            &case,
            "select count(*) from ox.history where target_kind = 'link'"
        ),
        6
    );

    // Hosts merge the same way.
    let alias = upsert_host(&mut case, &host_spec("ws01.acme.local"), &analyst()).unwrap();
    link(&mut case, e1, RecordKind::Host, alias.id, &analyst()).unwrap();
    let ws01 = upsert_host(&mut case, &host_spec("WS01"), &analyst()).unwrap();
    merge_records(&mut case, RecordKind::Host, alias.id, ws01.id, &analyst()).unwrap();
    assert_eq!(host_record(&case, ws01.id).unwrap().entries, 1);

    // What a merge refuses.
    let other = upsert_account(&mut case, &account_spec("x", None), &analyst()).unwrap();
    for (from, into, want) in [
        (into.id, into.id, "invalid"),
        (from.id, into.id, "refused"),
        (into.id, from.id, "refused"),
        (999_999, into.id, "not_found"),
        (other.id, 999_999, "not_found"),
    ] {
        assert_eq!(
            code(merge_records(
                &mut case,
                RecordKind::Account,
                from,
                into,
                &analyst()
            ))
            .0,
            want,
            "{from} into {into}"
        );
    }
}

#[test]
fn links_are_checked() {
    let (_d, mut case) = loaded();
    let e = entry(&mut case, "lead", None, &analyst());
    let ioc = upsert_ioc(
        &mut case,
        &ioc_spec(IocType::Md5, &"a".repeat(32)),
        &analyst(),
    )
    .unwrap();
    link(&mut case, e, RecordKind::Ioc, ioc.id, &analyst()).unwrap();
    let before = n(&case, "select count(*) from ox.history");
    // Linking again changes nothing and writes nothing.
    link(&mut case, e, RecordKind::Ioc, ioc.id, &analyst()).unwrap();
    assert_eq!(n(&case, "select count(*) from ox.history"), before);
    assert_eq!(n(&case, "select count(*) from ox.entry_ioc"), 1);
    assert_eq!(
        code(link(&mut case, e, RecordKind::Ioc, 999_999, &analyst())).0,
        "not_found"
    );
    assert_eq!(
        code(link(
            &mut case,
            999_999,
            RecordKind::Ioc,
            ioc.id,
            &analyst()
        ))
        .0,
        "not_found"
    );
    assert_eq!(
        code(unlink(&mut case, e, RecordKind::Account, 1, &analyst())).0,
        "not_found"
    );
    unlink(&mut case, e, RecordKind::Ioc, ioc.id, &analyst()).unwrap();
    assert_eq!(n(&case, "select count(*) from ox.entry_ioc"), 0);
    assert_eq!(
        code(unlink(&mut case, e, RecordKind::Ioc, ioc.id, &analyst())).0,
        "not_found"
    );
    // A deleted record or entry takes no link.
    delete_record(&mut case, RecordKind::Ioc, ioc.id, &analyst()).unwrap();
    assert_eq!(
        code(link(&mut case, e, RecordKind::Ioc, ioc.id, &analyst())).0,
        "refused"
    );
    assert_eq!(
        code(delete_record(
            &mut case,
            RecordKind::Ioc,
            ioc.id,
            &analyst()
        ))
        .0,
        "refused"
    );
    let h = upsert_host(&mut case, &host_spec("WS01"), &analyst()).unwrap();
    delete_entry(&mut case, e, &analyst()).unwrap();
    assert_eq!(
        code(link(&mut case, e, RecordKind::Host, h.id, &analyst())).0,
        "refused"
    );
}

#[test]
fn unlinking_a_host_with_evidence_from_it_is_refused() {
    let (_d, mut case) = loaded();
    let e = promote_at(&mut case, FAILED_LOGON_WS01, &analyst());
    let ws01 = hosts(&case, 10, 0).unwrap().items.remove(0);
    assert_eq!(ws01.name, "WS01");
    let (c, why) = code(unlink(&mut case, e, RecordKind::Host, ws01.id, &analyst()));
    assert_eq!(c, "refused", "{why}");
    assert!(
        why.contains("WS01") && why.contains(&format!("E-{e}")),
        "{why}"
    );
    assert_eq!(n(&case, "select count(*) from ox.entry_host"), 1);
    // Another host linked by hand unlinks freely.
    let srv = upsert_host(&mut case, &host_spec("SRV01"), &analyst()).unwrap();
    link(&mut case, e, RecordKind::Host, srv.id, &analyst()).unwrap();
    // Evidence from SRV01 on the entry holds that link too.
    let share = n(&case, SHARE_SRV01);
    attach_rows(&mut case, e, &[share], &analyst()).unwrap();
    assert_eq!(
        code(unlink(&mut case, e, RecordKind::Host, srv.id, &analyst())).0,
        "refused"
    );
    detach_row(&mut case, e, share, &analyst()).unwrap();
    unlink(&mut case, e, RecordKind::Host, srv.id, &analyst()).unwrap();
    // Once the WS01 row is detached, WS01 unlinks too.
    let failed = n(&case, FAILED_LOGON_WS01);
    detach_row(&mut case, e, failed, &analyst()).unwrap();
    unlink(&mut case, e, RecordKind::Host, ws01.id, &analyst()).unwrap();
    assert_eq!(n(&case, "select count(*) from ox.entry_host"), 0);
}

#[test]
fn agent_records_are_unreviewed_and_labelled() {
    let (_d, mut case) = loaded();
    let h = upsert_host(&mut case, &host_spec("WS01"), &agent()).unwrap();
    let a = upsert_account(&mut case, &account_spec("jdoe", None), &agent()).unwrap();
    let i = upsert_ioc(&mut case, &ioc_spec(IocType::Ip, "203.0.113.7"), &agent()).unwrap();
    for (status, added, by) in [
        (h.status, h.agent_added, &h.created_by),
        (a.status, a.agent_added, &a.created_by),
        (i.status, i.agent_added, &i.created_by),
    ] {
        assert_eq!(
            (status, added, by.as_str()),
            (RecordStatus::Unreviewed, true, "agent:claude")
        );
    }
    let mine = upsert_host(&mut case, &host_spec("SRV01"), &analyst()).unwrap();
    assert_eq!(
        (mine.status, mine.agent_added),
        (RecordStatus::Reviewed, false)
    );

    // The agent edits and deletes its own unreviewed records.
    let mut edit = host_spec("WS01");
    edit.id = Some(h.id);
    edit.role = "workstation".into();
    assert_eq!(
        upsert_host(&mut case, &edit, &agent()).unwrap().role,
        "workstation"
    );
    delete_record(&mut case, RecordKind::Ioc, i.id, &agent()).unwrap();
    // It links them from its own entry.
    let lead = entry(&mut case, "agent lead", None, &agent());
    link(&mut case, lead, RecordKind::Account, a.id, &agent()).unwrap();
    unlink(&mut case, lead, RecordKind::Account, a.id, &agent()).unwrap();
    // Only an analyst reviews, and the label survives it.
    let (c, why) = code(set_record_status(
        &mut case,
        RecordKind::Host,
        h.id,
        RecordStatus::Reviewed,
        &agent(),
    ));
    assert_eq!(c, "refused", "{why}");
    assert!(why.contains("analyst"), "{why}");
    set_record_status(
        &mut case,
        RecordKind::Host,
        h.id,
        RecordStatus::Reviewed,
        &analyst(),
    )
    .unwrap();
    let h = host_record(&case, h.id).unwrap();
    assert_eq!((h.status, h.agent_added), (RecordStatus::Reviewed, true));
    assert_eq!(h.created_by, "agent:claude");
    // Reviewed, it is no longer the agent's to change.
    assert_eq!(code(upsert_host(&mut case, &edit, &agent())).0, "refused");
    assert_eq!(
        code(delete_record(&mut case, RecordKind::Host, h.id, &agent())).0,
        "refused"
    );
}

#[test]
fn agent_changes_to_analyst_records_are_refused() {
    let (_d, mut case) = loaded();
    let theirs = entry(&mut case, "analyst lead", None, &analyst());
    let mine = entry(&mut case, "agent lead", None, &agent());
    let h = upsert_host(&mut case, &host_spec("WS01"), &analyst()).unwrap();
    let a = upsert_account(&mut case, &account_spec("jdoe", None), &analyst()).unwrap();
    let i = upsert_ioc(&mut case, &ioc_spec(IocType::Ip, "203.0.113.7"), &analyst()).unwrap();
    let ah = upsert_host(&mut case, &host_spec("ws01-alias"), &agent()).unwrap();
    let aa = upsert_account(&mut case, &account_spec("jdoe2", None), &agent()).unwrap();
    let ai = upsert_ioc(&mut case, &ioc_spec(IocType::Ip, "203.0.113.8"), &agent()).unwrap();
    link(&mut case, theirs, RecordKind::Ioc, i.id, &analyst()).unwrap();
    let before = n(&case, "select count(*) from ox.history");

    let mut host_edit = host_spec("WS01");
    host_edit.id = Some(h.id);
    host_edit.note = "agent's".into();
    let mut account_edit = account_spec("jdoe", None);
    account_edit.id = Some(a.id);
    account_edit.privilege = Privilege::DomainAdmin;
    let mut ioc_edit = ioc_spec(IocType::Ip, "203.0.113.7");
    ioc_edit.id = Some(i.id);
    ioc_edit.tool = Some("x".into());
    let kinds = [
        (RecordKind::Host, h.id, ah.id),
        (RecordKind::Account, a.id, aa.id),
        (RecordKind::Ioc, i.id, ai.id),
    ];
    let mut outcomes = vec![
        (
            "edit host",
            code(upsert_host(&mut case, &host_edit, &agent())),
        ),
        (
            "edit account",
            code(upsert_account(&mut case, &account_edit, &agent())),
        ),
        ("edit ioc", code(upsert_ioc(&mut case, &ioc_edit, &agent()))),
    ];
    for (kind, theirs_id, mine_id) in kinds {
        outcomes.push((
            "delete",
            code(delete_record(&mut case, kind, theirs_id, &agent())),
        ));
        outcomes.push((
            "merge mine into theirs",
            code(merge_records(&mut case, kind, mine_id, theirs_id, &agent())),
        ));
        outcomes.push((
            "merge theirs into mine",
            code(merge_records(&mut case, kind, theirs_id, mine_id, &agent())),
        ));
        outcomes.push((
            "status",
            code(set_record_status(
                &mut case,
                kind,
                theirs_id,
                RecordStatus::Unreviewed,
                &agent(),
            )),
        ));
        outcomes.push((
            "link from an analyst's entry",
            code(link(&mut case, theirs, kind, mine_id, &agent())),
        ));
    }
    outcomes.push((
        "unlink from an analyst's entry",
        code(unlink(&mut case, theirs, RecordKind::Ioc, i.id, &agent())),
    ));
    for (what, (c, why)) in &outcomes {
        assert_eq!(*c, "refused", "{what}: {why}");
        assert!(why.contains("analyst:tester"), "{what}: {why}");
        assert!(why.contains("Review tab"), "{what}: {why}");
    }
    // Nothing a refusal touched was written.
    assert_eq!(n(&case, "select count(*) from ox.history"), before);
    assert_eq!(host_record(&case, h.id).unwrap().note, "");
    // From its own entry, the agent links an analyst's record.
    link(&mut case, mine, RecordKind::Host, h.id, &agent()).unwrap();
}

#[test]
fn every_change_writes_history() {
    let (_d, mut case) = loaded();
    let actions = |case: &Case, kind: &str| -> Vec<String> {
        let mut out: Vec<String> = history(case, Some(kind), None, 100, 0)
            .unwrap()
            .items
            .into_iter()
            .map(|h| h.action)
            .collect();
        out.reverse();
        out
    };
    let e = entry(&mut case, "lead", None, &analyst());
    let h = upsert_host(&mut case, &host_spec("WS01"), &analyst()).unwrap();
    let mut edit = host_spec("WS01");
    edit.id = Some(h.id);
    edit.role = "workstation".into();
    upsert_host(&mut case, &edit, &analyst()).unwrap();
    // An edit that changes nothing writes nothing.
    upsert_host(&mut case, &edit, &analyst()).unwrap();
    set_record_status(
        &mut case,
        RecordKind::Host,
        h.id,
        RecordStatus::Unreviewed,
        &analyst(),
    )
    .unwrap();
    set_record_status(
        &mut case,
        RecordKind::Host,
        h.id,
        RecordStatus::Unreviewed,
        &analyst(),
    )
    .unwrap();
    link(&mut case, e, RecordKind::Host, h.id, &analyst()).unwrap();
    unlink(&mut case, e, RecordKind::Host, h.id, &analyst()).unwrap();
    let other = upsert_host(&mut case, &host_spec("SRV01"), &analyst()).unwrap();
    merge_records(&mut case, RecordKind::Host, other.id, h.id, &analyst()).unwrap();
    delete_record(&mut case, RecordKind::Host, h.id, &analyst()).unwrap();
    assert_eq!(
        actions(&case, "host"),
        [
            "host_create",
            "host_edit",
            "host_status",
            "host_create",
            "host_merge",
            "host_delete"
        ]
    );
    assert_eq!(actions(&case, "link"), ["link_add", "link_remove"]);

    let a = upsert_account(&mut case, &account_spec("jdoe", None), &analyst()).unwrap();
    let mut a_edit = account_spec("jdoe", Some("ACME"));
    a_edit.id = Some(a.id);
    upsert_account(&mut case, &a_edit, &analyst()).unwrap();
    let i = upsert_ioc(&mut case, &ioc_spec(IocType::Ip, "::1"), &analyst()).unwrap();
    link(&mut case, e, RecordKind::Ioc, i.id, &analyst()).unwrap();
    delete_record(&mut case, RecordKind::Ioc, i.id, &analyst()).unwrap();
    // A tombstone: the record is kept and flagged, its link rows gone.
    assert!(ioc_record(&case, i.id).unwrap().deleted);
    assert_eq!(n(&case, "select count(*) from ox.entry_ioc"), 0);
    assert_eq!(
        actions(&case, "account"),
        ["account_create", "account_edit"]
    );
    assert_eq!(actions(&case, "ioc"), ["ioc_create", "ioc_delete"]);
    // Deleting a record drops its links, each with its row.
    assert_eq!(
        actions(&case, "link"),
        ["link_add", "link_remove", "link_add", "link_remove"]
    );

    // Before and after are the record as it was and became.
    let edited = history(&case, Some("account"), None, 1, 0)
        .unwrap()
        .items
        .remove(0);
    assert_eq!(edited.target_id, a.id.to_string());
    assert_eq!(edited.by, "analyst:tester");
    assert_eq!(edited.before.unwrap()["domain"], serde_json::Value::Null);
    assert_eq!(edited.after.unwrap()["domain"], "ACME");
    let status = history(&case, Some("host"), None, 100, 0)
        .unwrap()
        .items
        .into_iter()
        .find(|h| h.action == "host_status")
        .unwrap();
    assert_eq!(status.before.unwrap()["status"], "reviewed");
    assert_eq!(status.after.unwrap()["status"], "unreviewed");
    let link_row = history(&case, Some("link"), None, 1, 0)
        .unwrap()
        .items
        .remove(0);
    assert_eq!(link_row.target_id, format!("entry_ioc:{e}:{}", i.id));
    assert!(link_row.before.is_some() && link_row.after.is_none());

    // A refused change writes nothing.
    let before = n(&case, "select count(*) from ox.history");
    let _ = upsert_host(&mut case, &host_spec(""), &analyst());
    let _ = upsert_account(&mut case, &account_spec("jdoe", Some("acme")), &analyst());
    let _ = delete_record(&mut case, RecordKind::Ioc, i.id, &analyst());
    let _ = set_record_status(
        &mut case,
        RecordKind::Account,
        a.id,
        RecordStatus::Reviewed,
        &agent(),
    );
    assert_eq!(n(&case, "select count(*) from ox.history"), before);
}

#[test]
fn renaming_a_host_with_evidence_from_it_is_refused() {
    let (_d, mut case) = loaded();
    let e = promote_at(&mut case, FAILED_LOGON_WS01, &analyst());
    let ws01 = hosts(&case, 10, 0).unwrap().items.remove(0);
    let mut rename = host_spec("SRV01");
    rename.id = Some(ws01.id);
    let before = n(&case, "select count(*) from ox.history");
    let (c, why) = code(upsert_host(&mut case, &rename, &analyst()));
    assert_eq!(c, "refused", "{why}");
    assert!(
        why.contains(&format!("E-{e}")) && why.contains("WS01"),
        "{why}"
    );
    assert_eq!(n(&case, "select count(*) from ox.history"), before);
    assert_eq!(host_record(&case, ws01.id).unwrap().name, "WS01");
    // A change of ASCII case keeps the name the evidence gives.
    rename.name = "ws01".into();
    assert_eq!(
        upsert_host(&mut case, &rename, &analyst()).unwrap().name,
        "ws01"
    );
    // Other fields change freely.
    rename.role = "workstation".into();
    upsert_host(&mut case, &rename, &analyst()).unwrap();
    // Evidence on a deleted entry holds nothing; nor does a detached row.
    let failed = n(&case, FAILED_LOGON_WS01);
    detach_row(&mut case, e, failed, &analyst()).unwrap();
    rename.name = "WS01-renamed".into();
    upsert_host(&mut case, &rename, &analyst()).unwrap();
    let again = promote_at(&mut case, FAILED_LOGON_WS01, &analyst());
    let fresh = hosts(&case, 10, 0)
        .unwrap()
        .items
        .into_iter()
        .find(|h| h.name == "WS01")
        .unwrap();
    delete_entry(&mut case, again, &analyst()).unwrap();
    let mut rename = host_spec("SRV02");
    rename.id = Some(fresh.id);
    upsert_host(&mut case, &rename, &analyst()).unwrap();
}

#[test]
fn merging_a_host_with_evidence_into_another_name_is_refused() {
    let (_d, mut case) = loaded();
    let e = promote_at(&mut case, FAILED_LOGON_WS01, &analyst());
    let ws01 = hosts(&case, 10, 0).unwrap().items.remove(0);
    let srv = upsert_host(&mut case, &host_spec("SRV01"), &analyst()).unwrap();
    let before = n(&case, "select count(*) from ox.history");
    let (c, why) = code(merge_records(
        &mut case,
        RecordKind::Host,
        ws01.id,
        srv.id,
        &analyst(),
    ));
    assert_eq!(c, "refused", "{why}");
    assert!(
        why.contains(&format!("E-{e}")) && why.contains("WS01"),
        "{why}"
    );
    assert_eq!(n(&case, "select count(*) from ox.history"), before);
    assert!(!host_record(&case, ws01.id).unwrap().deleted);
    // The other way round moves no evidence-backed link: SRV01 has none.
    let lead = entry(&mut case, "lead", None, &analyst());
    link(&mut case, lead, RecordKind::Host, srv.id, &analyst()).unwrap();
    merge_records(&mut case, RecordKind::Host, srv.id, ws01.id, &analyst()).unwrap();
    assert_eq!(host_record(&case, ws01.id).unwrap().entries, 2);
    // Once the evidence is detached, WS01 merges into another name.
    let failed = n(&case, FAILED_LOGON_WS01);
    detach_row(&mut case, e, failed, &analyst()).unwrap();
    let other = upsert_host(&mut case, &host_spec("SRV03"), &analyst()).unwrap();
    merge_records(&mut case, RecordKind::Host, ws01.id, other.id, &analyst()).unwrap();
}
