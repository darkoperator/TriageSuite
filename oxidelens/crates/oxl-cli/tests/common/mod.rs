#![allow(dead_code)] // each test file uses a subset of these helpers

use assert_cmd::Command;
use serde_json::Value;

/// Run `oxidelens --json <args>`; returns the exit code and parsed stdout.
pub fn ox(args: &[&str]) -> (i32, Value) {
    let (code, stdout) = ox_raw(args);
    let v = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {stdout}"));
    (code, v)
}

pub fn ox_raw(args: &[&str]) -> (i32, String) {
    let out = Command::cargo_bin("oxidelens")
        .unwrap()
        .env_remove("OXIDELENS_CASE")
        .env_remove("OXIDELENS_ACTOR")
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
    )
}

/// The JSON between the wrapper lines of an `untrusted` string.
pub fn inner(wrapped: &Value) -> Value {
    let text = wrapped.as_str().expect("untrusted is a string");
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines
        .first()
        .unwrap()
        .starts_with("<untrusted_data boundary="));
    assert!(lines
        .last()
        .unwrap()
        .starts_with("</untrusted_data boundary="));
    serde_json::from_str(&lines[1..lines.len() - 1].join("\n")).unwrap()
}

/// The boundary named on the wrapper's first line.
pub fn boundary_of(wrapped: &Value) -> String {
    let text = wrapped.as_str().unwrap();
    let first = text.lines().next().unwrap();
    first
        .trim_start_matches("<untrusted_data boundary=")
        .trim_end_matches('>')
        .to_string()
}
