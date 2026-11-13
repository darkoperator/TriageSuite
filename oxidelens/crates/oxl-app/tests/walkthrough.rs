//! The whole app, end to end: an analyst's first session with a case, as
//! the window drives it, step by step through the same commands.
//!
//! New case in a scratch folder → load a run → open `evtxtriage_events` →
//! filter row `Security` on `Channel` → Values tab: exclude `4624` from
//! `EventId` → group by `Channel`, then `EventId` → brush the histogram →
//! switch the display zone to `America/New_York` and check a time → save a
//! view → close and reopen the case → apply the view → export CSV → open a
//! run session.
//!
//! `walk` runs the steps and checks what holds for any data (a narrowing
//! never adds rows, groups add up to the rows, a brush selects exactly its
//! buckets, the zone is kept with the case, an applied view shows what was
//! saved, an export writes what the grid counts). `the_walkthrough_on_two_hosts`
//! runs it on the two-host fixture and states every number.
//!
//! The same walk runs on a copy of a real case, and the grid's costs are
//! measured there, in two ignored tests that take the case from the
//! environment and print what they saw:
//!
//! ```text
//! OXL_WALKTHROUGH_CASE=<copy of a case> \
//!   cargo test -p oxl-app --test walkthrough -- --ignored --nocapture real_case_walkthrough
//! OXL_PERF_CASE=<copy of a case> [OXL_PERF_VALUES_RELATION=r OXL_PERF_VALUES_COLUMN=c]
//!   [OXL_PERF_GROUP=c] [OXL_PERF_FILTER_COLUMN=c OXL_PERF_FILTER_TEXT=t] \
//!   [OXL_PERF_LOAD_RUN=<a run output root to load into the case first>] \
//!   cargo test -p oxl-app --test walkthrough -- --ignored --nocapture real_case_performance
//! ```
//!
//! Both open the case through the app, which upgrades a format-1 case in
//! place, and the walkthrough writes to it (the display zone, a saved view):
//! give them a copy, never the analyst's case.

mod common;

use common::{grid, state};
use oxl_app::cmd::{case, grid as g, load, row, views, At};
use oxl_app::events::{JOB_FINISHED, JOB_STARTED};
use oxl_app::state::AppState;
use oxl_engine::{
    Bucket, Chip, ChipOrigin, ColumnRole, GridState, PageOptions, TimeSemantics, ValueOrder,
    ViewDoc, ViewKind,
};
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What the walk does, by name: the relation, columns and values the
/// analyst picks.
struct Plan<'a> {
    relation: &'a str,
    filter_column: &'a str,
    filter_text: &'a str,
    values_column: &'a str,
    exclude: &'a str,
    group_by: [&'a str; 2],
    zone: &'a str,
    view_name: &'a str,
    view_pattern: &'a str,
}

const PLAN: Plan<'static> = Plan {
    relation: "evtxtriage_events",
    filter_column: "Channel",
    filter_text: "Security",
    values_column: "EventId",
    exclude: "4624",
    group_by: ["Channel", "EventId"],
    zone: "America/New_York",
    view_name: "Security without 4624",
    view_pattern: "evtxtriage_*",
};

/// The bucket cap the window asks for on a wide strip.
const MAX_BUCKETS: u32 = 240;
/// The rows the window asks for at a time (`app/src/grid/blocks.ts`).
const BLOCK_ROWS: u32 = 200;
/// The groups the window asks for at a time (`app/src/grid/groupTree.ts`).
const GROUP_PAGE: u32 = 200;
/// The values the Values tab asks for (`app/src/grid/ValuesTab.tsx`).
const VALUES_LIMIT: u32 = 300;

/// One group: its key and count.
type Group = (Option<String>, u64);

/// One tab a session opens: its relation, the hosts it is scoped to, and
/// the rows it shows.
type SessionTab = (String, Vec<String>, u64);

/// What each step saw, and how long it took.
#[derive(Default)]
struct Seen {
    steps: Vec<(&'static str, Duration, String)>,
    total: u64,
    filtered: u64,
    /// The excluded value's count under the filter-row chip, as the Values
    /// tab listed it.
    excluded_value: u64,
    excluded: u64,
    level1: Vec<Group>,
    level2: Vec<Group>,
    buckets: Vec<Bucket>,
    untimed: u64,
    brushed: u64,
    /// The first row's primary time: UTC, then in the zone, then in the
    /// detail pane in the zone.
    time_utc: Option<String>,
    time_zone: Option<String>,
    time_detail: Option<String>,
    reopened_zone: String,
    applied: u64,
    exported: u64,
    /// The first session: its name, each relation with its hosts and the
    /// rows its tab shows, and how many of its files feed nothing.
    session: Option<(String, Vec<SessionTab>, u32)>,
}

impl Seen {
    fn step<T>(&mut self, name: &'static str, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = f();
        self.steps.push((name, start.elapsed(), String::new()));
        out
    }

    /// Say what the last step saw.
    fn saw(&mut self, what: String) {
        self.steps.last_mut().expect("a step ran").2 = what;
    }

    fn print(&self, title: &str) {
        println!("\n{title}");
        println!("| step | ms | saw |");
        println!("|---|---:|---|");
        for (name, took, what) in &self.steps {
            println!("| {name} | {:.0} | {what} |", took.as_secs_f64() * 1000.0);
        }
    }
}

fn at(slot: &str) -> At<'_> {
    At::new("walk", slot)
}

fn zone(zone: &str) -> PageOptions {
    serde_json::from_value(json!({ "zone": zone })).unwrap()
}

fn chip(id: &str, label: &str, origin: &str, filter: serde_json::Value) -> Chip {
    serde_json::from_value(json!({
        "id": id, "label": label, "enabled": true, "origin": origin, "filter": filter,
    }))
    .unwrap()
}

fn counts(state: &AppState, grid: &GridState) -> u64 {
    g::grid_counts(state, at("tab:walk:counts"), grid)
        .unwrap()
        .rows
}

fn groups(state: &AppState, grid: &GridState, by: &[&str], parent: &[Group]) -> (Vec<Group>, bool) {
    let by: Vec<String> = by.iter().map(|s| s.to_string()).collect();
    let parent: Vec<Option<String>> = parent.iter().map(|(k, _)| k.clone()).collect();
    let listing = g::grid_groups(
        state,
        at("tab:walk:groups"),
        grid,
        &by,
        &parent,
        GROUP_PAGE,
        0,
        "UTC",
    )
    .unwrap();
    let items = listing
        .items
        .iter()
        .map(|g| (g.key.clone(), g.count))
        .collect();
    (items, listing.truncated)
}

/// The walk from opening the relation on, over the case open in `state` at
/// `case`, exporting to `export`. Checks what holds for any data; the
/// caller checks the numbers.
fn walk(state: &Arc<AppState>, case_path: &Path, export: &Path, plan: &Plan<'_>) -> Seen {
    let mut seen = Seen::default();
    let mut st = grid(plan.relation);

    // Open the relation: its columns, the status bar's counts, the first block.
    let (total, first) = seen.step("open the relation", || {
        let cols = case::columns(state, at("cols"), plan.relation).unwrap();
        assert!(cols.iter().any(|c| c.name == plan.filter_column));
        assert!(cols.iter().any(|c| c.name == plan.values_column));
        let c = g::grid_counts(state, at("tab:walk:counts"), &st).unwrap();
        let page = g::grid_page(
            state,
            at("tab:walk:page:0"),
            &st,
            BLOCK_ROWS,
            0,
            &zone("UTC"),
        )
        .unwrap();
        (c.total, page.row_ids.len() as u64)
    });
    assert_eq!(first, total.min(u64::from(BLOCK_ROWS)));
    seen.total = total;
    seen.saw(format!("{total} rows; first block {first}"));

    // The filter row: a case-insensitive `contains`, as `filterRowChip` makes it.
    let filter_row = chip(
        "fr-walk",
        &format!("{} contains \"{}\"", plan.filter_column, plan.filter_text),
        "filter_row",
        json!({ "column": plan.filter_column, "op": "contains", "value": plan.filter_text,
                "case_sensitive": false }),
    );
    st.chips.push(filter_row);
    let (filtered, page) = seen.step("filter row", || {
        let n = counts(state, &st);
        let page = g::grid_page(
            state,
            at("tab:walk:page:0"),
            &st,
            BLOCK_ROWS,
            0,
            &zone("UTC"),
        )
        .unwrap();
        (n, page)
    });
    assert!(filtered <= total);
    let fc = page
        .columns
        .iter()
        .position(|c| c.name == plan.filter_column)
        .unwrap();
    let needle = plan.filter_text.to_lowercase();
    assert!(page.cells.iter().all(|r| r[fc]
        .text
        .as_deref()
        .is_some_and(|t| t.to_lowercase().contains(&needle))));
    seen.filtered = filtered;
    seen.saw(format!("{filtered} of {total} rows"));

    // The Values tab: the column's values under the other chips, then exclude one.
    let values_id = "vl-walk".to_string();
    let excluded_value = seen.step("values tab: exclude", || {
        let listed = g::grid_values(
            state,
            at("tab:walk:values"),
            &st,
            plan.values_column,
            std::slice::from_ref(&values_id),
            Some(plan.exclude),
            ValueOrder::Commonest,
            VALUES_LIMIT,
            0,
        )
        .unwrap();
        listed
            .items
            .iter()
            .find(|v| v.value.as_deref() == Some(plan.exclude))
            .map_or(0, |v| v.count)
    });
    st.chips.push(chip(
        &values_id,
        &format!("{} is not \"{}\"", plan.values_column, plan.exclude),
        "values",
        json!({ "column": plan.values_column, "op": "not_in", "value": [plan.exclude],
                "case_sensitive": true }),
    ));
    let excluded = counts(state, &st);
    assert_eq!(
        excluded,
        filtered - excluded_value,
        "the exclusion removed exactly its rows"
    );
    seen.excluded_value = excluded_value;
    seen.excluded = excluded;
    seen.saw(format!(
        "{} listed {excluded_value} times; {excluded} rows left",
        plan.exclude
    ));

    // Group by the first column, then open the largest group by the second.
    let ((level1, more1), (level2, more2)) = seen.step("group by two columns", || {
        let l1 = groups(state, &st, &plan.group_by[..1], &[]);
        let parent = l1.0.first().cloned().into_iter().collect::<Vec<_>>();
        let l2 = if parent.is_empty() {
            (Vec::new(), false)
        } else {
            groups(state, &st, &plan.group_by, &parent)
        };
        (l1, l2)
    });
    if !more1 {
        assert_eq!(level1.iter().map(|g| g.1).sum::<u64>(), excluded);
    }
    if !more2 {
        if let Some((_, parent)) = level1.first() {
            assert_eq!(level2.iter().map(|g| g.1).sum::<u64>(), *parent);
        }
    }
    if plan.group_by[1] == plan.values_column {
        assert!(level2.iter().all(|g| g.0.as_deref() != Some(plan.exclude)));
    }
    seen.saw(format!(
        "{} groups{}; the first holds {} groups{}",
        level1.len(),
        if more1 { "+" } else { "" },
        level2.len(),
        if more2 { "+" } else { "" },
    ));
    seen.level1 = level1;
    seen.level2 = level2;

    // The histogram, then a brush over the first half of its buckets.
    let h = seen.step("histogram", || {
        g::grid_histogram(state, at("tab:walk:hist"), &st, None, MAX_BUCKETS, "UTC").unwrap()
    });
    assert_eq!(
        h.buckets.iter().map(|b| b.count).sum::<u64>() + h.untimed,
        excluded,
        "every row is in a bucket or untimed"
    );
    seen.saw(format!(
        "{} buckets of {} s over {} slots; {} untimed",
        h.buckets.len(),
        h.bucket_seconds,
        h.slots,
        h.untimed
    ));
    let take = h.buckets.len().div_ceil(2);
    let brushed_buckets = &h.buckets[..take];
    let brush = seen.step("brush the histogram", || {
        let (first, last) = (&brushed_buckets[0], &brushed_buckets[take - 1]);
        let mut made = g::date_range_chip(
            state,
            at("tab:walk:hist-brush"),
            plan.relation,
            &h.column,
            Some(&first.label),
            Some(&last.end_label),
            "UTC",
        )
        .unwrap();
        // As the strip does: the chip is the engine's, its origin the histogram's.
        made.origin = ChipOrigin::Histogram;
        made
    });
    st.chips.push(brush);
    let brushed = counts(state, &st);
    assert_eq!(
        brushed,
        brushed_buckets.iter().map(|b| b.count).sum::<u64>(),
        "the brush selects exactly its buckets"
    );
    seen.brushed = brushed;
    seen.saw(format!("{take} buckets brushed: {brushed} rows"));
    seen.buckets = h.buckets.clone();
    seen.untimed = h.untimed;

    // The display zone: a write kept with the case; times shown in it.
    seen.step("display zone", || {
        case::display_zone_set(state, plan.zone).unwrap();
    });
    let summary = case::case_summary(state, at("summary")).unwrap();
    assert_eq!(summary.display_zone, plan.zone);
    let utc_page = g::grid_page(
        state,
        at("tab:walk:page:0"),
        &st,
        BLOCK_ROWS,
        0,
        &zone("UTC"),
    )
    .unwrap();
    let zone_page = g::grid_page(
        state,
        at("tab:walk:page:0"),
        &st,
        BLOCK_ROWS,
        0,
        &zone(plan.zone),
    )
    .unwrap();
    assert_eq!(
        utc_page.row_ids, zone_page.row_ids,
        "a zone never reorders rows"
    );
    assert_eq!(zone_page.zone, plan.zone);
    let time_col = utc_page
        .columns
        .iter()
        .position(|c| c.name == h.column)
        .unwrap();
    if let Some(&first_id) = zone_page.row_ids.first() {
        let utc_cell = &utc_page.cells[0][time_col];
        let zone_cell = &zone_page.cells[0][time_col];
        let detail = row::row_view(state, at("detail"), first_id, plan.zone).unwrap();
        let field = detail
            .fields
            .iter()
            .find(|f| f.meta.name == h.column)
            .unwrap();
        if field.meta.time_semantics == Some(TimeSemantics::Utc) && utc_cell.display.is_some() {
            assert_ne!(
                utc_cell.display, zone_cell.display,
                "the zone moved the time"
            );
            let shown = field.display.as_deref().unwrap();
            assert!(
                shown.starts_with(zone_cell.display.as_deref().unwrap()),
                "the detail pane shows the grid's time: {shown}"
            );
            assert!(
                shown.ends_with(" -05:00") || shown.ends_with(" -04:00"),
                "the detail pane names the offset: {shown}"
            );
        }
        seen.time_utc = utc_cell.display.clone();
        seen.time_zone = zone_cell.display.clone();
        seen.time_detail = field.display.clone();
    }
    seen.saw(format!(
        "{} → {} (detail {})",
        seen.time_utc.as_deref().unwrap_or("-"),
        seen.time_zone.as_deref().unwrap_or("-"),
        seen.time_detail.as_deref().unwrap_or("-"),
    ));

    // Save what the tab shows as a view, as `Grid.tsx`'s saveView does.
    let rules = g::default_colour_rules(state, at("rules"), plan.relation).unwrap();
    let doc: ViewDoc = serde_json::from_value(json!({
        "chips": st.chips, "search": st.search, "hosts": st.hosts,
        "group_by": plan.group_by, "sort": st.sort, "layout": null, "colour_rules": rules,
    }))
    .unwrap();
    let saved = seen.step("save a view", || {
        views::view_save(
            state,
            plan.view_name,
            ViewKind::View,
            plan.view_pattern,
            &doc,
        )
        .unwrap()
    });
    assert_eq!(saved.doc, doc);
    seen.saw(format!("view {} saved", saved.id));

    // Close the case and open it again: the zone and the view are kept with it.
    let reopened = seen.step("close and reopen", || {
        case::case_close(state).unwrap();
        assert_eq!(
            case::case_summary(state, at("summary")).unwrap_err().code,
            "no_case"
        );
        case::case_open(state, case_path).unwrap()
    });
    assert_eq!(
        reopened.display_zone, plan.zone,
        "the zone is kept with the case"
    );
    assert_eq!(reopened.format, 2);
    seen.reopened_zone = reopened.display_zone.clone();
    let offered = views::views_matching(state, at("views"), 50, 0).unwrap();
    let entry = offered
        .items
        .iter()
        .find(|m| m.view.id == saved.id)
        .expect("the saved view is offered after a reopen");
    assert!(entry.relations.iter().any(|r| r == plan.relation));
    seen.saw(format!("zone {}; view offered", reopened.display_zone));

    // Apply the view to the relation: everything it saved, nothing missing.
    let applied = seen.step("apply the view", || {
        views::view_apply(state, at("views:apply"), saved.id, plan.relation).unwrap()
    });
    assert!(applied.warnings.is_empty(), "{:?}", applied.warnings);
    assert_eq!(applied.state.chips, st.chips);
    assert_eq!(applied.group_by, plan.group_by);
    let applied_rows = counts(state, &applied.state);
    assert_eq!(
        applied_rows, brushed,
        "the applied view shows what was saved"
    );
    seen.applied = applied_rows;
    seen.saw(format!(
        "{} chips, grouped by {}; {applied_rows} rows",
        applied.state.chips.len(),
        applied.group_by.join(" > ")
    ));

    // Export what the grid shows, as a job.
    let _ = std::fs::remove_file(export);
    let report = seen.step("export CSV", || {
        g::grid_export(state, at("tab:walk:export"), &applied.state, export, None).unwrap()
    });
    assert_eq!(
        report.rows, applied_rows,
        "the export writes what the grid counts"
    );
    assert!(export.is_file());
    seen.exported = report.rows;
    seen.saw(format!(
        "{} rows, {} bytes",
        report.rows,
        std::fs::metadata(export).unwrap().len()
    ));

    // Open the first run session that has a tab with rows: one tab per
    // relation it feeds, scoped to its hosts. A run's sessions are the
    // window's way in, so a case that offers none, or none with a tab that
    // shows rows, fails.
    let session = seen.step("open a session", || {
        let sessions = case::sessions(state, at("sessions")).unwrap();
        assert!(!sessions.is_empty(), "the case offers no session");
        sessions
            .iter()
            .map(|s| {
                let mut tabs: Vec<SessionTab> = Vec::new();
                for r in &s.relations {
                    match tabs.iter_mut().find(|t| t.0 == r.relation) {
                        Some(t) if !t.1.contains(&r.host) => t.1.push(r.host.clone()),
                        Some(_) => {}
                        None => tabs.push((r.relation.clone(), vec![r.host.clone()], 0)),
                    }
                }
                for tab in &mut tabs {
                    let mut scoped = grid(&tab.0);
                    scoped.hosts = tab.1.clone();
                    tab.2 = counts(state, &scoped);
                    assert!(tab.2 <= counts(state, &grid(&tab.0)));
                }
                (s.name.clone(), tabs, s.unmatched)
            })
            .find(|(_, tabs, _)| tabs.iter().any(|t| t.2 > 0))
            .unwrap_or_else(|| {
                panic!(
                    "none of the case's {} sessions has a tab with rows",
                    sessions.len()
                )
            })
    });
    seen.saw(format!(
        "{}: {} tabs ({} rows); {} files unmatched",
        session.0,
        session.1.len(),
        session.1.iter().map(|t| t.2).sum::<u64>(),
        session.2
    ));
    seen.session = Some(session);
    seen
}

#[test]
fn the_walkthrough_on_two_hosts() {
    let (events, state) = state();
    let scratch = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::two_hosts(scratch.path());
    let case_path = scratch.path().join("walk.case.duckdb");

    let summary = case::case_new(&state, &case_path, "WS01 triage").unwrap();
    assert_eq!((summary.format, summary.display_zone.as_str()), (2, "UTC"));
    let report = load::load_run(&state, &run).unwrap();
    assert_eq!(report.relations.len(), 3);
    events.clear();

    let seen = walk(&state, &case_path, &scratch.path().join("walk.csv"), &PLAN);
    seen.print("two_hosts");

    // 11 events; 8 on the Security channel, 3 of them 4624 (WS01 1, SRV01 2).
    assert_eq!((seen.total, seen.filtered), (11, 8));
    assert_eq!((seen.excluded_value, seen.excluded), (3, 5));
    let key = |k: &str| Some(k.to_string());
    assert_eq!(seen.level1, [(key("Security"), 5)]);
    let mut level2 = seen.level2.clone();
    level2.sort();
    assert_eq!(
        level2,
        [
            (key("1102"), 2),
            (key("4625"), 1),
            (key("4688"), 1),
            (key("5140"), 1)
        ]
    );
    // Four timed rows in 10-second buckets (02:13:50, 02:22:00, 02:33:20) and
    // `not-a-time`; the brush takes the first two buckets: the failed logon
    // and the share access.
    let starts: Vec<(&str, u64)> = seen
        .buckets
        .iter()
        .map(|b| (b.label.as_str(), b.count))
        .collect();
    assert_eq!(
        starts,
        [
            ("2026-01-05 02:13:50", 1),
            ("2026-01-05 02:22:00", 1),
            ("2026-01-05 02:33:20", 2)
        ]
    );
    assert_eq!((seen.untimed, seen.brushed), (1, 2));
    // The failed logon at 02:13:58.2040000Z, in New York.
    assert_eq!(
        seen.time_utc.as_deref(),
        Some("2026-01-05 02:13:58.2040000")
    );
    assert_eq!(
        seen.time_zone.as_deref(),
        Some("2026-01-04 21:13:58.2040000")
    );
    assert_eq!(
        seen.time_detail.as_deref(),
        Some("2026-01-04 21:13:58.2040000 -05:00")
    );
    assert_eq!(seen.reopened_zone, "America/New_York");
    assert_eq!((seen.applied, seen.exported), (2, 2));
    let csv = std::fs::read_to_string(scratch.path().join("walk.csv")).unwrap();
    assert_eq!(csv.lines().count(), 3, "{csv}");
    assert!(csv.contains("4625") && csv.contains("5140"), "{csv}");
    // The export is a job; nothing else announced one.
    let jobs: Vec<String> = events
        .names()
        .into_iter()
        .filter(|n| n == JOB_STARTED || n == JOB_FINISHED)
        .collect();
    assert_eq!(jobs, [JOB_STARTED, JOB_FINISHED]);
    // The run's session: each host's Execution_Analysis (in its own
    // Processed-* folder) merged, so each tab is scoped to both hosts' rows;
    // one file is elsewhere.
    let (name, tabs, unmatched) = seen.session.expect("the run has a session");
    assert_eq!(name, "Execution_Analysis");
    let tabs: Vec<(&str, Vec<&str>, u64)> = tabs
        .iter()
        .map(|(r, h, n)| (r.as_str(), h.iter().map(String::as_str).collect(), *n))
        .collect();
    assert_eq!(
        tabs,
        [
            ("evtxtriage_events", vec!["SRV01", "WS01"], 11),
            ("petriage_timeline", vec!["SRV01", "WS01"], 5)
        ]
    );
    assert_eq!(unmatched, 1);
}

/// The case an ignored test runs on, from `var`. An ignored test runs only
/// when asked for, so a missing variable is a mistake in how it was run, and
/// fails it: a green run always means the case was walked.
fn case_from_env(var: &str) -> std::path::PathBuf {
    match std::env::var_os(var) {
        Some(p) if !p.is_empty() => p.into(),
        _ => panic!("{var} is not set: name a copy of a case to run this test on"),
    }
}

/// The walkthrough on a copy of a real case, opened through the app (which
/// upgrades a format-1 case in place). It exports next to the case.
#[test]
#[ignore = "needs a copy of a real case in OXL_WALKTHROUGH_CASE"]
fn real_case_walkthrough() {
    let case_path = case_from_env("OXL_WALKTHROUGH_CASE");
    let (_events, state) = state();
    let start = Instant::now();
    let summary = case::case_open(&state, &case_path).unwrap();
    println!(
        "opened in {:.0} ms: format {}, {} relations, {} sources",
        start.elapsed().as_secs_f64() * 1000.0,
        summary.format,
        summary.relations.len(),
        summary.sources.len()
    );
    let export = case_path.with_file_name("walkthrough-export.csv");
    let seen = walk(&state, &case_path, &export, &PLAN);
    seen.print("real case");
    let _ = std::fs::remove_file(&export);

    // Every session the case offers, with the rows each of its tabs shows.
    println!("\n| source | session | tab: relation [hosts] rows | unmatched |");
    println!("|---:|---|---|---:|");
    for s in case::sessions(&state, at("sessions")).unwrap() {
        let mut tabs: Vec<SessionTab> = Vec::new();
        for r in &s.relations {
            match tabs.iter_mut().find(|t| t.0 == r.relation) {
                Some(t) => t.1.push(r.host.clone()),
                None => tabs.push((r.relation.clone(), vec![r.host.clone()], 0)),
            }
        }
        let shown: Vec<String> = tabs
            .iter_mut()
            .map(|t| {
                let mut scoped = grid(&t.0);
                scoped.hosts = t.1.clone();
                t.2 = counts(&state, &scoped);
                format!("{} [{}] {}", t.0, t.1.join(", "), t.2)
            })
            .collect();
        println!(
            "| {} | {} | {} | {} |",
            s.source_id,
            s.name,
            shown.join("; "),
            s.unmatched
        );
    }
}

/// Warm timings: one call first, then `RUNS` calls; min, median and max.
const RUNS: usize = 5;

fn timed<T>(mut f: impl FnMut() -> T) -> (T, [Duration; 3]) {
    let mut out = f();
    let mut took = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let start = Instant::now();
        out = f();
        took.push(start.elapsed());
    }
    took.sort();
    (out, [took[0], took[RUNS / 2], took[RUNS - 1]])
}

fn ms(d: Duration) -> String {
    format!("{:.0}", d.as_secs_f64() * 1000.0)
}

fn row(what: &str, t: [Duration; 3], target: &str, note: &str) {
    println!(
        "| {what} | {} | {} | {} | {target} | {note} |",
        ms(t[0]),
        ms(t[1]),
        ms(t[2])
    );
}

/// The grid's costs on a copy of a real case, warm.
#[test]
#[ignore = "needs a copy of a real case in OXL_PERF_CASE"]
fn real_case_performance() {
    let case_path = case_from_env("OXL_PERF_CASE");
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let (_events, state) = state();
    let start = Instant::now();
    let mut summary = case::case_open(&state, &case_path).unwrap();
    let opened = start.elapsed();
    // A run to load first, through the app, to grow the case.
    if let Some(run) = env("OXL_PERF_LOAD_RUN") {
        let start = Instant::now();
        let report = load::load_run(&state, Path::new(&run)).unwrap();
        println!(
            "\nloaded a run of {} relations in {} ms",
            report.relations.len(),
            ms(start.elapsed())
        );
        summary = case::case_summary(&state, at("summary")).unwrap();
    }
    let rows: u64 = summary.relations.iter().map(|r| r.rows).sum();
    println!(
        "\n{} relations, {rows} rows; opened (format {} after) in {} ms",
        summary.relations.len(),
        summary.format,
        ms(opened)
    );
    let largest = summary
        .relations
        .iter()
        .max_by_key(|r| r.rows)
        .expect("a relation");
    let rel = largest.name.as_str();
    let all = grid(rel);
    let utc = zone("UTC");
    println!("largest relation: {rel} ({} rows)", largest.rows);
    println!("| what | min ms | median ms | max ms | target | note |");
    println!("|---|---:|---:|---:|---|---|");

    let (page, t) =
        timed(|| g::grid_page(&state, at("perf:page"), &all, BLOCK_ROWS, 0, &utc).unwrap());
    row(
        "first page, largest relation",
        t,
        "< 300 ms",
        &format!(
            "{} rows x {} columns",
            page.row_ids.len(),
            page.columns.len()
        ),
    );

    // A filter-row edit: the window asks for the first block, the counts and
    // the histogram at once, and the change is done when all three answer.
    let host_col = largest.host_column.clone().unwrap_or_default();
    let filter_column = env("OXL_PERF_FILTER_COLUMN").unwrap_or(host_col.clone());
    let filter_text = env("OXL_PERF_FILTER_TEXT").unwrap_or_else(|| {
        largest
            .hosts
            .first()
            .map(|h| h.chars().take(3).collect())
            .unwrap_or_default()
    });
    let mut filtered = grid(rel);
    filtered.chips.push(chip(
        "fr-perf",
        "perf",
        "filter_row",
        json!({ "column": filter_column, "op": "contains", "value": filter_text,
                "case_sensitive": false }),
    ));
    let (n, t) = timed(|| {
        let threads: Vec<_> = (0..3)
            .map(|i| {
                let state = state.clone();
                let st = filtered.clone();
                std::thread::spawn(move || match i {
                    0 => {
                        g::grid_page(&state, at("perf:f:page"), &st, BLOCK_ROWS, 0, &zone("UTC"))
                            .unwrap();
                        0
                    }
                    1 => {
                        g::grid_counts(&state, at("perf:f:counts"), &st)
                            .unwrap()
                            .rows
                    }
                    _ => {
                        g::grid_histogram(&state, at("perf:f:hist"), &st, None, MAX_BUCKETS, "UTC")
                            .unwrap();
                        0
                    }
                })
            })
            .collect();
        threads.into_iter().map(|t| t.join().unwrap()).sum::<u64>()
    });
    row(
        "filter-row change (page + counts + histogram at once)",
        t,
        "< 1.5 s",
        &format!("contains on the chosen column: {n} rows"),
    );
    let (_, t) = timed(|| g::grid_counts(&state, at("perf:counts"), &filtered).unwrap());
    row("  of which counts alone", t, "", "");

    let group = env("OXL_PERF_GROUP").unwrap_or(host_col);
    let ((items, more), t) = timed(|| groups(&state, &all, &[group.as_str()], &[]));
    row(
        "group headers, largest relation",
        t,
        "< 2 s",
        &format!("{} groups{}", items.len(), if more { "+" } else { "" }),
    );

    let (h, t) = timed(|| {
        g::grid_histogram(&state, at("perf:hist"), &all, None, MAX_BUCKETS, "UTC").unwrap()
    });
    row(
        "histogram, largest relation",
        t,
        "",
        &format!("{} buckets of {} s", h.buckets.len(), h.bucket_seconds),
    );
    let (_, t) = timed(|| {
        g::grid_histogram(&state, at("perf:hist"), &all, None, MAX_BUCKETS, PLAN.zone).unwrap()
    });
    row("histogram, largest relation, in a zone", t, "", "");

    if let (Some(vr), Some(vc)) = (
        env("OXL_PERF_VALUES_RELATION"),
        env("OXL_PERF_VALUES_COLUMN"),
    ) {
        let (listed, t) = timed(|| {
            g::grid_values(
                &state,
                at("perf:values"),
                &grid(&vr),
                &vc,
                &[],
                None,
                ValueOrder::Commonest,
                VALUES_LIMIT,
                0,
            )
            .unwrap()
        });
        row(
            "value counts, high-cardinality column",
            t,
            "",
            &format!("{} listed, more: {}", listed.items.len(), listed.truncated),
        );
    }

    // The scroll cost: a full-width block of the widest relation, and of the
    // widest relation of at least `BIG` rows, at the top and in the middle.
    const BIG: u64 = 100_000;
    let widths: Vec<(usize, usize, &oxl_engine::RelationInfo)> = summary
        .relations
        .iter()
        .map(|r| {
            let cols = case::columns(&state, at("cols"), &r.name).unwrap();
            let data = cols.iter().filter(|c| c.role == ColumnRole::Data).count();
            (cols.len(), data, r)
        })
        .collect();
    let widest = widths.iter().max_by_key(|(n, _, r)| (*n, r.rows));
    let widest_big = widths
        .iter()
        .filter(|(_, _, r)| r.rows >= BIG)
        .max_by_key(|(n, _, r)| (*n, r.rows));
    for (label, found) in [("widest", widest), ("widest of 100k+ rows", widest_big)] {
        let Some((n, data, r)) = found else { continue };
        println!(
            "{label} relation: {} ({n} columns, {data} data; {} rows)",
            r.name, r.rows
        );
        let wide = grid(&r.name);
        for offset in [0, r.rows / 2] {
            let (p, t) = timed(|| {
                g::grid_page(&state, at("perf:wide"), &wide, BLOCK_ROWS, offset, &utc).unwrap()
            });
            row(
                &format!("full-width block, {label} relation, offset {offset}"),
                t,
                "",
                &format!("{} cells", p.row_ids.len() * p.columns.len()),
            );
        }
    }
    let (_, t) = timed(|| {
        g::grid_page(
            &state,
            at("perf:deep"),
            &all,
            BLOCK_ROWS,
            largest.rows / 2,
            &utc,
        )
        .unwrap()
    });
    row(
        "block in the middle of the largest relation",
        t,
        "",
        &format!("offset {}", largest.rows / 2),
    );
}
