// The open tabs of one case and where each is scrolled, kept in the webview's localStorage under
// `oxl.tabs.v1.<case_id>` (per machine, never in the case or the settings). Every storage call is in try/catch: storage
// can be absent, full or throwing, and the shell then simply starts with the Overview tab. What is read back is
// validated, because the file is not trusted.

export interface Tab {
  id: string;
  kind: "overview" | "dataset";
  /** The relation a dataset tab shows; `null` for the Overview. */
  relation: string | null;
}

export interface TabsState {
  /** The Overview first, then dataset tabs in the order they were opened. */
  tabs: Tab[];
  active: string;
}

export const OVERVIEW_ID = "overview";
const OVERVIEW: Tab = { id: OVERVIEW_ID, kind: "overview", relation: null };

export const storageKey = (caseId: string) => `oxl.tabs.v1.${caseId}`;
export const datasetTabId = (relation: string) => `dataset:${relation}`;

export const initialTabs = (): TabsState => ({ tabs: [OVERVIEW], active: OVERVIEW_ID });

export function openDataset(s: TabsState, relation: string): TabsState {
  const id = datasetTabId(relation);
  if (s.tabs.some((t) => t.id === id)) return { ...s, active: id };
  return { tabs: [...s.tabs, { id, kind: "dataset", relation }], active: id };
}

export function activate(s: TabsState, id: string): TabsState {
  return s.tabs.some((t) => t.id === id) ? { ...s, active: id } : s;
}

/** Closes a dataset tab (the Overview stays); the active tab becomes its left neighbour when it was the one closed. */
export function closeTab(s: TabsState, id: string): TabsState {
  const ix = s.tabs.findIndex((t) => t.id === id);
  if (ix <= 0) return s;
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
      const rel = typeof t === "object" && t !== null ? (t as Record<string, unknown>).relation : undefined;
      if (typeof rel === "string" && known.has(rel)) state = openDataset(state, rel);
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
      tabs: s.tabs.filter((t) => t.kind === "dataset").map((t) => ({ relation: t.relation })),
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
