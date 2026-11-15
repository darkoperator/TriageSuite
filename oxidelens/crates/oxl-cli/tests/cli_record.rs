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
            "mark",
            "modified_at",
            "modified_by",
            "needs_time",
            "reviewed_by",
            "shown",
            "status",
            "tactic",
            "technique_ids",
            "time",
            "time_shown",
            "time_source",
            "time_us",
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
    let r = svch0st(&p);
    let r2 = row(&p, "evtxtriage_events", "rclone.exe").to_string();
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
    ok(&p, &["host", "add", "--name", "SRV01"]);
    ok(&p, &["account", "add", "--name", "jdoe"]);
    ok(&p, &["account", "add", "--name", "other"]);
    ok(
        &p,
        &["ioc", "add", "--type", "ip", "--value", "203.0.113.7"],
    );
    ok(
        &p,
        &["ioc", "add", "--type", "domain", "--value", "a.acme.local"],
    );
    ok(&p, &["entry", "attach", "1", "--rows", &r2]);
    ok(
        &p,
        &[
            "link", "add", "--entry", "1", "--kind", "account", "--id", "1",
        ],
    );
    ok(&p, &["mark", "set", "--rows", &r.to_string()]);

    // Each refusal is exit 4 `refused`, and its reason names the owner.
    let refused = |args: &[&str]| {
        let (c, v) = run(&p, AGENT, args);
        assert_eq!(
            (c, v["error"]["code"].as_str()),
            (4, Some("refused")),
            "{args:?} -> {v}"
        );
        let message = inner(&v["error"]["untrusted"])["message"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(message.contains(ANALYST), "{args:?}: {message}");
    };
    refused(&["entry", "edit", "1", "--title", "mine now"]);
    refused(&["entry", "delete", "1"]);
    refused(&["entry", "attach", "1", "--rows", &r.to_string()]);
    refused(&["entry", "detach", "1", "--row", &r2]);
    refused(&["entry", "status", "1", "reviewed"]);
    refused(&["host", "edit", "1", "--role", "x"]);
    refused(&["host", "delete", "1"]);
    refused(&["host", "merge", "--from", "2", "--into", "1"]);
    refused(&["host", "status", "1", "reviewed"]);
    refused(&["account", "edit", "1", "--note", "x"]);
    refused(&["account", "delete", "1"]);
    refused(&["account", "merge", "--from", "2", "--into", "1"]);
    refused(&["account", "status", "1", "reviewed"]);
    refused(&["ioc", "edit", "1", "--note", "x"]);
    refused(&["ioc", "delete", "1"]);
    refused(&["ioc", "merge", "--from", "2", "--into", "1"]);
    refused(&["ioc", "status", "1", "reviewed"]);
    // Linking and unlinking from an analyst's entry.
    refused(&["link", "add", "--entry", "1", "--kind", "host", "--id", "1"]);
    refused(&[
        "link", "remove", "--entry", "1", "--kind", "account", "--id", "1",
    ]);

    // An agent clearing an analyst's mark is an outcome, not an error, that
    // names the owner; the mark stays.
    let (c, v) = run(&p, AGENT, &["mark", "clear", "--rows", &r.to_string()]);
    assert_eq!(
        (
            c,
            v["data"]["done"].as_u64(),
            v["data"]["not_done"].as_u64()
        ),
        (0, Some(0), Some(1)),
        "{v}"
    );
    let reason = v["data"]["outcomes"][0]["reason"].as_str().unwrap();
    assert!(reason.contains(ANALYST), "{reason}");
    let marks = ok(&p, &["mark", "list", "--by", ANALYST]);
    assert!(
        marks["data"]["row_ids"]
            .as_array()
            .unwrap()
            .contains(&json!(r)),
        "{marks}"
    );

    // The analyst's items are untouched.
    let v = ok(&p, &["entry", "show", "1"]);
    assert_eq!(inner(&v["data"]["untrusted"])["title"], "analyst entry");
    assert_eq!(v["data"]["evidence_rows"], 1);
    assert_eq!(v["data"]["status"], "draft");

    // The agent's own entry: unreviewed, labelled, editable, linkable to an
    // analyst's record; it cannot review it; an analyst's review keeps the label.
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
    assert_eq!((c, v["data"]["linked"].as_bool()), (0, Some(true)), "{v}");
    let (c, v) = run(
        &p,
        AGENT,
        &["entry", "attach", &id, "--rows", &r.to_string()],
    );
    assert_eq!((c, v["data"]["done"].as_u64()), (0, Some(1)), "{v}");
    let (c, v) = run(&p, AGENT, &["entry", "status", &id, "reviewed"]);
    assert_eq!(
        (c, v["error"]["code"].as_str()),
        (4, Some("refused")),
        "{v}"
    );
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
    let agent_host = v["data"]["id"].as_i64().unwrap().to_string();
    let (c, v) = run(&p, AGENT, &["host", "status", &agent_host, "reviewed"]);
    assert_eq!(
        (c, v["error"]["code"].as_str()),
        (4, Some("refused")),
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
}

/// Text from evidence never appears outside the wrapper in the record
/// commands (values taken from rows, account and host names, evidence
/// summaries, titles made from rows), and each call really returns data: a
/// call is expected to succeed (or fail with the code named), and the text
/// it is about must be found inside its wrapper.
#[test]
fn evidence_text_is_wrapped() {
    let (d, p) = case();
    let r = svch0st(&p).to_string();
    let out = d.path().join("evidence.csv").to_string_lossy().into_owned();
    // (arguments, expected exit code, text that must be inside the wrapper).
    let calls: Vec<(Vec<&str>, i32, Option<&str>)> = vec![
        (vec!["entry", "promote", "--rows", &r], 0, None),
        (
            vec![
                "entry",
                "add",
                "--title",
                "svc_backup from 203.0.113.7",
                "--description",
                "svch0st on SRV01",
            ],
            0,
            Some("svc_backup from 203.0.113.7"),
        ),
        (
            vec!["entry", "list"],
            0,
            Some("svc_backup from 203.0.113.7"),
        ),
        (vec!["entry", "show", "1"], 0, Some("Network connection")),
        (
            vec!["entry", "edit", "2", "--title", "svc_backup"],
            0,
            Some("svc_backup"),
        ),
        (
            vec!["evidence", "list", "--entry", "1"],
            0,
            Some("evtxtriage_events"),
        ),
        (vec!["host", "list"], 0, Some("WS01")),
        (vec!["host", "show", "1"], 0, Some("WS01")),
        (
            vec!["host", "add", "--name", "SRV01", "--note", "svch0st"],
            0,
            Some("svch0st"),
        ),
        (
            vec!["account", "add", "--name", "svc_backup", "--domain", "ACME"],
            0,
            Some("ACME"),
        ),
        (vec!["account", "list"], 0, Some("svc_backup")),
        (vec!["account", "show", "1"], 0, Some("svc_backup")),
        (
            vec!["account", "logons", "--id", "1"],
            0,
            Some("evtxtriage_events"),
        ),
        (
            vec![
                "ioc",
                "add",
                "--type",
                "ip",
                "--value",
                "203.0.113.7",
                "--tool",
                "svch0st",
            ],
            0,
            Some("203.0.113.7"),
        ),
        (vec!["ioc", "list"], 0, Some("203.0.113.7")),
        (vec!["ioc", "show", "1"], 0, Some("203.0.113.7")),
        (vec!["ioc", "sightings", "--id", "1"], 0, Some("WS01")),
        (vec!["candidates", "--row", &r], 0, Some("203.0.113.7")),
        (
            vec!["link", "add", "--entry", "2", "--kind", "host", "--id", "2"],
            0,
            None,
        ),
        (vec!["history", "--kind", "entry"], 0, Some("svc_backup")),
        (
            vec!["history", "--kind", "host", "--id", "H-1"],
            0,
            Some("WS01"),
        ),
        // Errors that quote evidence-shaped text: wrapped, with the right code.
        (
            vec!["ioc", "add", "--type", "ip", "--value", "svc_backup"],
            2,
            Some("svc_backup"),
        ),
        // The promoted row already made the WS01 host record.
        (vec!["host", "add", "--name", "WS01"], 4, None),
        (vec!["entry", "show", "999"], 3, None),
        (vec!["export", "timeline", &out], 0, Some("evidence.csv")),
    ];

    let markers = [
        "svc_backup",
        "203.0.113.7",
        "svch0st",
        "WS01",
        "SRV01",
        "ACME",
        "evtxtriage_events",
    ];
    for (args, want, inside) in &calls {
        let (code, raw) = {
            let mut full = vec!["--case", p.as_str(), "--as", ANALYST];
            full.extend(args.iter().copied());
            common::ox_raw(&full)
        };
        let label = args.join(" ");
        assert_eq!(code, *want, "{label}: {raw}");
        let outside = strip_wrappers(&raw);
        for m in markers {
            assert!(
                !outside.contains(m),
                "{label}: {m:?} escaped the wrapper:\n{outside}"
            );
        }
        if let Some(text) = inside {
            let inner_text = wrapped_text(&raw);
            assert!(
                inner_text.contains(text),
                "{label}: {text:?} is not inside the wrapper:\n{raw}"
            );
        }
        // A successful call carries its data; an error says why.
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["ok"], *want == 0, "{label}: {raw}");
    }
}

/// Everything between the wrapper lines, joined.
fn wrapped_text(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<untrusted_data boundary=") {
        let tail = &rest[start..];
        let b_end = tail.find('>').unwrap();
        let boundary = &tail["<untrusted_data boundary=".len()..b_end];
        let close = format!("</untrusted_data boundary={boundary}>");
        let end = tail.find(&close).expect("every wrapper is closed");
        out.push_str(&tail[b_end + 1..end]);
        rest = &tail[end + close.len()..];
    }
    out
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

const HOST_KEYS: [&str; 13] = [
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
    "untrusted",
];

/// A reply's keys, plus `extra` ones, sorted.
fn with(base: &[&'static str], extra: &[&'static str]) -> Vec<&'static str> {
    let mut all: Vec<&'static str> = base.iter().chain(extra).copied().collect();
    all.sort_unstable();
    all
}

/// `v` as an integer, whether the reply wrote it as a number or a string.
fn int(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

// The wrapped (inner) key sets, by kind of item. Sorted.
const ENTRY_IN: &[&str] = &["description", "hosts", "id", "links", "title"];
const EVIDENCE_IN: &[&str] = &["host", "relation", "row_id", "summary"];
const HOST_IN: &[&str] = &["id", "name", "note", "role"];
const ACCOUNT_IN: &[&str] = &[
    "description",
    "domain",
    "hosts_seen",
    "id",
    "name",
    "note",
    "sid",
];
const IOC_IN: &[&str] = &["hosts_seen", "id", "note", "tool", "value"];
const LOGON_IN: &[&str] = &["filter", "host", "index"];
const CANDIDATE_IN: &[&str] = &["column", "domain", "index", "sid", "value"];

const ENTRY_OUT: &[&str] = &[
    "agent_added",
    "created_at",
    "created_by",
    "evidence_rows",
    "id",
    "mark",
    "modified_at",
    "modified_by",
    "needs_time",
    "reviewed_by",
    "shown",
    "status",
    "tactic",
    "technique_ids",
    "time",
    "time_shown",
    "time_source",
    "time_us",
    "untrusted",
];
const ENTRY_SHOW_OUT: &[&str] = &[
    "agent_added",
    "created_at",
    "created_by",
    "evidence",
    "evidence_rows",
    "id",
    "mark",
    "modified_at",
    "modified_by",
    "needs_time",
    "reviewed_by",
    "shown",
    "status",
    "tactic",
    "technique_ids",
    "time",
    "time_shown",
    "time_source",
    "time_us",
    "untrusted",
];
const HOST_OUT: &[&str] = &[
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
    "untrusted",
];
const HOST_SHOW_OUT: &[&str] = &[
    "agent_added",
    "created_at",
    "created_by",
    "deleted",
    "entries",
    "first_seen",
    "id",
    "last_seen",
    "linked_entries",
    "modified_at",
    "modified_by",
    "shown",
    "status",
    "untrusted",
];
const ACCOUNT_OUT: &[&str] = &[
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
    "privilege",
    "shown",
    "status",
    "untrusted",
];
const ACCOUNT_SHOW_OUT: &[&str] = &[
    "agent_added",
    "created_at",
    "created_by",
    "deleted",
    "entries",
    "first_seen",
    "id",
    "last_seen",
    "linked_entries",
    "modified_at",
    "modified_by",
    "privilege",
    "shown",
    "status",
    "untrusted",
];
const IOC_OUT: &[&str] = &[
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
    "untrusted",
];
const IOC_SHOW_OUT: &[&str] = &[
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
    "linked_entries",
    "modified_at",
    "modified_by",
    "shown",
    "status",
    "untrusted",
];

/// What a command's reply must hold, exactly: its outer keys, the keys of the
/// JSON inside its `untrusted` wrapper (none when it has no wrapper), and for
/// each array in that wrapper the keys of its items.
struct Shape {
    outer: &'static [&'static str],
    wrapped: Option<&'static [&'static str]>,
    items: &'static [(&'static str, &'static [&'static str])],
}

fn shape_of(command: &str) -> Shape {
    let s = |outer, wrapped, items| Shape {
        outer,
        wrapped,
        items,
    };
    let none: &'static [(&'static str, &'static [&'static str])] = &[];
    match command {
        "entry add" | "entry edit" | "entry status" => s(ENTRY_OUT, Some(ENTRY_IN), none),
        "entry show" => s(
            ENTRY_SHOW_OUT,
            Some(&["description", "evidence", "hosts", "id", "links", "title"]),
            &[("evidence", EVIDENCE_IN)],
        ),
        "entry list" => s(
            &["count", "entries", "untrusted"],
            Some(&["entries"]),
            &[("entries", ENTRY_IN)],
        ),
        "entry delete" => s(&["id", "shown"], None, none),
        "entry promote" => s(&["created", "done", "not_done", "outcomes"], None, none),
        "entry attach" => s(&["done", "not_done", "outcomes"], None, none),
        "entry detach" => s(&["entry", "row_id"], None, none),
        "evidence list" => s(
            &["count", "evidence", "untrusted"],
            Some(&["evidence"]),
            &[("evidence", EVIDENCE_IN)],
        ),
        "host add" | "host edit" => s(HOST_OUT, Some(HOST_IN), none),
        "host show" => s(
            HOST_SHOW_OUT,
            Some(&["id", "linked_entries", "name", "note", "role"]),
            &[("linked_entries", ENTRY_IN)],
        ),
        "host list" => s(
            &["count", "hosts", "untrusted"],
            Some(&["hosts"]),
            &[("hosts", HOST_IN)],
        ),
        "host merge" | "account merge" | "ioc merge" => s(&["from", "into"], None, none),
        "host status" | "account status" | "ioc status" => {
            s(&["id", "shown", "status"], None, none)
        }
        "host delete" | "account delete" | "ioc delete" => s(&["id", "shown"], None, none),
        "account add" | "account edit" => s(ACCOUNT_OUT, Some(ACCOUNT_IN), none),
        "account show" => s(
            ACCOUNT_SHOW_OUT,
            Some(&[
                "description",
                "domain",
                "hosts_seen",
                "id",
                "linked_entries",
                "name",
                "note",
                "sid",
            ]),
            &[("linked_entries", ENTRY_IN)],
        ),
        "account list" => s(
            &["accounts", "count", "untrusted"],
            Some(&["accounts"]),
            &[("accounts", ACCOUNT_IN)],
        ),
        "account logons" => s(
            &["count", "id", "logons", "unparsed", "untrusted"],
            Some(&["logons", "source"]),
            &[("logons", LOGON_IN)],
        ),
        "ioc add" | "ioc edit" => s(IOC_OUT, Some(IOC_IN), none),
        "ioc show" => s(
            IOC_SHOW_OUT,
            Some(&[
                "hosts_seen",
                "id",
                "linked_entries",
                "note",
                "tool",
                "value",
            ]),
            &[("linked_entries", ENTRY_IN)],
        ),
        "ioc list" => s(
            &["count", "iocs", "untrusted"],
            Some(&["iocs"]),
            &[("iocs", IOC_IN)],
        ),
        "ioc sightings" => s(
            &["first", "hits", "id", "last", "untrusted"],
            Some(&["hosts", "relations"]),
            none,
        ),
        "link add" | "link remove" => s(&["entry", "id", "kind", "linked"], None, none),
        "candidates" => s(
            &["candidates", "count", "untrusted"],
            Some(&["candidates"]),
            &[("candidates", CANDIDATE_IN)],
        ),
        "export timeline" | "export workbook" | "export report" => {
            s(&["rows", "untrusted"], Some(&["path"]), none)
        }
        "history" => s(&["entries", "untrusted"], Some(&["entries"]), none),
        other => panic!("no shape pinned for {other}"),
    }
}

/// Assert `v`'s outer keys, the keys inside its wrapper and those of the
/// wrapper's array items, exactly (sorted list equality).
fn pin(v: &Value) {
    let command = v["command"].as_str().unwrap();
    let shape = shape_of(command);
    assert_eq!(keys(&v["data"]), shape.outer, "{command}: outer keys");
    let Some(wrapped) = shape.wrapped else {
        assert!(v["data"].get("untrusted").is_none(), "{command}: {v}");
        return;
    };
    let text = inner(&v["data"]["untrusted"]);
    assert_eq!(keys(&text), wrapped, "{command}: wrapped keys");
    for (array, item_keys) in shape.items {
        for item in text[*array].as_array().unwrap() {
            assert_eq!(keys(item), *item_keys, "{command}: wrapped {array} item");
        }
    }
}

/// `ok`, then the reply's shape pinned.
fn pinned(p: &str, args: &[&str]) -> Value {
    let v = ok(p, args);
    pin(&v);
    v
}
/// Every command's reply: its keys and the values that mean something (ids,
/// shown ids, statuses, counts), pinned. Commands the other tests pin in
/// more detail are repeated here for the whole set.
#[test]
fn reply_shapes_of_every_command() {
    // Every reply below goes through `pinned`: outer and wrapped keys, exactly.
    let ok = pinned;
    let (d, p) = case();
    let r = svch0st(&p);
    let r2 = row(&p, "evtxtriage_events", "rclone.exe");
    let rs = r.to_string();
    let r2s = r2.to_string();

    // entry add / promote / attach / detach / status / show / delete
    let v = ok(
        &p,
        &[
            "entry",
            "add",
            "--title",
            "t",
            "--time",
            "2026-01-05T02:14:07Z",
        ],
    );
    assert_eq!(v["command"], "entry add");
    let v = ok(&p, &["entry", "promote", "--rows", &rs]);
    assert_eq!(v["command"], "entry promote");
    assert_eq!(
        keys(&v["data"]),
        ["created", "done", "not_done", "outcomes"]
    );
    assert_eq!(
        (
            v["data"]["created"].as_u64(),
            v["data"]["done"].as_u64(),
            v["data"]["not_done"].as_u64()
        ),
        (Some(1), Some(1), Some(0))
    );
    let o = &v["data"]["outcomes"][0];
    assert_eq!(keys(o), ["created", "entry_id", "reason", "row_id"]);
    assert_eq!((int(&o["entry_id"]), int(&o["row_id"])), (Some(2), Some(r)));

    let v = ok(&p, &["entry", "attach", "1", "--rows", &r2s]);
    assert_eq!(v["command"], "entry attach");
    assert_eq!(keys(&v["data"]), ["done", "not_done", "outcomes"]);
    assert_eq!(
        (v["data"]["done"].as_u64(), v["data"]["not_done"].as_u64()),
        (Some(1), Some(0))
    );
    assert_eq!(v["data"]["outcomes"][0]["done"], true);

    let evidence_keys = ["attached_at", "attached_by", "row_id", "standing"];
    let v = ok(&p, &["entry", "show", "1"]);
    assert_eq!(
        keys(&v["data"]),
        [
            "agent_added",
            "created_at",
            "created_by",
            "evidence",
            "evidence_rows",
            "id",
            "mark",
            "modified_at",
            "modified_by",
            "needs_time",
            "reviewed_by",
            "shown",
            "status",
            "tactic",
            "technique_ids",
            "time",
            "time_shown",
            "time_source",
            "time_us",
            "untrusted"
        ]
    );
    assert_eq!(
        (
            v["data"]["id"].as_i64(),
            v["data"]["evidence_rows"].as_u64()
        ),
        (Some(1), Some(1))
    );
    assert_eq!(keys(&v["data"]["evidence"][0]), evidence_keys);
    assert_eq!(v["data"]["evidence"][0]["standing"], "unchanged");

    let v = ok(&p, &["evidence", "list", "--entry", "1"]);
    assert_eq!(v["command"], "evidence list");
    assert_eq!(v["data"]["count"], 1);
    assert_eq!(keys(&v["data"]["evidence"][0]), evidence_keys);

    let v = ok(&p, &["entry", "detach", "1", "--row", &r2s]);
    assert_eq!(keys(&v["data"]), ["entry", "row_id"]);
    assert_eq!(
        (int(&v["data"]["entry"]), int(&v["data"]["row_id"])),
        (Some(1), Some(r2))
    );

    let entry_keys = [
        "agent_added",
        "created_at",
        "created_by",
        "evidence_rows",
        "id",
        "mark",
        "modified_at",
        "modified_by",
        "needs_time",
        "reviewed_by",
        "shown",
        "status",
        "tactic",
        "technique_ids",
        "time",
        "time_shown",
        "time_source",
        "time_us",
        "untrusted",
    ];
    let v = ok(
        &p,
        &[
            "entry",
            "edit",
            "1",
            "--title",
            "t2",
            "--description",
            "d",
            "--tactic",
            "Execution",
            "--technique",
            "T1059",
        ],
    );
    assert_eq!(v["command"], "entry edit");
    assert_eq!(keys(&v["data"]), entry_keys);
    assert_eq!(
        (
            v["data"]["id"].as_i64(),
            v["data"]["shown"].as_str(),
            v["data"]["status"].as_str(),
            v["data"]["tactic"].as_str(),
            v["data"]["time"].as_str()
        ),
        (
            Some(1),
            Some("E-1"),
            Some("draft"),
            Some("Execution"),
            Some("2026-01-05T02:14:07.000000Z")
        )
    );
    assert_eq!(v["data"]["technique_ids"], json!(["T1059"]));
    let text = inner(&v["data"]["untrusted"]);
    assert_eq!(
        (text["title"].as_str(), text["description"].as_str()),
        (Some("t2"), Some("d"))
    );

    let v = ok(&p, &["entry", "status", "1", "reviewed"]);
    assert_eq!(v["command"], "entry status");
    assert_eq!(
        (
            v["data"]["id"].as_i64(),
            v["data"]["status"].as_str(),
            v["data"]["reviewed_by"].as_str()
        ),
        (Some(1), Some("reviewed"), Some(ANALYST))
    );
    let v = ok(&p, &["entry", "delete", "2"]);
    assert_eq!(v["data"], json!({"id": 2, "shown": "E-2"}));

    // hosts: the promoted row made WS01 (H-1).
    let v = ok(&p, &["host", "show", "1"]);
    assert_eq!(keys(&v["data"]), with(&HOST_KEYS, &["linked_entries"]));
    assert_eq!(
        (v["data"]["id"].as_i64(), v["data"]["shown"].as_str()),
        (Some(1), Some("H-1"))
    );
    let listed = ok(&p, &["entry", "list"]);
    assert_eq!(
        keys(&v["data"]["linked_entries"][0]),
        keys(&listed["data"]["entries"][0])
    );
    let v = ok(&p, &["host", "list"]);
    assert_eq!(keys(&v["data"]), ["count", "hosts", "untrusted"]);
    let item: Vec<&str> = with(&HOST_KEYS, &[])
        .into_iter()
        .filter(|k| *k != "untrusted")
        .collect();
    assert_eq!(keys(&v["data"]["hosts"][0]), item);
    let v = ok(&p, &["host", "edit", "1", "--role", "Server"]);
    assert_eq!(v["command"], "host edit");
    assert_eq!(keys(&v["data"]), with(&HOST_KEYS, &[]));
    assert_eq!(
        (
            v["data"]["id"].as_i64(),
            v["data"]["shown"].as_str(),
            v["data"]["status"].as_str()
        ),
        (Some(1), Some("H-1"), Some("reviewed"))
    );
    assert_eq!(
        inner(&v["data"]["untrusted"]),
        json!({"id": 1, "name": "WS01", "role": "Server", "note": ""})
    );
    ok(&p, &["host", "add", "--name", "SRV01"]);
    let v = ok(&p, &["host", "merge", "--from", "2", "--into", "1"]);
    assert_eq!(v["data"], json!({"from": 2, "into": 1}));
    let v = ok(&p, &["host", "status", "1", "unreviewed"]);
    assert_eq!(
        v["data"],
        json!({"id": 1, "shown": "H-1", "status": "unreviewed"})
    );
    let v = ok(&p, &["host", "delete", "1"]);
    assert_eq!(v["data"], json!({"id": 1, "shown": "H-1"}));

    // accounts
    let account_keys = [
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
        "privilege",
        "shown",
        "status",
        "untrusted",
    ];
    ok(
        &p,
        &["account", "add", "--name", "jdoe", "--domain", "ACME"],
    );
    ok(&p, &["account", "add", "--name", "other"]);
    let v = ok(&p, &["account", "show", "1"]);
    assert_eq!(keys(&v["data"]), with(&account_keys, &["linked_entries"]));
    assert_eq!(
        (v["data"]["shown"].as_str(), v["data"]["privilege"].as_str()),
        (Some("A-1"), Some("unknown"))
    );
    let v = ok(&p, &["account", "list"]);
    assert_eq!(keys(&v["data"]), ["accounts", "count", "untrusted"]);
    assert_eq!(v["data"]["count"], 2);
    let v = ok(
        &p,
        &[
            "account",
            "edit",
            "1",
            "--privilege",
            "local_admin",
            "--note",
            "n",
        ],
    );
    assert_eq!(v["command"], "account edit");
    assert_eq!(keys(&v["data"]), account_keys);
    assert_eq!(
        (
            v["data"]["id"].as_i64(),
            v["data"]["shown"].as_str(),
            v["data"]["privilege"].as_str(),
            v["data"]["status"].as_str()
        ),
        (Some(1), Some("A-1"), Some("local_admin"), Some("reviewed"))
    );
    let text = inner(&v["data"]["untrusted"]);
    assert_eq!(
        (
            text["name"].as_str(),
            text["domain"].as_str(),
            text["note"].as_str()
        ),
        (Some("jdoe"), Some("ACME"), Some("n"))
    );
    let v = ok(&p, &["account", "merge", "--from", "2", "--into", "1"]);
    assert_eq!(v["data"], json!({"from": 2, "into": 1}));
    let v = ok(&p, &["account", "status", "1", "unreviewed"]);
    assert_eq!(
        v["data"],
        json!({"id": 1, "shown": "A-1", "status": "unreviewed"})
    );
    let v = ok(&p, &["account", "logons", "--id", "1"]);
    assert_eq!(
        keys(&v["data"]),
        ["count", "id", "logons", "unparsed", "untrusted"]
    );
    assert_eq!(v["data"]["id"], 1);
    let logons = v["data"]["logons"].as_array().unwrap();
    assert_eq!(v["data"]["count"], logons.len());
    assert!(!logons.is_empty());
    for l in logons {
        assert_eq!(keys(l), ["count", "index", "logon_type", "outcome"]);
    }
    let v = ok(&p, &["account", "delete", "1"]);
    assert_eq!(v["data"], json!({"id": 1, "shown": "A-1"}));

    // iocs
    let ioc_keys = [
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
        "untrusted",
    ];
    ok(
        &p,
        &["ioc", "add", "--type", "ip", "--value", "203.0.113.7"],
    );
    ok(
        &p,
        &["ioc", "add", "--type", "domain", "--value", "a.acme.local"],
    );
    let v = ok(
        &p,
        &[
            "ioc",
            "edit",
            "1",
            "--tool",
            "rclone",
            "--attack-id",
            "T1567",
        ],
    );
    assert_eq!(v["command"], "ioc edit");
    assert_eq!(keys(&v["data"]), ioc_keys);
    assert_eq!(
        (
            v["data"]["id"].as_i64(),
            v["data"]["shown"].as_str(),
            v["data"]["ioc_type"].as_str(),
            v["data"]["status"].as_str()
        ),
        (Some(1), Some("I-1"), Some("ip"), Some("reviewed"))
    );
    assert_eq!(v["data"]["attack_ids"], json!(["T1567"]));
    let text = inner(&v["data"]["untrusted"]);
    assert_eq!(
        (text["value"].as_str(), text["tool"].as_str()),
        (Some("203.0.113.7"), Some("rclone"))
    );
    let v = ok(&p, &["ioc", "show", "1"]);
    assert_eq!(keys(&v["data"]), with(&ioc_keys, &["linked_entries"]));
    assert_eq!(
        (v["data"]["shown"].as_str(), v["data"]["ioc_type"].as_str()),
        (Some("I-1"), Some("ip"))
    );
    let v = ok(&p, &["ioc", "list"]);
    assert_eq!(keys(&v["data"]), ["count", "iocs", "untrusted"]);
    assert_eq!(v["data"]["count"], 2);
    let v = ok(&p, &["ioc", "sightings", "--id", "1"]);
    assert_eq!(
        keys(&v["data"]),
        ["first", "hits", "id", "last", "untrusted"]
    );
    assert_eq!(v["data"]["id"], 1);
    let v = ok(&p, &["ioc", "merge", "--from", "2", "--into", "1"]);
    assert_eq!(v["data"], json!({"from": 2, "into": 1}));
    let v = ok(&p, &["ioc", "status", "1", "unreviewed"]);
    assert_eq!(
        v["data"],
        json!({"id": 1, "shown": "I-1", "status": "unreviewed"})
    );
    let v = ok(&p, &["ioc", "delete", "1"]);
    assert_eq!(v["data"], json!({"id": 1, "shown": "I-1"}));

    // link, candidates
    ok(&p, &["host", "add", "--name", "WS01"]);
    let v = ok(
        &p,
        &["link", "add", "--entry", "1", "--kind", "host", "--id", "3"],
    );
    assert_eq!(
        v["data"],
        json!({"entry": 1, "kind": "host", "id": 3, "linked": true})
    );
    let v = ok(
        &p,
        &[
            "link", "remove", "--entry", "1", "--kind", "host", "--id", "3",
        ],
    );
    assert_eq!(
        v["data"],
        json!({"entry": 1, "kind": "host", "id": 3, "linked": false})
    );
    let v = ok(&p, &["candidates", "--row", &rs]);
    assert_eq!(keys(&v["data"]), ["candidates", "count", "untrusted"]);
    let found = v["data"]["candidates"].as_array().unwrap();
    assert_eq!(v["data"]["count"], found.len());
    assert!(!found.is_empty());
    for c in found {
        assert_eq!(keys(c), ["existing_id", "index", "ioc_type", "kind"]);
    }

    // exports and history
    let target = d.path().join("shape.csv").to_string_lossy().into_owned();
    let v = ok(&p, &["export", "timeline", &target]);
    assert_eq!(keys(&v["data"]), ["rows", "untrusted"]);
    assert_eq!(v["data"]["rows"], 1);
    for (kind, name) in [("workbook", "shape.xlsx"), ("report", "shape.md")] {
        let target = d.path().join(name).to_string_lossy().into_owned();
        let v = ok(&p, &["export", kind, &target]);
        assert_eq!(v["command"], format!("export {kind}"));
        assert_eq!(keys(&v["data"]), ["rows", "untrusted"]);
        assert!(v["data"]["rows"].as_u64().unwrap() >= 1, "{v}");
        assert!(std::fs::metadata(&target).unwrap().len() > 0);
    }
    let v = ok(&p, &["history", "--kind", "host"]);
    assert_eq!(keys(&v["data"]), ["entries", "untrusted"]);
    assert!(v["data"]["entries"].as_u64().unwrap() >= 3);
}
