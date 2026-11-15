//! The case-record commands, called as the window calls them, over a case
//! with the two-host run (with WS01's Security logons) loaded: marks, tags
//! and notes on rows, timeline entries and their evidence, host, account and
//! IOC records and their links, the reads over them, and the three record
//! exports. The engine's tests cover each rule in depth; these check that
//! each command reaches its rule whole, as a write (`case-changed`) or as a
//! read on the window's slot.

mod common;

use common::{state, Recorder};
use oxl_app::cmd::record::{
    self as r, CaseRecord, EntryChanges, RecordQuery, RecordSpec, TextChange,
};
use oxl_app::cmd::{self, case, grid as g, load, At};
use oxl_app::contract::{self, COMMANDS};
use oxl_app::events::{CASE_CHANGED, JOB_FINISHED, JOB_STARTED};
use oxl_app::state::AppState;
use oxl_engine::{
    EntryQuery, EntryStatus, EvidenceStanding, GridState, HostSpec, IocQuery, IocSpec, IocType,
    PageOptions, RecordKind, RecordStatus,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

const EVENTS: &str = "evtxtriage_events";

/// A case with `two_hosts_logons` loaded through the app, its events
/// cleared. Keep `dir` alive as long as the state.
struct Setup {
    dir: tempfile::TempDir,
    run: PathBuf,
    events: Arc<Recorder>,
    state: Arc<AppState>,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let run = oxl_fixtures::two_hosts_logons(dir.path());
    let (events, state) = state();
    case::case_new(&state, &dir.path().join("case.duckdb"), "WS01 triage").unwrap();
    load::load_run(&state, &run).unwrap();
    events.clear();
    Setup {
        dir,
        run,
        events,
        state,
    }
}

fn at(slot: &str) -> At<'_> {
    At::new("q1", slot)
}

fn grid_json(value: Value) -> GridState {
    serde_json::from_value(value).unwrap()
}

/// The ids of the events rows whose text holds `text`, in the grid's order.
fn rows_with(s: &Setup, text: &str) -> Vec<i64> {
    let state = grid_json(json!({
        "relation": EVENTS,
        "search": { "text": text, "regex": false, "case_sensitive": false, "mode": "filter" },
    }));
    let options: PageOptions = serde_json::from_value(json!({})).unwrap();
    g::grid_page(&s.state, at("t1:page"), &state, 100, 0, &options)
        .unwrap()
        .row_ids
}

/// The one events row whose text holds `text`.
fn row_with(s: &Setup, text: &str) -> i64 {
    let rows = rows_with(s, text);
    assert_eq!(rows.len(), 1, "{text}: {rows:?}");
    rows[0]
}

fn title(t: &str) -> EntryChanges {
    EntryChanges {
        title: Some(t.to_string()),
        ..EntryChanges::default()
    }
}

/// The kinds of each `case-changed` since the last look, and clears them.
fn changes(s: &Setup) -> Vec<Vec<String>> {
    let kinds = s
        .events
        .named(CASE_CHANGED)
        .iter()
        .map(|p| serde_json::from_value(p["kinds"].clone()).unwrap())
        .collect();
    s.events.clear();
    kinds
}

fn host_spec(id: Option<i64>, name: &str, note: &str) -> RecordSpec {
    RecordSpec::Host(HostSpec {
        id,
        name: name.to_string(),
        role: String::new(),
        note: note.to_string(),
    })
}

fn ioc_spec(ioc_type: IocType, value: &str) -> RecordSpec {
    RecordSpec::Ioc(IocSpec {
        id: None,
        ioc_type,
        value: value.to_string(),
        tool: None,
        attack_ids: Vec::new(),
        note: String::new(),
    })
}

/// A record export command.
type Export =
    fn(&AppState, At<'_>, &std::path::Path) -> Result<oxl_engine::ExportReport, oxl_app::CmdError>;

fn record_id(record: &CaseRecord) -> i64 {
    match record {
        CaseRecord::Host(h) => h.id,
        CaseRecord::Account(a) => a.id,
        CaseRecord::Ioc(i) => i.id,
    }
}

/// The commands that existed before the case record, which take no
/// `queryId` and are not the record's writes.
const OTHER_NON_READS: &[&str] = &[
    "case_new",
    "case_open",
    "case_close",
    "load_run",
    "load_file",
    "zone_names",
    "display_zone_set",
    "view_save",
    "view_delete",
    "view_import",
    "settings_get",
    "settings_update",
    "recent_clear",
    "recent_remove",
    "query_cancel",
];

/// Every write of the case record, in the order this test calls them,
/// answers once with `case-changed` naming what it touched; a refused write
/// sends none. The list is held to the contract: a new write command that
/// this test does not call fails it.
#[test]
fn every_write_emits_case_changed() {
    let s = setup();
    let st = &s.state;
    let rclone = row_with(&s, "rclone.exe");
    let svch0st = row_with(&s, "svch0st.exe:443");
    let mut called: Vec<(&str, Vec<&str>)> = Vec::new();
    let mut expect = |name: &'static str, kinds: &[&'static str], s: &Setup| {
        assert_eq!(
            changes(s),
            vec![kinds.iter().map(|k| k.to_string()).collect::<Vec<_>>()],
            "{name}"
        );
        called.push((name, kinds.to_vec()));
    };

    r::mark_set(st, &[rclone]).unwrap();
    expect("mark_set", &["marks"], &s);
    r::mark_clear(st, &[rclone]).unwrap();
    expect("mark_clear", &["marks"], &s);
    r::tag_add(st, &[rclone], "triage").unwrap();
    expect("tag_add", &["tags"], &s);
    r::tag_remove(st, &[rclone], "triage").unwrap();
    expect("tag_remove", &["tags"], &s);
    let note = r::note_add(st, rclone, "looked at").unwrap();
    expect("note_add", &["notes"], &s);
    r::note_delete(st, note.id).unwrap();
    expect("note_delete", &["notes"], &s);

    let promoted = r::rows_promote(st, &[svch0st]).unwrap();
    expect("rows_promote", &["entries", "marks", "records"], &s);
    let promoted = promoted[0].entry_id.unwrap();
    let typed = r::entry_new(st, &title("Lateral movement")).unwrap();
    expect("entry_new", &["entries"], &s);
    r::entry_edit(st, typed.id, &title("Lateral movement to SRV01")).unwrap();
    expect("entry_edit", &["entries"], &s);
    r::rows_attach(st, typed.id, &[rclone]).unwrap();
    expect("rows_attach", &["entries", "marks", "records"], &s);
    r::evidence_detach(st, typed.id, rclone).unwrap();
    expect("evidence_detach", &["entries", "marks"], &s);
    r::entry_status(st, promoted, EntryStatus::Reviewed).unwrap();
    expect("entry_status", &["entries", "marks"], &s);

    let host = r::record_upsert(st, &host_spec(None, "SRV99", "")).unwrap();
    expect("record_upsert", &["records", "entries"], &s);
    let other = r::record_upsert(st, &host_spec(None, "SRV98", "")).unwrap();
    changes(&s);
    r::record_link(st, typed.id, RecordKind::Host, record_id(&host)).unwrap();
    expect("record_link", &["entries", "records"], &s);
    r::record_unlink(st, typed.id, RecordKind::Host, record_id(&host)).unwrap();
    expect("record_unlink", &["entries", "records"], &s);
    r::record_status(
        st,
        RecordKind::Host,
        record_id(&host),
        RecordStatus::Unreviewed,
    )
    .unwrap();
    expect("record_status", &["records"], &s);
    r::record_merge(st, RecordKind::Host, record_id(&other), record_id(&host)).unwrap();
    expect("record_merge", &["records", "entries"], &s);
    r::record_delete(st, RecordKind::Host, record_id(&host)).unwrap();
    expect("record_delete", &["records", "entries"], &s);
    r::entry_delete(st, typed.id).unwrap();
    expect("entry_delete", &["entries", "marks", "records"], &s);

    // A write the engine refuses changes nothing and says so to no one.
    assert_eq!(
        r::entry_edit(st, 999, &title("nothing")).unwrap_err().code,
        "not_found"
    );
    assert_eq!(r::entry_delete(st, typed.id).unwrap_err().code, "refused");
    assert!(s.events.named(CASE_CHANGED).is_empty());

    // Every command that is not a read, outside the ones that came before
    // the record, is one this test called.
    let cfg = contract::config();
    let writes: BTreeSet<&str> = contract::signatures(&cfg)
        .iter()
        .filter(|sig| sig.args.first().map(|a| a.name) != Some("queryId"))
        .map(|sig| sig.name)
        .filter(|name| !OTHER_NON_READS.contains(name))
        .collect();
    let tested: BTreeSet<&str> = called.iter().map(|(name, _)| *name).collect();
    assert_eq!(writes, tested);
}

/// The record's commands are the contract's and the handler's, in one
/// order: the names the window builds against.
#[test]
fn the_record_commands_are_in_the_contract() {
    let record = [
        "mark_set",
        "mark_clear",
        "tag_add",
        "tag_remove",
        "tags_list",
        "note_add",
        "note_delete",
        "notes_list",
        "entries_list",
        "entry_get",
        "entry_new",
        "entry_edit",
        "entry_status",
        "entry_delete",
        "rows_promote",
        "rows_attach",
        "evidence_detach",
        "evidence_list",
        "records_list",
        "record_get",
        "record_upsert",
        "record_delete",
        "record_merge",
        "record_status",
        "record_link",
        "record_unlink",
        "record_entries",
        "candidates",
        "ioc_sightings",
        "logon_summary",
        "history_list",
        "export_timeline",
        "export_workbook",
        "export_report",
    ];
    let start = COMMANDS
        .iter()
        .position(|c| *c == record[0])
        .expect("mark_set is a command");
    assert_eq!(&COMMANDS[start..start + record.len()], record);
    assert_eq!(oxl_app::HANDLERS, COMMANDS);
}

/// M on a row makes an entry timed by the row, its evidence the row, linked
/// to the row's host; each read finds it where the window looks.
#[test]
fn a_promoted_row_is_an_entry_with_its_evidence_and_host() {
    let s = setup();
    let row = row_with(&s, "svch0st.exe:443");
    let outcome = r::rows_promote(&s.state, &[row]).unwrap();
    assert!(outcome[0].created);
    let id = outcome[0].entry_id.unwrap();

    let entry = r::entry_get(&s.state, at("entry"), id).unwrap();
    assert_eq!(entry.time_source.as_deref(), Some("row"));
    assert_eq!(entry.time.as_deref(), Some("2026-01-05T02:15:46.018000Z"));
    assert_eq!(entry.status, EntryStatus::Draft);
    assert_eq!(entry.hosts, ["WS01"]);
    assert_eq!(entry.evidence_rows, 1);

    let evidence = r::evidence_list(&s.state, at("evidence"), id).unwrap();
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].row_id, row);
    assert_eq!(evidence[0].relation, EVENTS);
    assert_eq!(evidence[0].standing, EvidenceStanding::Unchanged);
    assert_eq!(
        r::evidence_list(&s.state, at("evidence"), 999)
            .unwrap_err()
            .code,
        "not_found"
    );

    let listed = r::entries_list(&s.state, at("timeline"), &EntryQuery::default(), 10, 0).unwrap();
    assert_eq!(listed.items.iter().map(|e| e.id).collect::<Vec<_>>(), [id]);
    let hosts = r::records_list(&s.state, at("hosts"), &RecordQuery::Host, 10, 0).unwrap();
    let CaseRecord::Host(ws01) = &hosts.items[0] else {
        panic!("a host list holds hosts: {:?}", hosts.items);
    };
    assert_eq!(ws01.name, "WS01");
    let linked =
        r::record_entries(&s.state, at("linked"), RecordKind::Host, ws01.id, 10, 0).unwrap();
    assert_eq!(linked.items.iter().map(|e| e.id).collect::<Vec<_>>(), [id]);
    let got = r::record_get(&s.state, at("record"), RecordKind::Host, ws01.id).unwrap();
    assert_eq!(got, CaseRecord::Host(ws01.clone()));

    // The row is now Malicious, so the summary's counts say so.
    let summary = case::case_summary(&s.state, at("summary")).unwrap();
    assert_eq!(summary.mark_counts.malicious, 1);
}

/// An edit changes only what it names: a title edit keeps a time the row
/// gave (`time_source` stays `row`), `set` types a time, `clear` clears it,
/// and an entry with no time cannot be reviewed.
#[test]
fn an_entry_edit_changes_only_what_it_names() {
    let s = setup();
    let row = row_with(&s, "svch0st.exe:443");
    let id = r::rows_promote(&s.state, &[row]).unwrap()[0]
        .entry_id
        .unwrap();
    let kept = r::entry_edit(&s.state, id, &title("C2 beacon")).unwrap();
    assert_eq!(kept.title, "C2 beacon");
    assert_eq!(kept.time_source.as_deref(), Some("row"));

    let undated = r::entry_new(&s.state, &title("Wiped logs")).unwrap();
    assert!(undated.needs_time);
    assert_eq!(
        r::entry_status(&s.state, undated.id, EntryStatus::Reviewed)
            .unwrap_err()
            .code,
        "refused"
    );
    let typed = EntryChanges {
        time: Some(TextChange::Set("2026-01-05T03:00:00Z".to_string())),
        tactic: Some(TextChange::Set("Defense Evasion".to_string())),
        ..EntryChanges::default()
    };
    let dated = r::entry_edit(&s.state, undated.id, &typed).unwrap();
    assert_eq!(dated.time_source.as_deref(), Some("typed"));
    assert_eq!(dated.tactic.as_deref(), Some("Defense Evasion"));
    assert_eq!(dated.title, "Wiped logs");
    let cleared = EntryChanges {
        time: Some(TextChange::Clear),
        ..EntryChanges::default()
    };
    let again = r::entry_edit(&s.state, undated.id, &cleared).unwrap();
    assert!(again.needs_time);
    assert_eq!(again.tactic.as_deref(), Some("Defense Evasion"));
}

/// The window's JSON for an entry edit, a record and a record query: what
/// `invoke` sends is what the commands take, and a record answers tagged by
/// its kind.
#[test]
fn the_window_json_is_what_the_commands_take() {
    let edit: EntryChanges = serde_json::from_value(json!({
        "title": null, "description": null, "time": { "set": "2026-01-05T03:00:00Z" },
        "tactic": "clear", "technique_ids": ["T1059"],
    }))
    .unwrap();
    assert_eq!(
        edit.time,
        Some(TextChange::Set("2026-01-05T03:00:00Z".into()))
    );
    assert_eq!(edit.tactic, Some(TextChange::Clear));
    assert!(serde_json::from_value::<EntryChanges>(json!({
        "title": null, "description": null, "time": null, "tactic": null,
        "technique_ids": null, "extra": 1,
    }))
    .is_err());

    let spec: RecordSpec = serde_json::from_value(json!({
        "kind": "ioc", "id": null, "ioc_type": "ip", "value": "203.0.113.7", "tool": null,
        "attack_ids": [], "note": "",
    }))
    .unwrap();
    assert!(matches!(&spec, RecordSpec::Ioc(i) if i.ioc_type == IocType::Ip));
    assert!(serde_json::from_value::<RecordSpec>(json!({
        "kind": "host", "id": null, "name": "WS01", "role": "", "note": "", "colour": "red",
    }))
    .is_err());
    let query: RecordQuery = serde_json::from_value(json!({
        "kind": "ioc", "ioc_type": null, "tool": null, "text": "203.0.113",
    }))
    .unwrap();
    assert!(matches!(
        query,
        RecordQuery::Ioc(IocQuery { text: Some(_), .. })
    ));
    let host: RecordQuery = serde_json::from_value(json!({ "kind": "host" })).unwrap();
    assert!(matches!(host, RecordQuery::Host));

    let s = setup();
    let hostile = oxl_fixtures::hostile_record_values();
    let made = r::record_upsert(&s.state, &host_spec(None, "WS01", hostile[0])).unwrap();
    let json = serde_json::to_value(&made).unwrap();
    assert_eq!(json["kind"], "host");
    assert_eq!(json["name"], "WS01");
    assert_eq!(json["note"], hostile[0]);
}

/// Records of every kind are made, edited, listed and filtered through the
/// one upsert and list; the hostile values come back byte for byte.
#[test]
fn records_of_every_kind_round_trip() {
    let s = setup();
    let hostile = oxl_fixtures::hostile_record_values();
    let mut kept = 0;
    for (n, value) in hostile.iter().enumerate() {
        let made = r::record_upsert(&s.state, &ioc_spec(IocType::Other, value));
        // The 5,000-character value is past what an IOC holds.
        if value.chars().count() > oxl_engine::MAX_IOC_VALUE {
            assert_eq!(made.unwrap_err().code, "invalid", "hostile value {n}");
            continue;
        }
        let CaseRecord::Ioc(ioc) = made.unwrap() else {
            panic!("an IOC spec makes an IOC");
        };
        assert_eq!(ioc.value, *value, "hostile value {n}");
        kept += 1;
    }
    let account: RecordSpec = serde_json::from_value(json!({
        "kind": "account", "id": null, "name": "jdoe", "domain": "ACME", "sid": null,
        "description": hostile[1], "privilege": "domain_admin", "note": "",
    }))
    .unwrap();
    let CaseRecord::Account(a) = r::record_upsert(&s.state, &account).unwrap() else {
        panic!("an account spec makes an account");
    };
    assert_eq!(a.description, hostile[1]);
    let host = r::record_upsert(&s.state, &host_spec(None, "SRV01", "")).unwrap();
    let edited = r::record_upsert(
        &s.state,
        &host_spec(Some(record_id(&host)), "SRV01", hostile[2]),
    )
    .unwrap();
    let CaseRecord::Host(h) = edited else {
        panic!("a host spec makes a host");
    };
    assert_eq!((h.id, h.note.as_str()), (record_id(&host), hostile[2]));

    let iocs = r::records_list(
        &s.state,
        at("iocs"),
        &RecordQuery::Ioc(IocQuery::default()),
        100,
        0,
    )
    .unwrap();
    assert_eq!(iocs.items.len(), kept);
    assert_eq!(kept, hostile.len() - 1);
    let found = r::records_list(
        &s.state,
        at("iocs"),
        &RecordQuery::Ioc(IocQuery {
            text: Some("onerror".into()),
            ..IocQuery::default()
        }),
        100,
        0,
    )
    .unwrap();
    assert_eq!(found.items.len(), 1);
    let accounts = r::records_list(&s.state, at("accounts"), &RecordQuery::Account, 10, 0).unwrap();
    assert!(matches!(&accounts.items[..], [CaseRecord::Account(x)] if x.name == "jdoe"));
    assert_eq!(
        r::record_upsert(&s.state, &ioc_spec(IocType::Ip, "not an address"))
            .unwrap_err()
            .code,
        "invalid"
    );
}

/// Marks, tags and notes on rows, and the reads of the tags and a row's
/// notes; the history of an entry lists its changes, newest first.
#[test]
fn row_annotations_and_history_read_back() {
    let s = setup();
    let row = row_with(&s, "rclone.exe");
    let done = r::mark_set(&s.state, &[row, 999_999]).unwrap();
    assert_eq!(
        done.iter().map(|o| o.done).collect::<Vec<_>>(),
        [true, false]
    );
    r::tag_add(&s.state, &[row], "triage").unwrap();
    let tags = r::tags_list(&s.state, at("tags"), 10, 0).unwrap();
    assert_eq!(tags.items[0].name, "triage");
    assert_eq!(tags.items[0].rows, 1);
    r::note_add(&s.state, row, "<img src=x onerror=1>").unwrap();
    let notes = r::notes_list(&s.state, at("notes"), row, 10, 0).unwrap();
    assert_eq!(notes.items[0].text, "<img src=x onerror=1>");

    let entry = r::entry_new(&s.state, &title("First look")).unwrap();
    r::entry_edit(&s.state, entry.id, &title("Second look")).unwrap();
    let id = entry.id.to_string();
    let history =
        r::history_list(&s.state, at("history"), Some("entry"), Some(&id), 10, 0).unwrap();
    let actions: Vec<&str> = history.items.iter().map(|h| h.action.as_str()).collect();
    assert_eq!(actions, ["entry_edit", "entry_create"]);
    let all = r::history_list(&s.state, at("history"), None, None, 100, 0).unwrap();
    assert!(all.items.len() > 2);
    assert_eq!(
        r::history_list(&s.state, at("history"), None, Some(&id), 10, 0)
            .unwrap_err()
            .code,
        "invalid"
    );
}

/// A row's candidates, an IOC's sightings and an account's logons, read as
/// the window reads them.
#[test]
fn candidates_sightings_and_logons() {
    let s = setup();
    let row = row_with(&s, "svch0st.exe:443");
    let found = r::candidates(&s.state, at("candidates"), row).unwrap();
    let ip = found
        .iter()
        .find(|c| c.value == "203.0.113.7")
        .expect("the address is a candidate");
    assert_eq!(ip.kind, RecordKind::Ioc);
    assert_eq!(ip.existing_id, None);

    let ioc = r::record_upsert(&s.state, &ioc_spec(IocType::Ip, "203.0.113.7")).unwrap();
    let sightings = r::ioc_sightings(&s.state, at("sightings"), record_id(&ioc)).unwrap();
    assert!(sightings.hits >= 2, "{sightings:?}");
    assert!(sightings.hosts.contains(&"WS01".to_string()));
    assert_eq!(
        r::ioc_sightings(&s.state, at("sightings"), 999)
            .unwrap_err()
            .code,
        "not_found"
    );

    let account: RecordSpec = serde_json::from_value(json!({
        "kind": "account", "id": null, "name": "jdoe", "domain": "acme", "sid": null,
        "description": "", "privilege": "unknown", "note": "",
    }))
    .unwrap();
    let account = r::record_upsert(&s.state, &account).unwrap();
    let logons = r::logon_summary(&s.state, at("logons"), record_id(&account)).unwrap();
    let mut got: Vec<(String, u32, u64)> = logons
        .rows
        .iter()
        .map(|l| {
            (
                serde_json::to_value(l.outcome)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string(),
                l.logon_type,
                l.count,
            )
        })
        .collect();
    got.sort();
    assert_eq!(got, [("failure".into(), 3, 1), ("success".into(), 10, 1)]);
    assert_eq!(logons.unparsed, 5);
    assert_eq!(logons.source.as_deref(), Some(EVENTS));
}

/// Sightings run as a read on the window's slot: an id cancelled before it
/// runs answers `interrupted`, and a newer query on the slot supersedes it.
#[test]
fn ioc_sightings_is_a_cancellable_read() {
    let s = setup();
    let ioc = r::record_upsert(&s.state, &ioc_spec(IocType::Ip, "203.0.113.7")).unwrap();
    cmd::query_cancel(&s.state, &["sweep-1".to_string()]).unwrap();
    let err =
        r::ioc_sightings(&s.state, At::new("sweep-1", "sightings"), record_id(&ioc)).unwrap_err();
    assert_eq!(err.code, "interrupted");
    // Not cancelled, the same read answers.
    let ok = r::ioc_sightings(&s.state, At::new("sweep-2", "sightings"), record_id(&ioc)).unwrap();
    assert!(ok.hits >= 2);
}

/// The three record exports, each a job that writes its file and changes
/// nothing in the case; the hostile title is in each file as the export's
/// rules write it.
#[test]
fn record_exports_are_jobs_that_write_their_files() {
    let s = setup();
    let row = row_with(&s, "svch0st.exe:443");
    r::rows_promote(&s.state, &[row]).unwrap();
    r::entry_new(&s.state, &title("=HYPERLINK(\"http://x\")")).unwrap();
    s.events.clear();
    let exports: [(&str, Export); 3] = [
        ("timeline.csv", r::export_timeline),
        ("record.xlsx", r::export_workbook),
        ("record.md", r::export_report),
    ];
    for (name, export) in exports {
        let out = s.dir.path().join(name);
        let report: oxl_engine::ExportReport = export(&s.state, at("export"), &out).unwrap();
        assert!(report.rows >= 2, "{name}: {report:?}");
        assert_eq!(report.path, out);
        assert!(out.is_file(), "{name}");
        assert_eq!(s.events.names(), [JOB_STARTED, JOB_FINISHED], "{name}");
        assert_eq!(
            s.events.named(JOB_STARTED)[0],
            json!({ "kind": "export", "label": name })
        );
        assert_eq!(s.events.named(JOB_FINISHED)[0]["ok"], json!(true));
        s.events.clear();
    }
    let csv = std::fs::read_to_string(s.dir.path().join("timeline.csv")).unwrap();
    assert!(csv.contains("\"=HYPERLINK(\"\"http://x\"\")\""), "{csv}");
    let md = std::fs::read_to_string(s.dir.path().join("record.md")).unwrap();
    assert!(md.contains("=HYPERLINK(\"http[:]//x\")"), "{md}");
}

/// A record export into the run is refused, and one cancelled before it
/// starts answers `interrupted`; neither leaves a file or its partial.
#[test]
fn record_exports_refuse_the_run_and_stop_when_cancelled() {
    let s = setup();
    r::entry_new(&s.state, &title("Wiped logs")).unwrap();
    let partial = |p: &std::path::Path| {
        let mut name = p.as_os_str().to_os_string();
        name.push(".partial");
        PathBuf::from(name)
    };
    let inside = s.run.join("timeline.csv");
    let err = r::export_timeline(&s.state, at("export"), &inside).unwrap_err();
    assert_eq!(err.code, "refused");
    assert!(!inside.exists());

    let out = s.dir.path().join("record.xlsx");
    cmd::query_cancel(&s.state, &["export-1".to_string()]).unwrap();
    s.events.clear();
    let err = r::export_workbook(&s.state, At::new("export-1", "export"), &out).unwrap_err();
    assert_eq!(err.code, "interrupted");
    assert!(!out.exists() && !partial(&out).exists());
    assert_eq!(s.events.named(JOB_FINISHED)[0]["ok"], json!(false));
}

/// With no case open, every record command answers `no_case`.
#[test]
fn the_record_commands_need_a_case() {
    let (_events, st) = state();
    assert_eq!(r::mark_set(&st, &[1]).unwrap_err().code, "no_case");
    assert_eq!(
        r::entries_list(&st, at("timeline"), &EntryQuery::default(), 10, 0)
            .unwrap_err()
            .code,
        "no_case"
    );
    let out = std::env::temp_dir().join("oxl-no-case.md");
    assert_eq!(
        r::export_report(&st, at("export"), &out).unwrap_err().code,
        "no_case"
    );
}
