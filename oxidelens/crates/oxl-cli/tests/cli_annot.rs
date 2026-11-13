mod common;

use common::{inner, ox};
use serde_json::json;

/// A case with the two-host run and the loose firewall file loaded.
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

/// The row id of the WS01 successful logon (event 4624).
fn ws01_4624(p: &str) -> i64 {
    let (c, v) = ox(&[
        "--case",
        p,
        "rows",
        "evtxtriage_events",
        "--host",
        "WS01",
        "--filter",
        r#"{"column":"EventId","op":"eq","value":"4624"}"#,
    ]);
    assert_eq!(c, 0, "{v}");
    let ids = v["data"]["row_ids"].as_array().unwrap();
    assert_eq!(ids.len(), 1, "{v}");
    ids[0].as_i64().unwrap()
}

/// Mark, tag (as an agent) and note row `r`.
fn round_trip(p: &str, r: i64) {
    let (c, v) = ox(&[
        "--case",
        p,
        "mark",
        "set",
        "--rows",
        &format!("{r},999999999"),
    ]);
    assert_eq!(
        (
            c,
            v["data"]["done"].as_u64(),
            v["data"]["not_done"].as_u64()
        ),
        (0, Some(1), Some(1)),
        "{v}"
    );
    let rs = r.to_string();
    let agent_tag = |tag: &str| {
        ox(&[
            "--case",
            p,
            "--as",
            "agent:claude",
            "tag",
            "add",
            "--rows",
            &rs,
            "--tag",
            tag,
        ])
        .0
    };
    assert_eq!(agent_tag("exfil"), 4);
    assert_eq!(agent_tag("ai:exfil"), 0);
    assert_eq!(
        ox(&[
            "--case",
            p,
            "note",
            "add",
            "--row",
            &rs,
            "--text",
            "check 1149"
        ])
        .0,
        0
    );
}

#[test]
fn mark_tag_and_note_round_trip_through_the_cli() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let r = ws01_4624(p);
    round_trip(p, r);
    let (_, list) = ox(&["--case", p, "mark", "list"]);
    assert_eq!(list["data"]["marks"], 1);
    // The engine's own values, outside the wrapper.
    assert_eq!(
        (
            list["data"]["row_ids"].clone(),
            list["data"]["active"].clone()
        ),
        (json!([r]), json!([true]))
    );
    let marks = &inner(&list["data"]["untrusted"])["marks"][0];
    assert_eq!(
        (marks["row_id"].as_i64(), marks["relation"].as_str()),
        (Some(r), Some("evtxtriage_events"))
    );
    let (_, tags) = ox(&["--case", p, "tag", "list"]);
    assert_eq!(tags["data"]["tags"][0]["name"], "ai:exfil");
    assert_eq!(tags["data"]["tags"][0]["rows"], 1);
    assert_eq!(tags["truncated"], false);

    let (_, note) = ox(&["--case", p, "note", "list", "--row", &r.to_string()]);
    assert_eq!(note["data"]["notes"], 1);
    assert_eq!(note["truncated"], false);
    assert_eq!(
        inner(&note["data"]["untrusted"])["notes"][0]["text"],
        "check 1149"
    );

    let (_, row) = ox(&["--case", p, "row", &r.to_string()]);
    assert_eq!(
        (row["data"]["mark"].as_str(), row["data"]["tags"].clone()),
        (Some("suspicious"), json!(["ai:exfil"]))
    );
    assert_eq!(
        inner(&row["data"]["untrusted"])["notes"][0]["text"],
        "check 1149"
    );
    let (_, marked) = ox(&[
        "--case",
        p,
        "rows",
        "evtxtriage_events",
        "--filter",
        r#"{"column":"_mark","op":"eq","value":"suspicious"}"#,
    ]);
    assert_eq!(marked["data"]["row_ids"], json!([r]));
}

#[test]
fn tag_remove_note_delete_and_mark_clear() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let r = ws01_4624(p);
    let rs = r.to_string();
    round_trip(p, r);
    let (c, v) = ox(&["--case", p, "mark", "clear", "--rows", &rs]);
    assert_eq!((c, v["data"]["done"].clone()), (0, json!(1)), "{v}");
    let (c, v) = ox(&[
        "--case", p, "tag", "remove", "--rows", &rs, "--tag", "ai:exfil",
    ]);
    assert_eq!((c, v["data"]["done"].clone()), (0, json!(1)), "{v}");
    let (_, notes) = ox(&["--case", p, "note", "list", "--row", &rs]);
    let id = inner(&notes["data"]["untrusted"])["notes"][0]["id"]
        .as_i64()
        .unwrap();
    assert_eq!(ox(&["--case", p, "note", "delete", &id.to_string()]).0, 0);
    let (_, notes) = ox(&["--case", p, "note", "list", "--row", &rs]);
    assert_eq!(notes["data"]["notes"], 0);
    // A second delete is refused, whole-call.
    assert_eq!(ox(&["--case", p, "note", "delete", &id.to_string()]).0, 4);
    assert_eq!(ox(&["--case", p, "note", "delete", "424242"]).0, 3);
}

#[test]
fn an_agent_cannot_clear_an_analysts_mark_through_the_cli() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let r = ws01_4624(p).to_string();
    assert_eq!(ox(&["--case", p, "mark", "set", "--rows", &r]).0, 0);
    let (c, v) = ox(&[
        "--case",
        p,
        "--as",
        "agent:claude",
        "mark",
        "clear",
        "--rows",
        &r,
    ]);
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
    assert!(reason.contains("only an analyst"), "{reason}");
    assert_eq!(ox(&["--case", p, "mark", "list"]).1["data"]["marks"], 1);
    let (_, by) = ox(&["--case", p, "mark", "list", "--by", "agent:claude"]);
    assert_eq!(by["data"]["marks"], 0);
}

#[test]
fn column_declare_and_primary_time() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let hayabusa = "ext_hayabusa_csv_timeline";
    let declare = |who: &str| {
        ox(&[
            "--case", p, "--as", who, "column", "declare", hayabusa, "EventID", "--type", "BIGINT",
        ])
    };
    assert_eq!(declare("analyst:tester").0, 0);
    let (c, v) = declare("agent:claude");
    assert_eq!((c, v["error"]["code"].as_str()), (4, Some("refused")));
    // Typed now: a count filtered on the number works.
    let (_, schema) = ox(&["--case", p, "schema", hayabusa]);
    let columns = inner(&schema["data"]["untrusted"])["columns"].clone();
    let typed = columns
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "EventID")
        .unwrap();
    assert_eq!(typed["sql_type"], "BIGINT");
    // The suite's own typing is kept.
    let (c, _) = ox(&[
        "--case",
        p,
        "column",
        "declare",
        "evtxtriage_events",
        "EventId",
        "--type",
        "BIGINT",
    ]);
    assert_eq!(c, 2);
    // A timestamp's zone is never guessed: --semantics is required.
    let ts = |extra: &[&str]| {
        let mut args = vec![
            "--case",
            p,
            "column",
            "declare",
            hayabusa,
            "Timestamp",
            "--type",
            "TIMESTAMP",
        ];
        args.extend_from_slice(extra);
        ox(&args)
    };
    let (c, v) = ts(&[]);
    assert_eq!(
        (c, v["error"]["code"].as_str()),
        (2, Some("invalid")),
        "{v}"
    );
    assert!(
        inner(&v["error"]["untrusted"])["message"]
            .as_str()
            .unwrap()
            .contains("semantics"),
        "{v}"
    );
    assert_eq!(ts(&["--semantics", "unknown"]).0, 0);
    let (_, schema) = ox(&["--case", p, "schema", hayabusa]);
    let columns = inner(&schema["data"]["untrusted"])["columns"].clone();
    let time = columns
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Timestamp")
        .unwrap()
        .clone();
    assert_eq!(
        (time["sql_type"].as_str(), time["time_semantics"].as_str()),
        (Some("TIMESTAMP"), Some("unknown"))
    );

    // An agent may not choose a relation's time.
    for args in [
        &[
            "--case",
            p,
            "--as",
            "agent:claude",
            "column",
            "primary-time",
            "fw_csv",
            "--none",
        ][..],
        &[
            "--case",
            p,
            "--as",
            "agent:claude",
            "column",
            "primary-time",
            "fw_csv",
            "time",
        ][..],
    ] {
        let (c, v) = ox(args);
        assert_eq!(
            (c, v["error"]["code"].as_str()),
            (4, Some("refused")),
            "{v}"
        );
    }

    let hist = ["--case", p, "histogram", "fw_csv"];
    assert_eq!(ox(&hist).0, 0);
    assert_eq!(
        ox(&["--case", p, "column", "primary-time", "fw_csv", "--none"]).0,
        0
    );
    assert_eq!(ox(&hist).0, 2);
    assert_eq!(
        ox(&["--case", p, "column", "primary-time", "fw_csv", "time"]).0,
        0
    );
    assert_eq!(ox(&hist).0, 0);
    // A column and --none together, or neither, are usage errors.
    assert_eq!(
        ox(&[
            "--case",
            p,
            "column",
            "primary-time",
            "fw_csv",
            "time",
            "--none"
        ])
        .0,
        2
    );
    assert_eq!(ox(&["--case", p, "column", "primary-time", "fw_csv"]).0, 2);
}

#[test]
fn history_lists_changes_newest_first() {
    let (_d, p) = loaded();
    let p = p.as_str();
    round_trip(p, ws01_4624(p));
    let (c, v) = ox(&["--case", p, "history"]);
    assert_eq!(c, 0);
    assert!(v["data"]["entries"].as_u64().unwrap() >= 3, "{v}");
    let entries = inner(&v["data"]["untrusted"])["entries"].clone();
    assert_eq!(entries[0]["action"], "note_add");
    let (_, only) = ox(&["--case", p, "history", "--kind", "row_tag"]);
    let tagged = inner(&only["data"]["untrusted"])["entries"].clone();
    assert!(tagged
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["target_kind"] == "row_tag"));
    assert_eq!(ox(&["--case", p, "history", "--kind", "bogus"]).0, 2);
    let (_, page) = ox(&["--case", p, "history", "--limit", "1"]);
    assert_eq!(
        (page["data"]["entries"].clone(), page["truncated"].clone()),
        (json!(1), json!(true))
    );
}

#[test]
fn tag_and_note_lists_page() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let r = ws01_4624(p).to_string();
    for tag in ["alpha", "beta"] {
        assert_eq!(
            ox(&["--case", p, "tag", "add", "--rows", &r, "--tag", tag]).0,
            0
        );
    }
    for text in ["one", "two"] {
        assert_eq!(
            ox(&["--case", p, "note", "add", "--row", &r, "--text", text]).0,
            0
        );
    }
    let (c, tags) = ox(&["--case", p, "tag", "list", "--limit", "1"]);
    assert_eq!(c, 0, "{tags}");
    assert_eq!(
        (
            tags["data"]["tags"][0]["name"].clone(),
            tags["truncated"].clone()
        ),
        (json!("alpha"), json!(true))
    );
    let (_, tags) = ox(&["--case", p, "tag", "list", "--limit", "1", "--offset", "1"]);
    assert_eq!(
        (
            tags["data"]["tags"][0]["name"].clone(),
            tags["truncated"].clone()
        ),
        (json!("beta"), json!(false))
    );
    let (_, notes) = ox(&["--case", p, "note", "list", "--row", &r, "--limit", "1"]);
    assert_eq!(
        (notes["data"]["notes"].clone(), notes["truncated"].clone()),
        (json!(1), json!(true))
    );
    assert_eq!(
        inner(&notes["data"]["untrusted"])["notes"][0]["text"],
        "one"
    );
    let (_, notes) = ox(&[
        "--case", p, "note", "list", "--row", &r, "--limit", "1", "--offset", "1",
    ]);
    assert_eq!(notes["truncated"], false);
    assert_eq!(
        inner(&notes["data"]["untrusted"])["notes"][0]["text"],
        "two"
    );
}

#[test]
fn bad_row_lists_are_invalid() {
    let (_d, p) = loaded();
    let p = p.as_str();
    for bad in ["", "a", "1,,2", "1;2", "1.5", "-1", "99999999999999999999"] {
        assert_eq!(
            ox(&["--case", p, "mark", "set", "--rows", bad]).0,
            2,
            "{bad:?}"
        );
        assert_eq!(
            ox(&["--case", p, "tag", "add", "--rows", bad, "--tag", "x"]).0,
            2,
            "{bad:?}"
        );
    }
}

/// Found on the second real run: a Windows command line holds about 3,500
/// row ids, so a larger batch could not be passed at all. `--rows @file`
/// reads them from a file, separated by commas, spaces or new lines.
#[test]
fn row_ids_can_come_from_a_file() {
    let (d, p) = loaded();
    let p = p.as_str();
    let r = ws01_4624(p);
    let list = d.path().join("rows.txt");
    std::fs::write(&list, format!("{r}\r\n999999999, {r}\n")).unwrap();
    let at = format!("@{}", list.to_string_lossy());
    let (c, v) = ox(&["--case", p, "mark", "set", "--rows", &at]);
    assert_eq!(c, 0, "{v}");
    let ids: Vec<i64> = v["data"]["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["row_id"].as_i64().unwrap())
        .collect();
    // A row named twice is one row, with one outcome.
    assert_eq!(ids, [r, 999_999_999]);
    assert_eq!(
        (v["data"]["done"].clone(), v["data"]["not_done"].clone()),
        (json!(1), json!(1))
    );
    let (c, v) = ox(&["--case", p, "tag", "add", "--rows", &at, "--tag", "batch"]);
    assert_eq!((c, v["data"]["done"].clone()), (0, json!(1)), "{v}");

    // A file is held to the same rule as the list: a stray comma is a typo.
    for bad in ["1,,2", "1\n\nx", "1.5", "-1"] {
        std::fs::write(&list, bad).unwrap();
        assert_eq!(
            ox(&["--case", p, "mark", "set", "--rows", &at]).0,
            2,
            "{bad:?}"
        );
    }
    let missing = format!("@{}", d.path().join("none.txt").to_string_lossy());
    let (c, v) = ox(&["--case", p, "mark", "clear", "--rows", &missing]);
    assert_eq!(c, 2, "{v}");
}

/// Found on the second real run: `note list` on a superseded row's id was
/// silently empty, the note having moved with the row. `row` and
/// `note list` now say where the row stands and which row is in its place.
#[test]
fn a_superseded_row_points_at_the_row_in_its_place() {
    let (d, p) = loaded();
    let p = p.as_str();
    let pf = |p: &str| {
        let (c, v) = ox(&[
            "--case",
            p,
            "rows",
            "petriage_timeline",
            "--host",
            "WS01",
            "--filter",
            r#"{"column":"ExecutableName","op":"eq","value":"SVCH0ST.EXE"}"#,
        ]);
        assert_eq!(c, 0, "{v}");
        v["data"]["row_ids"][0].as_i64().unwrap()
    };
    let old = pf(p);
    let r = old.to_string();
    assert_eq!(
        ox(&[
            "--case",
            p,
            "note",
            "add",
            "--row",
            &r,
            "--text",
            "first seen"
        ])
        .0,
        0
    );
    let (c, v) = ox(&["--case", p, "note", "list", "--row", &r]);
    assert_eq!(
        (
            c,
            v["data"]["standing"].clone(),
            v["data"]["current_row"].clone()
        ),
        (0, json!("active"), json!(null)),
        "{v}"
    );
    let rerun = oxl_fixtures::rerun_ws01_prefetch(&d.path().join("rerun"));
    assert_eq!(
        ox(&["--case", p, "load", "run", &rerun.to_string_lossy()]).0,
        0
    );
    let new = pf(p);
    for command in [vec!["note", "list", "--row", &r], vec!["row", &r]] {
        let mut args = vec!["--case", p];
        args.extend(command.iter().copied());
        let (c, v) = ox(&args);
        assert_eq!(
            (
                c,
                v["data"]["standing"].clone(),
                v["data"]["current_row"].clone()
            ),
            (0, json!("superseded"), json!(new)),
            "{command:?}: {v}"
        );
    }
    let (_, v) = ox(&["--case", p, "note", "list", "--row", &new.to_string()]);
    assert_eq!(v["data"]["notes"], 1, "{v}");
    // An id no source loaded has no standing.
    let (c, v) = ox(&["--case", p, "note", "list", "--row", "999999999"]);
    assert_eq!((c, v["data"]["standing"].clone()), (0, json!(null)), "{v}");
}

#[test]
fn whole_call_refusals_exit_4_and_empty_input_exits_2() {
    let (_d, p) = loaded();
    let p = p.as_str();
    let rows = (1..=1001)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let (c, v) = ox(&[
        "--case",
        p,
        "--as",
        "agent:claude",
        "mark",
        "set",
        "--rows",
        &rows,
    ]);
    assert_eq!((c, v["error"]["code"].as_str()), (4, Some("refused")));
    assert_eq!(
        ox(&["--case", p, "tag", "add", "--rows", "1", "--tag", "Bad Tag!"]).0,
        2
    );
}

#[test]
fn note_text_can_come_from_a_file() {
    let (d, p) = loaded();
    let p = p.as_str();
    let r = ws01_4624(p).to_string();
    let f = d.path().join("note.txt");
    std::fs::write(&f, "from a file").unwrap();
    let arg = format!("@{}", f.display());
    assert_eq!(
        ox(&["--case", p, "note", "add", "--row", &r, "--text", &arg]).0,
        0
    );
    let (_, notes) = ox(&["--case", p, "note", "list", "--row", &r]);
    assert_eq!(
        inner(&notes["data"]["untrusted"])["notes"][0]["text"],
        "from a file"
    );
    let missing = format!("@{}", d.path().join("nope.txt").display());
    assert_eq!(
        ox(&["--case", p, "note", "add", "--row", &r, "--text", &missing]).0,
        2
    );
}
