// DEV AND TEST ONLY: a fake backend so the UI can be worked on in a plain browser (`npm run dev:mock`) and tested
// without the desktop app. `api.ts` loads it by a dynamic import behind `__OXL_MOCK__`, so a production build holds
// none of it; tests reach it through `test/backend.ts`.
//
// It answers exactly the commands `api.ts` calls (the typed table below is checked against the generated contract by
// the compiler and against `api.ts` by `backend.test.ts`), with the contract's shapes, from the seeded data in
// `generated.ts` and the recorded answers in `test/fixtures`. It keeps the engine's behaviour where a screen depends
// on it: no case open answers `no_case`, a newer query on a slot supersedes the older one, `query_cancel` interrupts
// by id, a second write while one runs answers `busy`, and a write emits `case-changed` (a load also a job).
//
// What it does NOT do: it holds no rules. Filters, sorts and groups are only as faithful as a screen needs; times are
// always shown in UTC whatever zone is asked (the answer says `UTC`); a regex is JavaScript's. Do not test engine
// behaviour here: that is `cargo test`'s.
import type { AppEvent, CommandArgs, CommandResult, EventPayload } from "../contract/generated/commands";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { Cell } from "../contract/generated/Cell";
import type { Chip } from "../contract/generated/Chip";
import type { ColourRule } from "../contract/generated/ColourRule";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";
import type { FilterNode } from "../contract/generated/FilterNode";
import type { GridState } from "../contract/generated/GridState";
import type { GroupView } from "../contract/generated/GroupView";
import type { Leaf } from "../contract/generated/Leaf";
import type { MarkCounts } from "../contract/generated/MarkCounts";
import type { MarkState } from "../contract/generated/MarkState";
import type { MergedView } from "../contract/generated/MergedView";
import type { RelationInfo } from "../contract/generated/RelationInfo";
import type { RelationMarks } from "../contract/generated/RelationMarks";
import type { SavedView } from "../contract/generated/SavedView";
import type { Settings } from "../contract/generated/Settings";
import type { ValueCount } from "../contract/generated/ValueCount";
import type { ValuePart } from "../contract/generated/ValuePart";
import type { ViewDoc } from "../contract/generated/ViewDoc";
import type { ViewWarning } from "../contract/generated/ViewWarning";
import * as fx from "../test/fixtures";
import { EVENT_COLUMNS, EVENT_ROWS, HAYABUSA_COLUMNS, HAYABUSA_FIRST_ID, HAYABUSA_ROWS, eventRows, hayabusaRows } from "./generated";

type CmdCode = "not_found" | "invalid" | "refused" | "no_case" | "busy" | "superseded" | "interrupted" | "internal";
const fail = (code: CmdCode, message: string): never => {
  throw { code, message };
};

// ------------------------------------------------------------------------------------------------------------ state

/** How long a mock answer takes; tests set 0 (see `test/backend.ts`), `npm run dev:mock` keeps a little. */
let latencyMs = 25;
export function setMockLatency(ms: number): void {
  latencyMs = ms;
}
const nap = () => new Promise<void>((resolve) => setTimeout(resolve, latencyMs));

const NOW = "2026-01-10T12:00:00.000000Z";
const MOCK_CASE_ID = "0123456789abcdef0123456789abcdef";

interface MockRelation {
  info: RelationInfo;
  columns: ColumnMeta[];
  rows: ReadonlyArray<ReadonlyArray<string | null>>;
  firstId: number;
}

function relations(): MockRelation[] {
  return [
    {
      info: {
        name: "mock_events", kind: "dataset", tool: "EvtxTriage", rows: EVENT_ROWS, hosts: ["SRV01", "WS01"],
        primary_time: "TimeCreated", evidence_path_column: "SourceFile", host_column: "_triage_host",
      },
      columns: EVENT_COLUMNS, rows: eventRows(), firstId: 1,
    },
    {
      info: {
        name: "mock_hayabusa", kind: "dataset", tool: "Hayabusa", rows: HAYABUSA_ROWS, hosts: ["WS01"],
        primary_time: "Timestamp", evidence_path_column: null, host_column: "_triage_host",
      },
      columns: HAYABUSA_COLUMNS, rows: hayabusaRows(), firstId: HAYABUSA_FIRST_ID,
    },
  ];
}

const relationNamed = (name: string): MockRelation =>
  relations().find((r) => r.info.name === name) ?? fail("not_found", `no relation named ${name}`);

interface MockCase {
  path: string;
  name: string;
  displayZone: string;
  views: SavedView[];
  nextViewId: number;
  marks: Map<number, MarkState>;
}

function defaultSettings(): Settings {
  return {
    version: 1, theme: "dark", font_size: 12, left_pane: "open", right_pane: "open", histogram: true,
    analyst: "analyst01", recent: ["/cases/acme.case.duckdb"], layouts: {}, colour_rules: {},
  };
}

let settings = defaultSettings();
let current: MockCase | null = null;
let generation = 0;
let writing = false;
/** The query each slot is waiting for an answer to, and the ids `query_cancel` named. */
const slotQuery = new Map<string, string>();
const cancelled = new Set<string>();
const listeners = new Map<AppEvent, Set<(payload: never) => void>>();

/** Back to a fresh app with no case open: tests call it before each test. */
export function resetMock(): void {
  settings = defaultSettings();
  current = null;
  generation = 0;
  writing = false;
  slotQuery.clear();
  cancelled.clear();
  listeners.clear();
  latencyMs = 25;
}

// ----------------------------------------------------------------------------------------------------------- events

/** The mock's event bus: `api.ts`'s `onCaseChanged` and `onJob` listen here under `__OXL_MOCK__`. */
export function mockOn<E extends AppEvent>(event: E, handler: (payload: EventPayload[E]) => void): () => void {
  const set = listeners.get(event) ?? new Set();
  set.add(handler as (payload: never) => void);
  listeners.set(event, set);
  return () => set.delete(handler as (payload: never) => void);
}

/** Sends `payload` to every listener of `event`. */
export function mockEmit<E extends AppEvent>(event: E, payload: EventPayload[E]): void {
  for (const handler of [...(listeners.get(event) ?? [])]) handler(payload as never);
}

// --------------------------------------------------------------------------------------------------- read and write

/** A read: needs a case, registers on its slot, takes its time, and ends `superseded` or `interrupted` if told to. */
async function read<T>(slot: string, queryId: string, work: () => T): Promise<T> {
  if (!current) fail("no_case", "no case is open");
  slotQuery.set(slot, queryId);
  await nap();
  if (cancelled.delete(queryId)) fail("interrupted", "the query was cancelled");
  if (slotQuery.get(slot) !== queryId) fail("superseded", "a newer query took this slot");
  slotQuery.delete(slot);
  if (!current) fail("no_case", "no case is open");
  return work();
}

/** A write: needs a case (`no_case`), one at a time (`busy`), then `case-changed` with `kinds`. A job announces itself first and finishes before the change. */
async function write<T>(kinds: string[], work: () => T, job?: { kind: string; label: string }): Promise<T> {
  if (!current) fail("no_case", "no case is open");
  if (writing) fail("busy", "another change is running");
  writing = true;
  try {
    if (job) mockEmit("job-started", job);
    let result: T;
    try {
      await nap();
      result = work();
    } catch (e) {
      if (job) mockEmit("job-finished", { kind: job.kind, ok: false, message: (e as { message?: string }).message ?? null });
      throw e;
    }
    generation += 1;
    // The app's order: job-started, the write, job-finished, then case-changed.
    if (job) mockEmit("job-finished", { kind: job.kind, ok: true, message: null });
    mockEmit("case-changed", { kinds, generation });
    return result;
  } finally {
    writing = false;
  }
}

function theCase(): MockCase {
  return current ?? fail("no_case", "no case is open");
}

function openMock(path: string, name: string): CaseSummary {
  const base = path.split(/[\\/]/).pop() ?? path;
  current = {
    path, name: name || base.replace(/\.duckdb$/, "").replace(/\.case$/, ""), displayZone: "UTC",
    views: fx.viewsList().items.map((v) => ({ ...v, pattern: "mock_*" })), nextViewId: 100,
    marks: new Map<number, MarkState>([[5, "suspicious"], [41, "suspicious"], [902, "suspicious"]]),
  };
  generation += 1;
  settings = { ...settings, recent: [path, ...settings.recent.filter((p) => p !== path)].slice(0, 10) };
  return summary();
}

/** An open or a create: one at a time, and (as in the app) no `case-changed`: the window re-reads on its own answer. */
async function open(path: string, name: string): Promise<CaseSummary> {
  if (writing) fail("busy", "another change is running");
  writing = true;
  try {
    await nap();
    return openMock(path, name);
  } finally {
    writing = false;
  }
}

function summary(): CaseSummary {
  const c = theCase();
  const template = fx.caseSummary();
  return {
    path: c.path, name: c.name, case_id: MOCK_CASE_ID, format: 3, display_zone: c.displayZone,
    hosts: ["SRV01", "WS01"], relations: relations().map((r) => r.info), sources: template.sources,
    mark_counts: markCounts(),
  };
}

/** The marks of the rows the mock holds, in all and per relation, as the engine's `mark_counts` counts them. */
function markCounts(): MarkCounts {
  const marks = theCase().marks;
  const by_relation: RelationMarks[] = [];
  for (const rel of [...relations()].sort((x, y) => (x.info.name < y.info.name ? -1 : x.info.name > y.info.name ? 1 : 0))) {
    const counts: RelationMarks = { relation: rel.info.name, suspicious: 0, malicious: 0, agent_malicious: 0 };
    for (let i = 0; i < rel.rows.length; i++) {
      const mark = marks.get(rowIdOf(rel, i));
      if (mark) counts[mark] += 1;
    }
    if (counts.suspicious + counts.malicious + counts.agent_malicious > 0) by_relation.push(counts);
  }
  const total = (k: MarkState) => by_relation.reduce((n, r) => n + r[k], 0);
  return { suspicious: total("suspicious"), malicious: total("malicious"), agent_malicious: total("agent_malicious"), by_relation };
}

// --------------------------------------------------------------------------------------------- filters, sorts, rows

type Row = ReadonlyArray<string | null>;

const isUtc = (col: ColumnMeta) => col.time_semantics === "utc";
/** The text a filter compares and a screen shows: a utc time as `YYYY-MM-DD HH:MM:SS.fffffff`. */
const shown = (col: ColumnMeta, v: string | null) => (v !== null && isUtc(col) ? v.replace("T", " ").replace(/Z$/, "") : v);
const numeric = (col: ColumnMeta) => col.sql_type === "BIGINT" || col.sql_type === "UBIGINT";

function leaves(node: FilterNode, out: Leaf[] = []): Leaf[] {
  if ("and" in node) node.and.forEach((n) => leaves(n, out));
  else if ("or" in node) node.or.forEach((n) => leaves(n, out));
  else if ("not" in node) leaves(node.not, out);
  else out.push(node);
  return out;
}

function compare(a: string, b: string, numbers: boolean): number {
  if (numbers) return Number(a) - Number(b);
  return a < b ? -1 : a > b ? 1 : 0;
}

/** `_mark`: `eq`, `ne` or `in` over `suspicious` / `none`, as the engine's filter compiler takes it. */
function markMatches(leaf: Leaf, mark: string | null): boolean {
  const is = (v: unknown) => (v === "none" ? mark === null : v === "suspicious" ? mark === "suspicious" : fail("invalid", `_mark is "suspicious" or "none"`));
  switch (leaf.op) {
    case "eq": return is(leaf.value);
    case "ne": return !is(leaf.value);
    case "in": return Array.isArray(leaf.value) && leaf.value.some(is);
    default: return fail("invalid", "_mark takes eq, ne or in");
  }
}

function leafMatches(leaf: Leaf, rel: MockRelation, row: Row, mark: string | null): boolean {
  if (leaf.column === "_mark") return markMatches(leaf, mark);
  const i = rel.columns.findIndex((c) => c.name === leaf.column);
  if (i < 0) fail("invalid", `the relation has no column ${leaf.column}`);
  const col = rel.columns[i];
  const raw = shown(col, row[i]);
  const fold = (s: string) => (leaf.case_sensitive ? s : s.toLowerCase());
  const text = raw ?? "";
  const want = leaf.value === null ? "" : Array.isArray(leaf.value) ? "" : String(leaf.value);
  const list = Array.isArray(leaf.value) ? leaf.value.map(String) : [want];
  switch (leaf.op) {
    case "eq": return raw !== null && fold(text) === fold(want);
    case "ne": return raw === null || fold(text) !== fold(want);
    case "contains": return fold(text).includes(fold(want));
    case "not_contains": return !fold(text).includes(fold(want));
    case "starts_with": return fold(text).startsWith(fold(want));
    case "ends_with": return fold(text).endsWith(fold(want));
    case "regex":
      try {
        return new RegExp(want, leaf.case_sensitive ? "" : "i").test(text);
      } catch {
        return fail("invalid", `not a regular expression: ${want}`);
      }
    case "in": return raw !== null && list.some((v) => fold(v) === fold(text));
    case "not_in": return raw === null || !list.some((v) => fold(v) === fold(text));
    case "between": return raw !== null && compare(text, list[0] ?? "", numeric(col)) >= 0 && compare(text, list[1] ?? "", numeric(col)) <= 0;
    case "gt": return raw !== null && compare(text, want, numeric(col)) > 0;
    case "gte": return raw !== null && compare(text, want, numeric(col)) >= 0;
    case "lt": return raw !== null && compare(text, want, numeric(col)) < 0;
    case "lte": return raw !== null && compare(text, want, numeric(col)) <= 0;
    case "is_empty": return raw === null || raw === "";
    case "not_empty": return raw !== null && raw !== "";
    case "conversion_failed": return false;
  }
}

function nodeMatches(node: FilterNode, rel: MockRelation, row: Row, mark: string | null): boolean {
  if ("and" in node) return node.and.every((n) => nodeMatches(n, rel, row, mark));
  if ("or" in node) return node.or.some((n) => nodeMatches(n, rel, row, mark));
  if ("not" in node) return !nodeMatches(node.not, rel, row, mark);
  return leafMatches(node, rel, row, mark);
}

function searchMatcher(state: GridState): ((row: Row) => boolean) | null {
  const { text, regex, case_sensitive } = state.search;
  if (text === "") return null;
  let re: RegExp | null = null;
  if (regex) {
    try {
      re = new RegExp(text, case_sensitive ? "" : "i");
    } catch {
      return fail("invalid", `not a regular expression: ${text}`);
    }
  }
  const needle = case_sensitive ? text : text.toLowerCase();
  return (row) => row.some((v) => v !== null && (re ? re.test(v) : (case_sensitive ? v : v.toLowerCase()).includes(needle)));
}

interface Selection {
  rel: MockRelation;
  /** Indices into `rel.rows`, filtered and sorted. */
  rows: number[];
  hits: (row: Row) => boolean;
}

/** The rows `state` shows. `skipChips` leaves out chips by id (the values popover counts without its own column's). */
function select(state: GridState, skipChips: readonly string[] = []): Selection {
  const rel = relationNamed(state.relation);
  const search = searchMatcher(state);
  const hostIx = rel.columns.findIndex((c) => c.role === "host");
  const chips = state.chips.filter((c) => c.enabled && !skipChips.includes(c.id));
  const hits = search ?? (() => false);
  const marks = theCase().marks;
  let rows: number[] = [];
  for (let i = 0; i < rel.rows.length; i++) {
    const row = rel.rows[i];
    if (state.hosts.length && !state.hosts.includes(row[hostIx] ?? "")) continue;
    const mark = marks.get(rowIdOf(rel, i)) ?? null;
    if (!chips.every((c) => nodeMatches(c.filter, rel, row, mark))) continue;
    if (search && state.search.mode === "filter" && !search(row)) continue;
    rows.push(i);
  }
  const keys = state.sort.length
    ? state.sort
    : rel.info.primary_time ? [{ column: rel.info.primary_time, descending: false }] : [];
  for (const key of [...keys].reverse()) {
    const c = rel.columns.findIndex((col) => col.name === key.column);
    if (c < 0) fail("invalid", `the relation has no column ${key.column}`);
    const sign = key.descending ? -1 : 1;
    rows = rows.sort((a, b) => {
      const x = shown(rel.columns[c], rel.rows[a][c]);
      const y = shown(rel.columns[c], rel.rows[b][c]);
      if (x === y) return 0;
      if (x === null) return 1;
      if (y === null) return -1;
      return sign * compare(x, y, numeric(rel.columns[c]));
    });
  }
  return { rel, rows, hits };
}

/** The cell a page carries: the text cut to the engine's 2,000 characters, a utc time also as display text. */
function gridCell(col: ColumnMeta, v: string | null): { text: string | null; display: string | null; failed: boolean; clipped: boolean } {
  const clipped = v !== null && v.length > 2000;
  const text = clipped ? v.slice(0, 2000) : v;
  return { text, display: isUtc(col) && text !== null ? shown(col, text) : null, failed: false, clipped };
}

const rowIdOf = (rel: MockRelation, index: number) => rel.firstId + index;

function findRow(rowId: number): { rel: MockRelation; row: Row } {
  for (const rel of relations()) {
    const index = rowId - rel.firstId;
    if (index >= 0 && index < rel.rows.length) return { rel, row: rel.rows[index] };
  }
  return fail("not_found", `no row ${rowId}`);
}

const clip = (text: string | null): [string | null, boolean] =>
  text !== null && text.length > 2000 ? [text.slice(0, 2000), true] : [text, false];

// ---------------------------------------------------------------------------------------------------- colour rules

function colourRulesFor(relation: string): ColourRule[] {
  if (relation === "mock_hayabusa") {
    // The engine's shape: ids `hayabusa-<level>`, names `Hayabusa level: <word>`, crit red, high orange, med amber, low green.
    const rule = (level: string, word: string, colour: ColourRule["colour"]): ColourRule => ({
      id: `hayabusa-${level}`, name: `Hayabusa level: ${word}`, enabled: true, colour, scope: "row",
      filter: { column: "Level", op: "eq", value: level, case_sensitive: false },
    });
    return [rule("crit", "critical", "red"), rule("high", "high", "orange"), rule("med", "medium", "amber"), rule("low", "low", "green")];
  }
  return [];
}

// ------------------------------------------------------------------------------------------------------- the table

function splitValue(text: string | null): ValuePart[] {
  if (text === null) return [{ key: null, value: "" }];
  if (text.includes("¦")) {
    const parts = text.split("¦").map((p) => p.trim());
    if (parts.length >= 2) {
      return parts.map((p) => {
        const at = p.indexOf(": ");
        return at < 0 ? { key: null, value: p } : { key: p.slice(0, at), value: p.slice(at + 2) };
      });
    }
  }
  return [{ key: null, value: text }];
}

const viewColumnsMissing = (doc: ViewDoc, names: Set<string>): ViewWarning[] => {
  const warnings: ViewWarning[] = [];
  for (const chip of doc.chips) {
    for (const leaf of leaves(chip.filter)) {
      if (!names.has(leaf.column)) {
        warnings.push({ part: "chip", name: leaf.column, message: `chip "${chip.label}" names the missing column ${leaf.column}` });
      }
    }
  }
  for (const key of doc.sort) {
    if (!names.has(key.column)) warnings.push({ part: "sort", name: key.column, message: `sort names the missing column ${key.column}` });
  }
  for (const column of doc.group_by) {
    if (!names.has(column)) warnings.push({ part: "group", name: column, message: `grouping names the missing column ${column}` });
  }
  for (const rule of doc.colour_rules) {
    for (const leaf of leaves(rule.filter)) {
      if (!names.has(leaf.column)) {
        warnings.push({ part: "colour_rule", name: leaf.column, message: `colour rule "${rule.name}" names the missing column ${leaf.column}` });
      }
    }
  }
  return warnings;
};

const globMatches = (pattern: string, name: string) =>
  new RegExp("^" + pattern.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*") + "$").test(name);

const analyst = () => `analyst:${settings.analyst ?? "analyst01"}`;

type Handlers = { [C in keyof CommandArgs]: (a: CommandArgs[C]) => Promise<CommandResult[C]> | CommandResult[C] };

const handlers: Handlers = {
  // case
  case_new: (a) => open(a.path, a.name),
  case_open: (a) => open(a.path, ""),
  case_close: () => {
    if (writing) fail("busy", "another change is running");
    current = null;
    generation += 1;
    slotQuery.clear();
    return null;
  },
  case_summary: (a) => read(a.slot, a.queryId, summary),

  // loading: the mock has no files; a load adds nothing but still runs as a job and announces a change
  load_run: (a) =>
    write(["sources"], () => ({ source_id: 3, relations: [], superseded: [], warnings: [], skipped: [] }), {
      kind: "load_run", label: a.path.split(/[\\/]/).pop() ?? a.path,
    }),
  load_file: (a) =>
    write(["sources"], () => ({ source_id: 3, relations: [], superseded: [], warnings: [], skipped: [] }), {
      kind: "load_file", label: a.path.split(/[\\/]/).pop() ?? a.path,
    }),

  // what the case holds
  columns: (a) => read(a.slot, a.queryId, () => relationNamed(a.relation).columns),
  sessions: (a) => read(a.slot, a.queryId, () => fx.sessions()),
  host_zones: (a) => read(a.slot, a.queryId, () => fx.hostZones()),
  zone_names: () => ["America/New_York", "Asia/Tokyo", "Etc/UTC", "Europe/London", "UTC"],
  display_zone_set: (a) =>
    write(["display_zone"], () => {
      theCase().displayZone = a.zone;
      return null;
    }),

  // the grid
  grid_page: (a) =>
    read(a.slot, a.queryId, () => {
      const sel = select(a.state);
      const names = a.state.columns ?? sel.rel.columns.map((c) => c.name);
      const cols = names.map((n) => sel.rel.columns.findIndex((c) => c.name === n)).filter((i) => i >= 0);
      const picked = sel.rows.slice(a.offset, a.offset + a.limit);
      const rules = a.options.rules;
      const ruleErrors: Array<[number, string]> = [];
      const usable = rules.map((r, i) => {
        const missing = leaves(r.filter).find((l) => l.column !== "_mark" && !sel.rel.columns.some((c) => c.name === l.column));
        if (missing && r.enabled) ruleErrors.push([i, `the relation has no column ${missing.column}`]);
        return r.enabled && !missing;
      });
      const marks = theCase().marks;
      return {
        columns: cols.map((i) => sel.rel.columns[i]),
        row_ids: picked.map((i) => rowIdOf(sel.rel, i)),
        marks: picked.map((i) => marks.get(rowIdOf(sel.rel, i)) ?? null),
        cells: picked.map((i) => cols.map((c) => gridCell(sel.rel.columns[c], sel.rel.rows[i][c]))),
        row_rule: picked.map((i) => {
          const at = rules.findIndex((r, n) => usable[n] && nodeMatches(r.filter, sel.rel, sel.rel.rows[i], marks.get(rowIdOf(sel.rel, i)) ?? null));
          return at < 0 ? null : at;
        }),
        rule_columns: rules.map((r) => [...new Set(leaves(r.filter).map((l) => l.column).filter((n) => n !== "_mark"))]),
        rule_errors: ruleErrors,
        search_hits: picked.map((i) => a.state.search.mode === "find" && sel.hits(sel.rel.rows[i])),
        truncated: a.offset + a.limit < sel.rows.length,
        zone: "UTC",
      };
    }),
  grid_counts: (a) =>
    read(a.slot, a.queryId, () => ({ rows: select(a.state).rows.length, total: relationNamed(a.state.relation).rows.length, failed_values: 0 })),
  grid_groups: (a) =>
    read(a.slot, a.queryId, () => {
      const sel = select(a.state);
      const level = a.parent.length;
      const idx = a.by.map((n) => sel.rel.columns.findIndex((c) => c.name === n));
      if (level >= a.by.length || idx.some((i) => i < 0)) fail("invalid", "grouping names a column the relation lacks");
      const timeIx = sel.rel.columns.findIndex((c) => c.name === sel.rel.info.primary_time);
      const marks = theCase().marks;
      const groups = new Map<string | null, GroupView>();
      for (const r of sel.rows) {
        const row = sel.rel.rows[r];
        if (a.parent.some((p, k) => row[idx[k]] !== p)) continue;
        const key = row[idx[level]];
        const g = groups.get(key) ?? { key, key_clipped: false, full_key: null, count: 0, first: null, last: null, suspicious: 0 };
        const [cut, clipped] = clip(key);
        g.key = cut;
        g.key_clipped = clipped;
        g.full_key = clipped ? key : null;
        g.count += 1;
        const t = timeIx >= 0 ? shown(sel.rel.columns[timeIx], row[timeIx]) : null;
        if (t !== null) {
          if (g.first === null || t < g.first) g.first = t;
          if (g.last === null || t > g.last) g.last = t;
        }
        if (marks.has(rowIdOf(sel.rel, r))) g.suspicious += 1;
        groups.set(key, g);
      }
      const all = [...groups.values()].sort((x, y) => y.count - x.count || String(x.key).localeCompare(String(y.key)));
      return { items: all.slice(a.offset, a.offset + a.limit), truncated: a.offset + a.limit < all.length };
    }),
  grid_histogram: (a) =>
    read(a.slot, a.queryId, () => {
      const sel = select(a.state);
      const name = a.column ?? sel.rel.info.primary_time;
      const ci = sel.rel.columns.findIndex((c) => c.name === name);
      if (ci < 0) fail("invalid", "the relation has no time column to bucket");
      const col = sel.rel.columns[ci];
      // Hour buckets (`YYYY-MM-DD HH`), merged into wider ones of whole hours when there are more than `maxBuckets`.
      const perHour = new Map<string, { count: number; suspicious: number }>();
      let untimed = 0;
      const marks = theCase().marks;
      for (const r of sel.rows) {
        const t = shown(col, sel.rel.rows[r][ci]);
        if (t === null) {
          untimed += 1;
          continue;
        }
        const b = perHour.get(t.slice(0, 13)) ?? { count: 0, suspicious: 0 };
        b.count += 1;
        if (marks.has(rowIdOf(sel.rel, r))) b.suspicious += 1;
        perHour.set(t.slice(0, 13), b);
      }
      const hours = [...perHour.entries()].sort(([x], [y]) => (x < y ? -1 : 1));
      const width = Math.max(1, Math.ceil(hours.length / Math.max(1, a.maxBuckets)));
      const buckets = [];
      for (let i = 0; i < hours.length; i += width) {
        const chunk = hours.slice(i, i + width);
        const first = chunk[0][0];
        const last = chunk[chunk.length - 1][0];
        buckets.push({
          start: `${first.replace(" ", "T")}:00:00.000000Z`, label: `${first}:00:00`, end_label: `${last}:59:59`, index: buckets.length,
          count: chunk.reduce((n, [, v]) => n + v.count, 0), suspicious: chunk.reduce((n, [, v]) => n + v.suspicious, 0),
        });
      }
      return { column: col.name, time_semantics: col.time_semantics, zone: "UTC", bucket_seconds: 3600 * width, buckets, slots: buckets.length, untimed };
    }),
  grid_values: (a) =>
    read(a.slot, a.queryId, () => {
      const sel = select(a.state, a.exceptChips);
      const ci = sel.rel.columns.findIndex((c) => c.name === a.column);
      if (ci < 0) fail("invalid", `the relation has no column ${a.column}`);
      const counts = new Map<string | null, number>();
      for (const r of sel.rows) {
        const v = shown(sel.rel.columns[ci], sel.rel.rows[r][ci]);
        if (a.contains && !(v ?? "").toLowerCase().includes(a.contains.toLowerCase())) continue;
        counts.set(v, (counts.get(v) ?? 0) + 1);
      }
      const all: ValueCount[] = [...counts].map(([value, count]) => {
        const [text, clipped] = clip(value);
        return { value: text, clipped, count };
      });
      all.sort((x, y) =>
        a.order === "commonest" ? y.count - x.count : a.order === "rarest" ? x.count - y.count : String(x.value).localeCompare(String(y.value)),
      );
      return { items: all.slice(a.offset, a.offset + a.limit), truncated: a.offset + a.limit < all.length };
    }),
  grid_empty_columns: (a) =>
    read(a.slot, a.queryId, () => {
      const sel = select(a.state);
      return sel.rel.columns.filter((_, c) => sel.rows.every((r) => (sel.rel.rows[r][c] ?? "") === "")).map((c) => c.name);
    }),
  grid_matches: (a) =>
    read(a.slot, a.queryId, () => {
      const sel = select(a.state);
      const all = a.state.search.mode === "find"
        ? sel.rows
            .map((r, offset) => ({ offset, r }))
            .filter(({ r }) => sel.hits(sel.rel.rows[r]))
            .map((m, i) => ({ ...m, ordinal: i + 1 }))
        : [];
      const none = a.from === null || a.from === undefined;
      const later = a.backward
        ? all.filter(({ offset }) => none || offset < a.from!).reverse()
        : all.filter(({ offset }) => none || offset > a.from!);
      return {
        items: later.slice(0, a.limit).map(({ offset, r, ordinal }) => ({ offset, row_id: rowIdOf(sel.rel, r), ordinal })),
        truncated: later.length > a.limit,
      };
    }),
  grid_match_count: (a) =>
    read(a.slot, a.queryId, () => {
      const sel = select(a.state);
      return a.state.search.mode === "find" ? sel.rows.filter((r) => sel.hits(sel.rel.rows[r])).length : 0;
    }),
  grid_row_offset: (a) =>
    read(a.slot, a.queryId, () => {
      const sel = select(a.state);
      const index = a.rowId - sel.rel.firstId;
      const at = sel.rows.indexOf(index);
      return index >= 0 && index < sel.rel.rows.length && at >= 0 ? at : null;
    }),
  grid_row_ids: (a) =>
    read(a.slot, a.queryId, () => {
      // As the engine: ranges merged where they overlap, counted before anything is read, refused past 10,000.
      const merged: Array<[number, number]> = [];
      for (const [from, to] of [...a.ranges].sort((x, y) => x[0] - y[0])) {
        if (to < from) fail("invalid", `a selected range ends before it starts (${from} to ${to})`);
        const last = merged[merged.length - 1];
        if (last && from <= last[1] + 1) last[1] = Math.max(last[1], to);
        else merged.push([from, to]);
      }
      const asked = merged.reduce((n, [from, to]) => n + (to - from + 1), 0);
      if (asked > 10_000) fail("invalid", `select at most 10,000 rows (you have ${asked.toLocaleString("en-US")})`);
      const sel = select(a.state);
      return merged.flatMap(([from, to]) => sel.rows.slice(from, to + 1).map((r) => rowIdOf(sel.rel, r)));
    }),
  grid_row_positions: (a) =>
    read(a.slot, a.queryId, () => {
      const sel = select(a.state);
      const at = new Map(sel.rows.map((r, i) => [rowIdOf(sel.rel, r), i]));
      return a.ids.map((id): [number, number | null] => [id, at.get(id) ?? null]);
    }),
  grid_export: async (a) => {
    const rows = await read(a.slot, a.queryId, () => select(a.state).rows.length);
    mockEmit("job-started", { kind: "export", label: a.path.split(/[\\/]/).pop() ?? a.path });
    mockEmit("job-finished", { kind: "export", ok: true, message: null });
    return { rows, path: a.path };
  },
  date_range_chip: (a) =>
    read(a.slot, a.queryId, (): Chip => {
      const rel = relationNamed(a.relation);
      const col = rel.columns.find((c) => c.name === a.column);
      if (!col || col.sql_type !== "TIMESTAMP") fail("invalid", `${a.column} is not a time column`);
      const stamp = /^\d{4}-\d\d-\d\d( \d\d:\d\d:\d\d)?$/;
      for (const v of [a.from, a.to]) if (v && !stamp.test(v)) fail("invalid", `not a date and time: ${v}`);
      if (!a.from && !a.to) fail("invalid", "a date range needs a start or an end");
      const filter: Leaf = a.from && a.to
        ? { column: a.column, op: "between", value: [a.from, a.to], case_sensitive: false }
        : { column: a.column, op: a.from ? "gte" : "lte", value: (a.from ?? a.to)!, case_sensitive: false };
      return { id: `range-${a.column}`, label: `${a.column}: ${a.from ?? "…"} to ${a.to ?? "…"}`, enabled: true, origin: "date_range", filter };
    }),
  default_colour_rules: (a) => read(a.slot, a.queryId, () => colourRulesFor(a.relation)),

  // one row
  row_view: (a) =>
    read(a.slot, a.queryId, () => {
      const { rel, row } = findRow(a.rowId);
      const hostIx = rel.columns.findIndex((c) => c.role === "host");
      const fileIx = rel.columns.findIndex((c) => c.name === rel.info.evidence_path_column);
      const outIx = rel.columns.findIndex((c) => c.role === "output_file");
      const cells: Array<[string, Cell]> = rel.columns.map((c, i) => [c.name, { text: row[i], failed: false, clipped: false }]);
      return {
        detail: {
          row_id: a.rowId, standing: "active", current_row: null, relation: rel.info.name, source_id: 1, host: row[hostIx],
          hash: a.rowId.toString(16).padStart(64, "0"), active: true, fields: cells, mark: theCase().marks.get(a.rowId) ?? null,
          marked_by: theCase().marks.has(a.rowId) ? "analyst:analyst01" : null, tags: [], notes: [],
          evidence_path: fileIx >= 0 ? row[fileIx] : null, output_file: row[outIx],
        },
        fields: rel.columns.map((meta, i) => ({ meta, text: row[i], display: isUtc(meta) && row[i] !== null ? `${shown(meta, row[i])} Z` : null, failed: false })),
        provenance: {
          host: row[hostIx], source_id: 1, run_id: row[0], evidence_path: fileIx >= 0 ? row[fileIx] : null,
          locator: ["row", String(a.rowId - rel.firstId + 2)], output_file: row[outIx], output_file_listed: true,
        },
        summary: row[rel.columns.findIndex((c) => c.name === "Message" || c.name === "Details")]?.slice(0, 300) || null, // the engine cuts at 300
        anchor: (() => {
          const time = row[rel.columns.findIndex((c) => c.name === rel.info.primary_time)];
          return time && row[hostIx] ? { host: row[hostIx]!, at: time } : null;
        })(),
        zone: "UTC",
      };
    }),
  cell_value: (a) =>
    read(a.slot, a.queryId, () => {
      const { rel, row } = findRow(a.rowId);
      const i = rel.columns.findIndex((c) => c.name === a.column);
      if (i < 0) fail("invalid", `the relation has no column ${a.column}`);
      return { text: row[i], parts: splitValue(row[i]) };
    }),
  around_totals: (a) =>
    read(a.slot, a.queryId, () => {
      const { rel } = findRow(a.rowId);
      // About one event every 17 seconds on this data; the real count is the engine's.
      return a.windows.map((seconds) => ({ seconds, rows: Math.min(rel.rows.length, 1 + 2 * Math.ceil(seconds / 17)) }));
    }),
  merged_page: (a) =>
    read(a.slot, a.queryId, () => {
      const marks = theCase().marks;
      const out: MergedView[] = [];
      for (const rel of relations()) {
        const timeIx = rel.columns.findIndex((c) => c.name === rel.info.primary_time);
        const hostIx = rel.columns.findIndex((c) => c.role === "host");
        const sumIx = rel.columns.findIndex((c) => c.name === "Message" || c.name === "Details");
        for (let i = 0; i < rel.rows.length; i++) {
          const row = rel.rows[i];
          const id = rowIdOf(rel, i);
          if ("find" in a.source) {
            const f = a.source.find;
            const needle = f.value.toLowerCase();
            const hit = row.some((v) => v !== null && (f.mode === "exact" ? v.toLowerCase() === needle : v.toLowerCase().includes(needle)));
            if (!hit || (f.hosts.length && !f.hosts.includes(row[hostIx] ?? ""))) continue;
          } else if ("around" in a.source) {
            if (row[hostIx] !== a.source.around.host) continue;
          } else {
            const mark = marks.get(id);
            if (!mark || !a.source.marked.marks.includes(mark)) continue;
          }
          out.push({
            row: {
              row_id: id, relation: rel.info.name, time: row[timeIx], time_semantics: rel.columns[timeIx].time_semantics,
              host: row[hostIx], summary: row[sumIx] ?? "", mark: marks.get(id) ?? null,
            },
            display_time: shown(rel.columns[timeIx], row[timeIx]),
          });
          if (out.length >= a.offset + a.limit + 1) break;
        }
      }
      return { items: out.slice(a.offset, a.offset + a.limit), truncated: out.length > a.offset + a.limit };
    }),

  // saved views and presets
  views_list: (a) =>
    read(a.slot, a.queryId, () => {
      const all = theCase().views.filter((v) => !a.relation || globMatches(v.pattern, a.relation));
      return { items: all.slice(a.offset, a.offset + a.limit), truncated: a.offset + a.limit < all.length, skipped: [] };
    }),
  views_matching: (a) =>
    read(a.slot, a.queryId, () => {
      const names = relations().map((r) => r.info.name).sort();
      const all = theCase()
        .views.map((view) => ({ view, relations: names.filter((n) => globMatches(view.pattern, n)) }))
        .filter((m) => m.relations.length > 0)
        .sort((x, y) => Number(x.view.kind === "preset") - Number(y.view.kind === "preset") || (x.view.name < y.view.name ? -1 : x.view.name > y.view.name ? 1 : 0));
      return { items: all.slice(a.offset, a.offset + a.limit), truncated: a.offset + a.limit < all.length, skipped: [] };
    }),
  view_save: (a) =>
    write(["views"], () => {
      const c = theCase();
      const saved: SavedView = {
        id: c.nextViewId++, name: a.name, kind: a.kind, pattern: a.pattern, doc: a.doc,
        created_by: analyst(), created_at: NOW, updated_by: analyst(), updated_at: NOW,
      };
      c.views.push(saved);
      return saved;
    }),
  view_delete: (a) =>
    write(["views"], () => {
      const c = theCase();
      if (!c.views.some((v) => v.id === a.id)) fail("not_found", `no saved view ${a.id}`);
      c.views = c.views.filter((v) => v.id !== a.id);
      return null;
    }),
  view_apply: (a) =>
    read(a.slot, a.queryId, () => {
      const view = theCase().views.find((v) => v.id === a.id) ?? fail("not_found", `no saved view ${a.id}`);
      const rel = relationNamed(a.relation);
      const names = new Set(rel.columns.map((c) => c.name));
      const warnings = viewColumnsMissing(view.doc, names);
      const missing = (chip: Chip) => leaves(chip.filter).some((l) => !names.has(l.column));
      return {
        state: {
          relation: a.relation, chips: view.doc.chips.map((c) => (missing(c) ? { ...c, enabled: false } : c)),
          search: view.doc.search, hosts: view.doc.hosts, sort: view.doc.sort.filter((k) => names.has(k.column)), columns: null,
        },
        group_by: view.doc.group_by.filter((n) => names.has(n)),
        layout: view.doc.layout,
        colour_rules: view.doc.colour_rules.map((r) => (leaves(r.filter).some((l) => !names.has(l.column)) ? { ...r, enabled: false } : r)),
        warnings,
      };
    }),
  view_export: (a) =>
    read(a.slot, a.queryId, () => {
      if (!theCase().views.some((v) => v.id === a.id)) fail("not_found", `no saved view ${a.id}`);
      return null;
    }),
  view_import: () =>
    write(["views"], () => {
      const c = theCase();
      const doc = fx.viewsList().items[0].doc;
      const saved: SavedView = {
        id: c.nextViewId++, name: "Imported view", kind: "view", pattern: "mock_*", doc,
        created_by: analyst(), created_at: NOW, updated_by: analyst(), updated_at: NOW,
      };
      c.views.push(saved);
      return saved;
    }),

  // settings (the file in the app's config folder, not the case: no `case-changed`)
  settings_get: () => ({ settings, damaged: null, actor: analyst() }),
  settings_update: (a) => {
    // The same refusal as the backend's `Settings::check`.
    if (a.settings.analyst !== null && !/^[A-Za-z0-9._-]{1,64}$/.test(a.settings.analyst))
      fail("invalid", "an analyst name is 1 to 64 characters of A-Z a-z 0-9 . _ -");
    // As the backend: the recent list is its own, so the one sent is ignored.
    settings = { ...a.settings, recent: settings.recent };
    return { settings, damaged: null, actor: analyst() };
  },
  recent_clear: () => {
    settings = { ...settings, recent: [] };
    return { settings, damaged: null, actor: analyst() };
  },
  recent_remove: (a) => {
    settings = { ...settings, recent: settings.recent.filter((p) => p !== a.path) };
    return { settings, damaged: null, actor: analyst() };
  },

  // cancelling
  query_cancel: (a) => {
    for (const id of a.queryIds) if ([...slotQuery.values()].includes(id)) cancelled.add(id);
    return null;
  },
};

/** Every command the mock answers: sorted, and (by the type of `handlers`) every command of the contract. */
export const MOCK_COMMANDS: readonly string[] = Object.keys(handlers).sort();

export async function mockCall<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const handler = (handlers as Record<string, ((a: never) => unknown) | undefined>)[cmd];
  if (!handler) return fail("internal", `the mock backend has no command ${cmd}`);
  return (await handler((args ?? {}) as never)) as T;
}
