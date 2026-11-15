//! The contract between the app and its window: the commands, what each
//! takes and answers, and the events, as TypeScript generated from the Rust
//! types with `ts-rs`.
//!
//! `generate` writes one `<Type>.ts` per type the contract reaches (ts-rs's
//! own output, `serde_json/JsonValue.ts` among them) and `commands.ts`: the
//! command names (`COMMANDS`, `Command`), each command's arguments and
//! result (`CommandArgs`, `CommandResult`), and the events (`EVENTS`,
//! `EventPayload`). The files are checked in under
//! `app/src/contract/generated/`; `tests/contract.rs` fails when they differ
//! from what the Rust types generate, and checks through Tauri's own IPC
//! that every command takes exactly the arguments `signatures` gives it.
//!
//! **The JSON and the types agree.** Every type derives `TS` beside its
//! serde derives and ts-rs reads the serde attributes, so a rename, a
//! default or an untagged variant is the same on both sides. An `Option`
//! is `T | null`: the app always writes the key. A 64-bit integer (row ids,
//! counts, offsets) is `number`, not ts-rs's default `bigint`, because what
//! the window receives is JSON that `JSON.parse` reads into a `number`
//! (`config`); every such value the app sends is far below 2^53. The
//! suite's `SqlType` and `TimeSemantics` are typed by stand-ins whose
//! spellings a test holds to their `Serialize` (`oxl_engine::ts`).
//!
//! **Arguments** are named as Tauri takes them from `invoke`: the Rust
//! parameter in camelCase (`query_id` is `queryId`). Every read takes
//! `queryId` and `slot` first. An argument typed `Option` may be left out.

use crate::cmd::case::CaseSummary;
use crate::cmd::record::{CaseRecord, EntryChanges, RecordQuery, RecordSpec};
use crate::error::CmdError;
use crate::events::{
    CaseChanged, JobFinished, JobStarted, CASE_CHANGED, JOB_FINISHED, JOB_STARTED, SETTINGS_DAMAGED,
};
use crate::settings::{DamagedSettings, Settings, SettingsReply};
use oxl_engine::{
    AppliedView, Candidate, CellValue, Chip, ColourRule, ColumnMeta, Entry, EntryQuery,
    EntryStatus, Evidence, ExportReport, FileOptions, GridCounts, GridPage, GridState, GroupView,
    Histogram, HistoryRow, HostZone, IocType, Listing, LoadReport, LogonSummary, MatchPos,
    MatchingView, MergedSource, MergedView, Note, PageOptions, PromoteOutcome, RecordKind,
    RecordStatus, RowOutcome, RowView, SavedView, Session, Sightings, TagInfo, ValueCount,
    ValueOrder, ViewDoc, ViewKind, ViewListing, WindowTotal,
};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use ts_rs::{Config, ExportError, TypeVisitor, TS};

/// Every command the window may call, in the contract's order. The app's
/// handler answers exactly these (`crate::HANDLERS`).
pub const COMMANDS: &[&str] = &[
    "case_new",
    "case_open",
    "case_close",
    "case_summary",
    "load_run",
    "load_file",
    "columns",
    "sessions",
    "host_zones",
    "zone_names",
    "ioc_guess",
    "tactics",
    "display_zone_set",
    "grid_page",
    "grid_counts",
    "grid_groups",
    "grid_histogram",
    "grid_values",
    "grid_empty_columns",
    "grid_matches",
    "grid_match_count",
    "grid_row_offset",
    "grid_row_ids",
    "grid_row_positions",
    "grid_export",
    "date_range_chip",
    "default_colour_rules",
    "row_view",
    "cell_value",
    "around_totals",
    "merged_page",
    "views_list",
    "views_matching",
    "view_save",
    "view_delete",
    "view_apply",
    "view_export",
    "view_import",
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
    "settings_get",
    "settings_update",
    "recent_clear",
    "recent_remove",
    "query_cancel",
];

/// Every event the app sends the window.
pub const EVENTS: &[&str] = &[CASE_CHANGED, JOB_STARTED, JOB_FINISHED, SETTINGS_DAMAGED];

/// How the contract is generated: 64-bit integers as `number`.
pub fn config() -> Config {
    Config::new().with_large_int("number")
}

/// One argument of a command, as `invoke` takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    /// The name `invoke` takes it under (camelCase).
    pub name: &'static str,
    /// The Rust parameter is an `Option`: the argument may be left out.
    pub optional: bool,
    /// Its TypeScript type.
    pub ts: String,
}

/// One command: its name, arguments in order, and the TypeScript type of
/// what it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub name: &'static str,
    pub args: Vec<Arg>,
    pub result: String,
}

/// A generated type the contract names: what `commands.ts` imports it as
/// and from, and how it is written with everything it needs.
struct Found {
    ident: String,
    /// Below the output folder, `.ts` included.
    path: PathBuf,
    export: fn(&Config) -> Result<(), ExportError>,
}

/// Collects the generated types a command's arguments and result reach.
struct Collector<'a> {
    cfg: &'a Config,
    found: BTreeMap<String, Found>,
}

impl TypeVisitor for Collector<'_> {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        if let Some(path) = T::output_path() {
            let ident = T::ident(self.cfg);
            self.found.entry(ident.clone()).or_insert(Found {
                ident,
                path,
                export: T::export_all,
            });
        }
        T::visit_generics(self);
    }
}

impl Collector<'_> {
    /// The TypeScript name of `T`, collecting the generated types it names.
    fn ty<T: TS + 'static>(&mut self) -> String {
        self.visit::<T>();
        T::name(self.cfg)
    }

    fn arg<T: TS + 'static>(&mut self, name: &'static str) -> Arg {
        Arg {
            name,
            optional: T::IS_OPTION,
            ts: self.ty::<T>(),
        }
    }

    /// `queryId` and `slot`, which every read takes first.
    fn read_args(&mut self) -> Vec<Arg> {
        vec![self.arg::<String>("queryId"), self.arg::<String>("slot")]
    }
}

/// `Signature`s, built from the Rust types: `cmd!(c, name(arg: T, …) -> R)`
/// for a command, `read!(…)` for a read, which takes `queryId` and `slot`
/// before its own arguments.
macro_rules! cmd {
    ($c:ident, $name:ident ( $($arg:ident : $ty:ty),* $(,)? ) -> $ret:ty) => {
        Signature {
            name: stringify!($name),
            args: vec![$($c.arg::<$ty>(stringify!($arg))),*],
            result: $c.ty::<$ret>(),
        }
    };
}

macro_rules! read {
    ($c:ident, $name:ident ( $($arg:ident : $ty:ty),* $(,)? ) -> $ret:ty) => {{
        let mut args = $c.read_args();
        args.extend(vec![$($c.arg::<$ty>(stringify!($arg))),*]);
        Signature {
            name: stringify!($name),
            args,
            result: $c.ty::<$ret>(),
        }
    }};
}

/// Every command in `COMMANDS`' order, with what it takes and answers.
fn table(c: &mut Collector<'_>) -> Vec<Signature> {
    vec![
        cmd!(c, case_new(path: String, name: String) -> CaseSummary),
        cmd!(c, case_open(path: String) -> CaseSummary),
        cmd!(c, case_close() -> ()),
        read!(c, case_summary() -> CaseSummary),
        cmd!(c, load_run(path: String) -> LoadReport),
        cmd!(
            c,
            load_file(path: String, options: Option<FileOptions>) -> LoadReport
        ),
        read!(c, columns(relation: String) -> Vec<ColumnMeta>),
        read!(c, sessions() -> Vec<Session>),
        read!(c, host_zones() -> Vec<HostZone>),
        cmd!(c, zone_names() -> Vec<String>),
        cmd!(c, ioc_guess(value: String) -> Option<IocType>),
        cmd!(c, tactics() -> Vec<String>),
        cmd!(c, display_zone_set(zone: String) -> ()),
        read!(
            c,
            grid_page(state: GridState, limit: u32, offset: u64, options: PageOptions) -> GridPage
        ),
        read!(c, grid_counts(state: GridState) -> GridCounts),
        read!(
            c,
            grid_groups(
                state: GridState,
                by: Vec<String>,
                parent: Vec<Option<String>>,
                limit: u32,
                offset: u64,
                zone: String,
            ) -> Listing<GroupView>
        ),
        read!(
            c,
            grid_histogram(
                state: GridState,
                column: Option<String>,
                maxBuckets: u32,
                zone: String,
            ) -> Histogram
        ),
        read!(
            c,
            grid_values(
                state: GridState,
                column: String,
                exceptChips: Vec<String>,
                contains: Option<String>,
                order: ValueOrder,
                limit: u32,
                offset: u64,
            ) -> Listing<ValueCount>
        ),
        read!(c, grid_empty_columns(state: GridState) -> Vec<String>),
        read!(
            c,
            grid_matches(
                state: GridState,
                from: Option<u64>,
                backward: bool,
                limit: u32,
            ) -> Listing<MatchPos>
        ),
        read!(c, grid_match_count(state: GridState) -> u64),
        read!(
            c,
            grid_row_offset(state: GridState, rowId: i64) -> Option<u64>
        ),
        read!(
            c,
            grid_row_ids(state: GridState, ranges: Vec<(u64, u64)>) -> Vec<i64>
        ),
        read!(
            c,
            grid_row_positions(state: GridState, ids: Vec<i64>) -> Vec<(i64, Option<u64>)>
        ),
        read!(
            c,
            grid_export(state: GridState, path: String, zoneTimes: Option<String>) -> ExportReport
        ),
        read!(
            c,
            date_range_chip(
                relation: String,
                column: String,
                from: Option<String>,
                to: Option<String>,
                zone: String,
            ) -> Chip
        ),
        read!(c, default_colour_rules(relation: String) -> Vec<ColourRule>),
        read!(c, row_view(rowId: i64, zone: String) -> RowView),
        read!(c, cell_value(rowId: i64, column: String) -> CellValue),
        read!(
            c,
            around_totals(rowId: i64, windows: Vec<u64>) -> Vec<WindowTotal>
        ),
        read!(
            c,
            merged_page(
                source: MergedSource,
                limit: u32,
                offset: u64,
                zone: String,
            ) -> Listing<MergedView>
        ),
        read!(
            c,
            views_list(relation: Option<String>, limit: u32, offset: u64) -> ViewListing<SavedView>
        ),
        read!(
            c,
            views_matching(limit: u32, offset: u64) -> ViewListing<MatchingView>
        ),
        cmd!(
            c,
            view_save(name: String, kind: ViewKind, pattern: String, doc: ViewDoc) -> SavedView
        ),
        cmd!(c, view_delete(id: i64) -> ()),
        read!(c, view_apply(id: i64, relation: String) -> AppliedView),
        read!(c, view_export(id: i64, path: String) -> ()),
        cmd!(c, view_import(path: String) -> SavedView),
        cmd!(c, mark_set(rows: Vec<i64>) -> Vec<RowOutcome>),
        cmd!(c, mark_clear(rows: Vec<i64>) -> Vec<RowOutcome>),
        cmd!(c, tag_add(rows: Vec<i64>, tag: String) -> Vec<RowOutcome>),
        cmd!(
            c,
            tag_remove(rows: Vec<i64>, tag: String) -> Vec<RowOutcome>
        ),
        read!(c, tags_list(limit: u32, offset: u64) -> Listing<TagInfo>),
        cmd!(c, note_add(rowId: i64, text: String) -> Note),
        cmd!(c, note_delete(id: i64) -> ()),
        read!(
            c,
            notes_list(rowId: i64, limit: u32, offset: u64) -> Listing<Note>
        ),
        read!(
            c,
            entries_list(query: EntryQuery, limit: u32, offset: u64) -> Listing<Entry>
        ),
        read!(c, entry_get(id: i64) -> Entry),
        cmd!(c, entry_new(edit: EntryChanges) -> Entry),
        cmd!(c, entry_edit(id: i64, edit: EntryChanges) -> Entry),
        cmd!(c, entry_status(id: i64, status: EntryStatus) -> Entry),
        cmd!(c, entry_delete(id: i64) -> ()),
        cmd!(c, rows_promote(rows: Vec<i64>) -> Vec<PromoteOutcome>),
        cmd!(
            c,
            rows_attach(entryId: i64, rows: Vec<i64>) -> Vec<RowOutcome>
        ),
        cmd!(c, evidence_detach(entryId: i64, rowId: i64) -> ()),
        read!(
            c,
            evidence_list(entryId: i64, limit: u32, offset: u64) -> Listing<Evidence>
        ),
        read!(
            c,
            records_list(query: RecordQuery, limit: u32, offset: u64) -> Listing<CaseRecord>
        ),
        read!(c, record_get(kind: RecordKind, id: i64) -> CaseRecord),
        cmd!(c, record_upsert(spec: RecordSpec) -> CaseRecord),
        cmd!(c, record_delete(kind: RecordKind, id: i64) -> ()),
        cmd!(
            c,
            record_merge(kind: RecordKind, fromId: i64, intoId: i64) -> ()
        ),
        cmd!(
            c,
            record_status(kind: RecordKind, id: i64, status: RecordStatus) -> ()
        ),
        cmd!(
            c,
            record_link(entryId: i64, kind: RecordKind, recordId: i64) -> ()
        ),
        cmd!(
            c,
            record_unlink(entryId: i64, kind: RecordKind, recordId: i64) -> ()
        ),
        read!(
            c,
            record_entries(kind: RecordKind, id: i64, limit: u32, offset: u64) -> Listing<Entry>
        ),
        read!(c, candidates(rowId: i64) -> Vec<Candidate>),
        read!(c, ioc_sightings(id: i64) -> Sightings),
        read!(c, logon_summary(id: i64) -> LogonSummary),
        read!(
            c,
            history_list(
                targetKind: Option<String>,
                targetId: Option<String>,
                limit: u32,
                offset: u64,
            ) -> Listing<HistoryRow>
        ),
        read!(c, export_timeline(path: String) -> ExportReport),
        read!(c, export_workbook(path: String) -> ExportReport),
        read!(c, export_report(path: String) -> ExportReport),
        cmd!(c, settings_get() -> SettingsReply),
        cmd!(c, settings_update(settings: Settings) -> SettingsReply),
        cmd!(c, recent_clear() -> SettingsReply),
        cmd!(c, recent_remove(path: String) -> SettingsReply),
        cmd!(c, query_cancel(queryIds: Vec<String>) -> ()),
    ]
}

/// Every command with what it takes and answers, in `COMMANDS`' order.
pub fn signatures(cfg: &Config) -> Vec<Signature> {
    let mut c = Collector {
        cfg,
        found: BTreeMap::new(),
    };
    table(&mut c)
}

/// Write the whole contract into `dir`: every type it reaches, and
/// `commands.ts`. `dir` should be empty: ts-rs merges a declaration into a
/// file this process already wrote.
pub fn generate(dir: &Path) -> Result<(), String> {
    let cfg = config().with_out_dir(dir);
    let mut c = Collector {
        cfg: &cfg,
        found: BTreeMap::new(),
    };
    let sigs = table(&mut c);
    let events = [
        (CASE_CHANGED, c.ty::<CaseChanged>()),
        (JOB_STARTED, c.ty::<JobStarted>()),
        (JOB_FINISHED, c.ty::<JobFinished>()),
        (SETTINGS_DAMAGED, c.ty::<DamagedSettings>()),
    ];
    let error = c.ty::<CmdError>();
    for found in c.found.values() {
        (found.export)(&cfg).map_err(|e| format!("{}: {e}", found.ident))?;
    }
    let text = commands_ts(&sigs, &events, &error, &c.found);
    std::fs::write(dir.join("commands.ts"), text).map_err(|e| e.to_string())
}

/// `commands.ts`: the names, arguments, results and events.
fn commands_ts(
    sigs: &[Signature],
    events: &[(&str, String)],
    error: &str,
    found: &BTreeMap<String, Found>,
) -> String {
    let mut out = String::new();
    out.push_str(
        "// Generated by oxl-app (src/contract.rs) from the Rust types. Do not edit:\n\
         // `OXL_WRITE_CONTRACT=1 cargo test -p oxl-app --test contract` rewrites it.\n",
    );
    for f in found.values() {
        let path = f.path.with_extension("");
        let path = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let _ = writeln!(out, "import type {{ {} }} from \"./{path}\";", f.ident);
    }

    out.push_str("\n/** Every command the app answers, in the contract's order. */\n");
    out.push_str("export const COMMANDS = [\n");
    for s in sigs {
        let _ = writeln!(out, "  \"{}\",", s.name);
    }
    out.push_str("] as const;\n\nexport type Command = (typeof COMMANDS)[number];\n");

    out.push_str(
        "\n/**\n * What each command takes, as `invoke` takes it. Every read takes `queryId`\n \
         * (the window's id for the query, which `query_cancel` names) and `slot`\n \
         * (what it fills on screen: a newer query on the slot supersedes it).\n \
         * An argument marked `?` may be left out.\n */\n",
    );
    out.push_str("export type CommandArgs = {\n");
    for s in sigs {
        if s.args.is_empty() {
            let _ = writeln!(out, "  {}: Record<string, never>;", s.name);
            continue;
        }
        let args: Vec<String> = s
            .args
            .iter()
            .map(|a| format!("{}{}: {}", a.name, if a.optional { "?" } else { "" }, a.ts))
            .collect();
        let _ = writeln!(out, "  {}: {{ {} }};", s.name, args.join("; "));
    }
    out.push_str("};\n");

    let _ = write!(
        out,
        "\n/**\n * What each command answers. A command that fails rejects with a\n * `{error}`: \
         `code` is what to branch on (`no_case`, `busy`, `superseded`,\n * `interrupted`, \
         and the engine's `not_found`, `invalid`, `refused`...).\n */\n"
    );
    out.push_str("export type CommandResult = {\n");
    for s in sigs {
        let _ = writeln!(out, "  {}: {};", s.name, s.result);
    }
    out.push_str("};\n");

    out.push_str("\n/** Every event the app sends the window. */\nexport const EVENTS = [\n");
    for (name, _) in events {
        let _ = writeln!(out, "  \"{name}\",");
    }
    out.push_str("] as const;\n\nexport type AppEvent = (typeof EVENTS)[number];\n");
    out.push_str("\n/** What each event carries. */\nexport type EventPayload = {\n");
    for (name, ts) in events {
        let _ = writeln!(out, "  \"{name}\": {ts};");
    }
    out.push_str("};\n\n/** What a failed command rejects with. */\n");
    let _ = writeln!(out, "export type CommandError = {error};");
    out
}
