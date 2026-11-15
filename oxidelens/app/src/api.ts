// The only caller of Tauri's `invoke` and `listen`: nothing else imports `@tauri-apps/api/core` or `/event`. Screens
// use the typed wrappers at the bottom, one per command; the JSON shapes are the Rust types' serde shapes unchanged
// (`contract/generated/`, written by `cargo test -p oxl-app --test contract`). No rule lives in the front end.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AppEvent, Command, CommandArgs, CommandResult, EventPayload } from "./contract/generated/commands";
import type { CaseChanged } from "./contract/generated/CaseChanged";
import type { DamagedSettings } from "./contract/generated/DamagedSettings";
import type { EntryChanges } from "./contract/generated/EntryChanges";
import type { EntryQuery } from "./contract/generated/EntryQuery";
import type { EntryStatus } from "./contract/generated/EntryStatus";
import type { RecordKind } from "./contract/generated/RecordKind";
import type { RecordQuery } from "./contract/generated/RecordQuery";
import type { RecordSpec } from "./contract/generated/RecordSpec";
import type { RecordStatus } from "./contract/generated/RecordStatus";
import type { JobFinished } from "./contract/generated/JobFinished";
import type { JobStarted } from "./contract/generated/JobStarted";
import type { FileOptions } from "./contract/generated/FileOptions";
import type { GridState } from "./contract/generated/GridState";
import type { MergedSource } from "./contract/generated/MergedSource";
import type { PageOptions } from "./contract/generated/PageOptions";
import type { Settings } from "./contract/generated/Settings";
import type { ValueOrder } from "./contract/generated/ValueOrder";
import type { ViewDoc } from "./contract/generated/ViewDoc";
import type { ViewKind } from "./contract/generated/ViewKind";

export type { Command } from "./contract/generated/commands";

/** What a command can reject with (the contract's `error.rs`): branch on `code`, show `message` through `displaySafe`. */
export const CMD_ERROR_CODES = [
  "not_found",
  "invalid",
  "refused",
  "case_locked",
  "needs_upgrade",
  "engine",
  "io",
  "internal",
  "no_case",
  "busy",
  "superseded",
  "interrupted",
] as const;

export interface CmdError {
  code: (typeof CMD_ERROR_CODES)[number];
  message: string;
}

export function isCmdError(e: unknown): e is CmdError {
  if (typeof e !== "object" || e === null) return false;
  const o = e as Record<string, unknown>;
  return typeof o.code === "string" && (CMD_ERROR_CODES as readonly string[]).includes(o.code) && typeof o.message === "string";
}

/**
 * Runs one backend command. Rejects with a `CmdError` only: a command error comes through as it is, and anything else
 * Tauri or the webview rejects with (a string, an `Error`, an unknown code) becomes `internal`, keeping its message
 * when it has one.
 */
export async function call<T>(cmd: Command, args?: Record<string, unknown>): Promise<T> {
  try {
    // Dev-only mock backend (`npm run dev:mock`): `__OXL_MOCK__` is false in every other build, so the import is dropped.
    if (__OXL_MOCK__) return await (await import("./mock/backend")).mockCall<T>(cmd, args);
    return await invoke<T>(cmd, args);
  } catch (e) {
    if (isCmdError(e)) throw e;
    const message =
      typeof e === "object" && e !== null && typeof (e as Record<string, unknown>).message === "string"
        ? ((e as Record<string, unknown>).message as string)
        : String(e);
    const err: CmdError = { code: "internal", message };
    throw err;
  }
}

let queryCounter = 0;

/**
 * A fresh id for one query: the window's name for it, which `queryCancel` names and a stale answer is recognised by.
 * A counter and a random UUID, never a clock.
 */
export function newQueryId(): string {
  queryCounter += 1;
  const random =
    typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : Math.random().toString(16).slice(2) + Math.random().toString(16).slice(2);
  return `q${queryCounter}-${random}`;
}

// ---------------------------------------------------------------------------------------------------------- events

export type JobEvent = ({ phase: "started" } & JobStarted) | ({ phase: "finished" } & JobFinished);

async function on<E extends AppEvent>(event: E, f: (payload: EventPayload[E]) => void): Promise<() => void> {
  if (__OXL_MOCK__) return (await import("./mock/backend")).mockOn(event, f);
  return listen<EventPayload[E]>(event, (e) => f(e.payload));
}

/** The case changed (an open, a close or a write). Resolves to the function that stops listening. */
export function onCaseChanged(f: (p: CaseChanged) => void): Promise<() => void> {
  return on("case-changed", f);
}

/** A load or an export started or finished. Resolves to the function that stops listening. */
export async function onJob(f: (e: JobEvent) => void): Promise<() => void> {
  const stops = await Promise.all([
    on("job-started", (p) => f({ phase: "started", ...p })),
    on("job-finished", (p) => f({ phase: "finished", ...p })),
  ]);
  return () => stops.forEach((stop) => stop());
}

/** The settings file was unreadable and the defaults are in use. */
export function onSettingsDamaged(f: (p: DamagedSettings) => void): Promise<() => void> {
  return on("settings-damaged", f);
}

// ------------------------------------------------------------------------------------------------------- commands
// One typed wrapper per command. Arguments go in the contract's order, then the `slot` a read fills, then its
// `queryId` (a fresh one unless the caller wants to cancel it by name). `satisfies CommandArgs[...]` makes the
// compiler check the argument names against the generated contract.

type Args<C extends Command> = CommandArgs[C];

// case
export const caseNew = (path: string, name: string) =>
  call<CommandResult["case_new"]>("case_new", { path, name } satisfies Args<"case_new">);
export const caseOpen = (path: string) => call<CommandResult["case_open"]>("case_open", { path } satisfies Args<"case_open">);
export const caseClose = () => call<CommandResult["case_close"]>("case_close");
export const caseSummary = (slot: string, queryId = newQueryId()) =>
  call<CommandResult["case_summary"]>("case_summary", { queryId, slot } satisfies Args<"case_summary">);

// loading
export const loadRun = (path: string) => call<CommandResult["load_run"]>("load_run", { path } satisfies Args<"load_run">);
export const loadFile = (path: string, options: FileOptions | null = null) =>
  call<CommandResult["load_file"]>("load_file", { path, options } satisfies Args<"load_file">);

// what the case holds
export const columns = (relation: string, slot: string, queryId = newQueryId()) =>
  call<CommandResult["columns"]>("columns", { queryId, slot, relation } satisfies Args<"columns">);
export const sessions = (slot: string, queryId = newQueryId()) =>
  call<CommandResult["sessions"]>("sessions", { queryId, slot } satisfies Args<"sessions">);
export const hostZones = (slot: string, queryId = newQueryId()) =>
  call<CommandResult["host_zones"]>("host_zones", { queryId, slot } satisfies Args<"host_zones">);
export const zoneNames = () => call<CommandResult["zone_names"]>("zone_names");
export const displayZoneSet = (zone: string) =>
  call<CommandResult["display_zone_set"]>("display_zone_set", { zone } satisfies Args<"display_zone_set">);

// the grid
export const gridPage = (
  state: GridState, limit: number, offset: number, options: PageOptions, slot: string, queryId = newQueryId(),
) => call<CommandResult["grid_page"]>("grid_page", { queryId, slot, state, limit, offset, options } satisfies Args<"grid_page">);
export const gridCounts = (state: GridState, slot: string, queryId = newQueryId()) =>
  call<CommandResult["grid_counts"]>("grid_counts", { queryId, slot, state } satisfies Args<"grid_counts">);
export const gridGroups = (
  state: GridState, by: string[], parent: Array<string | null>, limit: number, offset: number, zone: string,
  slot: string, queryId = newQueryId(),
) =>
  call<CommandResult["grid_groups"]>(
    "grid_groups", { queryId, slot, state, by, parent, limit, offset, zone } satisfies Args<"grid_groups">,
  );
export const gridHistogram = (
  state: GridState, column: string | null, maxBuckets: number, zone: string, slot: string, queryId = newQueryId(),
) =>
  call<CommandResult["grid_histogram"]>(
    "grid_histogram", { queryId, slot, state, column, maxBuckets, zone } satisfies Args<"grid_histogram">,
  );
export const gridValues = (
  state: GridState, column: string, exceptChips: string[], contains: string | null, order: ValueOrder,
  limit: number, offset: number, slot: string, queryId = newQueryId(),
) =>
  call<CommandResult["grid_values"]>(
    "grid_values", { queryId, slot, state, column, exceptChips, contains, order, limit, offset } satisfies Args<"grid_values">,
  );
export const gridEmptyColumns = (state: GridState, slot: string, queryId = newQueryId()) =>
  call<CommandResult["grid_empty_columns"]>("grid_empty_columns", { queryId, slot, state } satisfies Args<"grid_empty_columns">);
/** Forward: the matches after `from` (all when null), nearest first; backward: those before it, nearest first. */
export const gridMatches = (state: GridState, from: number | null, backward: boolean, limit: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["grid_matches"]>("grid_matches", { queryId, slot, state, from, backward, limit } satisfies Args<"grid_matches">);
export const gridMatchCount = (state: GridState, slot: string, queryId = newQueryId()) =>
  call<CommandResult["grid_match_count"]>("grid_match_count", { queryId, slot, state } satisfies Args<"grid_match_count">);
/** The row's place in what `state` shows (its offset in the state's order), or null when `state` leaves it out. */
export const gridRowOffset = (state: GridState, rowId: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["grid_row_offset"]>("grid_row_offset", { queryId, slot, state, rowId } satisfies Args<"grid_row_offset">);
/** The row ids at the inclusive index ranges of `state`'s order, in that order; at most 10,000 rows, else `invalid`. */
export const gridRowIds = (state: GridState, ranges: Array<[number, number]>, slot: string, queryId = newQueryId()) =>
  call<CommandResult["grid_row_ids"]>("grid_row_ids", { queryId, slot, state, ranges } satisfies Args<"grid_row_ids">);
/** Each id with its index in `state`'s order now, or null when `state` no longer shows it. */
export const gridRowPositions = (state: GridState, ids: number[], slot: string, queryId = newQueryId()) =>
  call<CommandResult["grid_row_positions"]>("grid_row_positions", { queryId, slot, state, ids } satisfies Args<"grid_row_positions">);
export const gridExport = (
  state: GridState, path: string, zoneTimes: string | null, slot: string, queryId = newQueryId(),
) => call<CommandResult["grid_export"]>("grid_export", { queryId, slot, state, path, zoneTimes } satisfies Args<"grid_export">);
export const dateRangeChip = (
  relation: string, column: string, from: string | null, to: string | null, zone: string, slot: string,
  queryId = newQueryId(),
) =>
  call<CommandResult["date_range_chip"]>(
    "date_range_chip", { queryId, slot, relation, column, from, to, zone } satisfies Args<"date_range_chip">,
  );
export const defaultColourRules = (relation: string, slot: string, queryId = newQueryId()) =>
  call<CommandResult["default_colour_rules"]>(
    "default_colour_rules", { queryId, slot, relation } satisfies Args<"default_colour_rules">,
  );

// one row
export const rowView = (rowId: number, zone: string, slot: string, queryId = newQueryId()) =>
  call<CommandResult["row_view"]>("row_view", { queryId, slot, rowId, zone } satisfies Args<"row_view">);
export const cellValue = (rowId: number, column: string, slot: string, queryId = newQueryId()) =>
  call<CommandResult["cell_value"]>("cell_value", { queryId, slot, rowId, column } satisfies Args<"cell_value">);
export const aroundTotals = (rowId: number, windows: number[], slot: string, queryId = newQueryId()) =>
  call<CommandResult["around_totals"]>("around_totals", { queryId, slot, rowId, windows } satisfies Args<"around_totals">);
export const mergedPage = (
  source: MergedSource, limit: number, offset: number, zone: string, slot: string, queryId = newQueryId(),
) => call<CommandResult["merged_page"]>("merged_page", { queryId, slot, source, limit, offset, zone } satisfies Args<"merged_page">);

// saved views and presets
export const viewsList = (relation: string | null, limit: number, offset: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["views_list"]>("views_list", { queryId, slot, relation, limit, offset } satisfies Args<"views_list">);
/** The views and presets that match at least one relation of the case, each with the relations it matches. */
export const viewsMatching = (limit: number, offset: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["views_matching"]>("views_matching", { queryId, slot, limit, offset } satisfies Args<"views_matching">);
export const viewSave =(name: string, kind: ViewKind, pattern: string, doc: ViewDoc) =>
  call<CommandResult["view_save"]>("view_save", { name, kind, pattern, doc } satisfies Args<"view_save">);
export const viewDelete = (id: number) => call<CommandResult["view_delete"]>("view_delete", { id } satisfies Args<"view_delete">);
export const viewApply = (id: number, relation: string, slot: string, queryId = newQueryId()) =>
  call<CommandResult["view_apply"]>("view_apply", { queryId, slot, id, relation } satisfies Args<"view_apply">);
export const viewExport = (id: number, path: string, slot: string, queryId = newQueryId()) =>
  call<CommandResult["view_export"]>("view_export", { queryId, slot, id, path } satisfies Args<"view_export">);
export const viewImport = (path: string) => call<CommandResult["view_import"]>("view_import", { path } satisfies Args<"view_import">);

// marks, tags and notes on rows (writes: `case-changed` with `marks`, `tags` or `notes`)
/** S: each row marked Suspicious, or why not. At most 10,000 rows (`gridRowIds` resolves a selection). */
export const markSet = (rows: number[]) => call<CommandResult["mark_set"]>("mark_set", { rows } satisfies Args<"mark_set">);
/** U: each row's Suspicious mark cleared, or why not. */
export const markClear = (rows: number[]) =>
  call<CommandResult["mark_clear"]>("mark_clear", { rows } satisfies Args<"mark_clear">);
export const tagAdd = (rows: number[], tag: string) =>
  call<CommandResult["tag_add"]>("tag_add", { rows, tag } satisfies Args<"tag_add">);
export const tagRemove = (rows: number[], tag: string) =>
  call<CommandResult["tag_remove"]>("tag_remove", { rows, tag } satisfies Args<"tag_remove">);
export const tagsList = (limit: number, offset: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["tags_list"]>("tags_list", { queryId, slot, limit, offset } satisfies Args<"tags_list">);
export const noteAdd = (rowId: number, text: string) =>
  call<CommandResult["note_add"]>("note_add", { rowId, text } satisfies Args<"note_add">);
export const noteDelete = (id: number) => call<CommandResult["note_delete"]>("note_delete", { id } satisfies Args<"note_delete">);
export const notesList = (rowId: number, limit: number, offset: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["notes_list"]>("notes_list", { queryId, slot, rowId, limit, offset } satisfies Args<"notes_list">);

// timeline entries and their evidence (writes: `case-changed` with `entries`, and `marks` and `records` when evidence
// or a review moves them)
/** The live entries `query` selects, in the Timeline's order: by time, the undated ("needs a time") last. */
export const entriesList = (query: EntryQuery, limit: number, offset: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["entries_list"]>("entries_list", { queryId, slot, query, limit, offset } satisfies Args<"entries_list">);
export const entryGet = (id: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["entry_get"]>("entry_get", { queryId, slot, id } satisfies Args<"entry_get">);
/** A new entry; `edit.title` is required, every other field `null` for none. */
export const entryNew = (edit: EntryChanges) => call<CommandResult["entry_new"]>("entry_new", { edit } satisfies Args<"entry_new">);
/** Changes only the fields `edit` gives; `null` leaves a field as it is (a time the evidence gave stays the row's). */
export const entryEdit = (id: number, edit: EntryChanges) =>
  call<CommandResult["entry_edit"]>("entry_edit", { id, edit } satisfies Args<"entry_edit">);
export const entryStatus = (id: number, status: EntryStatus) =>
  call<CommandResult["entry_status"]>("entry_status", { id, status } satisfies Args<"entry_status">);
export const entryDelete = (id: number) =>
  call<CommandResult["entry_delete"]>("entry_delete", { id } satisfies Args<"entry_delete">);
/** M: an entry per row, timed by the row when it gives a UTC time; a row already evidence answers its entry. */
export const rowsPromote = (rows: number[]) =>
  call<CommandResult["rows_promote"]>("rows_promote", { rows } satisfies Args<"rows_promote">);
/** A: the rows attached to entry `entryId` as evidence, linking each row's host. */
export const rowsAttach = (entryId: number, rows: number[]) =>
  call<CommandResult["rows_attach"]>("rows_attach", { entryId, rows } satisfies Args<"rows_attach">);
export const evidenceDetach = (entryId: number, rowId: number) =>
  call<CommandResult["evidence_detach"]>("evidence_detach", { entryId, rowId } satisfies Args<"evidence_detach">);
/** Every row attached to the entry, each re-checked against the case (`standing`). */
export const evidenceList = (entryId: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["evidence_list"]>("evidence_list", { queryId, slot, entryId } satisfies Args<"evidence_list">);

// host, account and IOC records (writes: `case-changed` with `records`, and `entries` when the Timeline shows it)
/** The live records of `query.kind`, each tagged with its kind; IOCs narrowed by type, tool and text. */
export const recordsList = (query: RecordQuery, limit: number, offset: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["records_list"]>("records_list", { queryId, slot, query, limit, offset } satisfies Args<"records_list">);
/** One record, a deleted one too (`deleted` says so). */
export const recordGet = (kind: RecordKind, id: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["record_get"]>("record_get", { queryId, slot, kind, id } satisfies Args<"record_get">);
/** A new record (`spec.id` null), or every field of record `spec.id`. */
export const recordUpsert = (spec: RecordSpec) =>
  call<CommandResult["record_upsert"]>("record_upsert", { spec } satisfies Args<"record_upsert">);
export const recordDelete = (kind: RecordKind, id: number) =>
  call<CommandResult["record_delete"]>("record_delete", { kind, id } satisfies Args<"record_delete">);
/** Every entry linked to record `fromId` ends linked to `intoId`, and `fromId` is deleted. */
export const recordMerge = (kind: RecordKind, fromId: number, intoId: number) =>
  call<CommandResult["record_merge"]>("record_merge", { kind, fromId, intoId } satisfies Args<"record_merge">);
export const recordStatus = (kind: RecordKind, id: number, status: RecordStatus) =>
  call<CommandResult["record_status"]>("record_status", { kind, id, status } satisfies Args<"record_status">);
export const recordLink = (entryId: number, kind: RecordKind, recordId: number) =>
  call<CommandResult["record_link"]>("record_link", { entryId, kind, recordId } satisfies Args<"record_link">);
export const recordUnlink = (entryId: number, kind: RecordKind, recordId: number) =>
  call<CommandResult["record_unlink"]>("record_unlink", { entryId, kind, recordId } satisfies Args<"record_unlink">);
/** The live entries linked to a record, in the Timeline's order. */
export const recordEntries = (kind: RecordKind, id: number, limit: number, offset: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["record_entries"]>("record_entries", { queryId, slot, kind, id, limit, offset } satisfies Args<"record_entries">);

// reads over a row, an IOC, an account, the history
/** The accounts and IOCs a row names, each with the record the case already has for it (`existing_id`). */
export const candidates = (rowId: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["candidates"]>("candidates", { queryId, slot, rowId } satisfies Args<"candidates">);
/** The slot a sightings sweep runs on, one IOC at a time, so a newer sweep supersedes the older (S5). */
export const SIGHTINGS_SLOT = "sightings";
/** Where an IOC's value occurs in the loaded data: computed each time, never stored. Cancellable by its query id. */
export const iocSightings = (id: number, slot: string = SIGHTINGS_SLOT, queryId = newQueryId()) =>
  call<CommandResult["ioc_sightings"]>("ioc_sightings", { queryId, slot, id } satisfies Args<"ioc_sightings">);
/** An account's logons by host, type and outcome, each with the filter document that opens its rows. */
export const logonSummary = (id: number, slot: string, queryId = newQueryId()) =>
  call<CommandResult["logon_summary"]>("logon_summary", { queryId, slot, id } satisfies Args<"logon_summary">);
/** History rows, newest first: all, of one target kind, or of one target (`targetKind` and `targetId`). */
export const historyList = (
  targetKind: string | null, targetId: string | null, limit: number, offset: number, slot: string, queryId = newQueryId(),
) =>
  call<CommandResult["history_list"]>(
    "history_list", { queryId, slot, targetKind, targetId, limit, offset } satisfies Args<"history_list">,
  );

// the record exports: each a read and a job (`job-started`, `job-finished`), cancelled by its query id
export const exportTimeline = (path: string, slot: string, queryId = newQueryId()) =>
  call<CommandResult["export_timeline"]>("export_timeline", { queryId, slot, path } satisfies Args<"export_timeline">);
export const exportWorkbook = (path: string, slot: string, queryId = newQueryId()) =>
  call<CommandResult["export_workbook"]>("export_workbook", { queryId, slot, path } satisfies Args<"export_workbook">);
export const exportReport = (path: string, slot: string, queryId = newQueryId()) =>
  call<CommandResult["export_report"]>("export_report", { queryId, slot, path } satisfies Args<"export_report">);

// settings
export const settingsGet =() => call<CommandResult["settings_get"]>("settings_get");
/** Saves the whole settings object; the backend keeps its own `recent` list whatever this one holds. */
export const settingsUpdate = (settings: Settings) =>
  call<CommandResult["settings_update"]>("settings_update", { settings } satisfies Args<"settings_update">);
/** Forgets every recent case (the list is the backend's). */
export const recentClear = () => call<CommandResult["recent_clear"]>("recent_clear");
/** Forgets one recent case. */
export const recentRemove = (path: string) =>
  call<CommandResult["recent_remove"]>("recent_remove", { path } satisfies Args<"recent_remove">);

// cancelling
export const queryCancel = (queryIds: string[]) =>
  call<CommandResult["query_cancel"]>("query_cancel", { queryIds } satisfies Args<"query_cancel">);
