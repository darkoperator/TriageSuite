//! CSV export of the grid's current rows: the state's rows in its order,
//! each cell as stored (or a UTC time in a chosen zone), never written into
//! evidence or run output.

mod common;

use common::{analyst, loaded};
use oxl_engine::{
    columns, export_csv, load_file, parse_filter, remove_source, sources, Case, Chip, ChipOrigin,
    EngineError, FileOptions, GridState, SearchMode, SearchState, SortKey,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const EVENTS: &str = "evtxtriage_events";
const NY: &str = "America/New_York";

fn state(relation: &str, chips: Vec<Chip>) -> GridState {
    GridState {
        relation: relation.into(),
        chips,
        search: SearchState::default(),
        hosts: Vec::new(),
        sort: Vec::new(),
        columns: None,
    }
}

fn chip(id: &str, filter: &str) -> Chip {
    Chip {
        id: id.into(),
        label: format!("chip {id}"),
        enabled: true,
        origin: ChipOrigin::FilterRow,
        filter: parse_filter(filter).unwrap(),
    }
}

fn sysmon() -> Chip {
    chip("c1", r#"{"column":"Channel","op":"eq","value":"Sysmon"}"#)
}

fn go() -> AtomicBool {
    AtomicBool::new(false)
}

/// The exported file: its header and records, as the `csv` crate reads them.
fn read(path: &Path) -> (Vec<String>, Vec<Vec<String>>) {
    let mut r = csv::Reader::from_path(path).unwrap();
    let header = r.headers().unwrap().iter().map(str::to_string).collect();
    let rows = r
        .records()
        .map(|rec| rec.unwrap().iter().map(str::to_string).collect())
        .collect();
    (header, rows)
}

fn at(header: &[String], name: &str) -> usize {
    header
        .iter()
        .position(|h| h == name)
        .unwrap_or_else(|| panic!("no column {name} in {header:?}"))
}

fn partial(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".partial");
    PathBuf::from(p)
}

fn the_run_root(case: &Case) -> PathBuf {
    PathBuf::from(&sources(case).unwrap()[0].path)
}

/// Load `text` as the loose file `name` beside the case; the relation name.
fn loose(case: &mut Case, dir: &Path, name: &str, text: &str) -> (PathBuf, String) {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    let rel = load_file(case, &path, FileOptions::default(), &analyst())
        .unwrap()
        .relations[0]
        .name
        .clone();
    (path, rel)
}

#[test]
fn export_writes_the_filtered_rows() {
    let (d, case) = loaded();
    let out = d.path().join("sysmon.csv");
    let report = export_csv(&case, &state(EVENTS, vec![sysmon()]), &out, None, &go()).unwrap();
    assert_eq!(report.rows, 3);
    assert_eq!(report.path, out);
    assert!(!partial(&out).exists());

    let (header, rows) = read(&out);
    let catalogued: Vec<String> = columns(&case, EVENTS)
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(header, catalogued);
    assert_eq!(rows.len(), 3);
    // The grid's default order, the primary time; each time as the CSV the
    // suite wrote has it, seven fractional digits and the `Z`.
    let t = at(&header, "TimeCreated");
    let times: Vec<&str> = rows.iter().map(|r| r[t].as_str()).collect();
    assert_eq!(
        times,
        [
            "2026-01-05T02:15:44.3800000Z",
            "2026-01-05T02:15:46.0180000Z",
            "2026-01-05T02:17:02.6450000Z",
        ]
    );
    let c = at(&header, "Channel");
    assert!(rows.iter().all(|r| r[c] == "Sysmon"));
    assert_eq!(rows[1][at(&header, "PayloadData1")], "svch0st.exe:443");
    assert_eq!(rows[0][at(&header, "RemoteHost")], "");
    assert_eq!(rows[0][at(&header, "EventId")], "1");

    // The state's columns, in its order, and its sort.
    let mut narrow = state(EVENTS, vec![sysmon()]);
    narrow.columns = Some(vec!["PayloadData1".into(), "TimeCreated".into()]);
    narrow.sort = vec![SortKey {
        column: "TimeCreated".into(),
        descending: true,
    }];
    let out2 = d.path().join("narrow.csv");
    export_csv(&case, &narrow, &out2, None, &go()).unwrap();
    let (header, rows) = read(&out2);
    assert_eq!(header, ["PayloadData1", "TimeCreated"]);
    assert_eq!(rows[0], ["rclone.exe", "2026-01-05T02:17:02.6450000Z"]);
    assert_eq!(rows.len(), 3);

    // Hosts narrow it; the search narrows it in filter mode only.
    let mut srv = state(EVENTS, Vec::new());
    srv.hosts = vec!["SRV01".into()];
    assert_eq!(export_csv(&case, &srv, &out2, None, &go()).unwrap().rows, 3);
    let mut searched = state(EVENTS, Vec::new());
    searched.search.text = "rclone".into();
    assert_eq!(
        export_csv(&case, &searched, &out2, None, &go())
            .unwrap()
            .rows,
        1
    );
    searched.search.mode = SearchMode::Find;
    assert_eq!(
        export_csv(&case, &searched, &out2, None, &go())
            .unwrap()
            .rows,
        11
    );

    // Quotes, commas and line breaks round-trip through the csv crate.
    let mut case = case;
    let source = "name,note\n\
                  plain,\"a, b\"\n\
                  quoted,\"say \"\"hi\"\"\"\n\
                  multi,\"one\ntwo\"\n";
    let (src, rel) = loose(&mut case, d.path(), "quoted.csv", source);
    let out3 = d.path().join("quoted-out.csv");
    let report = export_csv(&case, &state(&rel, Vec::new()), &out3, None, &go()).unwrap();
    assert_eq!(report.rows, 3);
    let (src_header, src_rows) = read(&src);
    let (header, rows) = read(&out3);
    let note = at(&header, "note");
    let name = at(&header, "name");
    let got: Vec<(String, String)> = rows
        .iter()
        .map(|r| (r[name].clone(), r[note].clone()))
        .collect();
    let want: Vec<(String, String)> = src_rows
        .iter()
        .map(|r| {
            (
                r[at(&src_header, "name")].clone(),
                r[at(&src_header, "note")].clone(),
            )
        })
        .collect();
    assert_eq!(got, want);
}

#[test]
fn export_in_a_zone() {
    let (d, case) = loaded();
    let out = d.path().join("ny.csv");
    export_csv(&case, &state(EVENTS, vec![sysmon()]), &out, Some(NY), &go()).unwrap();
    let (header, rows) = read(&out);
    let t = at(&header, "TimeCreated");
    assert_eq!(rows[0][t], "2026-01-04 21:15:44.3800000 -05:00");
    // A typed column that is not a time is written as stored.
    assert_eq!(rows[0][at(&header, "EventId")], "1");

    // A time that does not convert, and a blank one, are written as stored.
    export_csv(&case, &state(EVENTS, Vec::new()), &out, Some(NY), &go()).unwrap();
    let (header, rows) = read(&out);
    let t = at(&header, "TimeCreated");
    assert!(rows.iter().any(|r| r[t] == "not-a-time"));
    assert!(rows.iter().any(|r| r[t].is_empty()));
    assert!(rows
        .iter()
        .any(|r| r[t] == "2026-01-04 21:13:58.2040000 -05:00"));

    // In UTC the offset is `Z`, as the row detail shows it.
    export_csv(
        &case,
        &state(EVENTS, vec![sysmon()]),
        &out,
        Some("UTC"),
        &go(),
    )
    .unwrap();
    let (header, rows) = read(&out);
    assert_eq!(
        rows[0][at(&header, "TimeCreated")],
        "2026-01-05 02:15:44.3800000 Z"
    );

    // An unknown zone is refused before anything is written.
    let bad = d.path().join("bad.csv");
    let err = export_csv(
        &case,
        &state(EVENTS, Vec::new()),
        &bad,
        Some("Mars/Base"),
        &go(),
    )
    .unwrap_err();
    assert_eq!(err.code(), "invalid");
    assert!(!bad.exists() && !partial(&bad).exists());
}

#[test]
fn export_refuses_the_run_folder() {
    let (d, mut case) = loaded();
    let root = the_run_root(&case);
    let st = state(EVENTS, Vec::new());

    let evidence = root
        .join("Processed-WS01-20260101000000")
        .join("EventLogs")
        .join("evtx.csv");
    let before = std::fs::read(&evidence).unwrap();
    for path in [
        root.join("out.csv"),
        root.join("Processed-WS01-20260101000000").join("out.csv"),
        evidence.clone(),
        // Not canonical: `..` and back in again.
        root.join("duckdb").join("..").join("out.csv"),
    ] {
        let err = export_csv(&case, &st, &path, None, &go()).unwrap_err();
        assert!(matches!(err, EngineError::Refused(_)), "{path:?}: {err}");
        assert!(!partial(&path).exists(), "{path:?}");
    }
    assert!(!root.join("out.csv").exists());
    assert!(!root
        .join("Processed-WS01-20260101000000")
        .join("out.csv")
        .exists());
    assert_eq!(std::fs::read(&evidence).unwrap(), before);

    // Nor over the case file itself.
    let err = export_csv(&case, &st, case.path(), None, &go()).unwrap_err();
    assert!(matches!(err, EngineError::Refused(_)), "{err}");

    // A folder is not a file to write.
    let err = export_csv(&case, &st, d.path(), None, &go()).unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "{err}");
    // A folder that does not exist is not there to write in.
    let err = export_csv(
        &case,
        &st,
        &d.path().join("nope").join("x.csv"),
        None,
        &go(),
    )
    .unwrap_err();
    assert_eq!(err.code(), "not_found");

    // A loose file is its own source: writing over it is refused, beside it
    // is not.
    let (fw, rel) = loose(&mut case, d.path(), "fw.csv", "a,b\n1,2\n");
    let err = export_csv(&case, &state(&rel, Vec::new()), &fw, None, &go()).unwrap_err();
    assert!(matches!(err, EngineError::Refused(_)), "{err}");
    assert_eq!(std::fs::read_to_string(&fw).unwrap(), "a,b\n1,2\n");
    let beside = d.path().join("fw-export.csv");
    assert_eq!(
        export_csv(&case, &state(&rel, Vec::new()), &beside, None, &go())
            .unwrap()
            .rows,
        1
    );

    // Once the run is removed its folder is no longer a source of the case.
    let run = sources(&case).unwrap()[0].id;
    remove_source(&mut case, run, true, &analyst()).unwrap();
    let out = root.join("out.csv");
    export_csv(&case, &state(&rel, Vec::new()), &out, None, &go()).unwrap();
    assert!(out.exists());
}

#[test]
fn export_cancel_removes_the_partial() {
    let (d, case) = loaded();
    let out = d.path().join("cancelled.csv");
    std::fs::write(&out, "an earlier export\n").unwrap();
    let cancel = AtomicBool::new(true);
    let err = export_csv(&case, &state(EVENTS, Vec::new()), &out, None, &cancel).unwrap_err();
    assert!(
        matches!(&err, EngineError::Invalid(m) if m.contains("cancelled")),
        "{err}"
    );
    assert!(!partial(&out).exists());
    // What was at the path is untouched.
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "an earlier export\n"
    );
}

/// The cancel flag is looked at every 10,000 rows, not only before the
/// first: set from another thread once the partial file has its first
/// bytes (the CSV writer's buffer is flushed only after the row-0 check),
/// the export stops part way, removes its partial file and leaves no file at
/// the path. The partial file is watched while it grows: an export that only
/// looked at the flag before the first row and after the last would write
/// all 400,000 rows first, far past the bound asserted on what was seen.
#[test]
fn export_stops_mid_file() {
    use std::sync::atomic::Ordering;
    const ROWS: u32 = 400_000;
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let mut text = String::from("n\n");
    for i in 0..ROWS {
        text.push_str(&format!("{i:07}\n"));
    }
    let (_, rel) = loose(&mut case, d.path(), "many.csv", &text);
    let mut st = state(&rel, Vec::new());
    st.columns = Some(vec!["n".into()]);
    let out = d.path().join("stopped.csv");
    let cancel = AtomicBool::new(false);
    let done = AtomicBool::new(false);
    let (result, largest) = std::thread::scope(|s| {
        let watcher = s.spawn(|| {
            let mut largest = 0u64;
            while !done.load(Ordering::SeqCst) {
                if let Ok(meta) = std::fs::metadata(partial(&out)) {
                    largest = largest.max(meta.len());
                    if meta.len() > 0 {
                        cancel.store(true, Ordering::SeqCst);
                    }
                }
                std::thread::yield_now();
            }
            largest
        });
        let result = export_csv(&case, &st, &out, None, &cancel);
        done.store(true, Ordering::SeqCst);
        (result, watcher.join().unwrap())
    });
    let err = result.unwrap_err();
    assert!(
        matches!(&err, EngineError::Invalid(m) if m.contains("cancelled")),
        "{err}"
    );
    assert!(
        cancel.load(Ordering::SeqCst),
        "the watcher never saw a byte"
    );
    assert!(!partial(&out).exists());
    assert!(!out.exists());
    // Eight bytes a row: what was seen stayed far below the whole file.
    let whole = u64::from(ROWS) * 8;
    assert!(largest < whole / 2, "{largest} of {whole} bytes written");
}

#[test]
fn export_hostile_text_is_unchanged() {
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let hostile = [
        r#"=HYPERLINK("http://x")"#,
        "+cmd|' /C calc'!A0",
        "-2+3",
        "@SUM(1+1)",
        "<img src=x onerror=alert(1)>",
        "\u{202E}exe.txt",
    ];
    let mut text = String::from("id,value\n");
    for (i, h) in hostile.iter().enumerate() {
        text.push_str(&format!("{i},\"{}\"\n", h.replace('"', "\"\"")));
    }
    let (_, rel) = loose(&mut case, d.path(), "hostile.csv", &text);
    let out = d.path().join("hostile-out.csv");
    let mut st = state(&rel, Vec::new());
    st.sort = vec![SortKey {
        column: "id".into(),
        descending: false,
    }];
    export_csv(&case, &st, &out, None, &go()).unwrap();

    let (header, rows) = read(&out);
    let v = at(&header, "value");
    let values: Vec<&str> = rows.iter().map(|r| r[v].as_str()).collect();
    assert_eq!(values, hostile);
    // The file is data, not a workbook: nothing is prefixed or escaped
    // beyond CSV's own quoting.
    let raw = std::fs::read_to_string(&out).unwrap();
    assert!(raw.contains(r#""=HYPERLINK(""http://x"")""#), "{raw}");
    assert!(!raw.contains("'="));
    assert!(raw.contains("\u{202E}exe.txt"));
}

#[test]
fn export_streams_many_chunks_in_order() {
    // Past DuckDB's 2,048-row chunk and the 10,000-row cancel interval,
    // sorted against the file's order.
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let mut text = String::from("n,label\n");
    for i in 0..25_000u32 {
        text.push_str(&format!("{i},row {}\n", 24_999 - i));
    }
    let (_, rel) = loose(&mut case, d.path(), "many.csv", &text);
    let mut st = state(&rel, Vec::new());
    st.sort = vec![SortKey {
        column: "label".into(),
        descending: false,
    }];
    st.columns = Some(vec!["label".into()]);
    let out = d.path().join("many-out.csv");
    assert_eq!(
        export_csv(&case, &st, &out, None, &go()).unwrap().rows,
        25_000
    );
    let (header, rows) = read(&out);
    assert_eq!(header, ["label"]);
    let labels: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
    let mut sorted = labels.clone();
    sorted.sort_by_key(|l| l.to_lowercase());
    assert_eq!(labels.len(), 25_000);
    assert_eq!(labels, sorted);
}

/// WS01's event file of the loaded run, and its bytes.
fn ws01_evidence(case: &Case) -> (PathBuf, Vec<u8>) {
    let path = the_run_root(case)
        .join("Processed-WS01-20260101000000")
        .join("EventLogs")
        .join("evtx.csv");
    let bytes = std::fs::read(&path).unwrap();
    (path, bytes)
}

fn file_link(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    return std::os::windows::fs::symlink_file(target, link);
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
}

fn assert_partial_exists_refusal(err: &EngineError) {
    assert!(
        matches!(err, EngineError::Invalid(m) if m.contains("already exists")),
        "{err}"
    );
}

#[test]
fn export_refuses_an_existing_partial_file() {
    let (d, case) = loaded();
    let out = d.path().join("out.csv");
    std::fs::write(partial(&out), "not this export's\n").unwrap();
    let err = export_csv(&case, &state(EVENTS, Vec::new()), &out, None, &go()).unwrap_err();
    assert_partial_exists_refusal(&err);
    // Neither overwritten nor removed, and nothing written at the path.
    assert_eq!(
        std::fs::read_to_string(partial(&out)).unwrap(),
        "not this export's\n"
    );
    assert!(!out.exists());
}

#[test]
fn export_never_follows_a_linked_partial() {
    let (d, case) = loaded();
    let (evidence, before) = ws01_evidence(&case);
    let out = d.path().join("out.csv");
    if let Err(e) = file_link(&evidence, &partial(&out)) {
        println!("note: skipped, this system refused to make a symbolic link: {e}");
        return;
    }
    let err = export_csv(&case, &state(EVENTS, Vec::new()), &out, None, &go()).unwrap_err();
    assert_partial_exists_refusal(&err);
    assert_eq!(std::fs::read(&evidence).unwrap(), before);
    // The link is left where it was, still a link.
    assert!(std::fs::symlink_metadata(partial(&out))
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(!out.exists());
}

/// A junction is the link a Windows user can make without a privilege.
#[cfg(windows)]
#[test]
fn export_never_follows_a_junction_partial() {
    let (d, case) = loaded();
    let (evidence, before) = ws01_evidence(&case);
    let folder = evidence.parent().unwrap();
    let listing = |p: &Path| -> Vec<_> {
        let mut names: Vec<_> = std::fs::read_dir(p)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let names = listing(folder);
    let out = d.path().join("out.csv");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(partial(&out))
        .arg(folder)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J exited with {status}");
    let err = export_csv(&case, &state(EVENTS, Vec::new()), &out, None, &go()).unwrap_err();
    assert_partial_exists_refusal(&err);
    assert_eq!(std::fs::read(&evidence).unwrap(), before);
    assert_eq!(listing(folder), names);
    assert!(std::fs::symlink_metadata(partial(&out)).is_ok());
    assert!(!out.exists());
}

#[test]
fn export_replaces_a_link_at_the_path_never_writing_through_it() {
    let (d, case) = loaded();
    let elsewhere = d.path().join("elsewhere.txt");
    std::fs::write(&elsewhere, "keep me\n").unwrap();
    let out = d.path().join("out.csv");
    if let Err(e) = file_link(&elsewhere, &out) {
        println!("note: skipped, this system refused to make a symbolic link: {e}");
        return;
    }
    let report = export_csv(&case, &state(EVENTS, vec![sysmon()]), &out, None, &go()).unwrap();
    assert_eq!(report.rows, 3);
    // The link was replaced by the export; what it pointed at is unchanged.
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "keep me\n");
    let meta = std::fs::symlink_metadata(&out).unwrap();
    assert!(!meta.file_type().is_symlink() && meta.is_file());
    assert_eq!(read(&out).1.len(), 3);

    // A link at the path that leads into a source is refused outright.
    let (evidence, before) = ws01_evidence(&case);
    let into = d.path().join("into.csv");
    file_link(&evidence, &into).unwrap();
    let err = export_csv(&case, &state(EVENTS, Vec::new()), &into, None, &go()).unwrap_err();
    assert!(matches!(err, EngineError::Refused(_)), "{err}");
    assert_eq!(std::fs::read(&evidence).unwrap(), before);
    assert!(!partial(&into).exists());
}
