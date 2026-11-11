mod common;

use common::{boundary_of, inner, ox, ox_raw};
use serde_json::json;

fn setup() -> (tempfile::TempDir, String, String) {
    let d = tempfile::tempdir().unwrap();
    let p = d
        .path()
        .join("t.case.duckdb")
        .to_string_lossy()
        .into_owned();
    let run = oxl_fixtures::two_hosts(&d.path().join("run"))
        .to_string_lossy()
        .into_owned();
    (d, p, run)
}

#[test]
fn case_new_info_and_load_run() {
    let (_d, p, run) = setup();
    let p = p.as_str();
    assert_eq!(ox(&["case", "new", p, "--name", "IR-1"]).0, 0);
    let (code, v) = ox(&["--case", p, "case", "info"]);
    assert_eq!(
        (code, v["ok"].as_bool(), v["data"]["format"].as_u64()),
        (0, Some(true), Some(1))
    );
    assert_eq!(inner(&v["data"]["untrusted"])["name"], "IR-1");
    let (code, v) = ox(&["--case", p, "load", "run", &run]);
    assert_eq!(
        (
            code,
            v["data"]["source_id"].as_i64().is_some(),
            v["data"]["relations"].as_u64()
        ),
        (0, true, Some(3))
    );
    assert_eq!(
        inner(&v["data"]["untrusted"])["hosts"],
        json!(["SRV01", "WS01"])
    );
}

#[test]
fn load_file_and_sources() {
    let (d, p, run) = setup();
    let p = p.as_str();
    ox(&["case", "new", p, "--name", "IR-1"]);
    ox(&["--case", p, "load", "run", &run]);
    let fw = oxl_fixtures::loose_fw_csv(d.path());
    let (code, raw) = ox_raw(&["--case", p, "load", "file", &fw.to_string_lossy()]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["data"]["relations"], 1);
    assert_eq!(
        inner(&v["data"]["untrusted"])["relations"][0]["name"],
        "fw_csv"
    );
    // The relation name is evidence-adjacent: only inside the wrapper.
    let outside = raw.split("<untrusted_data").next().unwrap();
    assert!(!outside.contains("fw_csv"));

    let (code, v) = ox(&["--case", p, "sources"]);
    assert_eq!(code, 0);
    assert_eq!(v["data"]["sources"], 2);
    let list = inner(&v["data"]["untrusted"]);
    let kinds: Vec<_> = list["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, ["run", "file"]);
}

#[test]
fn errors_use_the_envelope_and_exit_codes() {
    let (_d, p, run) = setup();
    let p = p.as_str();
    let (code, v) = ox(&["--case", "nope.case.duckdb", "case", "info"]);
    assert_eq!(code, 3);
    assert_eq!(v["ok"], false);
    assert_eq!(v["command"], "case info");
    assert_eq!(v["error"]["code"], "not_found");
    assert!(v["error"]["message"].is_string());

    ox(&["case", "new", p, "--name", "IR-1"]);
    ox(&["--case", p, "load", "run", &run]);
    assert_eq!(ox(&["--case", p, "load", "run", &run]).0, 4);
    assert_eq!(ox(&["--case", p, "--as", "root:x", "case", "info"]).0, 2);
    let (code, v) = ox(&["case", "info"]);
    assert_eq!((code, v["error"]["code"].as_str()), (2, Some("invalid")));
}

#[test]
fn a_locked_case_reports_case_locked() {
    let (_d, p, _run) = setup();
    ox(&["case", "new", &p, "--name", "IR-1"]);
    let _held = oxl_engine::Case::open(std::path::Path::new(&p)).unwrap();
    let (code, v) = ox(&["--case", &p, "case", "info"]);
    assert_eq!(code, 5);
    assert_eq!(v["error"]["code"], "case_locked");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("open in another"));
}

#[test]
fn the_untrusted_boundary_changes_every_call_and_wraps_evidence() {
    let (_d, p, run) = setup();
    let p = p.as_str();
    ox(&["case", "new", p, "--name", "IR-1"]);
    ox(&["--case", p, "load", "run", &run]);
    let (_, a) = ox(&["--case", p, "sources"]);
    let (_, b) = ox(&["--case", p, "sources"]);
    let (ba, bb) = (
        boundary_of(&a["data"]["untrusted"]),
        boundary_of(&b["data"]["untrusted"]),
    );
    assert_eq!(ba.len(), 16);
    assert_ne!(ba, bb);

    let (_, raw) = ox_raw(&["--case", p, "sources"]);
    let start = raw.find("<untrusted_data").unwrap();
    let end = raw.rfind("</untrusted_data").unwrap();
    for (at, _) in raw.match_indices("WS01") {
        assert!(at > start && at < end, "WS01 outside the wrapper");
    }
    assert!(raw.contains("WS01"));
}

#[test]
fn an_agent_cannot_remove_a_source() {
    let (_d, p, run) = setup();
    let p = p.as_str();
    ox(&["case", "new", p, "--name", "IR-1"]);
    ox(&["--case", p, "load", "run", &run]);
    assert_eq!(
        ox(&["--case", p, "--as", "agent:claude", "source", "remove", "1"]).0,
        4
    );
    let (code, v) = ox(&["--case", p, "source", "remove", "1"]);
    assert_eq!(code, 0);
    assert!(v["data"]["orphaned_marks"].is_u64());
}

#[test]
fn capabilities_needs_no_case_and_lists_limits() {
    let (code, v) = ox(&["capabilities"]);
    assert_eq!(code, 0);
    assert_eq!(v["data"]["limits"]["rows_max"], 1000);
    assert_eq!(v["data"]["case_format"], 1);
    assert!(!v["data"]["commands"].as_array().unwrap().is_empty());
    assert!(v["data"]["untrusted_wrapper"]
        .as_str()
        .unwrap()
        .contains("untrusted_data"));
}

#[test]
fn without_json_the_output_is_pretty_printed() {
    let out = assert_cmd::Command::cargo_bin("oxidelens")
        .unwrap()
        .arg("capabilities")
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.lines().count() > 1);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["data"]["limits"]["rows_max"], 1000);
}
