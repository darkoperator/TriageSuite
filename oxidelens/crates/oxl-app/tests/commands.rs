//! Every command of the contract, called as the window calls it, over a
//! case with the two-host run and the host-zones run loaded. Each test
//! states the key fields of the answer with the engine tests' numbers; the
//! engine's own tests cover each rule in depth, these that the command
//! reaches it whole.

mod common;

use common::{grid, loaded, loaded_with_zones, state, Loaded};
use oxl_app::cmd::{self, case, grid as g, load, row, views, At};
use oxl_app::contract::COMMANDS;
use oxl_app::events::{CASE_CHANGED, JOB_FINISHED, JOB_STARTED};
use oxl_engine::{
    ChipOrigin, ColumnRole, FileOptions, GridState, MarkState, MergedSource, PageOptions,
    RowStanding, SqlType, TimeSemantics, ValueOrder, ViewDoc, ViewKind,
};
use serde_json::json;

const EVENTS: &str = "evtxtriage_events";
const HAYABUSA: &str = "ext_hayabusa_csv_timeline";
const PREFETCH: &str = "petriage_timeline";
const NY: &str = "America/New_York";

/// Where a test's reads run.
fn at(slot: &str) -> At<'_> {
    At::new("q1", slot)
}

fn utc() -> PageOptions {
    serde_json::from_value(json!({})).unwrap()
}

fn in_zone(zone: &str) -> PageOptions {
    serde_json::from_value(json!({ "zone": zone })).unwrap()
}

fn state_json(value: serde_json::Value) -> GridState {
    serde_json::from_value(value).unwrap()
}

/// The first row of the events grid in its default order (the earliest
/// event, WS01's failed logon at 02:13:58.2040000Z).
fn first_event(l: &Loaded) -> i64 {
    let page = g::grid_page(&l.state, at("t1:page"), &grid(EVENTS), 1, 0, &utc()).unwrap();
    page.row_ids[0]
}

fn column(page: &oxl_engine::GridPage, name: &str) -> usize {
    page.columns.iter().position(|c| c.name == name).unwrap()
}

#[test]
fn every_command_is_registered() {
    assert_eq!(oxl_app::HANDLERS, COMMANDS);
}

#[test]
fn case_new_answers_the_summary() {
    let d = tempfile::tempdir().unwrap();
    let (events, state) = state();
    let summary = case::case_new(&state, &d.path().join("new.duckdb"), "WS01 triage").unwrap();
    assert_eq!(summary.name, "WS01 triage");
    assert_eq!(summary.format, 2);
    assert_eq!(summary.display_zone, "UTC");
    assert!(summary.relations.is_empty() && summary.sources.is_empty());
    assert_eq!(summary.case_id.len(), 32);
    assert!(events.names().is_empty(), "{:?}", events.names());
}

#[test]
fn case_open_reopens_a_closed_case() {
    let l = loaded_with_zones();
    case::case_close(&l.state).unwrap();
    let summary = case::case_open(&l.state, &l.case).unwrap();
    assert_eq!(summary.name, "WS01 triage");
    assert_eq!(summary.relations.len(), 4);
}

#[test]
fn case_close_leaves_no_case() {
    let l = loaded();
    case::case_close(&l.state).unwrap();
    let err = case::case_summary(&l.state, at("summary")).unwrap_err();
    assert_eq!(err.code, "no_case");
    // Closing with nothing open is fine.
    case::case_close(&l.state).unwrap();
}

#[test]
fn case_summary_lists_relations_hosts_and_sources() {
    let l = loaded_with_zones();
    let s = case::case_summary(&l.state, at("summary")).unwrap();
    assert_eq!(s.hosts, ["SRV01", "WS01"]);
    let names: Vec<&str> = s.relations.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        [EVENTS, HAYABUSA, PREFETCH, "retriage_timezoneinfo_system"]
    );
    let events = &s.relations[0];
    assert_eq!(events.rows, 11);
    assert_eq!(events.hosts, ["SRV01", "WS01"]);
    assert_eq!(events.primary_time.as_deref(), Some("TimeCreated"));
    assert_eq!(s.sources.len(), 2);
    assert_eq!(s.sources[0].run_id.as_deref(), Some("20260101000000-aaaa"));
    assert_eq!(s.mark_counts.suspicious, 0);
    assert_eq!(s.display_zone, "UTC");
    assert_eq!(s.path, l.case.display().to_string());
}

#[test]
fn load_run_reports_what_it_loaded() {
    let d = tempfile::tempdir().unwrap();
    let (events, state) = state();
    case::case_new(&state, &d.path().join("case.duckdb"), "c").unwrap();
    let run = oxl_fixtures::two_hosts(d.path());
    let report = load::load_run(&state, &run).unwrap();
    let rows: Vec<(&str, u64)> = report
        .relations
        .iter()
        .map(|r| (r.name.as_str(), r.rows))
        .collect();
    assert_eq!(rows, [(EVENTS, 11), (PREFETCH, 5), (HAYABUSA, 5)]);
    assert_eq!(events.names(), [JOB_STARTED, JOB_FINISHED, CASE_CHANGED]);
}

#[test]
fn load_file_takes_its_options() {
    let l = loaded();
    let file = oxl_fixtures::loose_fw_csv(l.dir.path());
    let options: FileOptions =
        serde_json::from_value(json!({ "name": "fw", "host": "WS01" })).unwrap();
    let report = load::load_file(&l.state, &file, options).unwrap();
    assert_eq!(report.relations.len(), 1);
    assert_eq!(report.relations[0].name, "fw");
    assert_eq!(report.relations[0].hosts, ["WS01"]);
    assert_eq!(report.relations[0].rows, 3);
}

#[test]
fn columns_describe_the_relation() {
    let l = loaded();
    let cols = case::columns(&l.state, at("cols"), EVENTS).unwrap();
    let time = cols.iter().find(|c| c.name == "TimeCreated").unwrap();
    assert_eq!(time.role, ColumnRole::Data);
    assert_eq!(time.sql_type, Some(SqlType::Timestamp));
    assert_eq!(time.time_semantics, Some(TimeSemantics::Utc));
    assert!(cols.iter().any(|c| c.role == ColumnRole::Host));
    let err = case::columns(&l.state, at("cols"), "nowhere").unwrap_err();
    assert_eq!(err.code, "not_found");
}

#[test]
fn sessions_are_the_runs_session_files() {
    let l = loaded_with_zones();
    let sessions = case::sessions(&l.state, at("sessions")).unwrap();
    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s.name, "Execution_Analysis");
    let fed: Vec<(&str, &str)> = s
        .relations
        .iter()
        .map(|r| (r.relation.as_str(), r.host.as_str()))
        .collect();
    // Each host's own Execution_Analysis, merged into one session.
    assert_eq!(
        fed,
        [
            (EVENTS, "SRV01"),
            (EVENTS, "WS01"),
            (PREFETCH, "SRV01"),
            (PREFETCH, "WS01")
        ]
    );
    assert_eq!(s.unmatched, 1);
}

#[test]
fn host_zones_name_each_hosts_zone() {
    let l = loaded_with_zones();
    let zones = case::host_zones(&l.state, at("zones")).unwrap();
    let got: Vec<(&str, &str, Option<&str>)> = zones
        .iter()
        .map(|z| (z.host.as_str(), z.windows_name.as_str(), z.iana.as_deref()))
        .collect();
    assert_eq!(
        got,
        [
            ("SRV01", "UTC", Some("Etc/UTC")),
            ("WS01", "Eastern Standard Time", Some(NY)),
        ]
    );
}

#[test]
fn zone_names_start_with_utc() {
    let names = case::zone_names();
    assert_eq!(names[0], "UTC");
    assert!(names.iter().any(|n| n == NY));
}

#[test]
fn display_zone_set_is_a_write() {
    let l = loaded();
    case::display_zone_set(&l.state, NY).unwrap();
    let s = case::case_summary(&l.state, at("summary")).unwrap();
    assert_eq!(s.display_zone, NY);
    let changed = l.events.named(CASE_CHANGED);
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["kinds"], json!(["display_zone"]));
    let err = case::display_zone_set(&l.state, "Mars/Olympus").unwrap_err();
    assert_eq!(err.code, "invalid");
}

#[test]
fn grid_page_shows_display_times_without_a_suffix() {
    let l = loaded();
    let page = g::grid_page(&l.state, at("t1:page"), &grid(EVENTS), 5, 0, &utc()).unwrap();
    assert_eq!(page.row_ids.len(), 5);
    assert!(page.truncated);
    assert_eq!(page.zone, "UTC");
    let t = column(&page, "TimeCreated");
    assert_eq!(
        page.cells[0][t].text.as_deref(),
        Some("2026-01-05T02:13:58.2040000Z")
    );
    assert_eq!(
        page.cells[0][t].display.as_deref(),
        Some("2026-01-05 02:13:58.2040000")
    );
    let ny = g::grid_page(&l.state, at("t1:page"), &grid(EVENTS), 5, 0, &in_zone(NY)).unwrap();
    assert_eq!(ny.row_ids, page.row_ids);
    assert_eq!(
        ny.cells[0][t].display.as_deref(),
        Some("2026-01-04 21:13:58.2040000")
    );
    let err = g::grid_page(&l.state, at("t1:page"), &grid(EVENTS), 0, 0, &utc()).unwrap_err();
    assert_eq!(err.code, "invalid");
}

#[test]
fn grid_counts_count_rows_and_failed_values() {
    let l = loaded();
    let c = g::grid_counts(&l.state, at("t1:count"), &grid(EVENTS)).unwrap();
    assert_eq!((c.rows, c.total, c.failed_values), (11, 11, 1));
    let ws01 = state_json(json!({ "relation": EVENTS, "hosts": ["WS01"] }));
    let c = g::grid_counts(&l.state, at("t1:count"), &ws01).unwrap();
    assert_eq!((c.rows, c.total), (8, 11));
}

#[test]
fn grid_groups_group_by_channel() {
    let l = loaded();
    let groups = g::grid_groups(
        &l.state,
        at("t1:groups"),
        &grid(EVENTS),
        &["Channel".to_string()],
        &[],
        10,
        0,
        "UTC",
    )
    .unwrap();
    let got: Vec<(Option<&str>, u64)> = groups
        .items
        .iter()
        .map(|g| (g.key.as_deref(), g.count))
        .collect();
    assert_eq!(got, [(Some("Security"), 8), (Some("Sysmon"), 3)]);
    assert!(!groups.truncated);
    let sysmon = &groups.items[1];
    assert_eq!(sysmon.first.as_deref(), Some("2026-01-05 02:15:44.3800000"));
    assert_eq!(sysmon.last.as_deref(), Some("2026-01-05 02:17:02.6450000"));
}

#[test]
fn grid_histogram_buckets_in_the_zone() {
    let l = loaded();
    let h = g::grid_histogram(&l.state, at("t1:hist"), &grid(EVENTS), None, 60, "UTC").unwrap();
    assert_eq!(h.column, "TimeCreated");
    assert_eq!(h.time_semantics, Some(TimeSemantics::Utc));
    assert_eq!(h.buckets.iter().map(|b| b.count).sum::<u64>(), 9);
    assert_eq!(h.untimed, 2);
    let ny = g::grid_histogram(&l.state, at("t1:hist"), &grid(EVENTS), None, 60, NY).unwrap();
    assert_eq!(ny.zone, NY);
    assert_eq!(ny.bucket_seconds, h.bucket_seconds);
    assert_eq!(ny.buckets[0].start, h.buckets[0].start);
    assert!(
        ny.buckets[0].label.starts_with("2026-01-04 21:"),
        "{}",
        ny.buckets[0].label
    );
}

#[test]
fn grid_values_count_a_columns_values() {
    let l = loaded();
    let values = g::grid_values(
        &l.state,
        at("t1:values"),
        &grid(EVENTS),
        "Channel",
        &[],
        None,
        ValueOrder::Commonest,
        10,
        0,
    )
    .unwrap();
    let got: Vec<(Option<&str>, u64)> = values
        .items
        .iter()
        .map(|v| (v.value.as_deref(), v.count))
        .collect();
    assert_eq!(got, [(Some("Security"), 8), (Some("Sysmon"), 3)]);
    // The chip on Channel is set aside, so its own values still count.
    let filtered = state_json(json!({
        "relation": EVENTS,
        "chips": [{ "id": "c1", "label": "Sysmon", "enabled": true, "origin": "values",
                    "filter": { "column": "Channel", "op": "eq", "value": "Sysmon" } }],
    }));
    let values = g::grid_values(
        &l.state,
        at("t1:values"),
        &filtered,
        "Channel",
        &["c1".to_string()],
        Some("sys"),
        ValueOrder::Value,
        10,
        0,
    )
    .unwrap();
    assert_eq!(values.items.len(), 1);
    assert_eq!(values.items[0].count, 3);
}

#[test]
fn grid_empty_columns_lists_columns_with_no_text() {
    let l = loaded();
    let sysmon = state_json(json!({
        "relation": EVENTS,
        "chips": [{ "id": "c1", "label": "no host", "enabled": true, "origin": "advanced",
                    "filter": { "column": "RemoteHost", "op": "is_empty" } }],
    }));
    // The fixture's events carry no identity metadata, so that column is
    // empty everywhere; RemoteHost only under the chip.
    let empty = g::grid_empty_columns(&l.state, at("t1:empty"), &sysmon).unwrap();
    assert_eq!(empty, ["_triage_identity", "RemoteHost"]);
    let all = g::grid_empty_columns(&l.state, at("t1:empty"), &grid(EVENTS)).unwrap();
    assert_eq!(all, ["_triage_identity"]);
}

#[test]
fn grid_matches_and_count_find_in_the_grids_order() {
    let l = loaded();
    let find = state_json(json!({
        "relation": EVENTS,
        "search": { "text": "rclone", "mode": "find" },
    }));
    let found = g::grid_matches(&l.state, at("t1:find"), &find, None, false, 10).unwrap();
    assert_eq!(found.items.len(), 1);
    assert_eq!(found.items[0].offset, 4);
    assert_eq!(found.items[0].ordinal, 1); // offset 4, but the first match
    assert!(!found.truncated);
    // Backward from nothing starts at the last match; from the only match there is nothing before it.
    let back = g::grid_matches(&l.state, at("t1:findb"), &find, None, true, 1).unwrap();
    assert_eq!(back.items[0].offset, 4);
    let before = g::grid_matches(&l.state, at("t1:findb"), &find, Some(4), true, 1).unwrap();
    assert!(before.items.is_empty());
    let n = g::grid_match_count(&l.state, at("t1:findn"), &find).unwrap();
    assert_eq!(n, 1);
    let err = g::grid_match_count(&l.state, at("t1:findn"), &grid(EVENTS)).unwrap_err();
    assert_eq!(err.code, "invalid");
}

#[test]
fn grid_row_offset_answers_the_rows_place_or_none() {
    let l = loaded();
    let state = grid(EVENTS);
    let page = g::grid_page(&l.state, at("t1:off"), &state, 20, 0, &utc()).unwrap();
    let id = page.row_ids[3];
    assert_eq!(
        g::grid_row_offset(&l.state, at("t1:offr"), &state, id).unwrap(),
        Some(3)
    );
    // A row id no source holds is not in the grid.
    assert_eq!(
        g::grid_row_offset(&l.state, at("t1:offr"), &state, -1).unwrap(),
        None
    );
}

#[test]
fn grid_export_is_a_job_that_writes_the_file() {
    let l = loaded();
    let out = l.dir.path().join("events.csv");
    let report = g::grid_export(&l.state, at("export"), &grid(EVENTS), &out, None).unwrap();
    assert_eq!(report.rows, 11);
    assert_eq!(report.path, out);
    let text = std::fs::read_to_string(&out).unwrap();
    assert_eq!(text.lines().count(), 12);
    assert_eq!(l.events.names(), [JOB_STARTED, JOB_FINISHED]);
    let started = &l.events.named(JOB_STARTED)[0];
    assert_eq!(started, &json!({ "kind": "export", "label": "events.csv" }));
    let finished = &l.events.named(JOB_FINISHED)[0];
    assert_eq!(finished["ok"], json!(true));
}

#[test]
fn grid_export_refuses_a_path_inside_the_run() {
    let l = loaded();
    let inside = l.run.join("events.csv");
    let err = g::grid_export(&l.state, at("export"), &grid(EVENTS), &inside, None).unwrap_err();
    assert_eq!(err.code, "refused");
    assert!(!inside.exists());
}

#[test]
fn grid_export_cancelled_before_it_starts_writes_nothing() {
    let l = loaded();
    let out = l.dir.path().join("events.csv");
    cmd::query_cancel(&l.state, &["export-1".to_string()]).unwrap();
    let err = g::grid_export(
        &l.state,
        At::new("export-1", "export"),
        &grid(EVENTS),
        &out,
        None,
    )
    .unwrap_err();
    assert_eq!(err.code, "interrupted");
    assert!(!out.exists());
    let mut partial = out.as_os_str().to_os_string();
    partial.push(".partial");
    assert!(!std::path::Path::new(&partial).exists());
    let finished = &l.events.named(JOB_FINISHED)[0];
    assert_eq!(finished["ok"], json!(false));
}

#[test]
fn date_range_chip_converts_from_the_zone() {
    let l = loaded();
    let chip = g::date_range_chip(
        &l.state,
        at("chip"),
        EVENTS,
        "TimeCreated",
        Some("2026-01-04 21:14:00"),
        Some("2026-01-04 21:20:00"),
        NY,
    )
    .unwrap();
    assert_eq!(chip.origin, ChipOrigin::DateRange);
    let mut st = grid(EVENTS);
    st.chips.push(chip);
    // 02:14:00Z to 02:20:59Z: 02:14:07, 02:15:44, 02:15:46 and 02:17:02.
    let c = g::grid_counts(&l.state, at("t1:count"), &st).unwrap();
    assert_eq!(c.rows, 4);
    let err = g::date_range_chip(
        &l.state,
        at("chip"),
        EVENTS,
        "TimeCreated",
        Some("2026-03-08 02:30:00"),
        None,
        NY,
    )
    .unwrap_err();
    assert_eq!(err.code, "invalid");
}

#[test]
fn default_colour_rules_for_hayabusa_only() {
    let l = loaded();
    let rules = g::default_colour_rules(&l.state, at("rules"), HAYABUSA).unwrap();
    let ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "hayabusa-crit",
            "hayabusa-high",
            "hayabusa-med",
            "hayabusa-low"
        ]
    );
    assert!(g::default_colour_rules(&l.state, at("rules"), EVENTS)
        .unwrap()
        .is_empty());
}

#[test]
fn row_view_shows_the_row_in_the_zone() {
    let l = loaded();
    let id = first_event(&l);
    let view = row::row_view(&l.state, at("detail"), id, NY).unwrap();
    assert_eq!(view.zone, NY);
    assert_eq!(view.detail.row_id, id);
    assert_eq!(view.detail.relation, EVENTS);
    assert_eq!(view.detail.standing, Some(RowStanding::Active));
    let time = view
        .fields
        .iter()
        .find(|f| f.meta.name == "TimeCreated")
        .unwrap();
    assert_eq!(
        time.display.as_deref(),
        Some("2026-01-04 21:13:58.2040000 -05:00")
    );
    assert_eq!(view.provenance.host.as_deref(), Some("WS01"));
    assert_eq!(
        view.provenance.run_id.as_deref(),
        Some("20260101000000-aaaa")
    );
}

#[test]
fn cell_value_splits_a_packed_value() {
    let l = loaded();
    let page = g::grid_page(&l.state, at("t2:page"), &grid(HAYABUSA), 1, 0, &utc()).unwrap();
    let id = page.row_ids[0];
    let value = row::cell_value(&l.state, at("value"), id, "Details").unwrap();
    assert_eq!(
        value.text.as_deref(),
        Some("User: Administrator ¦ SrcIP: 203.0.113.7")
    );
    let keys: Vec<Option<&str>> = value.parts.iter().map(|p| p.key.as_deref()).collect();
    assert_eq!(keys, [Some("User"), Some("SrcIP")]);
}

#[test]
fn around_totals_count_the_hosts_rows_near_the_row() {
    let l = loaded();
    let id = first_event(&l);
    let totals = row::around_totals(&l.state, at("pivot"), id, &[60, 3600]).unwrap();
    let got: Vec<(u64, u64)> = totals.iter().map(|t| (t.seconds, t.rows)).collect();
    // ±60 s: two events and Hayabusa's alert at the same instant. ±1 h:
    // WS01's 7 timed events, 3 prefetch runs and 4 timed Hayabusa alerts
    // (its fifth time is not in the form the app map types).
    assert_eq!(got, [(60, 3), (3600, 14)]);
}

#[test]
fn merged_page_lists_rows_around_a_time() {
    let l = loaded();
    let source: MergedSource = serde_json::from_value(json!({
        "around": { "host": "WS01", "at": "2026-01-05T02:14:00Z", "window_seconds": 60 }
    }))
    .unwrap();
    let merged = row::merged_page(&l.state, at("merged"), &source, 50, 0, NY).unwrap();
    let got: Vec<(&str, Option<&str>)> = merged
        .items
        .iter()
        .map(|m| (m.row.relation.as_str(), m.display_time.as_deref()))
        .collect();
    // Hayabusa's timeline is timed through the app map, so its alert at the
    // same instant is among them; ties go by relation name.
    assert_eq!(
        got,
        [
            (EVENTS, Some("2026-01-04 21:13:58.204000")),
            (HAYABUSA, Some("2026-01-04 21:13:58.204000")),
            (EVENTS, Some("2026-01-04 21:14:07.911000")),
        ]
    );
    assert!(merged.items.iter().all(|m| m.row.mark.is_none()));
    let bad = serde_json::from_value::<MergedSource>(json!({
        "around": { "host": "WS01", "at": "2026-01-05T02:14:00Z", "window_seconds": 60, "x": 1 }
    }));
    assert!(bad.is_err(), "an unknown field is refused");
}

fn logons() -> ViewDoc {
    serde_json::from_value(json!({
        "chips": [{ "id": "c1", "label": "4624", "enabled": true, "origin": "cell",
                    "filter": { "column": "EventId", "op": "eq", "value": "4624" } }],
        "group_by": ["Channel"],
    }))
    .unwrap()
}

#[test]
fn view_save_and_views_list() {
    let l = loaded();
    let saved = views::view_save(
        &l.state,
        "Logons",
        ViewKind::View,
        "evtxtriage_*",
        &logons(),
    )
    .unwrap();
    assert_eq!(saved.name, "Logons");
    assert_eq!(saved.kind, ViewKind::View);
    assert_eq!(l.events.named(CASE_CHANGED)[0]["kinds"], json!(["views"]));
    let listed = views::views_list(&l.state, at("views"), Some(EVENTS), 10, 0).unwrap();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].id, saved.id);
    let none = views::views_list(&l.state, at("views"), Some(PREFETCH), 10, 0).unwrap();
    assert!(none.items.is_empty());
    let all = views::views_list(&l.state, at("views"), None, 10, 0).unwrap();
    assert_eq!(all.items.len(), 1);
}

#[test]
fn views_matching_lists_views_with_their_relations() {
    let l = loaded();
    views::view_save(
        &l.state,
        "Logons",
        ViewKind::View,
        "evtxtriage_*",
        &logons(),
    )
    .unwrap();
    views::view_save(&l.state, "Nowhere", ViewKind::View, "nomatch_*", &logons()).unwrap();
    let got = views::views_matching(&l.state, at("views"), 10, 0).unwrap();
    assert_eq!(got.items.len(), 1);
    assert_eq!(got.items[0].view.name, "Logons");
    assert_eq!(got.items[0].relations, [EVENTS]);
    assert!(!got.truncated);
}

#[test]
fn view_delete_removes_it() {
    let l = loaded();
    let saved = views::view_save(&l.state, "Logons", ViewKind::View, "*", &logons()).unwrap();
    views::view_delete(&l.state, saved.id).unwrap();
    let all = views::views_list(&l.state, at("views"), None, 10, 0).unwrap();
    assert!(all.items.is_empty());
    let changed = l.events.named(CASE_CHANGED);
    assert_eq!(changed.len(), 2);
    assert_eq!(changed[1]["kinds"], json!(["views"]));
    assert_eq!(
        views::view_delete(&l.state, saved.id).unwrap_err().code,
        "refused"
    );
}

/// A grid export cancelled part way through its file (once the partial file
/// has its first bytes, which the writer flushes only after the row-0 check)
/// answers `interrupted` and leaves neither the file nor its partial. The
/// cancel both sets the export's flag and interrupts DuckDB, so either one
/// stops it; the partial file is watched while it grows, and the bound on
/// what was seen says it stopped part way, not after all 400,000 rows. (The
/// engine's `export_stops_mid_file` pins the flag on its own.)
#[test]
fn grid_export_cancelled_mid_file_is_interrupted() {
    use std::sync::atomic::{AtomicBool, Ordering};
    const ROWS: u32 = 400_000;
    let l = loaded();
    let source = l.dir.path().join("many.csv");
    let mut text = String::from("n\n");
    for i in 0..ROWS {
        text.push_str(&format!("{i:07}\n"));
    }
    std::fs::write(&source, text).unwrap();
    let options: FileOptions = serde_json::from_value(json!({ "name": "many" })).unwrap();
    let rel = load::load_file(&l.state, &source, options)
        .unwrap()
        .relations[0]
        .name
        .clone();
    let mut state = grid(&rel);
    state.columns = Some(vec!["n".into()]);
    let out = l.dir.path().join("stopped.csv");
    let mut partial = out.as_os_str().to_os_string();
    partial.push(".partial");
    let partial = std::path::PathBuf::from(partial);

    let done = AtomicBool::new(false);
    let (answer, largest) = std::thread::scope(|s| {
        let watcher = s.spawn(|| {
            let (mut largest, mut cancelled) = (0u64, false);
            while !done.load(Ordering::SeqCst) {
                if let Ok(meta) = std::fs::metadata(&partial) {
                    largest = largest.max(meta.len());
                    if meta.len() > 0 && !cancelled {
                        cmd::query_cancel(&l.state, &["export-mid".to_string()]).unwrap();
                        cancelled = true;
                    }
                }
                std::thread::yield_now();
            }
            (largest, cancelled)
        });
        let answer = g::grid_export(
            &l.state,
            At::new("export-mid", "export"),
            &state,
            &out,
            None,
        );
        done.store(true, Ordering::SeqCst);
        (answer, watcher.join().unwrap())
    });
    let (largest, cancelled) = largest;
    assert!(cancelled, "the watcher never saw a byte");
    assert_eq!(answer.unwrap_err().code, "interrupted");
    assert!(!out.exists());
    assert!(!partial.exists());
    // Eight bytes a row: what was seen stayed far below the whole file.
    let whole = u64::from(ROWS) * 8;
    assert!(largest < whole / 2, "{largest} of {whole} bytes written");
}

#[test]
fn view_apply_disables_what_the_dataset_lacks() {
    let l = loaded();
    let saved = views::view_save(&l.state, "Logons", ViewKind::View, "*", &logons()).unwrap();
    let applied = views::view_apply(&l.state, at("apply"), saved.id, EVENTS).unwrap();
    assert_eq!(applied.state.relation, EVENTS);
    assert!(applied.state.chips[0].enabled);
    assert!(applied.warnings.is_empty());
    let applied = views::view_apply(&l.state, at("apply"), saved.id, PREFETCH).unwrap();
    assert!(!applied.state.chips[0].enabled);
    let parts: Vec<&str> = applied.warnings.iter().map(|w| w.part.as_str()).collect();
    assert_eq!(parts, ["chip:c1", "group_by"]);
}

#[test]
fn view_export_and_import_round_trip_through_a_file() {
    let l = loaded();
    let saved = views::view_save(&l.state, "Logons", ViewKind::View, "*", &logons()).unwrap();
    let file = l.dir.path().join("logons.oxlview.json");
    views::view_export(&l.state, at("vexport"), saved.id, &file).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("\"oxidelens_view\": 1"), "{text}");
    views::view_delete(&l.state, saved.id).unwrap();
    let imported = views::view_import(&l.state, &file).unwrap();
    assert_eq!(imported.name, "Logons");
    assert_eq!(imported.doc, logons());
    // Never inside a loaded source; never over the cap.
    let inside = l.run.join("logons.json");
    let err = views::view_export(&l.state, at("vexport"), imported.id, &inside).unwrap_err();
    assert_eq!(err.code, "refused");
    assert!(!inside.exists());
    let big = l.dir.path().join("big.json");
    std::fs::write(&big, vec![b' '; oxl_engine::MAX_VIEW_FILE_BYTES + 1]).unwrap();
    assert_eq!(
        views::view_import(&l.state, &big).unwrap_err().code,
        "invalid"
    );
    assert_eq!(
        views::view_import(&l.state, &l.dir.path().join("missing.json"))
            .unwrap_err()
            .code,
        "not_found"
    );
}

/// A view export stays registered as its query until its file is in place:
/// cancelled while it waits for the write gate to rename (a write holds the
/// gate), it answers `interrupted`, removes its partial file and leaves
/// nothing at the path. Registered only while it read the case, its cancel
/// would be remembered for a query that never comes, and the file would
/// land once the write ended.
#[test]
fn view_export_can_be_cancelled() {
    use std::sync::mpsc;
    let l = loaded();
    let saved = views::view_save(&l.state, "Logons", ViewKind::View, "*", &logons()).unwrap();
    let file = l.dir.path().join("cancelled.oxlview.json");
    let mut partial = file.as_os_str().to_os_string();
    partial.push(".partial");
    let partial = std::path::PathBuf::from(partial);

    let (holding, held) = mpsc::channel::<()>();
    let (release, released) = mpsc::channel::<()>();
    std::thread::scope(|s| {
        let state = &l.state;
        let writer = s.spawn(move || {
            state.write(&[], |_| {
                holding.send(()).unwrap();
                released.recv().unwrap();
                Ok(())
            })
        });
        held.recv().unwrap();
        let export =
            s.spawn(|| views::view_export(state, At::new("vexport-1", "vexport"), saved.id, &file));
        // The partial file is written before the export waits for the gate.
        let start = std::time::Instant::now();
        while !partial.exists() {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(30),
                "the export never wrote its partial file"
            );
            std::thread::yield_now();
        }
        cmd::query_cancel(state, &["vexport-1".to_string()]).unwrap();
        // The write ends after the cancel, so an export the cancel missed
        // gets the gate and lands its file.
        release.send(()).unwrap();
        writer.join().unwrap().unwrap();
        let answer = export.join().unwrap();
        assert_eq!(answer.unwrap_err().code, "interrupted");
    });
    assert!(!partial.exists());
    assert!(!file.exists());
}

#[test]
fn settings_get_and_update_answer_the_reply() {
    let l = loaded();
    let reply = cmd::settings::settings_get(&l.state).unwrap();
    assert_eq!(reply.settings.font_size, 12);
    assert!(reply.damaged.is_none());
    let mut settings = reply.settings;
    settings.font_size = 14;
    let reply = cmd::settings::settings_update(&l.state, settings).unwrap();
    assert_eq!(reply.settings.font_size, 14);
}

#[test]
fn query_cancel_checks_its_ids() {
    let l = loaded();
    cmd::query_cancel(&l.state, &["q1".to_string()]).unwrap();
    let err = case::case_summary(&l.state, At::new("q1", "summary")).unwrap_err();
    assert_eq!(err.code, "interrupted");
    assert_eq!(
        cmd::query_cancel(&l.state, &["a b".to_string()])
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[test]
fn marks_reach_the_page() {
    // A mark set in the case shows on the page the command answers.
    let l = loaded();
    let id = first_event(&l);
    {
        let state = l.state.clone();
        state
            .write(&["marks"], |c| {
                oxl_engine::mark_suspicious(c, &[id], &oxl_engine::Actor::Analyst("a".into()))
            })
            .unwrap();
    }
    let page = g::grid_page(&l.state, at("t1:page"), &grid(EVENTS), 1, 0, &utc()).unwrap();
    assert_eq!(page.marks[0], Some(MarkState::Suspicious));
    let s = case::case_summary(&l.state, at("summary")).unwrap();
    assert_eq!(s.mark_counts.suspicious, 1);
}
