//! Contract tests: promises the whole CLI makes, checked over every command.

mod common;

use common::{inner, ox, ox_plain, ox_raw};
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

/// Stand-ins in `calls` for ids a reply gives: the `note add` reply's id and
/// the last source `sources` lists. `run_calls` replaces them just before a call.
const NOTE_ID: &str = "{note id}";
const SOURCE_ID: &str = "{source id}";

/// The id of the newest source the case's own `sources` reply lists as not
/// removed.
fn newest_source(p: &str) -> i64 {
    let (_, v) = ox(&["--case", p, "sources"]);
    let list = inner(&v["data"]["untrusted"])["sources"].clone();
    list.as_array()
        .unwrap()
        .iter()
        .filter(|s| s["removed"] == false)
        .map(|s| s["id"].as_i64().unwrap())
        .max()
        .unwrap()
}

/// Run each call in `calls` in order, with the stand-in ids replaced by what
/// the case's own replies gave. `check` sees the arguments, the exit code and
/// the raw output.
fn run_calls(p: &str, dir: &std::path::Path, mut check: impl FnMut(&[String], i32, &str)) {
    let mut note_id = "1".to_string();
    for mut args in calls(p, dir) {
        for a in args.iter_mut() {
            if a == NOTE_ID {
                *a = note_id.clone();
            } else if a == SOURCE_ID {
                *a = newest_source(p).to_string();
            }
        }
        let mut full = vec!["--case", p];
        full.extend(args.iter().map(String::as_str));
        let (code, raw) = ox_raw(&full);
        if args.len() > 1 && args[..2] == ["note", "add"] && code == 0 {
            let v: Value = serde_json::from_str(&raw).unwrap();
            note_id = v["data"]["id"].as_i64().unwrap().to_string();
        }
        check(&args, code, &raw);
    }
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
    // A fresh name per call list: compacting never writes over a file.
    let compact = (0..)
        .map(|i| dir.join(format!("compact{i}.case.duckdb")))
        .find(|p| !p.exists())
        .unwrap();
    let (rerun, fw_two, new_case, compact) = (
        rerun.to_string_lossy().into_owned(),
        fw_two.to_string_lossy().into_owned(),
        new_case.to_string_lossy().into_owned(),
        compact.to_string_lossy().into_owned(),
    );
    let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let ev = "evtxtriage_events";
    vec![
        s(&["case", "new", &new_case, "--name", "WS01 second case"]),
        s(&["case", "info"]),
        s(&["case", "compact", &compact]),
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
        s(&["note", "delete", NOTE_ID]),
        s(&["source", "remove", SOURCE_ID]),
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
    run_calls(&p, d.path(), |args, code, raw| {
        let v: Value = serde_json::from_str(raw).unwrap();
        assert_eq!(code, 0, "{args:?} -> {v}");
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["command", "data", "ok", "truncated"], "{args:?}");
        exercised.insert(path_of(args, &known));
        assert_eq!(v["ok"], true, "{args:?}");
        let name = v["command"].as_str().unwrap().to_string();
        assert!(known.contains(&name), "{args:?} -> {name}");
    });
    // A command added without a contract call here would pass every test in
    // this file unchecked, so the calls must cover clap's whole tree.
    let all: std::collections::BTreeSet<String> = known.into_iter().collect();
    assert_eq!(exercised, all, "commands with no contract call, or unknown");
}

/// The commands that need the case named by `--case` to themselves: every
/// one that changes it, and `case compact`, which only reads it but writes
/// its copy from a case opened that way. Every other command only reads it
/// (`case new` writes a different file; `capabilities` opens none).
const CHANGES_THE_CASE: &[&str] = &[
    "case compact",
    "load run",
    "load file",
    "source remove",
    "mark set",
    "mark clear",
    "tag add",
    "tag remove",
    "note add",
    "note delete",
    "column declare",
    "column primary-time",
];

/// Found on the first real run: every command opened the case for writing,
/// so of four `find` calls started together three reported `case_locked`.
#[test]
fn a_command_that_only_reads_runs_while_the_case_is_read_elsewhere() {
    let (d, p) = loaded();
    let known = oxl_cli::command_paths();
    for path in CHANGES_THE_CASE {
        assert!(
            known.contains(&path.to_string()),
            "{path} is no known command"
        );
    }

    // This process reads the case for as long as the commands below run.
    let _reader = oxl_engine::Case::open_read_only(std::path::Path::new(&p)).unwrap();
    run_calls(&p, d.path(), |args, code, raw| {
        let v: Value = serde_json::from_str(raw).unwrap();
        if CHANGES_THE_CASE.contains(&path_of(args, &known).as_str()) {
            // A change needs the case to itself, and says so.
            assert_eq!(
                (code, v["error"]["code"].as_str()),
                (5, Some("case_locked")),
                "{args:?} -> {v}"
            );
        } else {
            assert_eq!(code, 0, "{args:?} -> {v}");
        }
    });
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
    run_calls(&p, d.path(), |args, code, raw| {
        assert_eq!(code, 0, "{args:?}: {raw}");
        assert_clean(&args.join(" "), raw);
        checked += 1;
    });
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

/// The ids `note delete` and `source remove` take come from the replies that
/// gave them, never from a count of what the call list did before.
#[test]
fn contract_ids_come_from_replies() {
    let (d, p) = loaded();
    let mut note_added = None;
    let mut deleted = None;
    let mut listed = None;
    let mut removed = None;
    run_calls(&p, d.path(), |args, code, raw| {
        assert_eq!(code, 0, "{args:?}: {raw}");
        let reply: Value = serde_json::from_str(raw).unwrap();
        match (args[0].as_str(), args.get(1).map(String::as_str)) {
            // The id the `note add` reply gave.
            ("note", Some("add")) => note_added = reply["data"]["id"].as_i64(),
            ("note", Some("delete")) => {
                deleted = Some(args[2].clone());
                // The `sources` reply taken just before the removal that
                // follows in the call list.
                listed = Some(newest_source(&p));
            }
            ("source", Some("remove")) => removed = Some(args[2].clone()),
            _ => {}
        }
    });
    let note_added = note_added.expect("a note was added");
    assert_eq!(deleted, Some(note_added.to_string()));
    let listed = listed.expect("sources were listed before the removal");
    assert_eq!(removed, Some(listed.to_string()));
}

#[test]
fn usage_errors_follow_the_compact_flag() {
    let (code, raw) = ox_plain(&["nonsense"]);
    assert_eq!(code, 2);
    assert!(raw.trim_end().lines().count() > 1, "{raw}");
    let (code, raw) = ox_plain(&["--json", "nonsense"]);
    assert_eq!(code, 2);
    assert_eq!(raw.trim_end().lines().count(), 1, "{raw}");
}

#[test]
fn usage_error_message_has_one_prefix() {
    let (code, raw) = ox_raw(&["nonsense"]);
    assert_eq!(code, 2);
    let v: Value = serde_json::from_str(&raw).unwrap();
    let message = inner(&v["error"]["untrusted"])["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(message.starts_with("invalid: "), "{message}");
    assert!(!message.contains("error: "), "{message}");
}

/// The trigger exists in debug builds only, which is what the tests run.
#[cfg(debug_assertions)]
#[test]
fn a_panic_prints_an_engine_envelope() {
    let out = assert_cmd::Command::cargo_bin("oxidelens")
        .unwrap()
        .env("OXIDELENS_TEST_PANIC", "1")
        .args(["--json", "capabilities"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(10));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.trim_end().lines().count(), 1, "{stdout}");
    let v: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["command"], "capabilities");
    assert_eq!(v["error"]["code"], "engine");
    assert!(inner(&v["error"]["untrusted"])["message"].is_string());
}

/// The random source failing while the panic reply is built (forced by a
/// debug-only switch) still ends in one envelope, with the fixed fallback
/// boundary and no panic text, instead of a second panic and empty stdout.
#[cfg(debug_assertions)]
#[test]
fn a_panic_in_the_panic_hook_still_prints_an_envelope() {
    let out = assert_cmd::Command::cargo_bin("oxidelens")
        .unwrap()
        .env("OXIDELENS_TEST_PANIC", "1")
        .env("OXIDELENS_TEST_BOUNDARY_FAIL", "1")
        .args(["--json", "capabilities"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(10));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.trim_end().lines().count(), 1, "{stdout}");
    let v: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["command"], "capabilities");
    assert_eq!(v["error"]["code"], "engine");
    assert_eq!(
        common::boundary_of(&v["error"]["untrusted"]),
        oxl_cli::envelope::FALLBACK_BOUNDARY
    );
    let message = inner(&v["error"]["untrusted"])["message"]
        .as_str()
        .unwrap()
        .to_string();
    // The fixed boundary is predictable, so the panic's own text stays out.
    assert!(!message.contains("OXIDELENS_TEST_PANIC"), "{message}");
}

#[test]
fn capabilities_lists_error_codes() {
    let (_, v) = ox(&["capabilities"]);
    let codes = v["data"]["error_codes"].as_array().unwrap();
    let find = |code: &str| {
        codes
            .iter()
            .find(|c| c["code"] == code)
            .unwrap_or_else(|| panic!("{code} is not listed"))
    };
    for (code, exit) in [
        ("invalid", 2),
        ("not_found", 3),
        ("refused", 4),
        ("case_locked", 5),
        ("needs_upgrade", 6),
        ("engine", 10),
        ("io", 10),
        ("internal", 11),
    ] {
        assert_eq!(find(code)["exit"], exit, "{code}");
    }
    assert_eq!(codes.len(), 8);
    let meaning = |code: &str| find(code)["meaning"].as_str().unwrap().to_string();
    assert!(
        meaning("invalid").contains("input"),
        "{}",
        meaning("invalid")
    );
    assert!(
        meaning("refused").contains("rule"),
        "{}",
        meaning("refused")
    );
}

#[test]
fn capabilities_states_rows_min() {
    let (_, v) = ox(&["capabilities"]);
    assert_eq!(v["data"]["limits"]["rows_min"], 1);
}

fn describe(v: &Value, name: &str) -> String {
    v["data"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap()["description"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn capabilities_semantics_wording() {
    let (_, v) = ox(&["capabilities"]);
    let declare = describe(&v, "column declare");
    assert!(
        declare
            .contains("--semantics is required for TIMESTAMP (without it the call is `invalid`)"),
        "{declare}"
    );
    let wrapper = v["data"]["untrusted_wrapper"].as_str().unwrap();
    assert!(wrapper.contains("caller-written"), "{wrapper}");
    assert!(wrapper.contains("[A-Za-z0-9._:-]"), "{wrapper}");
    assert!(
        !wrapper.contains("produced by the tool itself"),
        "{wrapper}"
    );
}

#[test]
fn capabilities_states_the_one_row_and_ai_tag_rules() {
    let (_, v) = ox(&["capabilities"]);
    let mark = describe(&v, "mark set");
    assert!(
        mark.contains("An agent may mark at most 1,000 rows per call (a row id given twice counts once toward that)"),
        "{mark}"
    );
    let tag = describe(&v, "tag add");
    assert!(
        tag.contains(
            "A row id given twice counts once, and counts once toward an agent's per-call cap"
        ),
        "{tag}"
    );
    assert!(describe(&v, "tag add").contains("at least one character after ai:"));
}
