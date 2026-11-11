//! Filter documents: the JSON an analyst's chips, the agent's `--filter` and
//! saved views compile through. It is untrusted input, so most of what is
//! tested here is what it refuses.

mod common;

use common::{loaded, n};
use oxl_engine::filter::{
    compile_filter, parse_filter, FilterNode, MAX_DEPTH, MAX_IN, MAX_LEAVES, MAX_REGEX,
};
use oxl_engine::{columns, Case, ColumnInfo};

const RELATION: &str = "evtxtriage_events";

/// A loaded case and the catalogued columns of the events relation.
fn setup() -> (tempfile::TempDir, Case, Vec<ColumnInfo>) {
    let (d, case) = loaded();
    let cols = columns(&case, RELATION).unwrap();
    (d, case, cols)
}

/// How many events rows the document selects.
fn count_in(case: &Case, cols: &[ColumnInfo], json: &str) -> i64 {
    let filter = parse_filter(json).unwrap_or_else(|e| panic!("{json}: {e}"));
    let fragment = compile_filter(&filter, cols).unwrap_or_else(|e| panic!("{json}: {e}"));
    let sql = format!("select count(*) from {RELATION} where {}", fragment.sql);
    case.scalar_i64_with(&sql, &fragment.params)
        .unwrap_or_else(|e| panic!("{json} -> {sql}: {e}"))
}

/// The error code `json` is refused with.
fn refusal(cols: &[ColumnInfo], json: &str) -> &'static str {
    match parse_filter(json).and_then(|f| compile_filter(&f, cols)) {
        Ok(fragment) => panic!("{json} was accepted as {}", fragment.sql),
        Err(e) => e.code(),
    }
}

fn leaf(column: &str, op: &str, value: &str) -> String {
    format!(r#"{{"column":"{column}","op":"{op}","value":{value}}}"#)
}

/// A leaf wrapped in `nots` until the document is `depth` levels deep.
fn nested(depth: usize) -> String {
    let mut doc = leaf("EventId", "eq", r#""4624""#);
    for _ in 1..depth {
        doc = format!(r#"{{"not":{doc}}}"#);
    }
    doc
}

#[test]
fn the_documented_operators_select_the_documented_rows() {
    let (_d, case, cols) = setup();
    let count = |json: &str| count_in(&case, &cols, json);
    assert_eq!(
        count(r#"{"column":"EventId","op":"in","value":["4624","4625"]}"#),
        4
    );
    assert_eq!(count(r#"{"column":"EventId","op":"gt","value":4000}"#), 6);
    assert_eq!(
        count(
            r#"{"column":"TimeCreated","op":"between","value":["2026-01-05T02:14:00Z","2026-01-05T02:16:00Z"]}"#
        ),
        3
    );
    assert_eq!(
        count(r#"{"column":"TimeCreated","op":"conversion_failed"}"#),
        1
    );
    assert_eq!(count(r#"{"column":"TimeCreated","op":"is_empty"}"#), 1);
    assert_eq!(
        count(r#"{"column":"UserName","op":"contains","value":"SVC_"}"#),
        9
    );
    assert_eq!(
        count(r#"{"column":"UserName","op":"contains","value":"SVC_","case_sensitive":true}"#),
        0
    );
    assert_eq!(
        count(r#"{"column":"PayloadData1","op":"regex","value":"^svch0st\\.exe(:\\d+)?$"}"#),
        2
    );
    assert_eq!(
        count(
            r#"{"and":[{"column":"Channel","op":"eq","value":"security"},{"not":{"column":"_triage_host","op":"eq","value":"SRV01"}}]}"#
        ),
        5
    );
    assert_eq!(
        count(
            r#"{"or":[{"column":"EventId","op":"eq","value":"1102"},{"column":"EventId","op":"eq","value":"5140"}]}"#
        ),
        3
    );
}

#[test]
fn wildcard_characters_match_literally() {
    let (_d, case, cols) = setup();
    let count = |json: &str| count_in(&case, &cols, json);
    assert_eq!(
        count(r#"{"column":"UserName","op":"contains","value":"svc%backup"}"#),
        0
    );
    assert_eq!(
        count(r#"{"column":"UserName","op":"contains","value":"svc_backup"}"#),
        9
    );
    // '_' is not a wildcard: "s_c" must not match "svc".
    assert_eq!(
        count(r#"{"column":"UserName","op":"contains","value":"s_c"}"#),
        0
    );
    // \\*\finance$
    assert_eq!(
        count(r#"{"column":"PayloadData1","op":"starts_with","value":"\\\\*\\"}"#),
        1
    );
}

#[test]
fn hostile_documents_are_refused_not_executed() {
    let (_d, case, cols) = setup();
    for bad in [
        r#"{"column":"EventId\" OR 1=1 --","op":"eq","value":"1"}"#,
        r#"{"column":"EventId","op":"drop","value":"1"}"#,
        r#"{"column":"EventId","op":"eq","value":"1","extra":true}"#,
        r#"{"column":"EventId","op":"between","value":["1"]}"#,
        r#"{"column":"TimeCreated","op":"gt","value":"yesterday"}"#,
        r#"{"column":"UserName","op":"conversion_failed"}"#,
        "",
        "[]",
        "null",
        "{",
    ] {
        assert_eq!(refusal(&cols, bad), "invalid", "{bad}");
    }
    let v = r#"{"column":"UserName","op":"eq","value":"x'; DROP TABLE ox.meta; --"}"#;
    assert_eq!(count_in(&case, &cols, v), 0);
    assert!(n(&case, "select count(*) from ox.meta") > 0);
}

#[test]
fn caps_are_enforced() {
    let (_d, case, cols) = setup();

    assert_eq!(refusal(&cols, &nested(MAX_DEPTH + 1)), "invalid");
    assert_eq!(count_in(&case, &cols, &nested(MAX_DEPTH)), 8);

    let leaves = |k: usize| {
        let one = leaf("EventId", "eq", r#""4624""#);
        format!(r#"{{"or":[{}]}}"#, vec![one; k].join(","))
    };
    assert_eq!(refusal(&cols, &leaves(MAX_LEAVES + 1)), "invalid");
    assert_eq!(count_in(&case, &cols, &leaves(MAX_LEAVES)), 3);

    let in_values = |k: usize| {
        let mut values: Vec<String> = (0..k - 1).map(|i| format!(r#""x{i}""#)).collect();
        values.push(r#""4624""#.into());
        leaf("EventId", "in", &format!("[{}]", values.join(",")))
    };
    assert_eq!(refusal(&cols, &in_values(MAX_IN + 1)), "invalid");
    assert_eq!(count_in(&case, &cols, &in_values(MAX_IN)), 3);

    let too_long = leaf(
        "UserName",
        "regex",
        &format!(r#""{}""#, "a".repeat(MAX_REGEX + 1)),
    );
    assert_eq!(refusal(&cols, &too_long), "invalid");
    // `a?` repeated matches every row, so the longest accepted pattern also runs.
    let longest = leaf(
        "UserName",
        "regex",
        &format!(r#""{}""#, "a?".repeat(MAX_REGEX / 2)),
    );
    assert_eq!(count_in(&case, &cols, &longest), 11);
}

#[test]
fn parse_filter_is_total() {
    let mb = 1 << 20;
    assert!(parse_filter(&"[".repeat(mb)).is_err());
    assert!(parse_filter(&r#"{"not":"#.repeat(mb / 7)).is_err());
    // A closed document far deeper than the cap, not just an unterminated one.
    let deep = format!(
        "{}{}{}",
        r#"{"not":"#.repeat(100),
        leaf("EventId", "eq", r#""1""#),
        "}".repeat(100)
    );
    assert!(parse_filter(&deep).is_err());

    // A fixed xorshift stream, so a failure is reproducible.
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    for _ in 0..2_000 {
        let len = (state % 64) as usize;
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()[0]
            })
            .collect();
        state = state.wrapping_add(1);
        assert!(parse_filter(&String::from_utf8_lossy(&bytes)).is_err());
    }
}

#[test]
fn the_mark_pseudo_column_filters_on_marks() {
    let (_d, case, cols) = setup();
    // The marks API is a later task; one row written directly stands in.
    n(
        &case,
        "insert into ox.row_mark (row_id, mark, marked_by, marked_at) \
         select _ox_row, 'suspicious', 'analyst:tester', '2026-01-05T00:00:00.000000Z' \
         from evtxtriage_events where \"PayloadData1\" = 'rclone.exe' returning row_id",
    );
    let count = |json: &str| count_in(&case, &cols, json);
    assert_eq!(
        count(r#"{"column":"_mark","op":"eq","value":"suspicious"}"#),
        1
    );
    assert_eq!(count(r#"{"column":"_mark","op":"eq","value":"none"}"#), 10);
    assert_eq!(count(r#"{"column":"_mark","op":"ne","value":"none"}"#), 1);
    assert_eq!(
        count(r#"{"column":"_mark","op":"in","value":["suspicious","none"]}"#),
        11
    );
    assert_eq!(
        refusal(&cols, r#"{"column":"_mark","op":"eq","value":"benign"}"#),
        "invalid"
    );
    assert_eq!(
        refusal(&cols, r#"{"column":"_mark","op":"contains","value":"s"}"#),
        "invalid"
    );
}

#[test]
fn a_serialised_document_parses_back_to_itself() {
    let json = r#"{"and":[{"column":"Channel","op":"eq","value":"security"},{"or":[{"not":{"column":"EventId","op":"gt","value":4000}},{"column":"UserName","op":"is_empty"}]}]}"#;
    let node: FilterNode = parse_filter(json).unwrap();
    let again = serde_json::to_string(&node).unwrap();
    assert_eq!(parse_filter(&again).unwrap(), node);
}
