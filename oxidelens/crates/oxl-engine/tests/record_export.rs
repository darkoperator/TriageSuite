//! Exports of the case record: the timeline as CSV, a workbook of the
//! Timeline, Hosts, Accounts and IOCs sheets, and a Markdown report, each
//! written whole or not at all and never into the case file or evidence;
//! and the history of one target.

mod common;

use common::{analyst, loaded, n};
use oxl_engine::{
    edit_entry, export_report, export_timeline_csv, export_workbook, history, new_entry,
    promote_rows, sources, upsert_account, upsert_host, upsert_ioc, AccountSpec, Case, EngineError,
    EntryEdit, HostSpec, IocSpec, IocType, MAX_TITLE,
};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const FAILED_LOGON_WS01: &str =
    "select _ox_row from evtxtriage_events where MapDescription = 'Failed logon'";

fn go() -> AtomicBool {
    AtomicBool::new(false)
}

fn partial(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".partial");
    PathBuf::from(p)
}

fn entry(case: &mut Case, title: &str, description: &str, time: Option<&str>) -> i64 {
    let edit = EntryEdit {
        title: Some(title.to_string()),
        description: Some(description.to_string()),
        time: time.map(|t| Some(t.to_string())),
        ..Default::default()
    };
    new_entry(case, &edit, &analyst()).unwrap().id
}

/// An entry, a host, an account and an IOC per hostile value of the
/// fixture, each value where its field holds it whole (a title holds 200
/// characters, an IOC value 4,096; the description and notes the rest).
fn hostile_record(case: &mut Case) {
    for (i, v) in oxl_fixtures::hostile_record_values()
        .into_iter()
        .enumerate()
    {
        let title = if v.chars().count() <= MAX_TITLE {
            v.to_string()
        } else {
            format!("long {i}")
        };
        entry(case, &title, v, Some("2026-01-05T10:00:00Z"));
        upsert_host(
            case,
            &HostSpec {
                name: format!("WS{i:02}"),
                note: v.to_string(),
                ..Default::default()
            },
            &analyst(),
        )
        .unwrap();
        upsert_account(
            case,
            &AccountSpec {
                name: format!("user{i}"),
                domain: Some("acme.local".into()),
                note: v.to_string(),
                ..Default::default()
            },
            &analyst(),
        )
        .unwrap();
        let value = if v.chars().count() <= oxl_engine::MAX_IOC_VALUE {
            v.to_string()
        } else {
            format!("long-{i}")
        };
        upsert_ioc(
            case,
            &IocSpec {
                id: None,
                ioc_type: IocType::Other,
                value,
                tool: None,
                attack_ids: Vec::new(),
                note: v.to_string(),
            },
            &analyst(),
        )
        .unwrap();
    }
}

fn read_csv(path: &Path) -> (Vec<String>, Vec<Vec<String>>) {
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

#[test]
fn timeline_csv_has_undated_last() {
    let (d, mut case) = loaded();
    let undated = entry(&mut case, "no time yet", "", None);
    let late = entry(&mut case, "late", "", Some("2026-01-05T10:00:00Z"));
    let early = entry(&mut case, "early", "", Some("2026-01-01T09:00:00Z"));
    let undated_too = entry(&mut case, "also no time", "", None);
    // An entry M made, whose time is its row's and whose evidence is the row.
    let row = n(&case, FAILED_LOGON_WS01);
    let promoted = promote_rows(&mut case, &[row], &analyst()).unwrap()[0]
        .entry_id
        .unwrap();
    // Hostile text goes out as the case holds it.
    let values = oxl_fixtures::hostile_record_values();
    let hostile = entry(
        &mut case,
        values[0],
        &format!("{}\nsecond line", values[3]),
        Some("2026-01-03T00:00:00Z"),
    );

    let out = d.path().join("timeline.csv");
    let report = export_timeline_csv(&case, &out, &go()).unwrap();
    assert_eq!(report.rows, 6);
    assert!(!partial(&out).exists());

    let (header, rows) = read_csv(&out);
    let ids: Vec<&str> = rows.iter().map(|r| r[at(&header, "id")].as_str()).collect();
    let promoted_time = rows
        .iter()
        .find(|r| r[at(&header, "id")] == format!("E-{promoted}"))
        .unwrap()[at(&header, "time")]
    .clone();
    // Dated by time (the promoted row's time is its own), undated last by id.
    let mut dated: Vec<(String, String)> = vec![
        ("2026-01-01T09:00:00.000000Z".into(), format!("E-{early}")),
        ("2026-01-03T00:00:00.000000Z".into(), format!("E-{hostile}")),
        ("2026-01-05T10:00:00.000000Z".into(), format!("E-{late}")),
        (promoted_time.clone(), format!("E-{promoted}")),
    ];
    dated.sort();
    let mut expected: Vec<String> = dated.into_iter().map(|(_, id)| id).collect();
    expected.push(format!("E-{undated}"));
    expected.push(format!("E-{undated_too}"));
    assert_eq!(ids, expected);
    assert!(!promoted_time.is_empty());

    let undated_row = &rows[4];
    assert_eq!(undated_row[at(&header, "time")], "");
    assert_eq!(undated_row[at(&header, "time_source")], "");
    assert_eq!(undated_row[at(&header, "needs_time")], "true");
    assert_eq!(rows[0][at(&header, "needs_time")], "false");
    assert_eq!(rows[0][at(&header, "time_source")], "typed");

    let by_id = |id: i64| {
        rows.iter()
            .find(|r| r[at(&header, "id")] == format!("E-{id}"))
            .unwrap()
    };
    // Byte for byte, a formula and a line break included.
    let h = by_id(hostile);
    assert_eq!(h[at(&header, "title")], values[0]);
    assert_eq!(
        h[at(&header, "description")],
        format!("{}\nsecond line", values[3])
    );
    let raw = std::fs::read_to_string(&out).unwrap();
    assert!(raw.contains("\"=HYPERLINK(\"\"http://x\"\")\""), "{raw}");
    // The promoted entry names its evidence row and its host.
    let p = by_id(promoted);
    assert_eq!(p[at(&header, "time_source")], "row");
    assert_eq!(
        p[at(&header, "evidence")],
        format!("evtxtriage_events#{row}")
    );
    assert_eq!(p[at(&header, "hosts")], "WS01");
    assert_eq!(p[at(&header, "evidence_rows")], "1");
}

/// Every `<c>` element's attributes and every `<f>` in a sheet's XML.
fn sheet_cells(xml: &str) -> Vec<&str> {
    xml.match_indices("<c ")
        .map(|(i, _)| {
            let end = xml[i..].find('>').unwrap();
            &xml[i..i + end]
        })
        .collect()
}

fn unxml(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn zip_text(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> String {
    let mut text = String::new();
    archive
        .by_name(name)
        .unwrap_or_else(|e| panic!("{name}: {e}"))
        .read_to_string(&mut text)
        .unwrap();
    text
}

#[test]
fn workbook_cells_are_strings() {
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    hostile_record(&mut case);
    let undated = entry(&mut case, "no time yet", "", None);

    let out = d.path().join("record.xlsx");
    let report = export_workbook(&case, &out, &go()).unwrap();
    // Six entries, five of each record.
    assert_eq!(report.rows, 6 + 5 + 5 + 5);
    assert!(!partial(&out).exists());

    let mut archive = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
    let workbook = zip_text(&mut archive, "xl/workbook.xml");
    let names: Vec<&str> = workbook
        .match_indices("<sheet name=\"")
        .map(|(i, m)| {
            let rest = &workbook[i + m.len()..];
            &rest[..rest.find('"').unwrap()]
        })
        .collect();
    assert_eq!(names, ["Timeline", "Hosts", "Accounts", "IOCs"]);

    let mut cells = 0;
    for sheet in 1..=4 {
        let xml = zip_text(&mut archive, &format!("xl/worksheets/sheet{sheet}.xml"));
        assert!(!xml.contains("<f>") && !xml.contains("<f "), "sheet{sheet}");
        for c in sheet_cells(&xml) {
            assert!(c.contains("t=\"s\""), "sheet{sheet}: {c}");
            cells += 1;
        }
    }
    assert!(cells > 100, "{cells} cells");
    // No formula anywhere in the package, nor a calculation chain.
    for i in 0..archive.len() {
        let name = archive.by_index(i).unwrap().name().to_string();
        assert_ne!(name, "xl/calcChain.xml");
    }

    let shared = unxml(&zip_text(&mut archive, "xl/sharedStrings.xml"));
    for v in oxl_fixtures::hostile_record_values() {
        assert!(shared.contains(&format!(">{v}<")), "{:.80}", v);
    }
    assert!(shared.contains(">=HYPERLINK(\"http://x\")<"));

    // The Timeline sheet's header, then its rows with the undated last.
    let timeline = zip_text(&mut archive, "xl/worksheets/sheet1.xml");
    let rows = timeline.matches("<row ").count();
    assert_eq!(rows, 1 + 6);
    assert!(shared.contains(">needs_time<"));
    assert!(shared.contains(&format!(">E-{undated}<")));
}

/// The pipes that delimit a table row's cells: those no backslash escapes.
fn bare_pipes(line: &str) -> usize {
    let b = line.as_bytes();
    (0..b.len())
        .filter(|&i| b[i] == b'|' && (i == 0 || b[i - 1] != b'\\'))
        .count()
}

#[test]
fn report_escapes_case_text() {
    let d = tempfile::tempdir().unwrap();
    let mut case = Case::create(
        &d.path().join("t.case.duckdb"),
        "Case <b>one</b> | *two* [x](y) `z` _w_ \\",
        &analyst(),
    )
    .unwrap();
    hostile_record(&mut case);
    // Every invisible formatting character the report shows as an escape.
    let invisible: Vec<char> = (0x200B..=0x200F)
        .chain(0x202A..=0x202E)
        .chain(0x2066..=0x2069)
        .chain([0xFEFF])
        .map(|c| char::from_u32(c).unwrap())
        .collect();
    let hidden: String = invisible.iter().map(|c| format!("{c}x")).collect();
    entry(&mut case, "hidden", &hidden, Some("2026-01-06T00:00:00Z"));
    let undated = entry(&mut case, "no time yet", "first\r\nsecond\nthird", None);

    let out = d.path().join("report.md");
    export_report(&case, &out, &go()).unwrap();
    assert!(!partial(&out).exists());
    let md = std::fs::read_to_string(&out).unwrap();

    // The title, the case's name escaped, the export time.
    let mut lines = md.lines();
    assert_eq!(lines.next(), Some("# Case record"));
    assert!(
        md.contains("Case: Case \\<b\\>one\\</b\\> \\| \\*two\\* \\[x\\](y) \\`z\\` \\_w\\_ \\\\")
    );
    assert!(md.contains("\nExported: "), "{md:.400}");

    // Each section in order, each a table.
    let at = |heading: &str| md.find(heading).unwrap_or_else(|| panic!("{heading}"));
    assert!(at("## Timeline") < at("## Hosts"));
    assert!(at("## Hosts") < at("## Accounts"));
    assert!(at("## Accounts") < at("## IOCs"));

    // Every hostile value, escaped as S11 says, appears; none raw.
    let values = oxl_fixtures::hostile_record_values();
    // A URL cannot autolink: every `://` in a value is defanged.
    assert!(md.contains("=HYPERLINK(\"http[:]//x\")"), "{md:.400}");
    assert!(!md.contains("://"));
    assert!(md.contains("\\<img src=x onerror=1\\>"));
    // No `<` or `>` but escaped ones: no tag can open.
    let unescaped = md.replace("\\\\", "").replace("\\<", "").replace("\\>", "");
    assert!(!unescaped.contains('<') && !unescaped.contains('>'));
    // A bidi override or an invisible character is shown, never applied.
    assert!(md.contains("invoice\\u{202E}fdp.exe"));
    let shown: String = invisible
        .iter()
        .map(|&c| format!("\\u{{{:04X}}}x", u32::from(c)))
        .collect();
    assert!(md.contains(&shown), "{shown}");
    assert!(shown.starts_with("\\u{200B}x") && shown.ends_with("\\u{FEFF}x"));
    for c in &invisible {
        assert!(
            !md.contains(*c),
            "U+{:04X} raw in the report",
            u32::from(*c)
        );
    }
    assert!(md.contains(values[3]));
    assert!(md.contains("a \\| b \\`c\\` d"));
    assert!(!md.contains("a | b"));
    // Line breaks in a description become spaces: one line per row.
    assert!(md.contains("first second third"));

    // Every table row has as many cells as its header.
    let mut tables = 0;
    let mut widths: Option<usize> = None;
    for line in md.lines() {
        if line.starts_with('|') {
            let pipes = bare_pipes(line);
            match widths {
                None => {
                    widths = Some(pipes);
                    tables += 1;
                }
                Some(w) => assert_eq!(pipes, w, "{line:.200}"),
            }
        } else {
            widths = None;
        }
    }
    assert_eq!(tables, 4);

    // The undated entry is the Timeline table's last row.
    let timeline = &md[at("## Timeline")..at("## Hosts")];
    let last = timeline.lines().rfind(|l| l.starts_with('|')).unwrap();
    assert!(last.starts_with(&format!("| E-{undated} |")), "{last}");
}

#[test]
fn exports_refuse_the_case_and_evidence_paths() {
    let (d, case) = loaded();
    let root = PathBuf::from(&sources(&case).unwrap()[0].path);
    let evidence = root
        .join("Processed-WS01-20260101000000")
        .join("EventLogs")
        .join("evtx.csv");
    let before = std::fs::read(&evidence).unwrap();
    let mut wal = case.path().as_os_str().to_owned();
    wal.push(".wal");
    let targets = [
        case.path().to_path_buf(),
        PathBuf::from(wal),
        evidence.clone(),
        root.join("record.out"),
        root.join("duckdb").join("..").join("record.out"),
    ];
    type Export = fn(&Case, &Path, &AtomicBool) -> oxl_engine::Result<oxl_engine::ExportReport>;
    let exports: [(&str, Export); 3] = [
        ("csv", export_timeline_csv),
        ("workbook", export_workbook),
        ("report", export_report),
    ];
    for (what, export) in exports {
        for path in &targets {
            let err = export(&case, path, &go()).unwrap_err();
            assert!(
                matches!(err, EngineError::Refused(_)),
                "{what} {path:?}: {err}"
            );
            assert!(!partial(path).exists(), "{what} {path:?}");
        }
        // Cancelled: nothing written, what was at the path left as it was.
        let out = d.path().join(format!("{what}.out"));
        std::fs::write(&out, "an earlier export\n").unwrap();
        let err = export(&case, &out, &AtomicBool::new(true)).unwrap_err();
        assert!(
            matches!(&err, EngineError::Invalid(m) if m.contains("cancelled")),
            "{what}: {err}"
        );
        assert!(!partial(&out).exists(), "{what}");
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "an earlier export\n"
        );
        // An existing partial file is not this export's: left as it is.
        std::fs::write(partial(&out), "not this export's\n").unwrap();
        let err = export(&case, &out, &go()).unwrap_err();
        assert!(
            matches!(&err, EngineError::Invalid(m) if m.contains("already exists")),
            "{what}: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(partial(&out)).unwrap(),
            "not this export's\n"
        );
        std::fs::remove_file(partial(&out)).unwrap();
        // And beside the case, it writes.
        export(&case, &out, &go()).unwrap();
        assert_ne!(std::fs::read(&out).unwrap(), b"an earlier export\n");
    }
    assert!(!root.join("record.out").exists());
    assert_eq!(std::fs::read(&evidence).unwrap(), before);
}

#[test]
fn history_of_one_entry() {
    let d = tempfile::tempdir().unwrap();
    let mut case = oxl_fixtures::new_case(d.path());
    let one = entry(&mut case, "one", "", None);
    let two = entry(&mut case, "two", "", None);
    let edit = EntryEdit {
        title: Some("one, edited".into()),
        ..Default::default()
    };
    edit_entry(&mut case, one, &edit, &analyst()).unwrap();
    // A host record with the same id as the entry, which its history must
    // not list.
    let host = upsert_host(
        &mut case,
        &HostSpec {
            name: "WS01".into(),
            ..Default::default()
        },
        &analyst(),
    )
    .unwrap();
    assert_eq!(host.id, one);

    let id = one.to_string();
    let rows = history(&case, Some("entry"), Some(&id), 50, 0).unwrap();
    let actions: Vec<(&str, &str)> = rows
        .items
        .iter()
        .map(|r| (r.target_id.as_str(), r.action.as_str()))
        .collect();
    assert_eq!(
        actions,
        [(id.as_str(), "entry_edit"), (id.as_str(), "entry_create")]
    );
    assert!(!rows.truncated);
    // Paged like every listing.
    let first = history(&case, Some("entry"), Some(&id), 1, 0).unwrap();
    assert_eq!(first.items.len(), 1);
    assert!(first.truncated);
    assert_eq!(first.items[0].action, "entry_edit");
    let second = history(&case, Some("entry"), Some(&id), 1, 1).unwrap();
    assert_eq!(second.items[0].action, "entry_create");
    assert!(!second.truncated);

    let other = history(&case, Some("entry"), Some(&two.to_string()), 50, 0).unwrap();
    assert_eq!(other.items.len(), 1);
    assert_eq!(other.items[0].target_id, two.to_string());
    let host_rows = history(&case, Some("host"), Some(&id), 50, 0).unwrap();
    assert!(host_rows.items.iter().all(|r| r.target_kind == "host"));
    assert_eq!(host_rows.items.len(), 1);
    // An id the history does not name lists nothing.
    assert!(history(&case, Some("entry"), Some("99"), 50, 0)
        .unwrap()
        .items
        .is_empty());
    // A hostile id is a bound value, matched as text.
    assert!(history(&case, Some("entry"), Some("1' OR '1'='1"), 50, 0)
        .unwrap()
        .items
        .is_empty());

    // An id means nothing without its kind; an over-long one is refused.
    let err = history(&case, None, Some(&id), 50, 0).unwrap_err();
    assert_eq!(err.code(), "invalid", "{err}");
    let long = "1".repeat(1_001);
    let err = history(&case, Some("entry"), Some(&long), 50, 0).unwrap_err();
    assert_eq!(err.code(), "invalid", "{err}");
    // Unfiltered, as before.
    assert_eq!(
        history(&case, Some("entry"), None, 50, 0)
            .unwrap()
            .items
            .len(),
        3
    );
}

/// Each export reads the case in one read transaction: a second connection
/// that commits between two of its reads (once the timeline is read, before
/// its evidence and the records) changes nothing the export writes. Without
/// the transaction the new evidence row and host record would be read.
#[test]
fn an_export_is_one_snapshot() {
    type Paused = fn(
        &Case,
        &Path,
        &AtomicBool,
        &mut dyn FnMut(),
    ) -> oxl_engine::Result<oxl_engine::ExportReport>;
    let exports: [(&str, Paused); 3] = [
        ("csv", oxl_engine::export_timeline_csv_paused),
        ("workbook", oxl_engine::export_workbook_paused),
        ("report", oxl_engine::export_report_paused),
    ];
    for (what, export) in exports {
        let (d, mut case) = loaded();
        let target = entry(&mut case, "target", "", Some("2026-01-05T10:00:00Z"));
        let row = n(&case, FAILED_LOGON_WS01);
        let mut other = case.try_clone().unwrap();
        let mut committed = false;
        let mut pause = || {
            oxl_engine::attach_rows(&mut other, target, &[row], &analyst()).unwrap();
            upsert_host(
                &mut other,
                &HostSpec {
                    name: "SRV77".into(),
                    ..Default::default()
                },
                &analyst(),
            )
            .unwrap();
            committed = true;
        };
        let during = d.path().join(format!("during.{what}"));
        let rows = export(&case, &during, &go(), &mut pause).unwrap().rows;
        assert!(committed, "{what}: the pause ran");
        // The commit is visible to the next read outside the export.
        let after = d.path().join(format!("after.{what}"));
        let rows_after = export(&case, &after, &go(), &mut || {}).unwrap().rows;

        match what {
            "csv" => {
                let (header, during) = read_csv(&during);
                let (_, after) = read_csv(&after);
                assert_eq!(during[0][at(&header, "evidence")], "", "{what}");
                assert_eq!(during[0][at(&header, "evidence_rows")], "0", "{what}");
                assert_eq!(
                    after[0][at(&header, "evidence")],
                    format!("evtxtriage_events#{row}")
                );
                assert_eq!(rows, rows_after);
            }
            "workbook" => {
                let shared = |path: &Path| {
                    let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
                    unxml(&zip_text(&mut z, "xl/sharedStrings.xml"))
                };
                assert!(!shared(&during).contains(">SRV77<"), "{what}");
                assert!(!shared(&during).contains("evtxtriage_events#"), "{what}");
                assert!(shared(&after).contains(">SRV77<"));
                assert!(shared(&after).contains(&format!(">evtxtriage_events#{row}<")));
                // SRV77, and WS01, which attaching its row created (S7).
                assert_eq!(rows + 2, rows_after, "{what}: two more hosts");
            }
            _ => {
                let during = std::fs::read_to_string(&during).unwrap();
                let after = std::fs::read_to_string(&after).unwrap();
                assert!(!during.contains("SRV77"), "{what}");
                assert!(!during.contains("evtxtriage\\_events#"), "{what}");
                assert!(after.contains("SRV77"));
                assert!(after.contains(&format!("evtxtriage\\_events#{row}")));
                // SRV77, and WS01, which attaching its row created (S7).
                assert_eq!(rows + 2, rows_after, "{what}: two more hosts");
            }
        }
    }
}

/// Each export reads the cancel flag while it reads an entry's evidence,
/// not only between pages: cancelled at the first evidence row it reads,
/// it reads no other, and writes nothing.
#[test]
fn an_export_is_cancelled_inside_the_evidence_loop() {
    type Hooked = fn(
        &Case,
        &Path,
        &AtomicBool,
        &mut dyn FnMut(),
    ) -> oxl_engine::Result<oxl_engine::ExportReport>;
    let exports: [(&str, Hooked); 3] = [
        ("csv", oxl_engine::export_timeline_csv_on_evidence),
        ("workbook", oxl_engine::export_workbook_on_evidence),
        ("report", oxl_engine::export_report_on_evidence),
    ];
    for (what, export) in exports {
        let (d, mut case) = loaded();
        let id = entry(&mut case, "target", "", Some("2026-01-05T10:00:00Z"));
        let rows: Vec<i64> = (0..3)
            .map(|k| {
                n(
                    &case,
                    &format!(
                        "select _ox_row from evtxtriage_events order by _ox_row limit 1 offset {k}"
                    ),
                )
            })
            .collect();
        oxl_engine::attach_rows(&mut case, id, &rows, &analyst()).unwrap();
        let out = d.path().join(format!("cancelled.{what}"));
        let cancel = AtomicBool::new(false);
        let mut read = 0;
        let err = export(&case, &out, &cancel, &mut || {
            read += 1;
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        })
        .unwrap_err();
        assert!(
            matches!(&err, EngineError::Invalid(m) if m.contains("cancelled")),
            "{what}: {err}"
        );
        assert_eq!(read, 1, "{what}: no evidence row is read after the cancel");
        assert!(!out.exists(), "{what}");
        assert!(!partial(&out).exists(), "{what}");
        // Not cancelled, the hook sees every evidence row once.
        let mut read = 0;
        export(&case, &out, &go(), &mut || read += 1).unwrap();
        assert_eq!(read, 3, "{what}");
    }
}
