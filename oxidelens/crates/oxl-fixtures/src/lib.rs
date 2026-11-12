//! Synthetic TriageSuite output for OxideLens tests.
//!
//! Each function writes a small run output root (CSV files plus the
//! `duckdb/datasets.json` the suite would have written) or a loose file, and
//! returns its path. The data is synthetic throughout: hosts `WS01` and
//! `SRV01`, addresses from the documentation ranges. The rows are chosen so
//! that every later test can state its expected counts exactly; changing one
//! changes those counts.
//!
//! Test scaffolding: every function panics on an I/O failure rather than
//! returning it, because a fixture that cannot be written is a broken test
//! environment, not a case a test should handle.

use oxl_engine::{Actor, Case};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use triage_core::output::duckdb::inventory::{
    Dataset, DatasetFile, EffectiveType, FileRole, Inventory, MetadataColumns, ReaderOptions,
    Status, SCHEMA_VERSION,
};
use triage_core::output::duckdb::types::{SqlType, TimeSemantics};

const EVTX_HEADER: &str =
    "TimeCreated,EventId,Channel,Computer,UserName,RemoteHost,MapDescription,PayloadData1,SourceFile";

const EVTX_WS01: &[&str] = &[
    r"2026-01-05T02:13:58.2040000Z,4625,Security,WS01,Administrator,203.0.113.7,Failed logon,LogonType 10,C:\Windows\System32\winevt\Logs\Security.evtx",
    r"2026-01-05T02:14:07.9110000Z,4624,Security,WS01,svc_backup,203.0.113.7,Successful logon,LogonType 10,C:\Windows\System32\winevt\Logs\Security.evtx",
    r"2026-01-05T02:15:44.3800000Z,1,Sysmon,WS01,svc_backup,,Process creation,svch0st.exe,C:\Windows\System32\winevt\Logs\Sysmon.evtx",
    r"2026-01-05T02:15:46.0180000Z,3,Sysmon,WS01,svc_backup,203.0.113.7,Network connection,svch0st.exe:443,C:\Windows\System32\winevt\Logs\Sysmon.evtx",
    r"2026-01-05T02:17:02.6450000Z,1,Sysmon,WS01,svc_backup,,Process creation,rclone.exe,C:\Windows\System32\winevt\Logs\Sysmon.evtx",
    // This row and the next are byte-identical on purpose: two occurrences
    // of one event, which row hashing must tell apart by occurrence.
    r"2026-01-05T02:33:20.4610000Z,1102,Security,WS01,svc_backup,,Audit log cleared,wevtutil.exe,C:\Windows\System32\winevt\Logs\Security.evtx",
    r"2026-01-05T02:33:20.4610000Z,1102,Security,WS01,svc_backup,,Audit log cleared,wevtutil.exe,C:\Windows\System32\winevt\Logs\Security.evtx",
    // A time that does not cast: NULL typed value, text kept.
    r"not-a-time,4688,Security,WS01,SYSTEM,,Process creation,cmd.exe,C:\Windows\System32\winevt\Logs\Security.evtx",
];

const EVTX_SRV01: &[&str] = &[
    r"2026-01-05T02:22:03.1180000Z,4624,Security,SRV01,svc_backup,WS01,Successful logon,LogonType 3,C:\Windows\System32\winevt\Logs\Security.evtx",
    r"2026-01-05T02:22:04.5510000Z,5140,Security,SRV01,svc_backup,10.0.0.5,Network share accessed,\\*\finance$,C:\Windows\System32\winevt\Logs\Security.evtx",
    // A blank time: NULL typed value and NULL text.
    r",4624,Security,SRV01,svc_backup,WS01,Successful logon,LogonType 3,C:\Windows\System32\winevt\Logs\Security.evtx",
];

const PF_HEADER: &str = "RunTime,ExecutableName,RunCount";

const PF_WS01: &[&str] = &[
    "2026-01-05T02:15:44.4120000Z,SVCH0ST.EXE,1",
    "2026-01-05T02:17:02.7010000Z,RCLONE.EXE,3",
    "2026-01-04T09:00:00.0000000Z,CHROME.EXE,412",
    "2026-01-05T02:33:20.0000000Z,WEVTUTIL.EXE,2",
];

const PF_SRV01: &[&str] = &["2026-01-03T08:00:00.0000000Z,CHROME.EXE,9"];

const HAYABUSA_HEADER: &str = "Timestamp,Computer,Channel,EventID,Level,RuleTitle,Details";

const HAYABUSA_WS01: &[&str] = &[
    "2026-01-05T02:13:58.204000Z,WS01,Sec,4625,med,Logon Failure (Wrong Password),User: Administrator ¦ SrcIP: 203.0.113.7",
    "2026-01-05T02:15:44.380000Z,WS01,Sysmon,1,high,Proc Exec,Proc: svch0st.exe ¦ User: svc_backup",
    "2026-01-05T02:17:02.645000Z,WS01,Sysmon,1,crit,Rclone Execution,Proc: rclone.exe ¦ User: svc_backup",
    "2026-01-05T02:33:20.461000Z,WS01,Sec,1102,crit,Security Log Cleared,User: svc_backup",
    // Not the Z form: a column the suite did not type, with one odd value.
    "2026-01-05 02:40:00,WS01,Sec,4624,info,Logon (Network),User: svc_backup",
];

const FW_CSV: &str = "time,src_ip,dst_ip,action,bytes
2026-01-05 02:14:00,203.0.113.7,10.0.0.5,allow,4100
2026-01-04 23:41:00,203.0.113.7,10.0.0.5,deny,0
2026-01-05 02:20:00,198.51.100.9,10.0.0.5,allow,120
";

/// One CSV file of a dataset.
struct FileSpec<'a> {
    host: &'a str,
    /// Relative to the run output root, `/`-separated.
    path: &'a str,
    header: &'a str,
    rows: &'a [&'a str],
}

/// One dataset of a run, as the suite's inventory describes it.
struct DatasetSpec<'a> {
    tool: &'a str,
    dataset_id: &'a str,
    view: &'a str,
    /// `"internal"` or `"external"`.
    source: &'a str,
    evidence_path_column: Option<&'a str>,
    typed: &'a [(&'a str, SqlType, Option<TimeSemantics>)],
    files: Vec<FileSpec<'a>>,
}

fn evtx_events(files: Vec<FileSpec<'_>>) -> DatasetSpec<'_> {
    DatasetSpec {
        tool: "EvtxTriage",
        dataset_id: "events",
        view: "evtxtriage_events",
        source: "internal",
        evidence_path_column: Some("SourceFile"),
        typed: &[
            ("TimeCreated", SqlType::Timestamp, Some(TimeSemantics::Utc)),
            ("EventId", SqlType::BigInt, None),
        ],
        files,
    }
}

fn petriage_timeline(files: Vec<FileSpec<'_>>) -> DatasetSpec<'_> {
    DatasetSpec {
        tool: "PETriage",
        dataset_id: "timeline",
        view: "petriage_timeline",
        source: "internal",
        evidence_path_column: None,
        typed: &[
            ("RunTime", SqlType::Timestamp, Some(TimeSemantics::Utc)),
            ("RunCount", SqlType::BigInt, None),
        ],
        files,
    }
}

/// The two-host run: `evtxtriage_events` (11 rows: WS01 8, SRV01 3),
/// `petriage_timeline` (5 rows) and `ext_hayabusa_csv_timeline` (5 rows,
/// WS01 only, untyped). Run id `20260101000000-aaaa`.
pub fn two_hosts(dir: &Path) -> PathBuf {
    let ws = "Processed-WS01-20260101000000";
    let srv = "Processed-SRV01-20260101000000";
    let evtx_ws = format!("{ws}/EventLogs/evtx.csv");
    let evtx_srv = format!("{srv}/EventLogs/evtx.csv");
    let pf_ws = format!("{ws}/Execution/pf.csv");
    let pf_srv = format!("{srv}/Execution/pf.csv");
    let hayabusa_ws = format!("{ws}/Hayabusa/timeline.csv");
    write_run(
        &dir.join("run-20260101000000-aaaa"),
        "20260101000000-aaaa",
        vec![
            evtx_events(vec![
                FileSpec {
                    host: "WS01",
                    path: &evtx_ws,
                    header: EVTX_HEADER,
                    rows: EVTX_WS01,
                },
                FileSpec {
                    host: "SRV01",
                    path: &evtx_srv,
                    header: EVTX_HEADER,
                    rows: EVTX_SRV01,
                },
            ]),
            petriage_timeline(vec![
                FileSpec {
                    host: "WS01",
                    path: &pf_ws,
                    header: PF_HEADER,
                    rows: PF_WS01,
                },
                FileSpec {
                    host: "SRV01",
                    path: &pf_srv,
                    header: PF_HEADER,
                    rows: PF_SRV01,
                },
            ]),
            DatasetSpec {
                tool: "Hayabusa",
                dataset_id: "csv_timeline",
                view: "ext_hayabusa_csv_timeline",
                source: "external",
                evidence_path_column: None,
                typed: &[],
                files: vec![FileSpec {
                    host: "WS01",
                    path: &hayabusa_ws,
                    header: HAYABUSA_HEADER,
                    rows: HAYABUSA_WS01,
                }],
            },
        ],
    )
}

/// A later run of WS01 holding only `petriage_timeline`: the four WS01 rows
/// of [`two_hosts`] plus one new one. Run id `20260102000000-bbbb`.
pub fn rerun_ws01_prefetch(dir: &Path) -> PathBuf {
    let mut rows = PF_WS01.to_vec();
    rows.push("2026-01-05T03:00:00.0000000Z,PSEXESVC.EXE,1");
    write_run(
        &dir.join("run-20260102000000-bbbb"),
        "20260102000000-bbbb",
        vec![petriage_timeline(vec![FileSpec {
            host: "WS01",
            path: "Processed-WS01-20260102000000/Execution/pf.csv",
            header: PF_HEADER,
            rows: &rows,
        }])],
    )
}

/// EvtxTriage's per-channel copy of some of WS01's events, as the suite
/// writes it beside the aggregate: dataset `Individual/<channel>`, view
/// `evtxtriage_individual_<channel lower-cased>`.
fn evtx_channel<'a>(
    dataset_id: &'a str,
    view: &'a str,
    path: &'a str,
    rows: &'a [&'a str],
) -> DatasetSpec<'a> {
    DatasetSpec {
        tool: "EvtxTriage",
        dataset_id,
        view,
        source: "internal",
        evidence_path_column: Some("SourceFile"),
        typed: &[
            ("TimeCreated", SqlType::Timestamp, Some(TimeSemantics::Utc)),
            ("EventId", SqlType::BigInt, None),
        ],
        files: vec![FileSpec {
            host: "WS01",
            path,
            header: EVTX_HEADER,
            rows,
        }],
    }
}

/// WS01's event rows of one channel.
fn ws01_channel(channel: &str) -> Vec<&'static str> {
    EVTX_WS01
        .iter()
        .copied()
        .filter(|row| row.split(',').nth(2) == Some(channel))
        .collect()
}

/// A run of WS01 with EvtxTriage's aggregate `evtxtriage_events` (8 rows)
/// and, as the suite writes by default, a per-channel copy of the same
/// events: `evtxtriage_individual_security` (5 rows) and
/// `evtxtriage_individual_sysmon` (3). With `aggregate` false the run has
/// the per-channel datasets alone. Run id `20260106000000-eeee` (with the
/// aggregate) or `20260107000000-ffff` (without).
pub fn with_channels(dir: &Path, aggregate: bool) -> PathBuf {
    let ws = "Processed-WS01-20260106000000/EventLogs";
    let all = format!("{ws}/evtx.csv");
    let security_path = format!("{ws}/Individual/Security.csv");
    let sysmon_path = format!("{ws}/Individual/Sysmon.csv");
    let security = ws01_channel("Security");
    let sysmon = ws01_channel("Sysmon");
    let mut specs = Vec::new();
    if aggregate {
        specs.push(evtx_events(vec![FileSpec {
            host: "WS01",
            path: &all,
            header: EVTX_HEADER,
            rows: EVTX_WS01,
        }]));
    }
    specs.push(evtx_channel(
        "Individual/Security",
        "evtxtriage_individual_security",
        &security_path,
        &security,
    ));
    specs.push(evtx_channel(
        "Individual/Sysmon",
        "evtxtriage_individual_sysmon",
        &sysmon_path,
        &sysmon,
    ));
    let run_id = if aggregate {
        "20260106000000-eeee"
    } else {
        "20260107000000-ffff"
    };
    write_run(&dir.join(format!("run-{run_id}")), run_id, specs)
}

/// A run of WS01 with datasets whose timeline the app chooses:
/// `petriage_main` (2 rows), where the suite types `PreviousRun0` but the
/// run's own time `LastRun` is text; `retriage_userassist_ntuser_dat`
/// (3 rows), whose `LastExecuted` is text, one value without a `Z`; and one
/// row each of `appcompattriage_appcompat`, `retriage_taskcache_software`
/// and `sbetriage_shellbags`, every time as text. Run id
/// `20260110000000-iiii`.
pub fn app_timed(dir: &Path) -> PathBuf {
    let ws = "Processed-WS01-20260110000000";
    let pf = format!("{ws}/Execution/pf_main.csv");
    let ua = format!("{ws}/Registry/userassist.csv");
    let ac = format!("{ws}/Execution/appcompat.csv");
    let tc = format!("{ws}/Registry/taskcache.csv");
    let sb = format!("{ws}/FileFolderAccess/shellbags.csv");
    write_run(
        &dir.join("run-20260110000000-iiii"),
        "20260110000000-iiii",
        vec![
            DatasetSpec {
                tool: "PETriage",
                dataset_id: "main",
                view: "petriage_main",
                source: "internal",
                evidence_path_column: None,
                typed: &[("PreviousRun0", SqlType::Timestamp, Some(TimeSemantics::Utc))],
                files: vec![FileSpec {
                    host: "WS01",
                    path: &pf,
                    header: "ExecutableName,LastRun,PreviousRun0",
                    rows: &[
                        "SVCH0ST.EXE,2026-01-05T02:15:44.4120000Z,2026-01-04T10:00:00.0000000Z",
                        "RCLONE.EXE,2026-01-05T02:17:02.7010000Z,",
                    ],
                }],
            },
            DatasetSpec {
                tool: "RETriage",
                dataset_id: "userassist_ntuser_dat",
                view: "retriage_userassist_ntuser_dat",
                source: "internal",
                evidence_path_column: None,
                typed: &[],
                files: vec![FileSpec {
                    host: "WS01",
                    path: &ua,
                    header: "BatchKeyPath,BatchValueName,ProgramName,RunCounter,LastExecuted",
                    rows: &[
                        "UserAssist\\{CEBFF5CD},rclone.exe,rclone.exe,3,2026-01-05T02:17:02.0000000Z",
                        "UserAssist\\{CEBFF5CD},notepad.exe,notepad.exe,1,2026-01-04 09:00:00",
                        "UserAssist\\{CEBFF5CD},calc.exe,calc.exe,0,",
                    ],
                }],
            },
            untyped(
                "AppCompatTriage",
                "appcompat",
                "appcompattriage_appcompat",
                &ac,
                "ControlSet,CacheEntryPosition,Path,LastModifiedTimeUTC,Executed",
                &["1,0,C:\\Tools\\rclone.exe,2026-01-05T02:16:00.0000000Z,Yes"],
            ),
            untyped(
                "RETriage",
                "taskcache_software",
                "retriage_taskcache_software",
                &tc,
                "Version,KeyName,Path,CreatedOn,LastStart,LastStop",
                &["1,{0F3A6D5E},\\Updater,2026-01-05T02:18:00.0000000Z,,"],
            ),
            untyped(
                "SBETriage",
                "shellbags",
                "sbetriage_shellbags",
                &sb,
                "BagPath,Slot,AbsolutePath,LastWriteTime,FirstInteracted,LastInteracted",
                &["BagMRU\\1,0,C:\\Users\\svc_backup\\stage,2026-01-05T02:19:00.0000000Z,,"],
            ),
        ],
    )
}

/// A run of WS01 with one row each of datasets the suite types nothing of
/// and whose times are text in the Z form: `browsertriage_history` and
/// `browsertriage_timeline` (column names with spaces, as BrowserTriage
/// writes them), `sumetriage_clients`, `retriage_recent_documents_ntuser_dat`
/// (one MRU slot with a time, one without), `retriage_office_mru_ntuser_dat`
/// (opened, never closed), `retriage_first_folder_ntuser_dat`, and
/// `retriage_taskcache_software_regback`, RETriage's copy of the TaskCache
/// read from the hive's RegBack backup. Run id `20260112000000-kkkk`.
pub fn app_timed_more(dir: &Path) -> PathBuf {
    let ws = "Processed-WS01-20260112000000";
    let history = format!("{ws}/Browser/history.csv");
    let timeline = format!("{ws}/Browser/timeline.csv");
    let clients = format!("{ws}/SUM/clients.csv");
    let recent = format!("{ws}/Registry/recentdocs.csv");
    let regback = format!("{ws}/Registry/taskcache_regback.csv");
    let office = format!("{ws}/Registry/office_mru.csv");
    let first_folder = format!("{ws}/Registry/first_folder.csv");
    write_run(
        &dir.join("run-20260112000000-kkkk"),
        "20260112000000-kkkk",
        vec![
            untyped(
                "BrowserTriage",
                "history",
                "browsertriage_history",
                &history,
                "Browser,Profile,Record Type,Visit Time,URL,Title,Last Visit Time",
                &["Chrome,Default,Visit,2026-01-05T02:16:30.0000000Z,https://files.example.com/rclone.zip,rclone download,2026-01-05T02:16:31.0000000Z"],
            ),
            untyped(
                "BrowserTriage",
                "timeline",
                "browsertriage_timeline",
                &timeline,
                "Timestamp,Timestamp Type,Browser,Profile,Artifact,Value",
                &["2026-01-05T02:16:40.0000000Z,Download Start,Chrome,Default,Download,C:\\Users\\svc_backup\\Downloads\\rclone.zip"],
            ),
            untyped(
                "SumETriage",
                "clients",
                "sumetriage_clients",
                &clients,
                "RoleGuid,RoleDescription,AuthenticatedUserName,TotalAccesses,InsertDate,LastAccess,IpAddress,ClientName",
                &["{10a9226f},File Server,LAB\\svc_backup,4,2026-01-05T02:17:30.0000000Z,2026-01-05T02:18:30.0000000Z,203.0.113.7,"],
            ),
            untyped(
                "RETriage",
                "recent_documents_ntuser_dat",
                "retriage_recent_documents_ntuser_dat",
                &recent,
                "Extension,BatchKeyPath,ValueName,BatchValueName,TargetName,LnkName,MruPosition,OpenedOn",
                &[
                    "RecentDocs,RecentDocs,0,0,stage.zip,stage.lnk,0,2026-01-05T02:18:10.0000000Z",
                    "RecentDocs,RecentDocs,1,1,notes.txt,notes.lnk,1,",
                ],
            ),
            untyped(
                "RETriage",
                "office_mru_ntuser_dat",
                "retriage_office_mru_ntuser_dat",
                &office,
                "ValueName,BatchKeyPath,LastOpened,BatchValueName,LastClosed,FileName",
                &["Item 1,Excel\\User MRU,2026-01-05T02:17:40.0000000Z,Item 1,,C:\\Users\\svc_backup\\stage\\list.xlsx"],
            ),
            untyped(
                "RETriage",
                "first_folder_ntuser_dat",
                "retriage_first_folder_ntuser_dat",
                &first_folder,
                "Executable,BatchKeyPath,FolderName,BatchValueName,MRUPosition,OpenedOn",
                &["C:\\Tools\\rclone.exe,ComDlg32\\FirstFolder,C:\\Users\\svc_backup\\stage\\,0,0,2026-01-05T02:17:50.0000000Z"],
            ),
            untyped(
                "RETriage",
                "taskcache_software_regback",
                "retriage_taskcache_software_regback",
                &regback,
                "Version,KeyName,Path,CreatedOn,LastStart,LastStop",
                &["1,{0F3A6D5E},\\Updater,2026-01-05T02:18:20.0000000Z,,"],
            ),
        ],
    )
}

/// A dataset of WS01 the suite types nothing of.
fn untyped<'a>(
    tool: &'a str,
    dataset_id: &'a str,
    view: &'a str,
    path: &'a str,
    header: &'a str,
    rows: &'a [&'a str],
) -> DatasetSpec<'a> {
    DatasetSpec {
        tool,
        dataset_id,
        view,
        source: "internal",
        evidence_path_column: None,
        typed: &[],
        files: vec![FileSpec {
            host: "WS01",
            path,
            header,
            rows,
        }],
    }
}

/// A loose firewall export, `fw.csv`, with no inventory: three rows whose
/// times are not in the Z form. Returns the file's path.
pub fn loose_fw_csv(dir: &Path) -> PathBuf {
    let path = dir.join("fw.csv");
    std::fs::write(&path, FW_CSV).expect("write fw.csv");
    path
}

/// A run whose one dataset, `rbtriage_main`, is a header and no rows. Run
/// id `20260103000000-cccc`.
pub fn empty_dataset(dir: &Path) -> PathBuf {
    write_run(
        &dir.join("run-20260103000000-cccc"),
        "20260103000000-cccc",
        vec![DatasetSpec {
            tool: "RBTriage",
            dataset_id: "main",
            view: "rbtriage_main",
            source: "internal",
            evidence_path_column: None,
            typed: &[("DeletedOn", SqlType::Timestamp, Some(TimeSemantics::Utc))],
            files: vec![FileSpec {
                host: "WS01",
                path: "Processed-WS01-20260103000000/FileSystem/recyclebin.csv",
                header: "DeletedOn,FileName",
                rows: &[],
            }],
        }],
    )
}

/// A new, empty case at `<dir>/t.case.duckdb`, named `T`, created by
/// `analyst:tester`.
pub fn new_case(dir: &Path) -> Case {
    Case::create(
        &dir.join("t.case.duckdb"),
        "T",
        &Actor::Analyst("tester".into()),
    )
    .expect("create the fixture case")
}

/// Write every dataset's CSVs under `root` and the inventory describing
/// them at `root/duckdb/datasets.json`, with file paths relative to `root`.
fn write_run(root: &Path, run_id: &str, specs: Vec<DatasetSpec<'_>>) -> PathBuf {
    let datasets = specs
        .into_iter()
        .map(|spec| write_dataset(root, spec))
        .collect();
    let inventory = Inventory {
        schema_version: SCHEMA_VERSION,
        generation: format!("{run_id}-g1"),
        run_id: run_id.to_string(),
        generated_utc: format!(
            "{}-{}-{}T00:00:00.0000000Z",
            &run_id[..4],
            &run_id[4..6],
            &run_id[6..8]
        ),
        out_root: root.to_path_buf(),
        status: Status::Ok,
        reader_options: ReaderOptions::default(),
        datasets,
        inventory_only: Vec::new(),
        dropped_overrides: Vec::new(),
        warnings: Vec::new(),
    };
    let duckdb_dir = root.join("duckdb");
    std::fs::create_dir_all(&duckdb_dir).expect("create duckdb/");
    let json = serde_json::to_string_pretty(&inventory).expect("serialize the inventory");
    std::fs::write(duckdb_dir.join("datasets.json"), json).expect("write datasets.json");
    root.to_path_buf()
}

fn write_dataset(root: &Path, spec: DatasetSpec<'_>) -> Dataset {
    let mut columns: Vec<String> = Vec::new();
    let mut files = Vec::new();
    for file in &spec.files {
        let header: Vec<String> = file.header.split(',').map(str::to_string).collect();
        for name in &header {
            if !columns.contains(name) {
                columns.push(name.clone());
            }
        }
        let mut text = String::new();
        for line in std::iter::once(&file.header).chain(file.rows) {
            text.push_str(line);
            text.push('\n');
        }
        let relative = PathBuf::from(file.path);
        let absolute = root.join(&relative);
        std::fs::create_dir_all(absolute.parent().expect("a file path has a parent"))
            .expect("create a dataset directory");
        std::fs::write(&absolute, text).expect("write a dataset CSV");
        files.push(DatasetFile {
            path: relative,
            host: file.host.to_string(),
            identity: None,
            role: FileRole::Primary,
            included_in_view: true,
            derived_into: None,
            header: Some(header),
            warnings: Vec::new(),
        });
    }
    let effective_types: BTreeMap<String, EffectiveType> = spec
        .typed
        .iter()
        .map(|&(column, sql_type, semantics)| {
            (
                column.to_string(),
                EffectiveType::new(sql_type, format!("{column}__text"), semantics),
            )
        })
        .collect();
    Dataset {
        view: spec.view.to_string(),
        raw_view: format!("raw_{}", spec.view),
        tool: spec.tool.to_string(),
        dataset_id: spec.dataset_id.to_string(),
        source: spec.source.to_string(),
        metadata_columns: MetadataColumns {
            run_id: "_triage_run_id".into(),
            host: "_triage_host".into(),
            identity: "_triage_identity".into(),
            output_file: "_triage_output_file".into(),
        },
        evidence_path_column: spec.evidence_path_column.map(str::to_string),
        columns,
        effective_types,
        files,
    }
}
