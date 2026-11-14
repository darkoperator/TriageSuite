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

/// `needle` occurs in `raw`, and only between the wrapper lines.
fn assert_only_inside(raw: &str, needle: &str) {
    let start = raw.find("<untrusted_data").unwrap();
    let end = raw.rfind("</untrusted_data").unwrap();
    assert!(raw.contains(needle), "{needle} absent from {raw}");
    for (at, _) in raw.match_indices(needle) {
        assert!(at > start && at < end, "{needle} outside the wrapper");
    }
}

#[test]
fn case_new_info_and_load_run() {
    let (_d, p, run) = setup();
    let p = p.as_str();
    assert_eq!(ox(&["case", "new", p, "--name", "IR-1"]).0, 0);
    let (code, v) = ox(&["--case", p, "case", "info"]);
    assert_eq!(
        (code, v["ok"].as_bool(), v["data"]["format"].as_u64()),
        (0, Some(true), Some(3))
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

/// Found on the second real run: removing a source left its space in the
/// file. `case info` says how much is free, and `case compact` writes a
/// copy without it, refusing a path that exists.
#[test]
fn case_info_reports_free_space_and_compact_writes_a_copy() {
    let (d, p, run) = setup();
    let p = p.as_str();
    assert_eq!(ox(&["case", "new", p, "--name", "IR-1"]).0, 0);
    let (_, v) = ox(&["--case", p, "load", "run", &run]);
    let source = v["data"]["source_id"].as_i64().unwrap().to_string();
    assert_eq!(ox(&["--case", p, "source", "remove", &source]).0, 0);
    let (code, v) = ox(&["--case", p, "case", "info"]);
    let (bytes, free) = (
        v["data"]["bytes"].as_u64().unwrap(),
        v["data"]["free_bytes"].as_u64().unwrap(),
    );
    assert!(code == 0 && bytes > 0 && free <= bytes, "{v}");

    let out = d.path().join("copy.case.duckdb");
    let out = out.to_string_lossy();
    let (raw_code, raw) = ox_raw(&["--case", p, "case", "compact", &out]);
    assert_eq!(raw_code, 0, "{raw}");
    assert_only_inside(&raw, "copy.case.duckdb");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["command"], "case compact");
    assert_eq!(
        v["data"]["bytes_after"].as_u64(),
        Some(std::fs::metadata(out.as_ref()).unwrap().len())
    );
    let (code, v) = ox(&["--case", &out, "sources"]);
    assert_eq!((code, v["data"]["sources"].clone()), (0, json!(1)), "{v}");
    // Never over a file that exists.
    let (code, v) = ox(&["--case", p, "case", "compact", &out]);
    assert_eq!(
        (code, v["error"]["code"].clone()),
        (2, json!("invalid")),
        "{v}"
    );
}

/// Found on the second real run: `--case <path> case new --name <name>`
/// was refused for want of a path, though every other command takes the
/// case from `--case`. `case new` now takes it from there too; a path
/// given to `case new` itself still comes first.
#[test]
fn case_new_takes_its_path_from_the_case_option() {
    let d = tempfile::tempdir().unwrap();
    let from_option = d.path().join("option.case.duckdb");
    let from_option = from_option.to_string_lossy();
    let (code, v) = ox(&["--case", &from_option, "case", "new", "--name", "IR-1"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(inner(&v["data"]["untrusted"])["path"], from_option.as_ref());
    assert_eq!(ox(&["--case", &from_option, "case", "info"]).0, 0);

    let given = d.path().join("given.case.duckdb");
    let given = given.to_string_lossy();
    let other = d.path().join("other.case.duckdb");
    let other = other.to_string_lossy();
    let (code, v) = ox(&["--case", &other, "case", "new", &given, "--name", "IR-2"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(inner(&v["data"]["untrusted"])["path"], given.as_ref());
    assert!(!std::path::Path::new(other.as_ref()).exists());

    let (code, v) = ox(&["case", "new", "--name", "IR-3"]);
    assert_eq!(
        (code, v["error"]["code"].clone()),
        (2, json!("invalid")),
        "{v}"
    );
}

#[test]
fn load_run_leaves_out_per_channel_copies_unless_asked() {
    let d = tempfile::tempdir().unwrap();
    for (flag, relations, skipped) in [(None, 1, 2), (Some("--per-channel"), 3, 0)] {
        let p = d
            .path()
            .join(format!("{relations}.case.duckdb"))
            .to_string_lossy()
            .into_owned();
        let run = oxl_fixtures::with_channels(&d.path().join(format!("run{relations}")), true)
            .to_string_lossy()
            .into_owned();
        assert_eq!(ox(&["case", "new", &p, "--name", "C"]).0, 0);
        let mut args = vec!["--case", p.as_str(), "load", "run", run.as_str()];
        args.extend(flag);
        let (raw_code, raw) = ox_raw(&args);
        assert_eq!(raw_code, 0, "{raw}");
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            (
                v["data"]["relations"].as_u64(),
                v["data"]["skipped"].as_u64()
            ),
            (Some(relations), Some(skipped)),
            "{flag:?}: {v}"
        );
        if skipped > 0 {
            // The left-out names carry channel names from the evidence.
            assert_only_inside(&raw, "Individual/Security");
            let list = inner(&v["data"]["untrusted"])["skipped"].clone();
            assert_eq!(list[0]["relation"], "evtxtriage_individual_security");
        }
    }
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
    assert!(v["error"]["message"].is_null());
    assert!(inner(&v["error"]["untrusted"])["message"].is_string());
    let (_, raw) = ox_raw(&["--case", "nope.case.duckdb", "case", "info"]);
    assert_only_inside(&raw, "nope.case.duckdb");
    let (code, raw) = ox_raw(&["<script>x"]);
    assert_eq!(code, 2);
    assert_only_inside(&raw, "<script>x");

    ox(&["case", "new", p, "--name", "IR-1"]);
    ox(&["--case", p, "load", "run", &run]);
    assert_eq!(ox(&["--case", p, "load", "run", &run]).0, 4);
    assert_eq!(ox(&["--case", p, "--as", "root:x", "case", "info"]).0, 2);
    let (code, v) = ox(&["case", "info"]);
    assert_eq!((code, v["error"]["code"].as_str()), (2, Some("invalid")));
}

/// A case written by a newer build is refused with `needs_upgrade`, exit
/// 6, and a message saying it is of a newer format, by a read and by a
/// command that would change it.
#[test]
fn a_format_3_case_refuses_older_binaries_politely() {
    let (_d, p, run) = setup();
    assert_eq!(ox(&["case", "new", &p, "--name", "IR-1"]).0, 0);
    {
        let conn = duckdb::Connection::open(&p).unwrap();
        conn.execute("UPDATE ox.meta SET value = '4' WHERE key = 'format'", [])
            .unwrap();
    }
    for args in [
        vec!["--case", p.as_str(), "case", "info"],
        vec!["--case", p.as_str(), "load", "run", run.as_str()],
    ] {
        let (code, v) = ox(&args);
        assert_eq!(code, 6, "{args:?}: {v}");
        assert_eq!(v["error"]["code"], "needs_upgrade");
        let message = inner(&v["error"]["untrusted"])["message"].clone();
        assert!(
            message.as_str().unwrap().contains("newer format"),
            "{message}"
        );
    }
}

#[test]
fn a_locked_case_reports_case_locked() {
    let (_d, p, _run) = setup();
    ox(&["case", "new", &p, "--name", "IR-1"]);
    let _held = oxl_engine::Case::open(std::path::Path::new(&p)).unwrap();
    let (code, v) = ox(&["--case", &p, "case", "info"]);
    assert_eq!(code, 5);
    assert_eq!(v["error"]["code"], "case_locked");
    let message = inner(&v["error"]["untrusted"])["message"].clone();
    assert!(message.as_str().unwrap().contains("open in another"));
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
    assert_eq!(v["data"]["case_format"], 3);
    assert!(!v["data"]["commands"].as_array().unwrap().is_empty());
    assert!(v["data"]["untrusted_wrapper"]
        .as_str()
        .unwrap()
        .contains("untrusted_data"));
}

#[test]
fn help_and_version_are_envelopes_that_hide_the_environment() {
    for (args, command) in [
        (vec!["--help"], "help"),
        (vec!["case", "info", "--help"], "help"),
        (vec!["--version"], "version"),
    ] {
        let out = assert_cmd::Command::cargo_bin("oxidelens")
            .unwrap()
            .env("OXIDELENS_CASE", "MARKER-IGNORE-PREVIOUS")
            .env("OXIDELENS_ACTOR", "MARKER-ACTOR")
            .arg("--json")
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(!text.contains("MARKER"), "{args:?}: env value leaked");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            (v["ok"].as_bool(), v["command"].as_str()),
            (Some(true), Some(command))
        );
        if command == "help" {
            assert!(inner(&v["data"]["untrusted"])["help"].is_string());
        } else {
            assert!(v["data"]["version"].is_string());
        }
    }
    let (code, v) = ox(&["help"]);
    assert_eq!((code, v["error"]["code"].as_str()), (2, Some("invalid")));
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
