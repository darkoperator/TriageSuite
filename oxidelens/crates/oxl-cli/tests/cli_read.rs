mod common;

use common::{inner, ox};
use serde_json::{json, Value};

/// A case with the two-host run loaded; the directory keeps it alive.
fn loaded() -> (tempfile::TempDir, String) {
    let d = tempfile::tempdir().unwrap();
    let p = d
        .path()
        .join("t.case.duckdb")
        .to_string_lossy()
        .into_owned();
    let run = oxl_fixtures::two_hosts(&d.path().join("run"));
    assert_eq!(ox(&["case", "new", &p, "--name", "T"]).0, 0);
    let (code, _) = ox(&["--case", &p, "load", "run", &run.to_string_lossy()]);
    assert_eq!(code, 0);
    (d, p)
}

fn ids(v: &Value) -> Vec<i64> {
    v["data"]["row_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i.as_i64().unwrap())
        .collect()
}

/// The row id of the WS01 Sysmon process creation of svch0st.exe.
fn svch0st_row(p: &str) -> i64 {
    let (_, v) = ox(&[
        "--case",
        p,
        "rows",
        "evtxtriage_events",
        "--search",
        "svch0st",
        "--sort",
        "TimeCreated",
        "--columns",
        "EventId,MapDescription",
    ]);
    let first = ids(&v)[0];
    assert_eq!(
        inner(&v["data"]["untrusted"])["cells"][0],
        json!(["1", "Process creation"])
    );
    first
}

fn assert_invalid(args: &[&str]) {
    let (code, v) = ox(args);
    assert_eq!(code, 2, "{args:?} -> {v}");
    assert_eq!(v["ok"], false, "{args:?}");
    assert_eq!(v["error"]["code"], "invalid", "{args:?}");
}

#[test]
fn pivots_say_how_their_times_are_zoned() {
    let (d, p) = loaded();
    let fw = oxl_fixtures::loose_fw_csv(d.path());
    assert_eq!(
        ox(&[
            "--case",
            &p,
            "load",
            "file",
            &fw.to_string_lossy(),
            "--host",
            "WS01"
        ])
        .0,
        0
    );
    let at = [
        "--host",
        "WS01",
        "--at",
        "2026-01-05T02:15:44Z",
        "--window",
        "5m",
    ];
    let mut around = vec!["--case", p.as_str(), "around"];
    around.extend(at);
    let (c, v) = ox(&around);
    assert_eq!(c, 0, "{v}");
    let relations: Vec<String> = inner(&v["data"]["untrusted"])["counts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["relation"].as_str().unwrap().to_string())
        .collect();
    let zones = v["data"]["time_semantics"].as_array().unwrap().clone();
    assert_eq!(zones.len(), relations.len());
    let zone_of = |rel: &str| zones[relations.iter().position(|r| r == rel).unwrap()].clone();
    assert_eq!(zone_of("fw_csv"), "unknown");
    assert_eq!(zone_of("evtxtriage_events"), "utc");

    around.push("--rows");
    let (_, v) = ox(&around);
    let zones = v["data"]["time_semantics"].as_array().unwrap();
    assert_eq!(zones.len(), ids(&v).len());
    assert!(zones.contains(&json!("unknown")) && zones.contains(&json!("utc")));

    let (_, v) = ox(&["--case", &p, "find", "203.0.113.7"]);
    let zones = v["data"]["time_semantics"].as_array().unwrap();
    assert_eq!(zones.len() as u64, v["data"]["hits"].as_u64().unwrap());
    assert!(zones.contains(&json!("unknown")) && zones.contains(&json!("utc")));
    let (_, v) = ox(&["--case", &p, "find", "203.0.113.7", "--rows"]);
    assert_eq!(
        v["data"]["time_semantics"].as_array().unwrap().len(),
        ids(&v).len()
    );

    let (_, v) = ox(&["--case", &p, "histogram", "fw_csv"]);
    assert_eq!(v["data"]["time_semantics"], "unknown");
    let (_, v) = ox(&[
        "--case",
        &p,
        "stack",
        "evtxtriage_events",
        "--by",
        "Channel",
    ]);
    assert_eq!(v["data"]["time_semantics"], "utc");
}

#[test]
fn groups_and_stack_clip_long_keys_and_say_so() {
    let (d, p) = loaded();
    let csv = d.path().join("long.csv");
    std::fs::write(&csv, format!("a,b\n{},1\nshort,2\n", "k".repeat(5_000))).unwrap();
    assert_eq!(
        ox(&["--case", &p, "load", "file", &csv.to_string_lossy()]).0,
        0
    );
    let (c, g) = ox(&["--case", &p, "groups", "long_csv", "--by", "a"]);
    assert_eq!(c, 0, "{g}");
    assert_eq!(g["data"]["cells_clipped"], true);
    let groups = inner(&g["data"]["untrusted"])["groups"].clone();
    let longest = groups
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|g| g["key"].as_str())
        .map(|k| k.chars().count())
        .max();
    assert_eq!(longest, Some(2_000));
    let (_, s) = ox(&["--case", &p, "stack", "long_csv", "--by", "a"]);
    assert_eq!(s["data"]["cells_clipped"], true);
    let (_, s) = ox(&["--case", &p, "stack", "long_csv", "--by", "b"]);
    assert_eq!(s["data"]["cells_clipped"], false);
    let (_, g) = ox(&["--case", &p, "groups", "long_csv", "--by", "b"]);
    assert_eq!(g["data"]["cells_clipped"], false);
}

#[test]
fn rows_with_filter_search_sort_and_limit() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let (c, v) = ox(&[
        "--case",
        p,
        "rows",
        "evtxtriage_events",
        "--filter",
        r#"{"column":"Channel","op":"eq","value":"Security"}"#,
        "--search",
        "svc_backup",
        "--host",
        "WS01",
        "--sort",
        "EventId:desc",
        "--columns",
        "EventId,UserName",
    ]);
    assert_eq!(
        (
            c,
            v["truncated"].as_bool(),
            v["data"]["row_ids"].as_array().unwrap().len()
        ),
        (0, Some(false), 3)
    );
    assert_eq!(
        inner(&v["data"]["untrusted"])["cells"][0],
        json!(["4624", "svc_backup"])
    );
    assert_eq!(v["data"]["cells_clipped"], false);
}

#[test]
fn rows_reports_truncation_and_failed_cells() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let (_, v) = ox(&["--case", p, "rows", "evtxtriage_events", "--limit", "4"]);
    assert_eq!(v["truncated"], true);
    assert_eq!(ids(&v).len(), 4);
    let (_, v) = ox(&["--case", p, "rows", "evtxtriage_events"]);
    assert_eq!(v["truncated"], false);
    assert_eq!(ids(&v).len(), 11);
    let failed = inner(&v["data"]["untrusted"])["failed"].clone();
    assert_eq!(failed.as_array().unwrap().len(), 1, "{failed}");
    assert_eq!(failed[0].as_array().unwrap().len(), 2);
}

#[test]
fn a_filter_can_come_from_a_file() {
    let (d, p) = loaded();
    let p = p.as_str();
    let doc = r#"{"column":"Channel","op":"eq","value":"Sysmon"}"#;
    let file = d.path().join("f.json");
    std::fs::write(&file, doc).unwrap();
    let at_file = format!("@{}", file.display());
    let inline = ox(&["--case", p, "rows", "evtxtriage_events", "--filter", doc]).1;
    let from_file = ox(&[
        "--case",
        p,
        "rows",
        "evtxtriage_events",
        "--filter",
        &at_file,
    ])
    .1;
    assert_eq!(ids(&inline).len(), 3);
    assert_eq!(ids(&inline), ids(&from_file));
    // A file that is not there is the caller's mistake.
    let missing = format!("@{}", d.path().join("nope.json").display());
    assert_invalid(&[
        "--case",
        p,
        "rows",
        "evtxtriage_events",
        "--filter",
        &missing,
    ]);
}

#[test]
fn count_groups_histogram_and_stack() {
    let (_d, p) = loaded();
    let p = p.as_str();
    assert_eq!(
        ox(&[
            "--case",
            p,
            "count",
            "evtxtriage_events",
            "--search",
            "svch0st"
        ])
        .1["data"]["count"],
        2
    );
    let (_, v) = ox(&[
        "--case",
        p,
        "groups",
        "evtxtriage_events",
        "--by",
        "Channel",
    ]);
    assert_eq!(
        inner(&v["data"]["untrusted"])["groups"][0],
        json!({"key":"Security","count":8,"first":"2026-01-05T02:13:58.204000Z",
               "last":"2026-01-05T02:33:20.461000Z","suspicious":0})
    );
    assert_eq!(v["truncated"], false);
    // A parent key selects the next level down.
    let (_, v) = ox(&[
        "--case",
        p,
        "groups",
        "evtxtriage_events",
        "--by",
        "Channel,EventId",
        "--parent",
        r#"["Sysmon"]"#,
    ]);
    assert_eq!(v["data"]["groups"], 2);
    // The list is cut at the limit.
    let (_, v) = ox(&[
        "--case",
        p,
        "groups",
        "evtxtriage_events",
        "--by",
        "EventId",
        "--limit",
        "2",
    ]);
    assert_eq!(
        (v["truncated"].as_bool(), v["data"]["groups"].as_u64()),
        (Some(true), Some(2))
    );
    let h = ox(&["--case", p, "histogram", "evtxtriage_events"]).1;
    assert_eq!(h["data"]["bucket_seconds"], 60);
    assert_eq!(inner(&h["data"]["untrusted"])["column"], "TimeCreated");
    assert!(h["data"]["buckets"][0]["count"].as_u64().is_some());
    let s = ox(&[
        "--case",
        p,
        "stack",
        "petriage_timeline",
        "--by",
        "ExecutableName",
    ])
    .1;
    assert_eq!(s["data"]["hosts_total"], 2);
    assert_eq!(s["data"]["distinct"], 4);
    let rows = &inner(&s["data"]["untrusted"])["rows"];
    assert_eq!(rows.as_array().unwrap().len(), 4);
    // Rarest first: the programs seen on one host before the one on both.
    assert_eq!(rows[3]["values"], json!(["CHROME.EXE"]));
    assert_eq!(rows[3]["hosts"], 2);
    let s = ox(&[
        "--case",
        p,
        "stack",
        "petriage_timeline",
        "--by",
        "ExecutableName",
        "--order",
        "commonest",
    ])
    .1;
    assert_eq!(
        inner(&s["data"]["untrusted"])["rows"][0]["values"],
        json!(["CHROME.EXE"])
    );
}

#[test]
fn find_summary_and_rows() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let (c, v) = ox(&["--case", p, "find", "203.0.113.7"]);
    assert_eq!(c, 0);
    assert_eq!(v["data"]["hits"], 1);
    assert_eq!(v["data"]["cancelled"], false);
    let body = inner(&v["data"]["untrusted"]);
    assert_eq!(body["hits"][0]["count"], 3);
    assert_eq!(body["hits"][0]["relation"], "evtxtriage_events");
    let (_, v) = ox(&["--case", p, "find", "203.0.113.7", "--rows"]);
    assert_eq!(ids(&v).len(), 3);
    assert_eq!(v["data"]["marks"].as_array().unwrap().len(), 3);
    let rows = &inner(&v["data"]["untrusted"])["rows"];
    assert_eq!(rows[0]["host"], "WS01");
    assert!(rows[0]["summary"].is_string());
}

#[test]
fn find_contains_also_matches_inside_longer_text() {
    let (_d, p) = loaded();
    // RemoteHost matches exactly; the Hayabusa Details text only contains it.
    let (_, v) = ox(&["--case", &p, "find", "203.0.113.7", "--mode", "contains"]);
    assert_eq!(v["data"]["hits"], 2);
}

#[test]
fn around_by_row_and_by_host_time() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let row = svch0st_row(p).to_string();
    let (c, v) = ox(&["--case", p, "around", "--row", &row, "--window", "1m"]);
    assert_eq!(c, 0, "{v}");
    let sum = |v: &Value| -> u64 {
        inner(&v["data"]["untrusted"])["counts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["count"].as_u64().unwrap())
            .sum()
    };
    assert_eq!(sum(&v), 4);
    let (c, v) = ox(&[
        "--case",
        p,
        "around",
        "--host",
        "WS01",
        "--at",
        "2026-01-05T02:15:44.380000Z",
        "--window",
        "60s",
    ]);
    assert_eq!((c, sum(&v)), (0, 4));
    let (_, v) = ox(&[
        "--case", p, "around", "--row", &row, "--window", "1m", "--rows",
    ]);
    assert_eq!(ids(&v).len(), 4);
    assert_invalid(&["--case", p, "around", "--row", &row, "--window", "2d"]);
    assert_invalid(&["--case", p, "around", "--row", &row, "--window", "25h"]);
    // An anchor is required, and a row excludes a host and time.
    assert_eq!(ox(&["--case", p, "around", "--window", "1m"]).0, 2);
    assert_eq!(
        ox(&["--case", p, "around", "--row", &row, "--host", "WS01", "--window", "1m"]).0,
        2
    );
    assert_eq!(
        ox(&["--case", p, "around", "--row", "999999", "--window", "1m"]).0,
        3
    );
}

#[test]
fn row_shows_full_detail() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let id = svch0st_row(p).to_string();
    let (c, v) = ox(&["--case", p, "row", &id]);
    assert_eq!(c, 0);
    assert_eq!(v["data"]["active"], true);
    let hash = v["data"]["hash"].as_str().unwrap();
    assert_eq!(hash.len(), 64);
    assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(v["data"]["tags"], json!([]));
    let body = inner(&v["data"]["untrusted"]);
    assert_eq!(body["host"], "WS01");
    let time = body["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f[0] == "TimeCreated")
        .unwrap();
    assert_eq!(time[1], "2026-01-05T02:15:44.3800000Z");
    assert_eq!(time[2], false);
    assert_eq!(ox(&["--case", p, "row", "999999"]).0, 3);
}

#[test]
fn query_runs_a_select_and_refuses_the_rest() {
    let (d, p) = loaded();
    let p = p.as_str();
    let (c, v) = ox(&[
        "--case",
        p,
        "query",
        "select count(*) from evtxtriage_events",
    ]);
    assert_eq!(c, 0, "{v}");
    assert_eq!(inner(&v["data"]["untrusted"])["rows"], json!([["11"]]));
    assert_eq!(v["data"]["rows_returned"], 1);
    assert_invalid(&["--case", p, "query", "drop table ox.meta"]);
    // A mistake in a SELECT is the caller's, not the tool's.
    assert_invalid(&[
        "--case",
        p,
        "query",
        "select NoSuchColumn from evtxtriage_events",
    ]);
    assert_invalid(&[
        "--case",
        p,
        "query",
        "select CAST(Channel AS INTEGER) from evtxtriage_events",
    ]);
    // The SQL can come from a file.
    let file = d.path().join("q.sql");
    std::fs::write(&file, "select 1 as one").unwrap();
    let (_, v) = ox(&["--case", p, "query", &format!("@{}", file.display())]);
    assert_eq!(inner(&v["data"]["untrusted"])["columns"], json!(["one"]));
    // A cut result says so.
    let (_, v) = ox(&[
        "--case",
        p,
        "query",
        "select * from evtxtriage_events",
        "--limit",
        "2",
    ]);
    assert_eq!(
        (
            v["truncated"].as_bool(),
            v["data"]["rows_returned"].as_u64()
        ),
        (Some(true), Some(2))
    );
}

#[test]
fn query_stops_at_its_time_limit() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let runaway = "select a.range from range(300000) a, range(300000) b \
                   where a.range * b.range = -1";
    let started = std::time::Instant::now();
    let (c, v) = ox(&["--case", p, "query", runaway, "--timeout", "1"]);
    assert_eq!(
        (c, v["error"]["code"].as_str()),
        (2, Some("invalid")),
        "{v}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(15));
    for bad in ["0", "601", "1.5", "-1", "x"] {
        assert_invalid(&["--case", p, "query", "select 1", "--timeout", bad]);
    }
    let (c, v) = ox(&["--case", p, "query", "select 1", "--timeout", "600"]);
    assert_eq!(c, 0, "{v}");
    // The default and the most a caller may set are in the contract.
    let (_, v) = ox(&["capabilities"]);
    let limits = &v["data"]["limits"];
    assert_eq!(
        (
            limits["query_seconds_default"].as_u64(),
            limits["query_seconds_max"].as_u64()
        ),
        (Some(30), Some(600)),
        "{limits}"
    );
    assert_eq!(
        limits["query_chars_max"].as_u64(),
        Some(100_000),
        "{limits}"
    );
}

#[test]
fn schema_lists_relations_and_columns() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let (c, v) = ox(&["--case", p, "schema"]);
    assert_eq!((c, v["data"]["relations"].as_u64()), (0, Some(3)));
    let list = inner(&v["data"]["untrusted"])["relations"].clone();
    assert_eq!(list[0]["name"], "evtxtriage_events");
    assert_eq!(list[0]["rows"], 11);
    assert_eq!(list[0]["primary_time"], "TimeCreated");
    let (c, v) = ox(&["--case", p, "schema", "evtxtriage_events"]);
    assert_eq!(c, 0, "{v}");
    let cols = inner(&v["data"]["untrusted"])["columns"].clone();
    let time = cols
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "TimeCreated")
        .unwrap();
    assert_eq!(time["sql_type"], "TIMESTAMP");
    assert_eq!(time["time_semantics"], "utc");
    assert_eq!(time["origin"], "suite");
    assert_eq!(ox(&["--case", p, "schema", "no_such_relation"]).0, 3);

    // Found on the first real run: asking for one relation returned every
    // relation of the case too (33 KB over 164 relations) beside its columns.
    let (_, v) = ox(&["--case", p, "schema", "evtxtriage_events"]);
    let relations = inner(&v["data"]["untrusted"])["relations"].clone();
    assert_eq!(v["data"]["relations"], 1, "{v}");
    assert_eq!(
        relations
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["evtxtriage_events"]
    );
    assert_eq!(relations[0]["rows"], 11);
}

#[test]
fn capabilities_lists_the_read_commands() {
    let (_, v) = ox(&["capabilities"]);
    let names: Vec<&str> = v["data"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    for name in [
        "schema",
        "rows",
        "count",
        "groups",
        "histogram",
        "stack",
        "find",
        "around",
        "row",
        "query",
    ] {
        assert!(names.contains(&name), "{name} missing from {names:?}");
    }
}

#[test]
fn hostile_arguments_do_not_panic() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let rows = |extra: &[&str]| -> Vec<String> {
        let mut a = vec!["--case", p, "rows", "evtxtriage_events"];
        a.extend_from_slice(extra);
        a.into_iter().map(String::from).collect()
    };
    for extra in [
        &["--filter", "{"][..],
        &["--sort", ":"],
        &["--sort", ""],
        &["--columns", ",,"],
        &["--columns", ""],
        &["--limit", "0"],
        &["--limit", "1001"],
        &["--offset", "-1"],
        &["--limit", "x"],
        &["--filter", "[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[["],
    ] {
        let args = rows(extra);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_invalid(&refs);
    }
    assert_invalid(&["--case", p, "around", "--row", "1", "--window", ""]);
    assert_invalid(&["--case", p, "groups", "evtxtriage_events", "--by", ",,"]);
    assert_invalid(&[
        "--case",
        p,
        "groups",
        "evtxtriage_events",
        "--by",
        "Channel",
        "--parent",
        "{",
    ]);
    assert_invalid(&[
        "--case",
        p,
        "histogram",
        "evtxtriage_events",
        "--buckets",
        "0",
    ]);
    assert_invalid(&["--case", p, "query", ""]);
    // Longer than a Windows command line allows, so it goes in-process.
    let long = "a".repeat(100_000);
    let args: Vec<std::ffi::OsString> = [
        "oxidelens",
        "--json",
        "--case",
        p,
        "rows",
        "evtxtriage_events",
        "--search",
        &long,
    ]
    .iter()
    .map(Into::into)
    .collect();
    let mut out = Vec::new();
    assert_eq!(oxl_cli::run(args, &mut out), 2);
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        (v["ok"].as_bool(), v["error"]["code"].as_str()),
        (Some(false), Some("invalid"))
    );
}

#[test]
fn read_commands_need_a_case() {
    assert_invalid(&["rows", "evtxtriage_events"]);
    assert_invalid(&["query", "select 1"]);
    assert_invalid(&["schema"]);
}

#[test]
fn find_limit_without_rows_is_invalid() {
    let (_d, p) = loaded();
    assert_invalid(&["--case", &p, "find", "203.0.113.7", "--limit", "5"]);
    assert_invalid(&["--case", &p, "find", "203.0.113.7", "--offset", "1"]);
    let rows = [
        "--case",
        &p,
        "find",
        "203.0.113.7",
        "--rows",
        "--limit",
        "5",
    ];
    assert_eq!(ox(&rows).0, 0);
}

#[test]
fn around_limit_without_rows_is_invalid() {
    let (_d, p) = loaded();
    let r = svch0st_row(&p).to_string();
    let base = [
        "--case",
        p.as_str(),
        "around",
        "--row",
        r.as_str(),
        "--window",
        "5m",
    ];
    for extra in [["--limit", "5"], ["--offset", "1"]] {
        let mut args = base.to_vec();
        args.extend(extra);
        assert_invalid(&args);
    }
    let mut args = base.to_vec();
    args.extend(["--rows", "--limit", "5"]);
    assert_eq!(ox(&args).0, 0);
}

#[test]
fn sort_direction_ignores_case() {
    let (_d, p) = loaded();
    let rows = |sort: &str| {
        let (code, v) = ox(&["--case", &p, "rows", "evtxtriage_events", "--sort", sort]);
        assert_eq!(code, 0, "{sort}: {v}");
        ids(&v)
    };
    let desc = rows("UserName:desc");
    assert_eq!(rows("UserName:DESC"), desc);
    assert_eq!(rows("UserName:Desc"), desc);
    assert_eq!(rows("UserName:ASC"), rows("UserName:asc"));
}
