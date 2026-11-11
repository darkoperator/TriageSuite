//! Contract tests: promises the whole CLI makes, checked over every command.

mod common;

use common::{inner, ox, ox_raw};
use serde_json::Value;

fn loaded() -> (tempfile::TempDir, String) {
    let d = tempfile::tempdir().unwrap();
    let p = d
        .path()
        .join("t.case.duckdb")
        .to_string_lossy()
        .into_owned();
    let run = oxl_fixtures::two_hosts(&d.path().join("run"));
    let loose = d.path().join("loose");
    std::fs::create_dir_all(&loose).unwrap();
    let fw = oxl_fixtures::loose_fw_csv(&loose);
    assert_eq!(ox(&["case", "new", &p, "--name", "T"]).0, 0);
    assert_eq!(
        ox(&["--case", &p, "load", "run", &run.to_string_lossy()]).0,
        0
    );
    assert_eq!(
        ox(&["--case", &p, "load", "file", &fw.to_string_lossy()]).0,
        0
    );
    (d, p)
}

fn first_row(p: &str, search: &str) -> String {
    let (_, v) = ox(&["--case", p, "rows", "evtxtriage_events", "--search", search]);
    v["data"]["row_ids"][0].as_i64().unwrap().to_string()
}

/// One representative successful call per command, in an order that works
/// (a mark exists before it is listed, a note before it is deleted).
/// `(arguments after --case <p>, an actor if not the default)`.
fn calls(p: &str, dir: &std::path::Path) -> Vec<Vec<String>> {
    let r = first_row(p, "svch0st");
    // A prefetch row of the first run: the later run loaded below supersedes it.
    let (_, pf) = ox(&["--case", p, "rows", "petriage_timeline", "--host", "WS01"]);
    let old = pf["data"]["row_ids"][0].as_i64().unwrap().to_string();
    let rerun = oxl_fixtures::rerun_ws01_prefetch(&dir.join("rerun"));
    let loose = dir.join("loose");
    let fw_two = loose.join("fw_two.csv");
    std::fs::copy(loose.join("fw.csv"), &fw_two).unwrap();
    let new_case = dir.join("second.case.duckdb");
    let (rerun, fw_two, new_case) = (
        rerun.to_string_lossy().into_owned(),
        fw_two.to_string_lossy().into_owned(),
        new_case.to_string_lossy().into_owned(),
    );
    let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let ev = "evtxtriage_events";
    vec![
        s(&["case", "new", &new_case, "--name", "WS01 second case"]),
        s(&["case", "info"]),
        s(&["sources"]),
        s(&["load", "run", &rerun]),
        s(&[
            "load", "file", &fw_two, "--name", "fw_two", "--host", "WS01",
        ]),
        s(&["mark", "set", "--rows", &old]),
        s(&["capabilities"]),
        s(&["schema"]),
        s(&["schema", ev]),
        s(&["rows", ev, "--columns", "EventId,UserName"]),
        s(&["count", ev]),
        s(&["groups", ev, "--by", "UserName"]),
        s(&["histogram", ev]),
        s(&["stack", ev, "--by", "UserName,RemoteHost"]),
        s(&["find", "203.0.113.7"]),
        s(&["find", "203.0.113.7", "--rows"]),
        s(&["around", "--row", &r, "--window", "5m"]),
        s(&["around", "--row", &r, "--window", "5m", "--rows"]),
        s(&["row", &r]),
        s(&["query", "SELECT 1 AS one"]),
        s(&["mark", "set", "--rows", &r]),
        s(&["mark", "list"]),
        s(&["tag", "add", "--rows", &r, "--tag", "triage"]),
        s(&["tag", "list"]),
        s(&["tag", "remove", "--rows", &r, "--tag", "triage"]),
        s(&[
            "note",
            "add",
            "--row",
            &r,
            "--text",
            "svc_backup on WS01 from 203.0.113.7",
        ]),
        s(&["note", "list", "--row", &r]),
        s(&["note", "delete", "1"]),
        s(&["source", "remove", "4"]),
        s(&[
            "column",
            "declare",
            "ext_hayabusa_csv_timeline",
            "EventID",
            "--type",
            "BIGINT",
        ]),
        s(&["column", "primary-time", "fw_csv", "--none"]),
        s(&["history"]),
        s(&["mark", "clear", "--rows", &r]),
    ]
}

#[test]
fn every_command_is_listed_in_capabilities_and_has_help() {
    let (c, v) = ox(&["capabilities"]);
    assert_eq!(c, 0);
    let listed: Vec<String> = v["data"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    let known = oxl_cli::command_paths();
    assert!(known.len() > 20, "{known:?}");
    for path in &known {
        assert!(listed.contains(path), "{path} is not in capabilities");
    }
    for name in &listed {
        assert!(known.contains(name), "capabilities lists unknown {name}");
    }
    for path in &known {
        let mut args: Vec<&str> = path.split(' ').collect();
        args.push("--help");
        let (code, help) = ox(&args);
        assert_eq!(code, 0, "{path} --help");
        assert_eq!(help["ok"], true, "{path} --help");
        assert!(help["data"]["untrusted"].is_string(), "{path} --help");
    }
}

#[test]
fn every_successful_command_emits_the_envelope_keys() {
    let (d, p) = loaded();
    let known = oxl_cli::command_paths();
    let mut exercised = std::collections::BTreeSet::new();
    for args in calls(&p, d.path()) {
        let mut full = vec!["--case", p.as_str()];
        full.extend(args.iter().map(String::as_str));
        let (code, v) = ox(&full);
        assert_eq!(code, 0, "{args:?} -> {v}");
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["command", "data", "ok", "truncated"], "{args:?}");
        exercised.insert(path_of(&args, &known));
        assert_eq!(v["ok"], true, "{args:?}");
        let name = v["command"].as_str().unwrap().to_string();
        assert!(known.contains(&name), "{args:?} -> {name}");
    }
    // A command added without a contract call here would pass every test in
    // this file unchecked, so the calls must cover clap's whole tree.
    let all: std::collections::BTreeSet<String> = known.into_iter().collect();
    assert_eq!(exercised, all, "commands with no contract call, or unknown");
}

/// The command path an argument list runs: the known path its leading words spell.
fn path_of(args: &[String], known: &[String]) -> String {
    known
        .iter()
        .find(|k| {
            let words: Vec<&str> = k.split(' ').collect();
            args.len() >= words.len() && words.iter().zip(args).all(|(w, a)| w == a)
        })
        .unwrap_or_else(|| panic!("{args:?} is no known command"))
        .clone()
}

/// Remove every `<untrusted_data …>…</untrusted_data …>` block.
fn strip_wrappers(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<untrusted_data boundary=") {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let boundary_end = tail.find('>').expect("an opening marker ends");
        let boundary = &tail["<untrusted_data boundary=".len()..boundary_end];
        let close = format!("</untrusted_data boundary={boundary}>");
        let end = tail.find(&close).expect("every wrapper is closed") + close.len();
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

const MARKERS: &[&str] = &[
    "svc_backup",
    "203.0.113.7",
    "svch0st",
    "WS01",
    "SRV01",
    "Security.evtx",
    "fw_csv",
    "petriage_timeline",
    "evtxtriage_events",
];

fn assert_clean(label: &str, raw: &str) {
    let outside = strip_wrappers(raw);
    for marker in MARKERS {
        assert!(
            !outside.contains(marker),
            "{label}: {marker:?} escaped the wrapper:\n{outside}"
        );
    }
}

#[test]
fn no_evidence_text_escapes_the_wrapper() {
    let (d, p) = loaded();
    let mut checked = 0;
    for args in calls(&p, d.path()) {
        let mut full = vec!["--case", p.as_str()];
        full.extend(args.iter().map(String::as_str));
        let (code, raw) = ox_raw(&full);
        assert_eq!(code, 0, "{args:?}: {raw}");
        assert_clean(&args.join(" "), &raw);
        checked += 1;
    }
    assert!(checked > 20);

    // The same calls as an agent: its own reasons and labels stay clean too.
    let r = first_row(&p, "svch0st");
    for args in [
        vec!["mark", "set", "--rows", r.as_str()],
        vec!["mark", "clear", "--rows", r.as_str()],
        vec!["mark", "list", "--by", "agent:claude"],
        vec!["tag", "add", "--rows", r.as_str(), "--tag", "ai:x"],
    ] {
        let mut full = vec!["--case", p.as_str(), "--as", "agent:claude"];
        full.extend(args.iter().copied());
        let (_, raw) = ox_raw(&full);
        assert_clean(&args.join(" "), &raw);
    }

    // Errors: the message quotes the evidence-shaped name, inside the wrapper.
    for name in ["svc_backup", "fw_csv_missing", "WS01"] {
        let (code, raw) = ox_raw(&["--case", &p, "rows", name]);
        assert_eq!(code, 3, "{raw}");
        assert_clean(&format!("rows {name}"), &raw);
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert!(inner(&v["error"]["untrusted"])["message"]
            .as_str()
            .unwrap()
            .contains(name));
    }
    let (code, raw) = ox_raw(&["--case", &p, "schema", "svch0st"]);
    assert_eq!(code, 3);
    assert_clean("schema svch0st", &raw);
    let (_, raw) = ox_raw(&["--case", &p, "find", "x", "--mode", "WS01"]);
    assert_clean("usage error", &raw);

    // Help: argv echo and argument names are inside the wrapper.
    for args in [
        &["--help"][..],
        &["rows", "--help"],
        &["mark", "set", "--help"],
    ] {
        let (code, raw) = ox_raw(args);
        assert_eq!(code, 0);
        assert_clean(&args.join(" "), &raw);
    }
}
