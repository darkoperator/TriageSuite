//! The row view the detail pane shows, one cell's whole value split into
//! its fields, the pivot totals around a row, and merged pages with their
//! time in the display zone.

mod common;

use common::{analyst, copy_tree, loaded, n};
use oxl_engine::catalog::appmap::LOCATOR;
use oxl_engine::{
    anchor_of, around, around_totals, cell_value, columns, load_file, load_run, merged_view,
    page_view, row_view, Case, EngineError, FileOptions, GridState, MergedSource, PageOptions,
    RowView, SearchState, TimeSemantics, ValuePart, CELL_CAP,
};
use oxl_fixtures::{
    loose_fw_csv, new_case, real_headers_run, two_hosts, two_hosts_with_manifest,
    write_run_manifest, ManifestFile,
};

const NY: &str = "America/New_York";

/// The first WS01 event: the 02:13:58 failed logon.
fn first_ws01_event(case: &Case) -> i64 {
    n(
        case,
        "select _ox_row from evtxtriage_events \
         where \"TimeCreated__text\" = '2026-01-05T02:13:58.2040000Z'",
    )
}

/// WS01's event whose time does not cast.
fn untimed_event(case: &Case) -> i64 {
    n(
        case,
        "select _ox_row from evtxtriage_events where \"TimeCreated__text\" = 'not-a-time'",
    )
}

fn field<'a>(view: &'a RowView, name: &str) -> &'a oxl_engine::FieldView {
    view.fields
        .iter()
        .find(|f| f.meta.name == name)
        .unwrap_or_else(|| panic!("no field {name}"))
}

fn kv(key: Option<&str>, value: &str) -> ValuePart {
    ValuePart {
        key: key.map(str::to_string),
        value: value.to_string(),
    }
}

fn loaded_from(root: &std::path::Path, dir: &std::path::Path) -> Case {
    let mut case = new_case(dir);
    load_run(&mut case, root, &analyst()).unwrap();
    case
}

#[test]
fn row_view_renders_and_provides() {
    let (_d, case) = loaded();
    let row = first_ws01_event(&case);
    let view = row_view(&case, row, NY).unwrap();
    assert_eq!(view.zone, NY);
    assert_eq!(view.detail.row_id, row);
    assert_eq!(view.detail.relation, "evtxtriage_events");

    // Every field of the row, in the detail's order, with its catalog meta.
    let names: Vec<&str> = view.fields.iter().map(|f| f.meta.name.as_str()).collect();
    let detail_names: Vec<&str> = view.detail.fields.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, detail_names);

    // A UTC time in the zone, its fraction as written, with the offset.
    let t = field(&view, "TimeCreated");
    assert_eq!(t.meta.time_semantics, Some(TimeSemantics::Utc));
    assert_eq!(t.text.as_deref(), Some("2026-01-05T02:13:58.2040000Z"));
    assert_eq!(
        t.display.as_deref(),
        Some("2026-01-04 21:13:58.2040000 -05:00")
    );
    assert!(!t.failed);
    // Not a time: no display.
    let user = field(&view, "UserName");
    assert_eq!(user.text.as_deref(), Some("Administrator"));
    assert_eq!(user.display, None);

    // Provenance.
    let p = &view.provenance;
    assert_eq!(p.host.as_deref(), Some("WS01"));
    assert_eq!(p.source_id, view.detail.source_id);
    assert_eq!(p.run_id.as_deref(), Some("20260101000000-aaaa"));
    assert_eq!(
        p.evidence_path.as_deref(),
        Some(r"C:\Windows\System32\winevt\Logs\Security.evtx")
    );
    let output = p.output_file.as_deref().unwrap().replace('\\', "/");
    assert!(
        output.ends_with("Processed-WS01-20260101000000/EventLogs/evtx.csv"),
        "{output}"
    );
    // This fixture's events have no EventRecordId column.
    assert_eq!(p.locator, None);
    // The run as built writes no run manifest.
    assert_eq!(p.output_file_listed, None);

    // In UTC the offset is `Z`.
    let utc = row_view(&case, row, "UTC").unwrap();
    assert_eq!(
        field(&utc, "TimeCreated").display.as_deref(),
        Some("2026-01-05 02:13:58.2040000 Z")
    );

    // A time that did not convert is failed and has no display.
    let untimed = row_view(&case, untimed_event(&case), NY).unwrap();
    let t = field(&untimed, "TimeCreated");
    assert!(t.failed);
    assert_eq!(t.display, None);
    assert_eq!(t.text.as_deref(), Some("not-a-time"));

    // Once the run also writes a manifest naming the file, it is listed.
    let d = tempfile::tempdir().unwrap();
    let case = loaded_from(&two_hosts_with_manifest(d.path()), d.path());
    for zone in [NY, "UTC"] {
        let view = row_view(&case, first_ws01_event(&case), zone).unwrap();
        assert_eq!(view.provenance.output_file_listed, Some(true));
    }
    // SRV01's rows and Hayabusa's (an external tool) are listed too.
    let srv = n(
        &case,
        "select min(_ox_row) from evtxtriage_events where \"Computer\" = 'SRV01'",
    );
    assert_eq!(
        row_view(&case, srv, NY)
            .unwrap()
            .provenance
            .output_file_listed,
        Some(true)
    );
    let hayabusa = n(&case, "select min(_ox_row) from ext_hayabusa_csv_timeline");
    assert_eq!(
        row_view(&case, hayabusa, NY)
            .unwrap()
            .provenance
            .output_file_listed,
        Some(true)
    );
}

#[test]
fn a_manifest_that_does_not_name_the_file_says_so() {
    let d = tempfile::tempdir().unwrap();
    let built = two_hosts(&d.path().join("a"));
    let root = d.path().join("b");
    copy_tree(&built, &root);
    // Names WS01's prefetch output only, and SRV01's events under SRV01:
    // neither is WS01's event file.
    write_run_manifest(
        &root,
        "20260101000000-aaaa",
        &[
            ManifestFile::tool(
                "WS01",
                "PETriage",
                "Processed-WS01-20260101000000/Execution/pf.csv",
            ),
            ManifestFile::tool(
                "SRV01",
                "EvtxTriage",
                "Processed-SRV01-20260101000000/EventLogs/evtx.csv",
            ),
        ],
    );
    let case = loaded_from(&root, d.path());
    let view = row_view(&case, first_ws01_event(&case), NY).unwrap();
    assert_eq!(view.provenance.output_file_listed, Some(false));
    let pf = n(
        &case,
        "select min(_ox_row) from petriage_timeline where \"ExecutableName\" = 'SVCH0ST.EXE'",
    );
    assert_eq!(
        row_view(&case, pf, NY)
            .unwrap()
            .provenance
            .output_file_listed,
        Some(true)
    );
}

#[test]
fn a_manifest_written_before_the_run_moved_still_lists_its_files() {
    // The manifest's absolute paths name where the run was written; the
    // files are matched by their place under the run root.
    let d = tempfile::tempdir().unwrap();
    let built = two_hosts_with_manifest(&d.path().join("a"));
    let root = d.path().join("b");
    copy_tree(&built, &root);
    let case = loaded_from(&root, d.path());
    let view = row_view(&case, first_ws01_event(&case), NY).unwrap();
    assert_eq!(view.provenance.output_file_listed, Some(true));
}

#[test]
fn a_loose_file_has_no_run_and_no_manifest_answer() {
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
    let row = n(&case, "select min(_ox_row) from fw_csv");
    let view = row_view(&case, row, NY).unwrap();
    let p = &view.provenance;
    assert_eq!(p.run_id, None);
    assert_eq!(p.output_file_listed, None);
    assert_eq!(p.locator, None);
    // An `unknown` time is shown as written, never converted.
    let t = field(&view, "time");
    assert_eq!(t.display, None);
    assert_eq!(t.text.as_deref(), Some("2026-01-05 02:14:00"));
}

#[test]
fn row_view_refuses_a_bad_zone_and_an_unknown_row() {
    let (_d, case) = loaded();
    let row = first_ws01_event(&case);
    assert!(matches!(
        row_view(&case, row, "Mars/Olympus").unwrap_err(),
        EngineError::Invalid(_)
    ));
    assert!(matches!(
        row_view(&case, -1, "UTC").unwrap_err(),
        EngineError::NotFound(_)
    ));
}

#[test]
fn a_json_cell_splits_into_raw_values() {
    // EvtxTriage's Payload is JSON: each top-level entry's value comes back
    // as the cell wrote it, a string with its quotes.
    let d = tempfile::tempdir().unwrap();
    let case = loaded_from(&real_headers_run(d.path()), d.path());
    let row = n(&case, "select min(_ox_row) from evtxtriage_events");
    let cell = cell_value(&case, row, "Payload").unwrap();
    assert_eq!(
        cell.text.as_deref(),
        Some(r#"{"EventData":{"TargetUserName":"Administrator"}}"#)
    );
    assert_eq!(
        cell.parts,
        vec![kv(
            Some("EventData"),
            r#"{"TargetUserName":"Administrator"}"#
        )]
    );
}

#[test]
fn locator_columns_exist() {
    // One fixture dataset per entry, each with the header its tool really
    // writes: an entry added here without one fails this test.
    assert_eq!(
        LOCATOR,
        &[
            ("evtxtriage_events", "EventRecordId"),
            ("mftriage_mft", "EntryNumber"),
        ]
    );
    let d = tempfile::tempdir().unwrap();
    let case = loaded_from(&real_headers_run(d.path()), d.path());
    let expected = [("evtxtriage_events", "7351"), ("mftriage_mft", "41")];
    for ((relation, column), (rel, value)) in LOCATOR.iter().zip(expected) {
        assert_eq!(*relation, rel);
        let catalogued = columns(&case, relation).unwrap();
        assert!(
            catalogued.iter().any(|c| c.name == *column),
            "{relation} has no catalogued {column}"
        );
        let row = n(&case, &format!("select min(_ox_row) from {relation}"));
        let view = row_view(&case, row, "UTC").unwrap();
        assert_eq!(
            view.provenance.locator,
            Some((column.to_string(), value.to_string())),
            "{relation}"
        );
        assert!(view.provenance.evidence_path.is_some(), "{relation}");
    }
}

#[test]
fn cell_value_is_unclipped() {
    let d = tempfile::tempdir().unwrap();
    let long = format!("User: {}", "x".repeat(4_994));
    assert_eq!(long.chars().count(), 5_000);
    let path = d.path().join("big.csv");
    std::fs::write(&path, format!("Name,Details\nfirst,{long}\n")).unwrap();
    let mut case = new_case(d.path());
    load_file(&mut case, &path, FileOptions::default(), &analyst()).unwrap();
    let row = n(&case, "select min(_ox_row) from big_csv");

    let cell = cell_value(&case, row, "Details").unwrap();
    assert_eq!(cell.text.as_deref(), Some(long.as_str()));
    assert_eq!(cell.parts, vec![kv(None, &long)]);

    // The grid cuts the same cell; the row view does not.
    let state = GridState {
        relation: "big_csv".into(),
        chips: Vec::new(),
        search: SearchState::default(),
        hosts: Vec::new(),
        sort: Vec::new(),
        columns: None,
    };
    let opts = PageOptions {
        zone: "UTC".into(),
        rules: Vec::new(),
    };
    let page = page_view(&case, &state, 10, 0, &opts).unwrap();
    assert!(page.cells[0][1].clipped);
    assert_eq!(
        page.cells[0][1].text.as_deref().unwrap().chars().count(),
        CELL_CAP
    );
    let view = row_view(&case, row, "UTC").unwrap();
    assert_eq!(field(&view, "Details").text.as_deref(), Some(long.as_str()));

    // A packed value comes back split.
    let (_d2, case) = loaded();
    let row = n(
        &case,
        "select _ox_row from ext_hayabusa_csv_timeline where \"EventID\" = '4625'",
    );
    let cell = cell_value(&case, row, "Details").unwrap();
    assert_eq!(
        cell.parts,
        vec![
            kv(Some("User"), "Administrator"),
            kv(Some("SrcIP"), "203.0.113.7")
        ]
    );
    // An empty cell has no text and no parts.
    let srv_blank = n(
        &case,
        "select _ox_row from evtxtriage_events where \"TimeCreated__text\" is null",
    );
    let cell = cell_value(&case, srv_blank, "TimeCreated").unwrap();
    assert_eq!(cell.text, None);
    assert!(cell.parts.is_empty());

    assert!(matches!(
        cell_value(&case, row, "NoSuchColumn").unwrap_err(),
        EngineError::NotFound(_)
    ));
    assert!(matches!(
        cell_value(&case, -1, "Details").unwrap_err(),
        EngineError::NotFound(_)
    ));
}

#[test]
fn around_totals_sum_relations() {
    let (_d, case) = loaded();
    let row = n(
        &case,
        "select _ox_row from evtxtriage_events \
         where \"TimeCreated__text\" = '2026-01-05T02:15:44.3800000Z'",
    );
    let (host, at) = anchor_of(&case, row).unwrap();
    assert_eq!(host, "WS01");
    let windows = [60, 300, 3_600];
    let totals = around_totals(&case, row, &windows).unwrap();
    assert_eq!(totals.len(), 3);
    for (total, window) in totals.iter().zip(windows) {
        assert_eq!(total.seconds, window);
        let summed: u64 = around(&case, &host, &at, window)
            .unwrap()
            .iter()
            .map(|c| c.count)
            .sum();
        assert_eq!(total.rows, summed, "window {window}");
    }
    // Wider windows hold at least as much.
    assert!(totals[0].rows > 0);
    assert!(totals[0].rows <= totals[1].rows && totals[1].rows <= totals[2].rows);

    // A window is at most a day.
    assert!(around_totals(&case, row, &[86_400]).is_ok());
    assert!(matches!(
        around_totals(&case, row, &[86_401]).unwrap_err(),
        EngineError::Invalid(_)
    ));
}

#[test]
fn around_totals_untimed_row_is_invalid() {
    let (_d, case) = loaded();
    assert!(matches!(
        around_totals(&case, untimed_event(&case), &[60]).unwrap_err(),
        EngineError::Invalid(_)
    ));
}

#[test]
fn merged_view_renders_time() {
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
    let source = MergedSource::Around {
        host: "WS01".into(),
        at: "2026-01-05T02:15:44.380000Z".into(),
        window_seconds: 300,
    };
    let page = merged_view(&case, &source, 1000, 0, NY).unwrap();
    assert!(!page.truncated);
    let svch0st = page
        .items
        .iter()
        .find(|m| {
            m.row.relation == "evtxtriage_events" && m.row.summary.contains("Process creation")
        })
        .unwrap();
    assert_eq!(
        svch0st.row.time.as_deref(),
        Some("2026-01-05T02:15:44.380000Z")
    );
    assert_eq!(
        svch0st.display_time.as_deref(),
        Some("2026-01-04 21:15:44.380000")
    );
    // Every UTC row has a display time; an `unknown` time has none.
    let fw: Vec<_> = page
        .items
        .iter()
        .filter(|m| m.row.relation == "fw_csv")
        .collect();
    assert!(!fw.is_empty());
    assert!(fw.iter().all(|m| m.display_time.is_none()));
    assert!(page
        .items
        .iter()
        .filter(|m| m.row.time_semantics == Some(TimeSemantics::Utc))
        .all(|m| m.display_time.is_some()));

    // The same page as `merged_rows`, in the same order.
    let plain = oxl_engine::merged_rows(&case, &source, 1000, 0).unwrap();
    let ids: Vec<i64> = page.items.iter().map(|m| m.row.row_id).collect();
    let plain_ids: Vec<i64> = plain.items.iter().map(|r| r.row_id).collect();
    assert_eq!(ids, plain_ids);

    assert!(matches!(
        merged_view(&case, &source, 10, 0, "Not/AZone").unwrap_err(),
        EngineError::Invalid(_)
    ));
    assert!(matches!(
        merged_view(&case, &source, 0, 0, NY).unwrap_err(),
        EngineError::Invalid(_)
    ));
}
