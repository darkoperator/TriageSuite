//! The case-record commands: entries, evidence, host, account and IOC
//! records, links, candidates, sightings, logons, exports and the history of
//! one target. Each command's JSON shape is pinned here; the engine's rules
//! have their own tests, so these check that the CLI carries them.

mod common;

use common::{inner, ox};
use serde_json::{json, Value};

const ANALYST: &str = "analyst:ana";
const AGENT: &str = "agent:x";

/// A case holding the logons run: WS01 and SRV01, with the WS01 Security
/// logons for `ACME\jdoe`.
fn case() -> (tempfile::TempDir, String) {
    let d = tempfile::tempdir().unwrap();
    let p = d
        .path()
        .join("t.case.duckdb")
        .to_string_lossy()
        .into_owned();
    let run = oxl_fixtures::two_hosts_logons(&d.path().join("run"));
    assert_eq!(ox(&["case", "new", &p, "--name", "T"]).0, 0);
    assert_eq!(
        ox(&["--case", &p, "load", "run", &run.to_string_lossy()]).0,
        0
    );
    (d, p)
}

/// Run `args` against `p` as `actor`.
fn run(p: &str, actor: &str, args: &[&str]) -> (i32, Value) {
    let mut full = vec!["--case", p, "--as", actor];
    full.extend_from_slice(args);
    ox(&full)
}

/// `run` as the analyst, which must succeed.
fn ok(p: &str, args: &[&str]) -> Value {
    let (code, v) = run(p, ANALYST, args);
    assert_eq!(code, 0, "{args:?} -> {v}");
    v
}

/// The id of the one row of `relation` that `search` finds.
fn row(p: &str, relation: &str, search: &str) -> i64 {
    let (c, v) = ox(&["--case", p, "rows", relation, "--search", search]);
    assert_eq!(c, 0, "{v}");
    let ids = v["data"]["row_ids"].as_array().unwrap();
    assert!(!ids.is_empty(), "no row for {search}: {v}");
    ids[0].as_i64().unwrap()
}

fn svch0st(p: &str) -> i64 {
    row(p, "evtxtriage_events", "svch0st.exe:443")
}

fn keys(v: &Value) -> Vec<&str> {
    let mut k: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    k.sort_unstable();
    k
}

#[test]
fn entry_commands_shape() {
    let (_d, p) = case();
    let v = ok(
        &p,
        &[
            "entry",
            "add",
            "--title",
            "Logon from 203.0.113.7",
            "--description",
            "svc_backup on WS01",
            "--time",
            "2026-01-05T02:14:07Z",
            "--tactic",
            "Lateral Movement",
            "--technique",
            "T1021.001,T1078",
        ],
    );
    let d = &v["data"];
    assert_eq!(
        keys(d),
        [
            "agent_added",
            "created_at",
            "created_by",
            "evidence_rows",
            "id",
            "modified_at",
            "modified_by",
            "needs_time",
            "reviewed_by",
            "shown",
            "status",
            "tactic",
            "technique_ids",
            "time",
            "time_source",
            "untrusted"
        ]
    );
    assert_eq!(
        (d["id"].as_i64(), d["shown"].as_str(), d["status"].as_str()),
        (Some(1), Some("E-1"), Some("draft"))
    );
    assert_eq!(d["time_source"], "typed");
    assert_eq!(d["tactic"], "Lateral Movement");
    assert_eq!(d["technique_ids"], json!(["T1021.001", "T1078"]));
    assert_eq!(d["created_by"], ANALYST);
    let text = inner(&d["untrusted"]);
    assert_eq!(text["id"], 1);
    assert_eq!(text["title"], "Logon from 203.0.113.7");
    assert_eq!(text["description"], "svc_backup on WS01");
    assert_eq!(text["hosts"], json!([]));

    // edit: only the fields given change; an empty time clears it.
    let v = ok(
        &p,
        &["entry", "edit", "E-1", "--title", "Renamed", "--time", ""],
    );
    assert_eq!(v["data"]["time"], Value::Null);
    assert_eq!(v["data"]["needs_time"], true);
    let text = inner(&v["data"]["untrusted"]);
    assert_eq!(text["title"], "Renamed");
    assert_eq!(text["description"], "svc_backup on WS01");
    assert_eq!(v["data"]["tactic"], "Lateral Movement");
    let v = ok(&p, &["entry", "edit", "1", "--clear-techniques"]);
    assert_eq!(v["data"]["technique_ids"], json!([]));

    // list: counts and ids outside, text inside, joined by id.
    let v = ok(&p, &["entry", "list"]);
    assert_eq!(keys(&v["data"]), ["count", "entries", "untrusted"]);
    assert_eq!(
        (v["data"]["count"].as_u64(), &v["truncated"]),
        (Some(1), &json!(false))
    );
    assert_eq!(v["data"]["entries"][0]["shown"], "E-1");
    assert!(v["data"]["entries"][0].get("title").is_none());
    let text = inner(&v["data"]["untrusted"]);
    assert_eq!(text["entries"][0]["id"], 1);
    assert_eq!(text["entries"][0]["title"], "Renamed");

    // a filter narrows.
    let v = ok(&p, &["entry", "list", "--status", "reviewed"]);
    assert_eq!(v["data"]["count"], 0);
    let v = ok(&p, &["entry", "list", "--text", "renamed"]);
    assert_eq!(v["data"]["count"], 1);

    // status: reviewed needs a time.
    let (c, v) = run(&p, ANALYST, &["entry", "status", "1", "reviewed"]);
    assert_eq!(
        (c, v["error"]["code"].as_str()),
        (4, Some("refused")),
        "{v}"
    );
    ok(
        &p,
        &["entry", "edit", "1", "--time", "2026-01-05T02:14:07Z"],
    );
    let v = ok(&p, &["entry", "status", "1", "reviewed"]);
    assert_eq!(v["data"]["status"], "reviewed");
    assert_eq!(v["data"]["reviewed_by"], ANALYST);

    // show: the entry, with an empty evidence list.
    let v = ok(&p, &["entry", "show", "E-1"]);
    assert_eq!(v["data"]["evidence"], json!([]));
    assert_eq!(inner(&v["data"]["untrusted"])["evidence"], json!([]));

    // delete, then it is gone.
    let v = ok(&p, &["entry", "delete", "1"]);
    assert_eq!(
        (v["data"]["id"].as_i64(), v["data"]["shown"].as_str()),
        (Some(1), Some("E-1"))
    );
    assert_eq!(ok(&p, &["entry", "list"])["data"]["count"], 0);
    let (c, v) = run(&p, ANALYST, &["entry", "show", "1"]);
    assert_eq!(
        (c, v["error"]["code"].as_str()),
        (3, Some("not_found")),
        "{v}"
    );
}

#[test]
fn entry_list_pages_and_says_when_truncated() {
    let (_d, p) = case();
    for t in ["a", "b", "c"] {
        ok(&p, &["entry", "add", "--title", t]);
    }
    let v = ok(&p, &["entry", "list", "--limit", "2"]);
    assert_eq!(
        (v["data"]["count"].as_u64(), &v["truncated"]),
        (Some(2), &json!(true))
    );
    let v = ok(&p, &["entry", "list", "--limit", "2", "--offset", "2"]);
    assert_eq!(
        (v["data"]["count"].as_u64(), &v["truncated"]),
        (Some(1), &json!(false))
    );
    let (c, _) = run(&p, ANALYST, &["entry", "list", "--limit", "0"]);
    assert_eq!(c, 2);
}

#[test]
fn promote_attach_detach_and_evidence() {
    let (_d, p) = case();
    let r = svch0st(&p);
    let v = ok(
        &p,
        &["entry", "promote", "--rows", &format!("{r},999999999")],
    );
    let d = &v["data"];
    assert_eq!(
        (
            d["done"].as_u64(),
            d["created"].as_u64(),
            d["not_done"].as_u64()
        ),
        (Some(1), Some(1), Some(1)),
        "{v}"
    );
    let made = d["outcomes"][0]["entry_id"].as_i64().unwrap();
    assert_eq!(d["outcomes"][0]["created"], true);
    assert_eq!(d["outcomes"][1]["entry_id"], Value::Null);

    // The entry took the row's time and the WS01 host record.
    let v = ok(&p, &["entry", "show", &made.to_string()]);
    assert_eq!(v["data"]["time_source"], "row");
    assert_eq!(v["data"]["evidence_rows"], 1);
    assert_eq!(v["data"]["evidence"][0]["row_id"], r);
    assert_eq!(v["data"]["evidence"][0]["standing"], "unchanged");
    assert!(v["data"]["evidence"][0].get("summary").is_none());
    let text = inner(&v["data"]["untrusted"]);
    assert_eq!(text["hosts"], json!(["WS01"]));
    assert_eq!(text["evidence"][0]["row_id"], r);
    assert_eq!(text["evidence"][0]["relation"], "evtxtriage_events");
    assert_eq!(text["evidence"][0]["host"], "WS01");
    assert!(text["evidence"][0]["summary"]
        .as_str()
        .unwrap()
        .contains("Network connection"));

    // attach a second row; a row twice is done and unchanged.
    let r2 = row(&p, "evtxtriage_events", "rclone.exe");
    let v = ok(
        &p,
        &[
            "entry",
            "attach",
            &made.to_string(),
            "--rows",
            &format!("{r2},{r2}"),
        ],
    );
    assert_eq!(
        (v["data"]["done"].as_u64(), v["data"]["not_done"].as_u64()),
        (Some(1), Some(0))
    );

    // evidence list: pages, with its own truncated.
    let id = made.to_string();
    let v = ok(&p, &["evidence", "list", "--entry", &id]);
    assert_eq!(keys(&v["data"]), ["count", "evidence", "untrusted"]);
    assert_eq!(v["data"]["count"], 2);
    assert_eq!(v["truncated"], false);
    let v = ok(&p, &["evidence", "list", "--entry", &id, "--limit", "1"]);
    assert_eq!(
        (v["data"]["count"].as_u64(), &v["truncated"]),
        (Some(1), &json!(true))
    );
    let (c, _) = run(&p, ANALYST, &["evidence", "list", "--entry", "999"]);
    assert_eq!(c, 3);

    // The host record the evidence made cannot be unlinked while it holds evidence.
    let hosts = ok(&p, &["host", "list"]);
    assert_eq!(
        inner(&hosts["data"]["untrusted"])["hosts"][0]["name"],
        "WS01"
    );
    let h = hosts["data"]["hosts"][0]["id"]
        .as_i64()
        .unwrap()
        .to_string();
    let (c, v) = run(
        &p,
        ANALYST,
        &[
            "link", "remove", "--entry", &id, "--kind", "host", "--id", &h,
        ],
    );
    assert_eq!(
        (c, v["error"]["code"].as_str()),
        (4, Some("refused")),
        "{v}"
    );

    // detach both rows.
    for r in [r, r2] {
        let v = ok(&p, &["entry", "detach", &id, "--row", &r.to_string()]);
        assert_eq!(
            (v["data"]["row_id"].as_i64(), v["data"]["entry"].as_i64()),
            (Some(r), Some(made))
        );
    }
    assert_eq!(
        ok(&p, &["evidence", "list", "--entry", &id])["data"]["count"],
        0
    );
}

#[test]
fn host_account_ioc_commands_shape() {
    let (_d, p) = case();

    // host
    let v = ok(
        &p,
        &[
            "host",
            "add",
            "--name",
            "WS01",
            "--role",
            "Workstation",
            "--note",
            "n",
        ],
    );
    let d = &v["data"];
    assert_eq!(
        keys(d),
        [
            "agent_added",
            "created_at",
            "created_by",
            "deleted",
            "entries",
            "first_seen",
            "id",
            "last_seen",
            "modified_at",
            "modified_by",
            "shown",
            "status",
            "untrusted"
        ]
    );
    assert_eq!(
        (d["shown"].as_str(), d["status"].as_str()),
        (Some("H-1"), Some("reviewed"))
    );
    assert_eq!(
        inner(&d["untrusted"]),
        json!({"id": 1, "name": "WS01", "role": "Workstation", "note": "n"})
    );
    let v = ok(&p, &["host", "edit", "H-1", "--role", "Server"]);
    assert_eq!(inner(&v["data"]["untrusted"])["role"], "Server");
    assert_eq!(inner(&v["data"]["untrusted"])["name"], "WS01");
    let v = ok(&p, &["host", "show", "1"]);
    assert_eq!(v["data"]["linked_entries"], json!([]));
    ok(&p, &["host", "add", "--name", "SRV01"]);
    ok(&p, &["host", "merge", "--from", "2", "--into", "1"]);
    assert_eq!(ok(&p, &["host", "list"])["data"]["count"], 1);
    let v = ok(&p, &["host", "status", "1", "unreviewed"]);
    assert_eq!(v["data"]["status"], "unreviewed");
    let v = ok(&p, &["host", "delete", "1"]);
    assert_eq!(v["data"]["shown"], "H-1");
    assert_eq!(ok(&p, &["host", "list"])["data"]["count"], 0);

    // account
    let v = ok(
        &p,
        &[
            "account",
            "add",
            "--name",
            "jdoe",
            "--domain",
            "ACME",
            "--privilege",
            "local_admin",
            "--description",
            "d",
        ],
    );
    let d = &v["data"];
    assert_eq!(
        (d["shown"].as_str(), d["privilege"].as_str()),
        (Some("A-1"), Some("local_admin"))
    );
    let text = inner(&d["untrusted"]);
    assert_eq!(
        (
            text["name"].as_str(),
            text["domain"].as_str(),
            text["description"].as_str()
        ),
        (Some("jdoe"), Some("ACME"), Some("d"))
    );
    let v = ok(
        &p,
        &[
            "account",
            "edit",
            "A-1",
            "--domain",
            "",
            "--privilege",
            "standard",
        ],
    );
    assert_eq!(inner(&v["data"]["untrusted"])["domain"], Value::Null);
    assert_eq!(v["data"]["privilege"], "standard");
    let (c, v) = run(
        &p,
        ANALYST,
        &["account", "add", "--name", "x", "--privilege", "root"],
    );
    assert_eq!(
        (c, v["error"]["code"].as_str()),
        (2, Some("invalid")),
        "{v}"
    );
    ok(&p, &["account", "add", "--name", "other"]);
    ok(&p, &["account", "merge", "--from", "2", "--into", "1"]);
    assert_eq!(ok(&p, &["account", "list"])["data"]["count"], 1);
    ok(&p, &["account", "status", "1", "unreviewed"]);
    ok(&p, &["account", "show", "1"]);
    ok(&p, &["account", "delete", "1"]);

    // ioc
    let v = ok(
        &p,
        &[
            "ioc",
            "add",
            "--type",
            "domain",
            "--value",
            "Evil.ACME.local",
            "--tool",
            "rclone",
            "--attack-id",
            "T1567",
            "--note",
            "n",
        ],
    );
    let d = &v["data"];
    assert_eq!(
        keys(d),
        [
            "agent_added",
            "attack_ids",
            "created_at",
            "created_by",
            "deleted",
            "entries",
            "first_seen",
            "id",
            "ioc_type",
            "last_seen",
            "modified_at",
            "modified_by",
            "shown",
            "status",
            "untrusted"
        ]
    );
    assert_eq!(
        (d["shown"].as_str(), d["ioc_type"].as_str()),
        (Some("I-1"), Some("domain"))
    );
    assert_eq!(d["attack_ids"], json!(["T1567"]));
    // Stored in canonical form (lower case).
    assert_eq!(inner(&d["untrusted"])["value"], "evil.acme.local");
    let v = ok(
        &p,
        &["ioc", "edit", "I-1", "--clear-attack-ids", "--tool", ""],
    );
    assert_eq!(v["data"]["attack_ids"], json!([]));
    assert_eq!(inner(&v["data"]["untrusted"])["tool"], Value::Null);
    let (c, v) = run(
        &p,
        ANALYST,
        &["ioc", "add", "--type", "ip", "--value", "not an ip"],
    );
    assert_eq!(
        (c, v["error"]["code"].as_str()),
        (2, Some("invalid")),
        "{v}"
    );
    let (c, _) = run(
        &p,
        ANALYST,
        &["ioc", "add", "--type", "nonsense", "--value", "x"],
    );
    assert_eq!(c, 2);
    ok(
        &p,
        &["ioc", "add", "--type", "ip", "--value", "203.0.113.7"],
    );
    let v = ok(&p, &["ioc", "list", "--type", "ip"]);
    assert_eq!(v["data"]["count"], 1);
    assert_eq!(
        inner(&v["data"]["untrusted"])["iocs"][0]["value"],
        "203.0.113.7"
    );
    assert_eq!(
        ok(&p, &["ioc", "list", "--text", "acme"])["data"]["count"],
        1
    );
    ok(&p, &["ioc", "merge", "--from", "2", "--into", "1"]);
    ok(&p, &["ioc", "status", "1", "unreviewed"]);
    ok(&p, &["ioc", "show", "I-1"]);
    ok(&p, &["ioc", "delete", "1"]);
}

#[test]
fn links_and_the_entries_a_record_shows() {
    let (_d, p) = case();
    ok(&p, &["entry", "add", "--title", "e"]);
    ok(&p, &["account", "add", "--name", "jdoe"]);
    let v = ok(
        &p,
        &[
            "link", "add", "--entry", "E-1", "--kind", "account", "--id", "A-1",
        ],
    );
    assert_eq!(
        v["data"],
        json!({"entry": 1, "kind": "account", "id": 1, "linked": true})
    );
    let v = ok(&p, &["account", "show", "1"]);
    assert_eq!(v["data"]["entries"], 1);
    assert_eq!(v["data"]["linked_entries"][0]["shown"], "E-1");
    assert_eq!(
        inner(&v["data"]["untrusted"])["linked_entries"][0]["title"],
        "e"
    );
    let v = ok(
        &p,
        &[
            "link", "remove", "--entry", "1", "--kind", "account", "--id", "1",
        ],
    );
    assert_eq!(v["data"]["linked"], false);
    assert_eq!(ok(&p, &["account", "show", "1"])["data"]["entries"], 0);
    // A wrong prefix is a usage error, an unknown kind invalid.
    let (c, _) = run(
        &p,
        ANALYST,
        &[
            "link", "add", "--entry", "1", "--kind", "account", "--id", "H-1",
        ],
    );
    assert_eq!(c, 2);
    let (c, _) = run(
        &p,
        ANALYST,
        &["link", "add", "--entry", "1", "--kind", "nope", "--id", "1"],
    );
    assert_eq!(c, 2);
}

#[test]
fn candidates_sightings_and_logons() {
    let (_d, p) = case();
    // A row that names an address.
    let r = row(&p, "evtxtriage_events", "svch0st.exe:443");
    let v = ok(&p, &["candidates", "--row", &r.to_string()]);
    assert_eq!(keys(&v["data"]), ["candidates", "count", "untrusted"]);
    let found = inner(&v["data"]["untrusted"]);
    let ip = found["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["value"] == "203.0.113.7")
        .unwrap_or_else(|| panic!("no 203.0.113.7 candidate: {found}"));
    let at = ip["index"].as_u64().unwrap() as usize;
    let safe = &v["data"]["candidates"][at];
    assert_eq!(
        (safe["kind"].as_str(), safe["ioc_type"].as_str()),
        (Some("ioc"), Some("ip"))
    );
    assert_eq!(safe["existing_id"], Value::Null);
    assert!(safe.get("value").is_none());

    // Once recorded, the candidate names the record.
    ok(
        &p,
        &["ioc", "add", "--type", "ip", "--value", "203.0.113.7"],
    );
    let v = ok(&p, &["candidates", "--row", &r.to_string()]);
    assert!(v["data"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["existing_id"] == 1));

    // Sightings: computed, counts outside, relations and hosts inside.
    let v = ok(&p, &["ioc", "sightings", "--id", "I-1"]);
    assert_eq!(
        keys(&v["data"]),
        ["first", "hits", "id", "last", "untrusted"]
    );
    assert!(v["data"]["hits"].as_u64().unwrap() >= 2, "{v}");
    let text = inner(&v["data"]["untrusted"]);
    assert!(text["hosts"].as_array().unwrap().contains(&json!("WS01")));
    assert!(text["relations"].to_string().contains("evtxtriage_events"));
    let (c, _) = run(&p, ANALYST, &["ioc", "sightings", "--id", "99"]);
    assert_eq!(c, 3);

    // Logons: the fixture has one success (type 10) and one failure (type 3)
    // for ACME\jdoe, and one 4624 with no account to read.
    ok(
        &p,
        &["account", "add", "--name", "jdoe", "--domain", "acme"],
    );
    let v = ok(&p, &["account", "logons", "--id", "A-1"]);
    assert_eq!(
        keys(&v["data"]),
        ["count", "id", "logons", "unparsed", "untrusted"]
    );
    assert!(v["data"]["unparsed"].as_u64().unwrap() >= 1, "{v}");
    let mut got: Vec<(String, u64, u64)> = v["data"]["logons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["outcome"].as_str().unwrap().to_string(),
                l["logon_type"].as_u64().unwrap(),
                l["count"].as_u64().unwrap(),
            )
        })
        .collect();
    got.sort();
    assert_eq!(got, [("failure".into(), 3, 1), ("success".into(), 10, 1)]);
    let text = inner(&v["data"]["untrusted"]);
    assert_eq!(text["source"], "evtxtriage_events");
    assert_eq!(text["logons"][0]["host"], "WS01");
    assert!(text["logons"][0]["filter"].is_object());
    let (c, _) = run(&p, ANALYST, &["account", "logons", "--id", "99"]);
    assert_eq!(c, 3);
}

#[test]
fn exports_write_their_files() {
    let (d, p) = case();
    let r = svch0st(&p);
    ok(&p, &["entry", "promote", "--rows", &r.to_string()]);
    ok(&p, &["entry", "add", "--title", "=HYPERLINK(\"http://x\")"]);
    for (kind, marker) in [
        ("timeline", "id,time"),
        ("workbook", "PK"),
        ("report", "# "),
    ] {
        let path = d.path().join(format!("out_{kind}"));
        let target = path.to_string_lossy().into_owned();
        let v = ok(&p, &["export", kind, &target]);
        assert_eq!(keys(&v["data"]), ["rows", "untrusted"], "{kind}");
        // The timeline counts its entries; the workbook and report also count the WS01 host record the promoted row made.
        let want = if kind == "timeline" { 2 } else { 3 };
        assert_eq!(v["data"]["rows"], want, "{kind}");
        assert!(inner(&v["data"]["untrusted"])["path"]
            .as_str()
            .unwrap()
            .ends_with(&format!("out_{kind}")));
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            String::from_utf8_lossy(&bytes).starts_with(marker),
            "{kind}: {:?}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(40)])
        );
    }
    // A folder is no file to write.
    let (c, v) = run(
        &p,
        ANALYST,
        &["export", "timeline", &d.path().to_string_lossy()],
    );
    assert_eq!(
        (c, v["error"]["code"].as_str()),
        (2, Some("invalid")),
        "{v}"
    );
}

#[test]
fn history_of_one_target() {
    let (_d, p) = case();
    ok(&p, &["entry", "add", "--title", "one"]);
    ok(&p, &["entry", "add", "--title", "two"]);
    ok(&p, &["entry", "edit", "1", "--title", "uno"]);
    let v = ok(&p, &["history", "--kind", "entry", "--id", "E-1"]);
    let rows = inner(&v["data"]["untrusted"])["entries"].clone();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|r| r["target_id"] == "1"));
    let all = ok(&p, &["history", "--kind", "entry"]);
    assert_eq!(all["data"]["entries"], 3);
    // --id names a target only within a kind.
    let (c, _) = run(&p, ANALYST, &["history", "--id", "1"]);
    assert_eq!(c, 2);
}

/// Review focus 5 through the CLI: an agent's changes to an analyst's items
/// are refused, naming the owner; the agent's own items it may change but not
/// review; the label survives an analyst's review.
#[test]
fn agent_ownership_outcomes() {
    let (_d, p) = case();
    ok(
        &p,
        &[
            "entry",
            "add",
            "--title",
            "analyst entry",
            "--time",
            "2026-01-05T02:14:07Z",
        ],
    );
    ok(&p, &["host", "add", "--name", "WS01"]);
    ok(&p, &["account", "add", "--name", "jdoe"]);
    ok(
        &p,
        &["ioc", "add", "--type", "ip", "--value", "203.0.113.7"],
    );
    ok(&p, &["host", "add", "--name", "SRV01"]);
    let r = svch0st(&p);

    let refused = |args: &[&str]| {
        let (c, v) = run(&p, AGENT, args);
        assert_eq!(
            (c, v["error"]["code"].as_str()),
            (4, Some("refused")),
            "{args:?} -> {v}"
        );
        // The refusal names the owner, inside the wrapper.
        let message = inner(&v["error"]["untrusted"])["message"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(message.contains(ANALYST), "{args:?}: {message}");
    };
    refused(&["entry", "edit", "1", "--title", "mine now"]);
    refused(&["entry", "delete", "1"]);
    refused(&["entry", "attach", "1", "--rows", &r.to_string()]);
    refused(&["host", "edit", "1", "--role", "x"]);
    refused(&["host", "delete", "1"]);
    refused(&["host", "merge", "--from", "2", "--into", "1"]);
    refused(&["account", "edit", "1", "--note", "x"]);
    refused(&["account", "delete", "1"]);
    refused(&["ioc", "edit", "1", "--note", "x"]);
    refused(&["ioc", "delete", "1"]);
    // Linking from an analyst's entry.
    refused(&["link", "add", "--entry", "1", "--kind", "host", "--id", "1"]);
    // Nobody but an analyst reviews, an agent's own items included.
    let (c, v) = run(&p, AGENT, &["entry", "status", "1", "reviewed"]);
    assert_eq!(
        (c, v["error"]["code"].as_str()),
        (4, Some("refused")),
        "{v}"
    );
    let (c, _) = run(&p, AGENT, &["host", "status", "1", "reviewed"]);
    assert_eq!(c, 4);
    let (c, _) = run(&p, AGENT, &["account", "status", "1", "reviewed"]);
    assert_eq!(c, 4);
    let (c, _) = run(&p, AGENT, &["ioc", "status", "1", "reviewed"]);
    assert_eq!(c, 4);

    // The analyst's items are untouched.
    let v = ok(&p, &["entry", "show", "1"]);
    assert_eq!(inner(&v["data"]["untrusted"])["title"], "analyst entry");
    assert_eq!(v["data"]["evidence_rows"], 0);

    // The agent's own entry: unreviewed, labelled, editable, linkable to
    // an analyst's record, and an analyst's review keeps the label.
    let (c, v) = run(
        &p,
        AGENT,
        &[
            "entry",
            "add",
            "--title",
            "agent entry",
            "--time",
            "2026-01-05T02:14:07Z",
        ],
    );
    assert_eq!(c, 0, "{v}");
    assert_eq!(
        (
            v["data"]["status"].as_str(),
            v["data"]["agent_added"].as_bool(),
            v["data"]["created_by"].as_str()
        ),
        (Some("unreviewed"), Some(true), Some(AGENT))
    );
    let id = v["data"]["id"].as_i64().unwrap().to_string();
    let (c, v) = run(
        &p,
        AGENT,
        &["entry", "edit", &id, "--description", "edited"],
    );
    assert_eq!(c, 0, "{v}");
    let (c, v) = run(
        &p,
        AGENT,
        &["link", "add", "--entry", &id, "--kind", "host", "--id", "1"],
    );
    assert_eq!(c, 0, "{v}");
    let (c, v) = run(
        &p,
        AGENT,
        &["entry", "attach", &id, "--rows", &r.to_string()],
    );
    assert_eq!((c, v["data"]["done"].as_u64()), (0, Some(1)), "{v}");
    let (c, v) = run(&p, AGENT, &["host", "add", "--name", "agent-host"]);
    assert_eq!(
        (
            c,
            v["data"]["status"].as_str(),
            v["data"]["agent_added"].as_bool()
        ),
        (0, Some("unreviewed"), Some(true)),
        "{v}"
    );
    let v = ok(&p, &["entry", "status", &id, "reviewed"]);
    assert_eq!(
        (
            v["data"]["status"].as_str(),
            v["data"]["agent_added"].as_bool(),
            v["data"]["reviewed_by"].as_str()
        ),
        (Some("reviewed"), Some(true), Some(ANALYST))
    );
    // Reviewed, it is no longer the agent's to change.
    let (c, _) = run(&p, AGENT, &["entry", "edit", &id, "--title", "again"]);
    assert_eq!(c, 4);
    // Promoting a row that is evidence on an analyst's reviewed entry is an
    // outcome, not an error, and makes nothing.
    let (c, v) = run(&p, AGENT, &["entry", "promote", "--rows", &r.to_string()]);
    assert_eq!(c, 0, "{v}");
    assert_eq!(v["data"]["created"], 0, "{v}");
}

/// Text from evidence never appears outside the wrapper in the record
/// commands: values taken from rows, account and host names, evidence
/// summaries, titles made from rows.
#[test]
fn evidence_text_is_wrapped() {
    let (d, p) = case();
    let r = svch0st(&p);
    let mut raws: Vec<(String, String)> = Vec::new();
    let mut go = |args: Vec<String>| {
        let mut full = vec![
            "--case".to_string(),
            p.clone(),
            "--as".into(),
            ANALYST.into(),
        ];
        full.extend(args.clone());
        let refs: Vec<&str> = full.iter().map(String::as_str).collect();
        let (code, raw) = common::ox_raw(&refs);
        raws.push((args.join(" "), raw.clone()));
        assert!([0, 2, 3, 4].contains(&code), "{args:?}: {code} {raw}");
        raw
    };
    let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    go(s(&["entry", "promote", "--rows", &r.to_string()]));
    go(s(&[
        "entry",
        "add",
        "--title",
        "svc_backup from 203.0.113.7",
        "--description",
        "svch0st on SRV01",
    ]));
    go(s(&["entry", "list"]));
    go(s(&["entry", "show", "1"]));
    go(s(&["entry", "edit", "2", "--title", "svc_backup"]));
    go(s(&["evidence", "list", "--entry", "1"]));
    go(s(&["host", "list"]));
    go(s(&["host", "show", "1"]));
    go(s(&["host", "add", "--name", "SRV01", "--note", "svch0st"]));
    go(s(&[
        "account",
        "add",
        "--name",
        "svc_backup",
        "--domain",
        "ACME",
    ]));
    go(s(&["account", "list"]));
    go(s(&["account", "show", "1"]));
    go(s(&["account", "logons", "--id", "1"]));
    go(s(&[
        "ioc",
        "add",
        "--type",
        "ip",
        "--value",
        "203.0.113.7",
        "--tool",
        "svch0st",
    ]));
    go(s(&["ioc", "list"]));
    go(s(&["ioc", "show", "1"]));
    go(s(&["ioc", "sightings", "--id", "1"]));
    go(s(&["candidates", "--row", &r.to_string()]));
    go(s(&[
        "link", "add", "--entry", "2", "--kind", "host", "--id", "2",
    ]));
    go(s(&["history", "--kind", "entry"]));
    go(s(&["history", "--kind", "host", "--id", "H-1"]));
    // Errors that quote evidence-shaped text.
    go(s(&["ioc", "add", "--type", "ip", "--value", "svc_backup"]));
    go(s(&["host", "add", "--name", "WS01"]));
    go(s(&["host", "add", "--name", "WS01"]));
    go(s(&["entry", "show", "999"]));
    let out = d.path().join("evidence.csv").to_string_lossy().into_owned();
    go(s(&["export", "timeline", &out]));

    let markers = [
        "svc_backup",
        "203.0.113.7",
        "svch0st",
        "WS01",
        "SRV01",
        "ACME",
        "evtxtriage_events",
    ];
    let mut inside_hits = 0;
    for (label, raw) in &raws {
        let outside = strip_wrappers(raw);
        for m in markers {
            assert!(
                !outside.contains(m),
                "{label}: {m:?} escaped the wrapper:\n{outside}"
            );
        }
        inside_hits += raw.matches("<untrusted_data").count();
    }
    assert!(
        inside_hits > 20,
        "the wrapper was barely used: {inside_hits}"
    );
}

fn strip_wrappers(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<untrusted_data boundary=") {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let b_end = tail.find('>').unwrap();
        let boundary = &tail["<untrusted_data boundary=".len()..b_end];
        let close = format!("</untrusted_data boundary={boundary}>");
        let end = tail.find(&close).expect("every wrapper is closed") + close.len();
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

#[test]
fn capabilities_list_the_record_commands_and_limits() {
    let (c, v) = ox(&["capabilities"]);
    assert_eq!(c, 0);
    let listed: Vec<&str> = v["data"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    for name in [
        "entry list",
        "entry show",
        "entry add",
        "entry edit",
        "entry status",
        "entry delete",
        "entry promote",
        "entry attach",
        "entry detach",
        "evidence list",
        "host list",
        "host show",
        "host add",
        "host edit",
        "host delete",
        "host merge",
        "host status",
        "account list",
        "account show",
        "account add",
        "account edit",
        "account delete",
        "account merge",
        "account status",
        "account logons",
        "ioc list",
        "ioc show",
        "ioc add",
        "ioc edit",
        "ioc delete",
        "ioc merge",
        "ioc status",
        "ioc sightings",
        "link add",
        "link remove",
        "candidates",
        "export timeline",
        "export workbook",
        "export report",
    ] {
        assert!(listed.contains(&name), "{name} is not in capabilities");
    }
    let limits = &v["data"]["limits"];
    for (key, value) in [
        ("entry_title_chars", oxl_engine::MAX_TITLE),
        ("entry_description_chars", oxl_engine::MAX_DESCRIPTION),
        ("entry_techniques", oxl_engine::MAX_TECHNIQUES),
        ("record_name_chars", oxl_engine::MAX_RECORD_NAME),
        ("record_label_chars", oxl_engine::MAX_RECORD_LABEL),
        ("record_note_chars", oxl_engine::MAX_RECORD_NOTE),
        ("ioc_value_chars", oxl_engine::MAX_IOC_VALUE),
        ("action_rows_max", oxl_engine::MAX_ACTION_ROWS),
    ] {
        assert_eq!(limits[key].as_u64(), Some(value as u64), "{key}");
    }
    assert!(limits["entry_title_chars"].as_u64().unwrap() > 0);
}

#[test]
fn bad_ids_are_usage_errors() {
    let (_d, p) = case();
    for args in [
        vec!["entry", "show", "H-1"],
        vec!["entry", "show", "abc"],
        vec!["host", "show", "E-1"],
        vec!["entry", "show", "-1"],
    ] {
        let (c, v) = run(&p, ANALYST, &args);
        assert_eq!(
            (c, v["error"]["code"].as_str()),
            (2, Some("invalid")),
            "{args:?} -> {v}"
        );
    }
}
