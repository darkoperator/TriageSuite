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
//! saved, an export writes what the grid counts).
//!
//! Then the case record (`record_walk`): S on three rows → M on the first →
//! the account and the IOC its row names, taken from the candidates → the
//! account's logons → the IOC's sightings → mark the entry reviewed → the
//! four sheets → export the timeline, workbook and report. It checks that S
//! marks every row, M moves one mark from Suspicious to Malicious in the left
//! pane's counts and links the row's host, every record counts the entries
//! it is linked to, each logon count's filter opens its rows, and each export
//! is a job. `supersede_walk` takes evidence through a later run of the same
//! host and removes the first run.
//!
//! `the_walkthrough_on_two_hosts` runs all three and states every number:
//! `walk` on the two-host fixture, `record_walk` on a case with WS01's
//! logons (`two_hosts_logons`, ruling R-P2), and `supersede_walk` back on the
//! first case, whose rows `rerun_ws01_events` repeats byte for byte (the
//! logons fixture's extra column gives its rows other hashes).
//!
//! `walk` and `record_walk` run on a copy of a real case, which also times
//! the case record there, and the grid's costs are measured there, in two
//! ignored tests that take the case from the environment and print what
//! they saw:
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
//! Both open the case through the app, which upgrades a format 1 or 2 case
//! in place, and the walkthrough writes to it (the display zone, a saved
//! view, marks, entries, records and IOCs) and exports next to it: give them
//! a copy, never the analyst's case.

mod common;

use common::{grid, state, Recorder};
use oxl_app::cmd::record::{self as r, CaseRecord, NameQuery, RecordQuery, RecordSpec};
use oxl_app::cmd::{case, grid as g, load, row, views, At};
use oxl_app::events::{JOB_FINISHED, JOB_STARTED};
use oxl_app::state::AppState;
use oxl_app::CmdError;
use oxl_engine::{
    AccountSpec, Bucket, Candidate, Chip, ChipOrigin, ColumnRole, Entry, EntryQuery, EntryStatus,
    EvidenceStanding, GridState, IocQuery, IocSpec, IocType, MarkState, PageOptions, Privilege,
    RecordKind, Sightings, TimeSemantics, ValueOrder, ViewDoc, ViewKind,
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
    assert_eq!(reopened.format, 3);
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

    // Export what the grid shows, as a job, to a file that is not there yet.
    assert!(!export.exists(), "{} is already there", export.display());
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

// ---- the case record ----

/// What the case-record walk does, by name: the relation, and the text of
/// the rows the analyst marks.
struct RecordPlan<'a> {
    relation: &'a str,
    /// The walk marks the first `SUSPICIOUS` rows whose text holds this
    /// Suspicious, then presses M on the first of them.
    rows_text: &'a str,
}

const RECORD_PLAN: RecordPlan<'static> = RecordPlan {
    relation: "evtxtriage_events",
    rows_text: "Target: ",
};

/// The rows the walk marks Suspicious.
const SUSPICIOUS: u64 = 3;
/// The records one sheet read lists (the engine's `MAX_LIMIT`, as
/// `records/RecordSheet.tsx` and `TimelineSheet.tsx` ask).
const SHEET_LIMIT: u32 = 1000;

/// Marked rows as the left pane counts them: suspicious, malicious, agent:
/// malicious.
type Marks = [u64; 3];

/// The case's marks, and those of `relation` (the left pane's counts, from
/// the summary the window reads after `case-changed`).
fn marks(state: &AppState, relation: &str) -> (Marks, Marks) {
    let m = case::case_summary(state, at("summary"))
        .unwrap()
        .mark_counts;
    let of = m
        .by_relation
        .iter()
        .find(|r| r.relation == relation)
        .map_or([0; 3], |r| [r.suspicious, r.malicious, r.agent_malicious]);
    ([m.suspicious, m.malicious, m.agent_malicious], of)
}

/// The grid over `relation` narrowed to the rows whose text holds `text`
/// (the search box in filter mode), on `hosts` (all when empty).
fn text_grid(relation: &str, text: &str, hosts: &[&str]) -> GridState {
    serde_json::from_value(json!({
        "relation": relation,
        "search": { "text": text, "regex": false, "case_sensitive": false, "mode": "filter" },
        "hosts": hosts,
    }))
    .unwrap()
}

/// The ids of the first `n` rows `grid` shows, as the grid resolves a
/// selection before it acts on it (S10).
fn first_ids(state: &AppState, grid: &GridState, n: u64) -> Vec<i64> {
    if n == 0 {
        return Vec::new();
    }
    g::grid_row_ids(state, at("tab:walk:ids"), grid, &[(0, n - 1)]).unwrap()
}

/// The one row of `relation` whose text holds `text` on `host`.
fn one_row(state: &AppState, relation: &str, text: &str, host: &str) -> i64 {
    let st = text_grid(relation, text, &[host]);
    assert_eq!(counts(state, &st), 1, "{text} on {host}");
    first_ids(state, &st, 1)[0]
}

fn record_id(record: &CaseRecord) -> i64 {
    match record {
        CaseRecord::Host(h) => h.id,
        CaseRecord::Account(a) => a.id,
        CaseRecord::Ioc(i) => i.id,
    }
}

/// What the candidates panel stores for a candidate the case lacks
/// (`records/Candidates.tsx`, `specOf`): its values as the engine read them.
fn candidate_spec(c: &Candidate) -> RecordSpec {
    match c.kind {
        RecordKind::Account => RecordSpec::Account(AccountSpec {
            id: None,
            name: c.value.clone(),
            domain: c.domain.clone(),
            sid: c.sid.clone(),
            description: String::new(),
            privilege: Privilege::Unknown,
            note: String::new(),
        }),
        _ => ioc(c.ioc_type.unwrap_or(IocType::Other), &c.value),
    }
}

fn ioc(ioc_type: IocType, value: &str) -> RecordSpec {
    RecordSpec::Ioc(IocSpec {
        id: None,
        ioc_type,
        value: value.to_string(),
        tool: None,
        attack_ids: Vec::new(),
        note: String::new(),
    })
}

/// A candidate offered, clicked: the record the case has, or a new one, then
/// linked to the entry, as the candidates panel does. The record's id.
fn take_candidate(state: &AppState, entry_id: i64, c: &Candidate) -> i64 {
    let id = match c.existing_id {
        Some(id) => id,
        None => record_id(&r::record_upsert(state, &candidate_spec(c)).unwrap()),
    };
    r::record_link(state, entry_id, c.kind, id).unwrap();
    id
}

/// An account's logons as `(outcome, logon type, host, count)`, sorted.
type Logon = (String, u32, Option<String>, u64);

/// Each entry's evidence as `(row id, standing)`, by row id.
fn standings(state: &AppState, entry_id: i64) -> Vec<(i64, EvidenceStanding)> {
    let mut rows: Vec<_> = r::evidence_list(state, at("timeline:evidence"), entry_id)
        .unwrap()
        .into_iter()
        .map(|e| (e.row_id, e.standing))
        .collect();
    rows.sort();
    rows
}

/// What the record walk saw.
#[derive(Default)]
struct RecordSeen {
    seen: Seen,
    /// The rows marked Suspicious; M took the first.
    marked: Vec<i64>,
    /// The left pane's counts before S, after S and after M: the case's,
    /// then the relation's.
    before: (Marks, Marks),
    after_s: (Marks, Marks),
    after_m: (Marks, Marks),
    /// The entry M made, as the Timeline lists it right after M.
    entry: Option<Entry>,
    /// The account candidate taken (`DOMAIN\name`) and its logons, and the
    /// rows the logon summary could not read.
    account: Option<(String, Vec<Logon>, u64)>,
    /// The IOC candidate taken (`type value`) and its sightings.
    ioc: Option<(String, Sightings)>,
    /// The entry's status after *Mark reviewed*, or the refusal's code.
    reviewed: Option<Result<EntryStatus, String>>,
    /// Each export's file name, rows and bytes.
    exports: Vec<(&'static str, u64, u64)>,
    /// What the Timeline, Hosts, Accounts and IOCs sheets list.
    sheets: [usize; 4],
    /// The entry's hosts, accounts and IOCs as the record lists them:
    /// each record's name (an account `DOMAIN\name`, an IOC its value) and
    /// how many entries it is linked to.
    records: Vec<(String, u64)>,
}

/// The case-record walk over the case open in `state`, writing its exports
/// into `out`: S on rows, M on one, the account and the IOC its row names
/// taken from the candidates, the account's logons, the IOC's sightings,
/// *Mark reviewed*, the three exports, and the four sheets. Checks what
/// holds for any data; the caller checks the numbers.
fn record_walk(
    state: &Arc<AppState>,
    events: &Recorder,
    out: &Path,
    plan: &RecordPlan<'_>,
) -> RecordSeen {
    let mut rs = RecordSeen {
        before: marks(state, plan.relation),
        ..RecordSeen::default()
    };
    let seen = &mut rs.seen;

    // S on the first rows that hold the text: resolved to ids first, no
    // dialog, every row taken.
    let st = text_grid(plan.relation, plan.rows_text, &[]);
    let marked = seen.step("S on rows", || {
        let held = counts(state, &st);
        assert!(
            held >= SUSPICIOUS,
            "the walk needs {SUSPICIOUS} rows of {} holding {:?}; the case has {held}",
            plan.relation,
            plan.rows_text
        );
        let n = SUSPICIOUS;
        let ids = first_ids(state, &st, n);
        let outcomes = r::mark_set(state, &ids).unwrap();
        assert!(outcomes.iter().all(|o| o.done), "{outcomes:?}");
        ids
    });
    rs.after_s = marks(state, plan.relation);
    seen.saw(format!(
        "{} rows; suspicious {} → {}",
        marked.len(),
        rs.before.0[0],
        rs.after_s.0[0]
    ));

    // M on the first: an entry with the row as evidence, no dialog; the
    // row's mark moves from Suspicious to Malicious.
    let row = marked[0];
    let promoted = seen.step("M on one", || r::rows_promote(state, &[row]).unwrap());
    assert_eq!(promoted.len(), 1);
    assert!(promoted[0].created, "{promoted:?}");
    let entry_id = promoted[0].entry_id.expect("M made an entry");
    rs.after_m = marks(state, plan.relation);
    for (after_s, after_m) in [(rs.after_s.0, rs.after_m.0), (rs.after_s.1, rs.after_m.1)] {
        assert_eq!(
            [after_m[0] + 1, after_m[1]],
            [after_s[0], after_s[1] + 1],
            "the row's mark moved from Suspicious to Malicious"
        );
    }
    let entry = r::entry_get(state, at("timeline:entry"), entry_id).unwrap();
    assert_eq!((entry.status, entry.evidence_rows), (EntryStatus::Draft, 1));
    let evidence = r::evidence_list(state, at("timeline:evidence"), entry_id).unwrap();
    assert_eq!(evidence.len(), 1);
    assert_eq!(
        (evidence[0].row_id, evidence[0].standing),
        (row, EvidenceStanding::Unchanged)
    );
    // S7: the entry links the row's host.
    assert_eq!(
        entry.hosts,
        evidence[0].host.clone().into_iter().collect::<Vec<_>>()
    );
    seen.saw(format!(
        "E-{entry_id} at {} ({}); hosts {:?}",
        entry.time.as_deref().unwrap_or("needs a time"),
        entry.time_source.as_deref().unwrap_or("-"),
        entry.hosts
    ));

    // The candidates the entry editor offers after M: the row's account,
    // then an IOC it names (an address first), each clicked.
    let found = seen.step("candidates", || {
        r::candidates(state, at("candidates"), row).unwrap()
    });
    seen.saw(format!("{} candidates", found.len()));
    let account = found
        .iter()
        .find(|c| c.kind == RecordKind::Account && c.domain.is_some())
        .or_else(|| found.iter().find(|c| c.kind == RecordKind::Account));
    {
        let c =
            account.unwrap_or_else(|| panic!("the M row offers no account candidate: {found:?}"));
        let id = take_candidate(state, entry_id, c);
        let logons = seen.step("account logons", || {
            r::logon_summary(state, at("sheet:accounts:logons"), id).unwrap()
        });
        let mut rows: Vec<Logon> = logons
            .rows
            .iter()
            .map(|l| {
                let outcome = serde_json::to_value(l.outcome).unwrap();
                (
                    outcome.as_str().unwrap().to_string(),
                    l.logon_type,
                    l.host.clone(),
                    l.count,
                )
            })
            .collect();
        rows.sort();
        // Each count's filter opens exactly its rows.
        for l in &logons.rows {
            let mut opened = grid(logons.source.as_deref().unwrap());
            opened.chips.push(chip(
                "lg",
                "logons",
                "values",
                serde_json::to_value(&l.filter).unwrap(),
            ));
            assert_eq!(counts(state, &opened), l.count, "{l:?}");
        }
        let name = match &c.domain {
            Some(d) => format!("{d}\\{}", c.value),
            None => c.value.clone(),
        };
        seen.saw(format!(
            "{name}: {} counts, {} unparsed",
            rows.len(),
            logons.unparsed
        ));
        rs.account = Some((name, rows, logons.unparsed));
    }
    let address = found
        .iter()
        .find(|c| c.ioc_type == Some(IocType::Ip))
        .or_else(|| found.iter().find(|c| c.kind == RecordKind::Ioc));
    {
        let c = address.unwrap_or_else(|| panic!("the M row offers no IOC candidate: {found:?}"));
        let id = take_candidate(state, entry_id, c);
        let sightings = seen.step("IOC sightings", || {
            r::ioc_sightings(state, at("sightings"), id).unwrap()
        });
        assert_eq!(
            sightings.relations.iter().map(|(_, n)| n).sum::<u64>(),
            sightings.hits,
            "the relations' hits add up"
        );
        let ioc_type = serde_json::to_value(c.ioc_type).unwrap();
        let name = format!("{} {}", ioc_type.as_str().unwrap(), c.value);
        seen.saw(format!(
            "{name}: {} hits in {} relations on {} hosts",
            sightings.hits,
            sightings.relations.len(),
            sightings.hosts.len()
        ));
        rs.ioc = Some((name, sightings));
    }

    // Mark reviewed: refused while the entry needs a time (S8).
    let reviewed = seen.step("mark reviewed", || {
        r::entry_status(state, entry_id, EntryStatus::Reviewed)
    });
    let reviewed = reviewed.map(|e| e.status).map_err(|e| e.code);
    if entry.needs_time {
        assert_eq!(reviewed, Err("refused".to_string()));
    } else {
        assert_eq!(reviewed, Ok(EntryStatus::Reviewed));
    }
    seen.saw(format!("{reviewed:?}"));
    rs.reviewed = Some(reviewed);

    // The four sheets, as their tabs read them.
    let sheets = seen.step("the four sheets", || {
        let timeline = r::entries_list(
            state,
            at("sheet:timeline"),
            &EntryQuery::default(),
            SHEET_LIMIT,
            0,
        )
        .unwrap();
        let kinds = [
            RecordQuery::Host(NameQuery::default()),
            RecordQuery::Account(NameQuery::default()),
            RecordQuery::Ioc(IocQuery::default()),
        ];
        let records: Vec<_> = kinds
            .iter()
            .map(|q| r::records_list(state, at("sheet:records"), q, SHEET_LIMIT, 0).unwrap())
            .collect();
        (timeline, records)
    });
    let (timeline, records) = sheets;
    assert!(timeline.items.iter().any(|e| e.id == entry_id));
    rs.sheets = [
        timeline.items.len(),
        records[0].items.len(),
        records[1].items.len(),
        records[2].items.len(),
    ];
    seen.saw(format!("{:?}", rs.sheets));
    // The records the entry links, each counting it among its entries.
    let linked = r::entry_get(state, at("timeline:entry"), entry_id).unwrap();
    for list in &records {
        for record in &list.items {
            let (name, entries) = match record {
                CaseRecord::Host(h) => (h.name.clone(), h.entries),
                CaseRecord::Account(a) => match &a.domain {
                    Some(d) => (format!("{d}\\{}", a.name), a.entries),
                    None => (a.name.clone(), a.entries),
                },
                CaseRecord::Ioc(i) => (i.value.clone(), i.entries),
            };
            let link = r::record_entries(
                state,
                at("sheet:records:entries"),
                kind_of(record),
                record_id(record),
                SHEET_LIMIT,
                0,
            )
            .unwrap();
            assert_eq!(link.items.len() as u64, entries, "{name}");
            if link.items.iter().any(|e| e.id == entry_id) {
                rs.records.push((name, entries));
            }
        }
    }
    assert_eq!(
        rs.records.len(),
        linked.hosts.len() + usize::from(rs.account.is_some()) + usize::from(rs.ioc.is_some()),
        "the entry is linked to its host, the account and the IOC"
    );
    rs.entry = Some(linked);

    // The three exports, each a job writing a new file.
    events.clear();
    type Export = fn(&AppState, At<'_>, &Path) -> Result<oxl_engine::ExportReport, CmdError>;
    let exports: [(&'static str, &'static str, Export); 3] = [
        (
            "export timeline CSV",
            "walk-timeline.csv",
            r::export_timeline,
        ),
        ("export workbook", "walk-record.xlsx", r::export_workbook),
        ("export report", "walk-record.md", r::export_report),
    ];
    for (step, name, export) in exports {
        let path = out.join(name);
        assert!(!path.exists(), "{} is already there", path.display());
        let report = seen.step(step, || export(state, at("case:export"), &path).unwrap());
        assert_eq!(report.path, path);
        let bytes = std::fs::metadata(&path).unwrap().len();
        seen.saw(format!("{} rows, {bytes} bytes", report.rows));
        rs.exports.push((name, report.rows, bytes));
    }
    // A timeline row per entry the Timeline lists.
    if !timeline.truncated {
        assert_eq!(rs.exports[0].1, timeline.items.len() as u64);
    }
    let jobs: Vec<String> = events
        .names()
        .into_iter()
        .filter(|n| n == JOB_STARTED || n == JOB_FINISHED)
        .collect();
    assert_eq!(jobs, [JOB_STARTED, JOB_FINISHED].repeat(3));
    rs.marked = marked;
    rs
}

fn kind_of(record: &CaseRecord) -> RecordKind {
    match record {
        CaseRecord::Host(_) => RecordKind::Host,
        CaseRecord::Account(_) => RecordKind::Account,
        CaseRecord::Ioc(_) => RecordKind::Ioc,
    }
}

/// What the supersession walk saw.
struct SupersedeSeen {
    seen: Seen,
    entry_id: i64,
    /// The failed logon M took, and the `rclone.exe` row A attached.
    failed: i64,
    rclone: i64,
    /// The failed logon's identical row in the later run.
    moved_to: i64,
    /// The later run's supersession of WS01's events: moved, left behind.
    superseded: (u64, u64),
    /// The entry's evidence after the later run, and after the first run
    /// was removed.
    after_load: Vec<(i64, EvidenceStanding)>,
    after_remove: Vec<(i64, EvidenceStanding)>,
    /// The entry's evidence count after the removal: every snapshot kept.
    evidence_rows: u64,
    /// The refusal without confirmation, and what the removal orphaned:
    /// marks, tags, notes, evidence.
    refusal: String,
    orphaned: [u64; 4],
    /// The left pane's counts after the load and after the removal.
    marks_after_load: (Marks, Marks),
    marks_after_remove: (Marks, Marks),
    /// The relations and sources (`id`, removed) after the removal.
    relations: Vec<(String, u64)>,
    sources: Vec<(i64, bool)>,
}

/// Evidence through a newer run, over the case at `case_path` open in
/// `state` with `two_hosts` loaded as source 1: M on WS01's failed logon,
/// A of its `rclone.exe` process creation to the same entry, then `rerun`
/// (a later run of WS01's events, identical but for the `rclone.exe` row)
/// loaded through the app; then the first run removed, refused until
/// confirmed, as `oxl source remove [--confirm-orphans]` does (the window
/// has no command for it).
fn supersede_walk(state: &Arc<AppState>, case_path: &Path, rerun: &Path) -> SupersedeSeen {
    const EVENTS: &str = "evtxtriage_events";
    let mut seen = Seen::default();
    let failed = one_row(state, EVENTS, "Administrator", "WS01");
    let rclone = one_row(state, EVENTS, "rclone.exe", "WS01");
    let entry_id = seen.step("M, then A", || {
        let entry = r::rows_promote(state, &[failed]).unwrap()[0]
            .entry_id
            .unwrap();
        let attached = r::rows_attach(state, entry, &[rclone]).unwrap();
        assert!(attached[0].done, "{attached:?}");
        entry
    });
    assert_eq!(
        standings(state, entry_id),
        [
            (failed, EvidenceStanding::Unchanged),
            (rclone, EvidenceStanding::Unchanged)
        ]
    );
    seen.saw(format!("E-{entry_id} holds rows {failed} and {rclone}"));

    let report = seen.step("load the later run", || {
        load::load_run(state, rerun).unwrap()
    });
    let ws01 = report
        .superseded
        .iter()
        .find(|s| s.relation == EVENTS && s.host == "WS01")
        .expect("the later run superseded WS01's events");
    let superseded = (ws01.moved, ws01.left_behind);
    let moved_to = one_row(state, EVENTS, "Administrator", "WS01");
    assert_ne!(moved_to, failed, "the failed logon is the later run's row");
    // The re-check, on the entry's next read.
    let after_load = standings(state, entry_id);
    let marks_after_load = marks(state, EVENTS);
    seen.saw(format!(
        "moved {}, left behind {}",
        superseded.0, superseded.1
    ));

    assert_eq!(report.source_id, 2);
    let first = 1;
    let (refusal, orphaned) = seen.step("remove the first run", || {
        case::case_close(state).unwrap();
        let actor = state.actor();
        let mut c = oxl_engine::Case::open(case_path).unwrap();
        let refusal = match oxl_engine::remove_source(&mut c, first, false, &actor) {
            Err(oxl_engine::EngineError::Refused(m)) => m,
            other => panic!("removal without confirmation: {other:?}"),
        };
        assert!(!oxl_engine::sources(&c).unwrap()[0].removed);
        let removed = oxl_engine::remove_source(&mut c, first, true, &actor).unwrap();
        drop(c);
        case::case_open(state, case_path).unwrap();
        (
            refusal,
            [
                removed.orphaned_marks,
                removed.orphaned_tags,
                removed.orphaned_notes,
                removed.orphaned_evidence,
            ],
        )
    });
    let after_remove = standings(state, entry_id);
    let evidence_rows = r::entry_get(state, at("timeline:entry"), entry_id)
        .unwrap()
        .evidence_rows;
    let summary = case::case_summary(state, at("summary")).unwrap();
    seen.saw(format!("refused, then confirmed: {orphaned:?} orphaned"));
    SupersedeSeen {
        seen,
        entry_id,
        failed,
        rclone,
        moved_to,
        superseded,
        after_load,
        after_remove,
        evidence_rows,
        refusal,
        orphaned,
        marks_after_load,
        marks_after_remove: marks(state, EVENTS),
        relations: summary
            .relations
            .iter()
            .map(|r| (r.name.clone(), r.rows))
            .collect(),
        sources: summary.sources.iter().map(|s| (s.id, s.removed)).collect(),
    }
}

#[test]
fn the_walkthrough_on_two_hosts() {
    let (events, state) = state();
    let scratch = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::two_hosts(scratch.path());
    let case_path = scratch.path().join("walk.case.duckdb");

    let summary = case::case_new(&state, &case_path, "WS01 triage").unwrap();
    assert_eq!((summary.format, summary.display_zone.as_str()), (3, "UTC"));
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

    // The case record, on a case of its own with WS01's Security logons
    // (`two_hosts_logons`, ruling R-P2): their extra column gives every
    // events row another hash, so a later run of `two_hosts` matches none
    // of them, and supersession is walked on the first case below.
    let logons_dir = scratch.path().join("logons");
    let logons_run = oxl_fixtures::two_hosts_logons(&logons_dir);
    case::case_new(
        &state,
        &logons_dir.join("record.case.duckdb"),
        "WS01 record",
    )
    .unwrap();
    load::load_run(&state, &logons_run).unwrap();
    let rs = record_walk(&state, &events, &logons_dir, &RECORD_PLAN);
    rs.seen.print("two_hosts_logons: the case record");
    check_record_walk(&state, &rs);

    // Supersession and removal, on the first case.
    case::case_open(&state, &case_path).unwrap();
    let rerun = oxl_fixtures::rerun_ws01_events(&scratch.path().join("rerun"));
    let sup = supersede_walk(&state, &case_path, &rerun);
    sup.seen.print("two_hosts: evidence through a later run");
    check_supersede_walk(&sup);
}

/// The record walk's numbers on `two_hosts_logons`.
fn check_record_walk(state: &AppState, rs: &RecordSeen) {
    // The three `Target: ` rows are WS01's logons at 02:40, 02:41 and 02:42;
    // M takes the first, jdoe's type-10 logon from 203.0.113.7.
    assert_eq!(rs.marked.len(), 3);
    let first = row::row_view(state, at("detail"), rs.marked[0], "UTC").unwrap();
    let field = |name: &str| {
        first
            .fields
            .iter()
            .find(|f| f.meta.name == name)
            .and_then(|f| f.text.clone())
    };
    assert_eq!(field("PayloadData1").as_deref(), Some(r"Target: ACME\jdoe"));
    assert_eq!(field("PayloadData2").as_deref(), Some("LogonType 10"));
    // The left pane: no marks, then three Suspicious, then one of them
    // Malicious, all in `evtxtriage_events`.
    assert_eq!(rs.before, ([0, 0, 0], [0, 0, 0]));
    assert_eq!(rs.after_s, ([3, 0, 0], [3, 0, 0]));
    assert_eq!(rs.after_m, ([2, 1, 0], [2, 1, 0]));
    // The entry M made: timed by the row, linked to its host, then to the
    // account and the IOC the candidates offered; reviewed.
    let entry = rs.entry.as_ref().unwrap();
    assert_eq!(entry.title, "Security · 4624 · Successful logon · jdoe");
    assert_eq!(
        (entry.time.as_deref(), entry.time_source.as_deref()),
        (Some("2026-01-05T02:40:00.000000Z"), Some("row"))
    );
    assert_eq!(
        (entry.status, entry.evidence_rows),
        (EntryStatus::Reviewed, 1)
    );
    assert_eq!(entry.mark, Some(MarkState::Malicious));
    let links: Vec<(RecordKind, &str)> = entry
        .links
        .iter()
        .map(|l| (l.kind, l.label.as_str()))
        .collect();
    assert_eq!(
        links,
        [
            (RecordKind::Host, "WS01"),
            (RecordKind::Account, r"ACME\jdoe"),
            (RecordKind::Ioc, "ip 203.0.113.7")
        ]
    );
    // jdoe's logons on WS01: one success of type 10 and one failure of type
    // 3. Five Security 4624/4625 rows do not read as S6 says: the four of
    // `two_hosts` with no `Target: ` and the one whose target is `-`.
    let (name, logons, unparsed) = rs.account.clone().expect("an account candidate");
    let ws01 = Some("WS01".to_string());
    assert_eq!(name, r"ACME\jdoe");
    assert_eq!(
        logons,
        [
            ("failure".to_string(), 3, ws01.clone(), 1),
            ("success".to_string(), 10, ws01, 1)
        ]
    );
    assert_eq!(unparsed, 5);
    // 203.0.113.7 is WS01's remote address on four events rows: the failed
    // Administrator logon, svc_backup's logon and network connection, and
    // jdoe's logon.
    let (name, sightings) = rs.ioc.clone().expect("an IOC candidate");
    assert_eq!(name, "ip 203.0.113.7");
    assert_eq!(
        (sightings.hits, sightings.relations, sightings.hosts),
        (
            4,
            vec![("evtxtriage_events".to_string(), 4)],
            vec!["WS01".to_string()]
        )
    );
    assert_eq!(
        (sightings.first.as_deref(), sightings.last.as_deref()),
        (
            Some("2026-01-05T02:13:58.204000Z"),
            Some("2026-01-05T02:40:00.000000Z")
        )
    );
    assert_eq!(rs.reviewed, Some(Ok(EntryStatus::Reviewed)));
    // One entry, one host, one account and one IOC, each linked to it.
    assert_eq!(rs.sheets, [1, 1, 1, 1]);
    let records: Vec<(&str, u64)> = rs.records.iter().map(|(n, e)| (n.as_str(), *e)).collect();
    assert_eq!(
        records,
        [("WS01", 1), (r"ACME\jdoe", 1), ("203.0.113.7", 1)]
    );
    // The timeline's one entry; the workbook's and the report's four rows,
    // an entry, a host, an account and an IOC.
    let exports: Vec<(&str, u64)> = rs.exports.iter().map(|(n, rows, _)| (*n, *rows)).collect();
    assert_eq!(
        exports,
        [
            ("walk-timeline.csv", 1),
            ("walk-record.xlsx", 4),
            ("walk-record.md", 4)
        ]
    );
}

/// The supersession walk's numbers on `two_hosts` and `rerun_ws01_events`.
fn check_supersede_walk(sup: &SupersedeSeen) {
    use EvidenceStanding::{SourceRemoved, Superseded, Unchanged};
    // The failed logon is byte for byte in the later run, so its evidence
    // moved there; the edited `rclone.exe` row has no twin, so its evidence
    // stayed, "from a superseded run". The later run's rows are numbered
    // after the first's.
    // The first case's first entry.
    assert_eq!(sup.entry_id, 1);
    assert_eq!(sup.superseded, (1, 1));
    assert!(sup.rclone < sup.moved_to);
    assert_eq!(
        sup.after_load,
        [(sup.rclone, Superseded), (sup.moved_to, Unchanged)]
    );
    assert!(!sup.after_load.iter().any(|(row, _)| *row == sup.failed));
    // The left pane counts the moved row, Malicious where it now is; the
    // superseded row is out of the view.
    assert_eq!(sup.marks_after_load, ([0, 1, 0], [0, 1, 0]));
    // Removing the first run is refused until confirmed, naming what it
    // would orphan: the one evidence row still on it.
    assert_eq!(
        sup.refusal,
        "source 1 has annotations on its rows: 0 marks, 0 tags, 0 notes, 1 evidence row; \
         removing it may orphan them (they are kept, evidence with its snapshot). Confirm to \
         remove it anyway"
    );
    assert_eq!(sup.orphaned, [0, 0, 0, 1]);
    // Confirmed: the row left behind is "source removed", kept with its
    // snapshot; the moved one is unchanged; the marks stay.
    assert_eq!(
        sup.after_remove,
        [(sup.rclone, SourceRemoved), (sup.moved_to, Unchanged)]
    );
    assert_eq!(sup.evidence_rows, 2);
    assert_eq!(sup.marks_after_remove, ([0, 1, 0], [0, 1, 0]));
    // Only the later run's WS01 events are left; SRV01's rows, the prefetch
    // timeline and the Hayabusa timeline went with the first run.
    assert_eq!(sup.relations, [("evtxtriage_events".to_string(), 8)]);
    assert_eq!(sup.sources, [(1, true), (2, false)]);
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
/// upgrades a format 1 or 2 case in place). It exports into a temporary
/// folder of its own.
#[test]
#[ignore = "needs a copy of a real case in OXL_WALKTHROUGH_CASE"]
fn real_case_walkthrough() {
    let case_path = case_from_env("OXL_WALKTHROUGH_CASE");
    let (events, state) = state();
    let start = Instant::now();
    let summary = case::case_open(&state, &case_path).unwrap();
    println!(
        "opened in {:.0} ms: format {}, {} relations, {} sources",
        start.elapsed().as_secs_f64() * 1000.0,
        summary.format,
        summary.relations.len(),
        summary.sources.len()
    );
    // Every export goes into a folder of this run's own, removed with
    // everything in it when the test ends: nothing beside the case is
    // touched.
    let scratch = tempfile::tempdir().unwrap();
    let out = scratch.path();
    let seen = walk(
        &state,
        &case_path,
        &out.join("walkthrough-export.csv"),
        &PLAN,
    );
    seen.print("real case");

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

    // The case record, walked as on the fixture.
    let rs = record_walk(&state, &events, out, &RECORD_PLAN);
    rs.seen.print("real case: the case record");
    let entry = rs.entry.as_ref().unwrap();
    println!(
        "\nmarks (case; {rel}) before {:?}, after S {:?}, after M {:?}",
        rs.before,
        rs.after_s,
        rs.after_m,
        rel = RECORD_PLAN.relation
    );
    println!(
        "entry E-{}: {:?} at {:?} ({:?}), {:?}, hosts {:?}, links {:?}",
        entry.id,
        entry.title,
        entry.time,
        entry.time_source,
        entry.status,
        entry.hosts,
        entry.links.iter().map(|l| &l.label).collect::<Vec<_>>()
    );
    println!("account: {:?}", rs.account);
    println!("ioc: {:?}", rs.ioc);
    println!("reviewed: {:?}; sheets {:?}", rs.reviewed, rs.sheets);
    println!("exports: {:?}", rs.exports);
    record_costs(&state, out);
}

/// The rows at index `from` and the `n - 1` after it in what `grid` shows.
fn ids_at(state: &AppState, grid: &GridState, from: u64, n: u64) -> Vec<i64> {
    g::grid_row_ids(state, at("tab:walk:ids"), grid, &[(from, from + n - 1)]).unwrap()
}

/// One write, timed once (it changes the case, so it is not repeated).
fn once<T>(f: impl FnOnce() -> T) -> (T, [Duration; 3]) {
    let start = Instant::now();
    let out = f();
    let took = start.elapsed();
    (out, [took; 3])
}

/// The case record's costs on the real case open in `state`, writing into
/// `out`: M on 1,000 rows, a re-check of 100 evidence rows, the four sheet
/// lists, *Sweep all* over 20 IOCs, and the three exports. A write or an
/// export runs once (min = median = max); a read is warm.
fn record_costs(state: &Arc<AppState>, out: &Path) {
    let summary = case::case_summary(state, at("summary")).unwrap();
    let largest = summary
        .relations
        .iter()
        .max_by_key(|r| r.rows)
        .expect("a relation");
    assert!(largest.rows >= 2_200, "the largest relation is too small");
    let all = grid(&largest.name);
    println!(
        "\ncase record costs; rows from {} ({} rows)",
        largest.name, largest.rows
    );
    println!("| what | min ms | median ms | max ms | target | note |");
    println!("|---|---:|---:|---:|---|---|");

    // M on 1,000 rows, from row 1,000 on (clear of the walk's own rows).
    let rows = ids_at(state, &all, 1_000, 1_000);
    let (promoted, t) = once(|| r::rows_promote(state, &rows).unwrap());
    let created = promoted.iter().filter(|p| p.created).count();
    assert_eq!(
        created, 1_000,
        "M made an entry for every row: {promoted:?}"
    );
    row("M on 1,000 rows", t, "", &format!("{created} entries made"));

    // A re-check of 100 evidence rows: an entry holding 100, listed.
    let entry = r::entry_new(
        state,
        &r::EntryChanges {
            title: Some("100 rows".into()),
            ..r::EntryChanges::default()
        },
    )
    .unwrap();
    let hundred = ids_at(state, &all, 2_100, 100);
    let (attached, t) = once(|| r::rows_attach(state, entry.id, &hundred).unwrap());
    let done = attached.iter().filter(|a| a.done).count();
    assert_eq!(done, 100, "every row attached: {attached:?}");
    row(
        "A of 100 rows to one entry",
        t,
        "",
        &format!("{done} attached"),
    );
    let (evidence, t) =
        timed(|| r::evidence_list(state, at("timeline:evidence"), entry.id).unwrap());
    let unchanged = evidence
        .iter()
        .filter(|e| e.standing == EvidenceStanding::Unchanged)
        .count();
    assert_eq!((evidence.len(), unchanged), (100, 100));
    row(
        "re-check of 100 evidence rows (evidence_list)",
        t,
        "",
        &format!("{} rows, {unchanged} unchanged", evidence.len()),
    );

    // 20 IOCs: the values the promoted rows offer, then documentation
    // addresses to make up the number.
    let mut values: Vec<(IocType, String)> = Vec::new();
    for &row_id in rows.iter().take(200) {
        for c in r::candidates(state, at("candidates"), row_id).unwrap() {
            let value = (c.ioc_type.unwrap_or(IocType::Other), c.value.clone());
            // Sightings search a value of at most Find's 1,000 characters.
            if c.kind == RecordKind::Ioc
                && c.existing_id.is_none()
                && c.value.chars().count() <= 1_000
                && !values.contains(&value)
            {
                values.push(value);
            }
        }
        if values.len() >= 20 {
            break;
        }
    }
    values.truncate(20);
    let from_rows = values.len();
    for n in 0..(20 - from_rows) {
        values.push((IocType::Ip, format!("203.0.113.{}", 100 + n)));
    }
    let iocs: Vec<i64> = values
        .iter()
        .map(
            |(ioc_type, value)| match r::record_upsert(state, &ioc(*ioc_type, value)) {
                Ok(record) => record_id(&record),
                Err(e) => panic!("IOC {ioc_type:?} {value:?} not created: {e:?}"),
            },
        )
        .collect();
    assert_eq!(iocs.len(), 20);

    // The four sheet lists, as their tabs read them.
    let (timeline, t) = timed(|| {
        r::entries_list(
            state,
            at("sheet:timeline"),
            &EntryQuery::default(),
            SHEET_LIMIT,
            0,
        )
        .unwrap()
    });
    let undated = timeline.items.iter().filter(|e| e.needs_time).count();
    row(
        "Timeline sheet list",
        t,
        "",
        &format!(
            "{} entries{} ({undated} undated)",
            timeline.items.len(),
            if timeline.truncated { "+" } else { "" }
        ),
    );
    let sheets = [
        ("Hosts", RecordQuery::Host(NameQuery::default())),
        ("Accounts", RecordQuery::Account(NameQuery::default())),
        ("IOCs", RecordQuery::Ioc(IocQuery::default())),
    ];
    for (name, query) in &sheets {
        let (list, t) =
            timed(|| r::records_list(state, at("sheet:records"), query, SHEET_LIMIT, 0).unwrap());
        row(
            &format!("{name} sheet list"),
            t,
            "",
            &format!("{} records", list.items.len()),
        );
    }

    // Sweep all: each IOC's sightings in turn on the one slot.
    let start = Instant::now();
    let mut each = Vec::new();
    let mut hits = 0;
    for id in &iocs {
        let one = Instant::now();
        hits += r::ioc_sightings(state, at("sightings"), *id)
            .unwrap_or_else(|e| panic!("I-{id}'s sightings: {e:?}"))
            .hits;
        each.push(one.elapsed());
    }
    assert_eq!(each.len(), 20, "every IOC was searched");
    let total = start.elapsed();
    each.sort();
    row(
        &format!("Sweep all over {} IOCs (total)", iocs.len()),
        [total; 3],
        "",
        &format!("{from_rows} values from rows; {hits} hits"),
    );
    if !each.is_empty() {
        row(
            "  one IOC's sightings",
            [each[0], each[each.len() / 2], each[each.len() - 1]],
            "",
            "",
        );
    }

    // The three exports, each once, to a new file.
    type Export = fn(&AppState, At<'_>, &Path) -> Result<oxl_engine::ExportReport, CmdError>;
    let exports: [(&str, &str, Export); 3] = [
        (
            "export timeline CSV",
            "costs-timeline.csv",
            r::export_timeline,
        ),
        ("export workbook", "costs-record.xlsx", r::export_workbook),
        ("export report", "costs-record.md", r::export_report),
    ];
    for (what, name, export) in exports {
        let path = out.join(name);
        assert!(!path.exists(), "{} is already there", path.display());
        let (report, t) = once(|| export(state, at("case:export"), &path).unwrap());
        let bytes = std::fs::metadata(&path).unwrap().len();
        row(what, t, "", &format!("{} rows, {bytes} bytes", report.rows));
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
