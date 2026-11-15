//! The TypeScript contract: the files in `app/src/contract/generated/` are
//! what the Rust types and the command table generate today, byte for byte,
//! and every command takes, through Tauri's own IPC, exactly the arguments
//! the contract names.
//!
//! `OXL_WRITE_CONTRACT=1 cargo test -p oxl-app --test contract` rewrites
//! the generated files instead of comparing them.

use oxl_app::contract::{self, COMMANDS};
use oxl_app::events::NoSink;
use oxl_app::settings::SettingsStore;
use oxl_app::state::AppState;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::Manager;

/// Where the generated files are checked in.
fn generated_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src/contract/generated")
}

/// Every file under `dir`, by its path below `dir` with `/` separators.
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(name, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

#[test]
fn generated_types_are_current() {
    let fresh = tempfile::tempdir().unwrap();
    contract::generate(fresh.path()).unwrap();
    let want = files(fresh.path());
    assert!(want.contains_key("commands.ts"), "{:?}", want.keys());

    let dir = generated_dir();
    if std::env::var("OXL_WRITE_CONTRACT").as_deref() == Ok("1") {
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        for (name, bytes) in &want {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        return;
    }

    let have = files(&dir);
    let missing: Vec<&String> = want.keys().filter(|k| !have.contains_key(*k)).collect();
    let stale: Vec<&String> = have.keys().filter(|k| !want.contains_key(*k)).collect();
    let changed: Vec<&String> = want
        .iter()
        .filter(|(k, v)| have.get(*k).is_some_and(|h| h != *v))
        .map(|(k, _)| k)
        .collect();
    assert!(
        missing.is_empty() && stale.is_empty() && changed.is_empty(),
        "the TypeScript contract in {} has drifted from the Rust types: \
         missing {missing:?}, stale {stale:?}, changed {changed:?}. \
         Run `OXL_WRITE_CONTRACT=1 cargo test -p oxl-app --test contract` and commit the result.",
        dir.display()
    );
}

#[test]
fn the_signatures_are_the_commands() {
    let cfg = contract::config();
    let names: Vec<&str> = contract::signatures(&cfg).iter().map(|s| s.name).collect();
    assert_eq!(names, COMMANDS);
}

/// A value each argument of command `cmd` takes, of the shape its type
/// asks for: by the argument's name, and for a name two commands type
/// differently, by the command.
fn sample(cmd: &str, arg: &str, dir: &Path) -> Value {
    let record = cmd.starts_with("record") || cmd == "records_list";
    match arg {
        "kind" if record => json!("host"),
        "query" if record => json!({ "kind": "ioc", "ioc_type": null, "tool": null, "text": null }),
        "query" => json!({ "status": [], "host": null, "tactic": null, "text": null, "row": null }),
        "status" => json!("reviewed"),
        "rows" => json!([1]),
        "tag" | "text" => json!("sample"),
        "value" => json!("203.0.113.7"),
        "entryId" | "recordId" | "fromId" => json!(1),
        "intoId" => json!(2),
        "edit" => {
            json!({ "title": "sample", "description": null, "time": { "set": "2026-01-05T03:00:00Z" },
                          "tactic": "clear", "technique_ids": null })
        }
        "spec" => json!({ "kind": "host", "id": null, "name": "WS01", "role": "", "note": "" }),
        "targetKind" | "targetId" => Value::Null,
        "queryId" => json!("contract-q"),
        "slot" => json!("contract"),
        "path" => json!(dir.join("sample.duckdb")),
        "name" => json!("sample"),
        "options" => json!({}),
        "relation" | "column" => json!("sample"),
        "zone" => json!("UTC"),
        "state" => json!({ "relation": "sample" }),
        "limit" | "maxBuckets" => json!(10),
        "offset" | "rowId" | "id" => json!(1),
        "by" => json!(["sample"]),
        "parent" | "exceptChips" => json!([]),
        "order" => json!("commonest"),
        "after" | "contains" | "zoneTimes" | "from" | "to" => Value::Null,
        "backward" => json!(false),
        "windows" => json!([60]),
        "ranges" => json!([[0, 1]]),
        "ids" => json!([1]),
        "source" => json!({ "around": { "host": "WS01", "at": "2026-01-05T02:14:00Z",
                                        "window_seconds": 60 } }),
        "kind" => json!("view"),
        "pattern" => json!("*"),
        "doc" => json!({}),
        "settings" => serde_json::to_value(oxl_app::settings::Settings::default()).unwrap(),
        "queryIds" => json!(["contract-cancel"]),
        other => panic!("no sample for argument {other}"),
    }
}

fn invoke(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    args: Value,
) -> Result<Value, Value> {
    let url = if cfg!(any(windows, target_os = "android")) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    };
    tauri::test::get_ipc_response(
        webview,
        tauri::webview::InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: url.parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .map(|body| body.deserialize::<Value>().unwrap())
}

/// Tauri's own refusal of a call whose arguments do not fit the command,
/// as opposed to the command's answer (a `CmdError` object).
fn refused_arguments(answer: &Result<Value, Value>) -> Option<String> {
    match answer {
        Err(Value::String(message)) => Some(message.clone()),
        _ => None,
    }
}

#[test]
fn every_command_takes_the_contract_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let app = tauri::test::mock_builder()
        .invoke_handler(oxl_app::invoke_handler())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    app.manage(AppState::new(SettingsStore::in_memory(), Arc::new(NoSink)));
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let cfg = contract::config();
    for sig in contract::signatures(&cfg) {
        let all: Map<String, Value> = sig
            .args
            .iter()
            .map(|a| (a.name.to_string(), sample(sig.name, a.name, dir.path())))
            .collect();
        // With every argument, the command runs: it answers, or fails with
        // its own error, never Tauri's refusal of the arguments.
        let answer = invoke(&webview, sig.name, Value::Object(all.clone()));
        if let Some(refusal) = refused_arguments(&answer) {
            panic!("{} refused the contract's arguments: {refusal}", sig.name);
        }
        if let Err(e) = &answer {
            assert!(
                e.get("code").is_some_and(Value::is_string),
                "{}: {e}",
                sig.name
            );
        }
        // Each argument the contract requires, left out, is missed by name;
        // each it marks optional is not.
        for arg in &sig.args {
            let mut fewer = all.clone();
            fewer.remove(arg.name);
            let answer = invoke(&webview, sig.name, Value::Object(fewer));
            match (arg.optional, refused_arguments(&answer)) {
                (false, Some(refusal)) => assert!(
                    refusal.contains(arg.name),
                    "{} without {}: {refusal}",
                    sig.name,
                    arg.name
                ),
                (false, None) => panic!(
                    "{} ran without {}, which the contract requires",
                    sig.name, arg.name
                ),
                (true, Some(refusal)) => panic!(
                    "{} refused to run without {}, which the contract makes optional: {refusal}",
                    sig.name, arg.name
                ),
                (true, None) => {}
            }
        }
    }
}
