//! Records the JSON the front end's tests use as the app's answers:
//! `app/src/test/fixtures/*.json`, each a command's result over a seeded
//! case, exactly as the command serialises it (pretty-printed).
//!
//! Run it on purpose, then commit what it wrote:
//!
//! ```text
//! cargo test -p oxl-app --test record_fixtures -- --ignored
//! ```
//!
//! Re-running it reproduces the files byte for byte. What would vary from
//! run to run is pinned or replaced:
//!
//! - every time the engine writes (`loaded_at`, `created_at`...) is pinned
//!   through `OXIDELENS_NOW` to `NOW`;
//! - every change is recorded under the analyst `ANALYST`;
//! - the case's random id is replaced by `CASE_ID`;
//! - a path below the temporary folder the case is built in becomes
//!   `/fixture/…` with `/` separators (the case is `/fixture/case.duckdb`,
//!   the runs `/fixture/run-…`).
//!
//! The data is the synthetic two-host run and host-zones run of
//! `oxl-fixtures`.

mod common;

use common::{grid, state};
use oxl_app::cmd::{self, case, grid as g, load, row, views, At};
use oxl_engine::{PageOptions, ValueOrder, ViewDoc, ViewKind};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The clock every engine write sees.
const NOW: &str = "2026-01-10T12:00:00.000000Z";
/// Who every change is recorded under.
const ANALYST: &str = "analyst01";
/// What the case's random id is replaced by.
const CASE_ID: &str = "0123456789abcdef0123456789abcdef";
/// What the temporary folder the case is built in is replaced by.
const ROOT: &str = "/fixture";

const EVENTS: &str = "evtxtriage_events";
const NY: &str = "America/New_York";

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/src/test/fixtures")
}

fn at(slot: &str) -> At<'_> {
    At::new("record", slot)
}

/// Replaces what varies between runs in recorded JSON.
struct Scrub {
    /// The forms the temporary folder's path takes in the engine's answers.
    roots: Vec<String>,
    case_id: String,
}

impl Scrub {
    fn value(&self, value: Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.text(s)),
            Value::Array(items) => Value::Array(items.into_iter().map(|v| self.value(v)).collect()),
            Value::Object(map) => {
                Value::Object(map.into_iter().map(|(k, v)| (k, self.value(v))).collect())
            }
            other => other,
        }
    }

    fn text(&self, s: String) -> String {
        if s == self.case_id {
            return CASE_ID.to_string();
        }
        for root in &self.roots {
            if let Some(rest) = s.strip_prefix(root.as_str()) {
                return format!("{ROOT}{}", rest.replace('\\', "/"));
            }
        }
        s
    }
}

fn write(scrub: &Scrub, name: &str, answer: &impl serde::Serialize) {
    let value = scrub.value(serde_json::to_value(answer).unwrap());
    let mut text = serde_json::to_string_pretty(&value).unwrap();
    text.push('\n');
    let dir = fixtures_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.json")), text).unwrap();
}

/// The forms `dir`'s path can take once the engine has made it canonical.
fn root_forms(dir: &Path) -> Vec<String> {
    let mut forms = vec![dir.display().to_string()];
    if let Ok(canonical) = std::fs::canonicalize(dir) {
        let text = canonical.display().to_string();
        if let Some(plain) = text.strip_prefix(r"\\?\") {
            forms.push(plain.to_string());
        }
        forms.push(text);
    }
    // Longest first, so that a form is never cut by a shorter one inside it.
    forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
    forms.dedup();
    forms
}

#[test]
#[ignore = "writes app/src/test/fixtures; run on purpose and commit the result"]
fn record_fixtures() {
    // This binary runs this one test, so nothing else reads the variable.
    std::env::set_var("OXIDELENS_NOW", NOW);

    let tmp = tempfile::tempdir().unwrap();
    // The canonical form, so that the case's own paths and the engine's
    // canonical source paths start the same way.
    let dir = std::fs::canonicalize(tmp.path()).unwrap();
    let dir = PathBuf::from(
        dir.display()
            .to_string()
            .strip_prefix(r"\\?\")
            .map(str::to_string)
            .unwrap_or_else(|| dir.display().to_string()),
    );

    let (_events, state) = state();
    state
        .settings
        .update(|s| s.analyst = Some(ANALYST.to_string()))
        .unwrap();
    case::case_new(&state, &dir.join("case.duckdb"), "WS01 triage").unwrap();
    load::load_run(&state, &oxl_fixtures::two_hosts(&dir)).unwrap();
    load::load_run(&state, &oxl_fixtures::host_zones_run(&dir)).unwrap();

    // Seed: a mark, a saved view and a preset.
    let first = g::grid_page(&state, at("seed"), &grid(EVENTS), 5, 0, &utc_page())
        .unwrap()
        .row_ids;
    let rclone = first[4];
    state
        .write(&["marks"], |c| {
            oxl_engine::mark_suspicious(
                c,
                &[rclone],
                &oxl_engine::Actor::Analyst(ANALYST.to_string()),
            )
        })
        .unwrap();
    let logons: ViewDoc = serde_json::from_value(json!({
        "chips": [{ "id": "c1", "label": "EventId = 4624", "enabled": true, "origin": "cell",
                    "filter": { "column": "EventId", "op": "eq", "value": "4624" } }],
        "group_by": ["Channel"],
        "sort": [{ "column": "TimeCreated", "descending": true }],
    }))
    .unwrap();
    views::view_save(&state, "Logons", ViewKind::View, "evtxtriage_*", &logons).unwrap();
    let sysmon: ViewDoc = serde_json::from_value(json!({
        "chips": [{ "id": "c1", "label": "Channel = Sysmon", "enabled": true, "origin": "values",
                    "filter": { "column": "Channel", "op": "eq", "value": "Sysmon" } }],
    }))
    .unwrap();
    views::view_save(&state, "Sysmon only", ViewKind::Preset, "*", &sysmon).unwrap();

    let summary = case::case_summary(&state, at("summary")).unwrap();
    let scrub = Scrub {
        roots: root_forms(&dir),
        case_id: summary.case_id.clone(),
    };
    write(&scrub, "case_summary", &summary);

    // A cell-scope colour rule, so the page carries a matched rule.
    let ruled = |zone: &str| -> PageOptions {
        serde_json::from_value(json!({
            "zone": zone,
            "rules": [{ "id": "sysmon", "name": "Sysmon", "enabled": true,
                        "filter": { "column": "Channel", "op": "eq", "value": "Sysmon" },
                        "colour": "blue", "scope": "cell" }],
        }))
        .unwrap()
    };
    let page = |zone: &str| g::grid_page(&state, at("t1:page"), &grid(EVENTS), 50, 0, &ruled(zone));
    write(&scrub, "grid_page_events", &page("UTC").unwrap());
    write(&scrub, "grid_page_events_ny", &page(NY).unwrap());

    write(
        &scrub,
        "grid_counts_events",
        &g::grid_counts(&state, at("t1:count"), &grid(EVENTS)).unwrap(),
    );
    write(
        &scrub,
        "grid_groups_channel",
        &g::grid_groups(
            &state,
            at("t1:groups"),
            &grid(EVENTS),
            &["Channel".to_string()],
            &[],
            50,
            0,
            "UTC",
        )
        .unwrap(),
    );
    let histogram = |zone: &str| {
        g::grid_histogram(&state, at("t1:hist"), &grid(EVENTS), None, 60, zone).unwrap()
    };
    write(&scrub, "grid_histogram_events", &histogram("UTC"));
    write(&scrub, "grid_histogram_events_ny", &histogram(NY));
    write(
        &scrub,
        "grid_values_channel",
        &g::grid_values(
            &state,
            at("t1:values"),
            &grid(EVENTS),
            "Channel",
            &[],
            None,
            ValueOrder::Commonest,
            50,
            0,
        )
        .unwrap(),
    );
    write(
        &scrub,
        "row_view_first",
        &row::row_view(&state, at("detail"), first[0], "UTC").unwrap(),
    );
    write(
        &scrub,
        "views_list",
        &views::views_list(&state, at("views"), None, 50, 0).unwrap(),
    );
    write(
        &scrub,
        "sessions",
        &case::sessions(&state, at("sessions")).unwrap(),
    );
    write(
        &scrub,
        "host_zones",
        &case::host_zones(&state, at("zones")).unwrap(),
    );

    cmd::case::case_close(&state).unwrap();
}

fn utc_page() -> PageOptions {
    serde_json::from_value(json!({})).unwrap()
}
