// The only caller of Tauri's `invoke` and `listen`: nothing else imports `@tauri-apps/api/core` or `/event`. Screens
// use the typed wrappers at the bottom, one per command; the JSON shapes are the Rust types' serde shapes unchanged
// (`contract/generated/`, written by `cargo test -p oxl-app --test contract`). No rule lives in the front end.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AppEvent, Command, CommandArgs, CommandResult, EventPayload } from "./contract/generated/commands";
import type { CaseChanged } from "./contract/generated/CaseChanged";
import type { DamagedSettings } from "./contract/generated/DamagedSettings";
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
export const viewSave = (name: string, kind: ViewKind, pattern: string, doc: ViewDoc) =>
  call<CommandResult["view_save"]>("view_save", { name, kind, pattern, doc } satisfies Args<"view_save">);
export const viewDelete = (id: number) => call<CommandResult["view_delete"]>("view_delete", { id } satisfies Args<"view_delete">);
export const viewApply = (id: number, relation: string, slot: string, queryId = newQueryId()) =>
  call<CommandResult["view_apply"]>("view_apply", { queryId, slot, id, relation } satisfies Args<"view_apply">);
export const viewExport = (id: number, path: string, slot: string, queryId = newQueryId()) =>
  call<CommandResult["view_export"]>("view_export", { queryId, slot, id, path } satisfies Args<"view_export">);
export const viewImport = (path: string) => call<CommandResult["view_import"]>("view_import", { path } satisfies Args<"view_import">);

// settings
export const settingsGet = () => call<CommandResult["settings_get"]>("settings_get");
export const settingsUpdate = (settings: Settings) =>
  call<CommandResult["settings_update"]>("settings_update", { settings } satisfies Args<"settings_update">);

// cancelling
export const queryCancel = (queryIds: string[]) =>
  call<CommandResult["query_cancel"]>("query_cancel", { queryIds } satisfies Args<"query_cancel">);
