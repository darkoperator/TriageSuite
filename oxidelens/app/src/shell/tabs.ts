// The open tabs of one case and where each is scrolled, kept in the webview's localStorage under
// `oxl.tabs.v1.<case_id>` (per machine, never in the case or the settings). Every storage call is in try/catch: storage
// can be absent, full or throwing, and the shell then simply starts with the Overview tab. What is read back is
// validated, because the file is not trusted.

import type { Chip } from "../contract/generated/Chip";
import type { ChipOrigin } from "../contract/generated/ChipOrigin";
import type { FilterNode } from "../contract/generated/FilterNode";
import type { GridState } from "../contract/generated/GridState";
import type { Leaf } from "../contract/generated/Leaf";
import type { MergedSource } from "../contract/generated/MergedSource";
import type { Op } from "../contract/generated/Op";
import type { SearchMode } from "../contract/generated/SearchMode";
import type { SortKey } from "../contract/generated/SortKey";
import { MARK_COLUMN, MARK_VALUES, MAX_CHIPS, MAX_CHIP_DEPTH, MAX_CHIP_LABEL } from "../grid/chips";

/** What a tab opened from a group starts with: the parent's state plus the group's chips, and the levels still to group by. */
export interface TabSeed {
  state: GridState;
  by: string[];
}

/** The four case sheets, pinned after the Overview. */
export type SheetKind = "timeline" | "hosts" | "accounts" | "iocs";
export const SHEETS: readonly { kind: SheetKind; title: string }[] = [
  { kind: "timeline", title: "Timeline" },
  { kind: "hosts", title: "Hosts" },
  { kind: "accounts", title: "Accounts" },
  { kind: "iocs", title: "IOCs" },
];
export const sheetTabId = (kind: SheetKind) => `sheet:${kind}`;

export interface Tab {
  id: string;
  kind: "overview" | "sheet" | "dataset" | "merged";
  /** A pinned case sheet: which one. It is never saved (the sheets always exist) and never closed. */
  sheet?: SheetKind;
  /** The relation a dataset tab shows; `null` for the Overview. */
  relation: string | null;
  /** A tab opened from a group: its own title and starting state. It is saved and restored with the case's tabs. */
  title?: string;
  seed?: TabSeed;
  /** A merged tab (the rows around one row, opened from the row detail): what it lists. It is not saved with the tabs. */
  merged?: { source: MergedSource };
  /** A tab opened to show one row (the *open* link's "unfiltered" tab): the row it scrolls to when it first opens. Not saved. */
  reveal?: number;
}

export interface TabsState {
  /** The Overview, the four pinned case sheets, then dataset tabs in the order they were opened. */
  tabs: Tab[];
  active: string;
}

export const OVERVIEW_ID = "overview";
const OVERVIEW: Tab = { id: OVERVIEW_ID, kind: "overview", relation: null };

export const storageKey = (caseId: string) => `oxl.tabs.v1.${caseId}`;
export const datasetTabId = (relation: string) => `dataset:${relation}`;

const PINNED: Tab[] = SHEETS.map((s) => ({ id: sheetTabId(s.kind), kind: "sheet", relation: null, sheet: s.kind }));

/** The Overview and the sheets stay open for good. */
export const isPinned = (t: Tab): boolean => t.kind === "overview" || t.kind === "sheet";

export const initialTabs = (): TabsState => ({ tabs: [OVERVIEW, ...PINNED], active: OVERVIEW_ID });

export function openDataset(s: TabsState, relation: string): TabsState {
  const id = datasetTabId(relation);
  if (s.tabs.some((t) => t.id === id)) return { ...s, active: id };
  return { tabs: [...s.tabs, { id, kind: "dataset", relation }], active: id };
}

/** Opens a tab made by the grid (a group's rows); an equal id switches to the tab already open. */
export function openScoped(s: TabsState, tab: Tab): TabsState {
  if (s.tabs.some((t) => t.id === tab.id)) return { ...s, active: tab.id };
  return { tabs: [...s.tabs, tab], active: tab.id };
}

export function activate(s: TabsState, id: string): TabsState {
  return s.tabs.some((t) => t.id === id) ? { ...s, active: id } : s;
}

/** Closes a dataset tab (the Overview and the sheets stay); the active tab becomes its left neighbour when it was the one closed. */
export function closeTab(s: TabsState, id: string): TabsState {
  const ix = s.tabs.findIndex((t) => t.id === id);
  if (ix < 0 || isPinned(s.tabs[ix])) return s;
  const tabs = s.tabs.filter((t) => t.id !== id);
  return { tabs, active: s.active === id ? tabs[ix - 1].id : s.active };
}

/** The tab `delta` places from the active one, wrapping round. */
export function neighbour(s: TabsState, delta: number): string {
  const ix = s.tabs.findIndex((t) => t.id === s.active);
  return s.tabs[(ix + delta + s.tabs.length) % s.tabs.length].id;
}

export interface Restored {
  state: TabsState;
  scroll: Record<string, number>;
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const isStrings = (v: unknown): v is string[] => Array.isArray(v) && v.every((x) => typeof x === "string");

// Runtime lists of the generated unions. A `Record` over the union must name every variant, so a variant added in Rust
// (and regenerated) breaks the build here until the list is brought up to date.
const OPS: Record<Op, true> = {
  eq: true, ne: true, contains: true, not_contains: true, starts_with: true, ends_with: true, regex: true, in: true, not_in: true,
  between: true, gt: true, gte: true, lt: true, lte: true, is_empty: true, not_empty: true, conversion_failed: true,
};
const ORIGINS: Record<ChipOrigin, true> = {
  filter_row: true, text: true, values: true, date_range: true, cell: true, histogram: true, advanced: true, group: true, preset: true,
};
const MODES: Record<SearchMode, true> = { filter: true, find: true };
const has = <T extends string>(table: Record<T, true>, v: unknown): v is T => typeof v === "string" && Object.hasOwn(table, v);

const scalar = (v: unknown): v is string | number | boolean => typeof v === "string" || typeof v === "number" || typeof v === "boolean";

/** A leaf's value in the shape the engine takes for its operator (filter/compile.rs `leaf_value`); null when it is not. */
function leafValue(op: Op, v: unknown): { ok: true; value: Leaf["value"] } | null {
  switch (op) {
    case "is_empty": case "not_empty": case "conversion_failed":
      return v === null ? { ok: true, value: null } : null;
    case "in": case "not_in":
      return isStrings(v) ? { ok: true, value: [...v] } : null;
    case "gt": case "gte": case "lt": case "lte":
      return scalar(v) ? { ok: true, value: v } : null;
    case "between":
      return Array.isArray(v) && v.length === 2 && scalar(v[0]) && scalar(v[1]) ? { ok: true, value: [v[0], v[1]] } : null;
    default:
      return typeof v === "string" ? { ok: true, value: v } : null;
  }
}

/** A filter node of at most `depth` levels, rebuilt from validated fields; null when it is not one. */
function readFilter(n: unknown, depth: number): FilterNode | null {
  if (depth < 1 || !isObj(n)) return null;
  const many = (list: unknown[]): FilterNode[] | null => {
    const out: FilterNode[] = [];
    for (const c of list) {
      const r = readFilter(c, depth - 1);
      if (r === null) return null;
      out.push(r);
    }
    return out;
  };
  if (Array.isArray(n.and)) {
    const r = many(n.and);
    return r && { and: r };
  }
  if (Array.isArray(n.or)) {
    const r = many(n.or);
    return r && { or: r };
  }
  if ("not" in n) {
    const r = readFilter(n.not, depth - 1);
    return r && { not: r };
  }
  if (typeof n.column !== "string" || !has(OPS, n.op) || typeof n.case_sensitive !== "boolean") return null;
  const value = leafValue(n.op, n.value);
  if (value === null) return null;
  // The `_mark` pseudo-column takes only eq, ne and in with suspicious or none (filter/compile.rs). Rules that need the
  // catalog (a typed column's ordered operators, an unknown column) cannot be checked at load: the engine answers
  // `invalid` when the restored tab first queries, and the grid shows it.
  if (n.column === MARK_COLUMN) {
    const marks = n.op === "in" ? value.value : n.op === "eq" || n.op === "ne" ? [value.value] : null;
    if (!Array.isArray(marks) || !marks.every((m) => (MARK_VALUES as readonly unknown[]).includes(m))) return null;
  }
  return { column: n.column, op: n.op, value: value.value, case_sensitive: n.case_sensitive };
}

/** A saved seed, built from validated fields only (nothing of the stored object is passed on); null when it is not one. */
function validSeed(v: unknown, relation: string): TabSeed | null {
  if (!isObj(v) || !isStrings(v.by) || v.by.length > 4 || new Set(v.by).size !== v.by.length) return null;
  const st = v.state;
  if (!isObj(st) || st.relation !== relation || !Array.isArray(st.chips) || st.chips.length > MAX_CHIPS) return null;
  const ids = new Set<string>();
  const chips: Chip[] = [];
  for (const c of st.chips) {
    if (!isObj(c) || typeof c.id !== "string" || !/^[A-Za-z0-9_-]{1,64}$/.test(c.id) || ids.has(c.id)) return null;
    ids.add(c.id);
    if (typeof c.label !== "string" || [...c.label].length > MAX_CHIP_LABEL || typeof c.enabled !== "boolean" || !has(ORIGINS, c.origin)) return null;
    const filter = readFilter(c.filter, MAX_CHIP_DEPTH);
    if (filter === null) return null;
    chips.push({ id: c.id, label: c.label, enabled: c.enabled, origin: c.origin, filter });
  }
  const se = st.search;
  if (!isObj(se) || typeof se.text !== "string" || typeof se.regex !== "boolean" || typeof se.case_sensitive !== "boolean" || !has(MODES, se.mode)) return null;
  if (!isStrings(st.hosts)) return null;
  const sort: SortKey[] = [];
  if (!Array.isArray(st.sort)) return null;
  for (const k of st.sort) {
    if (!isObj(k) || typeof k.column !== "string" || typeof k.descending !== "boolean") return null;
    sort.push({ column: k.column, descending: k.descending });
  }
  if (st.columns !== null && !isStrings(st.columns)) return null;
  return {
    by: [...v.by],
    state: {
      relation,
      chips,
      search: { text: se.text, regex: se.regex, case_sensitive: se.case_sensitive, mode: se.mode },
      hosts: [...st.hosts],
      sort,
      columns: st.columns === null ? null : [...(st.columns as string[])],
    },
  };
}

/** The saved tabs of a case, keeping only datasets that still exist; the default when nothing usable is saved. */
export function loadTabs(caseId: string, relations: readonly string[]): Restored {
  const empty: Restored = { state: initialTabs(), scroll: {} };
  try {
    const raw = localStorage.getItem(storageKey(caseId));
    if (raw === null) return empty;
    const doc: unknown = JSON.parse(raw);
    if (typeof doc !== "object" || doc === null) return empty;
    const d = doc as Record<string, unknown>;
    if (d.v !== 1 || !Array.isArray(d.tabs)) return empty;
    const known = new Set(relations);
    let state = initialTabs();
    for (const t of d.tabs) {
      const o = isObj(t) ? t : {};
      const rel = o.relation;
      if (typeof rel !== "string" || !known.has(rel)) continue;
      if (o.seed === undefined) {
        state = openDataset(state, rel);
        continue;
      }
      const seed = validSeed(o.seed, rel);
      if (seed === null || typeof o.id !== "string" || o.id.length > 100 || typeof o.title !== "string" || o.title.length > 300) continue;
      state = openScoped(state, { id: o.id, kind: "dataset", relation: rel, title: o.title, seed });
    }
    state = { ...state, active: state.tabs.some((t) => t.id === d.active) ? (d.active as string) : OVERVIEW_ID };
    const scroll: Record<string, number> = {};
    if (typeof d.scroll === "object" && d.scroll !== null) {
      for (const [id, n] of Object.entries(d.scroll)) {
        if (state.tabs.some((t) => t.id === id) && typeof n === "number" && Number.isFinite(n) && n >= 0) scroll[id] = n;
      }
    }
    return { state, scroll };
  } catch {
    return empty;
  }
}

export function saveTabs(caseId: string, s: TabsState, scroll: Record<string, number>): void {
  try {
    const kept: Record<string, number> = {};
    for (const t of s.tabs) if (t.id in scroll) kept[t.id] = scroll[t.id];
    const doc = {
      v: 1,
      tabs: s.tabs
        .filter((t) => t.kind === "dataset")
        .map((t) => (t.seed ? { relation: t.relation, id: t.id, title: t.title, seed: t.seed } : { relation: t.relation })),
      active: s.active,
      scroll: kept,
    };
    localStorage.setItem(storageKey(caseId), JSON.stringify(doc));
  } catch {
    // Storage is a convenience: the tabs just are not remembered.
  }
}

/**
 * Scroll offsets by tab id. They change at scroll speed, so they are not React state: `set` records the offset and
 * calls `quiet` 250 ms after the last change, when the shell saves the tabs.
 */
export class ScrollStore {
  offsets: Record<string, number> = {};
  private timer: ReturnType<typeof setTimeout> | null = null;

  get(id: string): number {
    return this.offsets[id] ?? 0;
  }

  set(id: string, offset: number, quiet: () => void): void {
    this.offsets[id] = offset;
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(quiet, 250);
  }

  replace(offsets: Record<string, number>): void {
    this.offsets = offsets;
  }

  dispose(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }
}
