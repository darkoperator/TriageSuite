//! The row view's summary line and pivot anchor.

mod common;

use common::{analyst, loaded, n};
use oxl_engine::{
    anchor_of, load_file, load_run, merged_view, row_view, FileOptions, MergedSource,
};
use oxl_fixtures::{loose_fw_csv, narrow_evtx_run};

const NY: &str = "America/New_York";

fn first_ws01_event(case: &oxl_engine::Case) -> i64 {
    n(
        case,
        "select _ox_row from evtxtriage_events \
         where \"TimeCreated__text\" = '2026-01-05T02:13:58.2040000Z'",
    )
}

#[test]
fn the_summary_is_the_text_a_merged_row_shows() {
    let (_d, case) = loaded();
    let row = first_ws01_event(&case);
    let view = row_view(&case, row, NY).unwrap();
    let summary = view
        .summary
        .as_deref()
        .expect("an events row has a summary");
    let source = MergedSource::Around {
        host: "WS01".into(),
        at: "2026-01-05T02:13:58.2040000Z".into(),
        window_seconds: 5,
    };
    let merged = merged_view(&case, &source, 100, 0, NY).unwrap();
    let same = merged.items.iter().find(|m| m.row.row_id == row).unwrap();
    assert_eq!(same.row.summary, summary);
    assert!(summary.starts_with("Security · 4625"), "{summary}");
}

#[test]
fn a_relation_without_a_summary_entry_takes_its_first_plain_values() {
    let (d, mut case) = loaded();
    load_file(
        &mut case,
        &loose_fw_csv(d.path()),
        FileOptions {
            host: Some("WS01".into()),
            ..FileOptions::default()
        },
        &analyst(),
    )
    .unwrap();
    let fw = n(&case, "select min(_ox_row) from fw_csv");
    let view = row_view(&case, fw, NY).unwrap();
    let summary = view.summary.expect("the fallback summary");
    assert!(!summary.is_empty());
    assert!(summary.chars().count() <= 300);
    // An `unknown` time anchors a pivot as written, as `anchor_of` says.
    let anchor = view.anchor.expect("the engine anchors an unknown time");
    assert_eq!(anchor.host, "WS01");
}

#[test]
fn the_anchor_is_the_engines_host_and_time() {
    let (_d, case) = loaded();
    let row = first_ws01_event(&case);
    let anchor = row_view(&case, row, NY)
        .unwrap()
        .anchor
        .expect("a timed row has an anchor");
    let (host, at) = anchor_of(&case, row).unwrap();
    assert_eq!(
        (anchor.host.as_str(), anchor.at.as_str()),
        (host.as_str(), at.as_str())
    );
    assert_eq!(anchor.at, "2026-01-05T02:13:58.204000Z"); // R1: six digits
                                                          // Handed on unchanged as a merged source, it lists this very row.
    let source = MergedSource::Around {
        host: anchor.host,
        at: anchor.at,
        window_seconds: 60,
    };
    let listed = merged_view(&case, &source, 100, 0, NY).unwrap();
    assert!(listed.items.iter().any(|m| m.row.row_id == row));
}

#[test]
fn a_row_of_a_source_lacking_the_summary_columns_reads_as_merged_rows_do() {
    // An older source whose table has none of the columns the relation's summary
    // lists: the merged list applies the rule to the relation's columns (all
    // empty for this row), and the detail title must say the same, not fall back
    // to the row's plain values.
    let (d, mut case) = loaded();
    let root = narrow_evtx_run(d.path());
    load_run(&mut case, &root, &analyst()).unwrap();
    let row = n(
        &case,
        "select _ox_row from evtxtriage_events where \"PayloadData1\" = 'narrow-row-payload'",
    );
    let source = MergedSource::Around {
        host: "WS01".into(),
        at: "2026-01-05T02:13:58.204000Z".into(),
        window_seconds: 5,
    };
    let merged = merged_view(&case, &source, 100, 0, NY).unwrap();
    let same = merged.items.iter().find(|m| m.row.row_id == row).unwrap();
    let view = row_view(&case, row, NY).unwrap();
    assert_eq!(view.summary.unwrap_or_default(), same.row.summary);
    assert_eq!(same.row.summary, "");
}

#[test]
fn an_untimed_row_has_no_anchor_but_a_summary() {
    let (_d, case) = loaded();
    let untimed = n(
        &case,
        "select _ox_row from evtxtriage_events where \"TimeCreated__text\" = 'not-a-time'",
    );
    let view = row_view(&case, untimed, NY).unwrap();
    assert_eq!(view.anchor, None);
    assert!(view.summary.is_some());
}
