// The grid's data: row blocks and counts for one `GridState`, fetched through `api.ts` and kept in a small external
// store the components read with `useSyncExternalStore`.
//
// Flow, which later tasks extend rather than change:
//  - The screen says which rows it draws (`want(first, last)`); the store fetches the 200-row blocks covering them
//    that it does not hold, one `grid_page` each, slot `tab:<id>:page:<offset>`. `grid_counts` (slot
//    `tab:<id>:counts`) runs beside them and never gates a row.
//  - A new state, zone or rules object, or `refresh()`, starts a new *version*: every in-flight id of the tab is
//    cancelled (`query_cancel`), the cache and counts are emptied, and an answer that left under an older version is
//    dropped when it arrives, whatever it holds. Cancelled and superseded answers are "not drawn", never errors.
//  - At most 12 blocks are kept; the least recently drawn go first (see `blocks.ts`).
//  - A block that fails (not superseded/interrupted) is not retried until the next version, and `onError` hears it.
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { cellValue, gridCounts, gridPage, isCmdError, newQueryId, queryCancel } from "../api";
import type { ColourRule } from "../contract/generated/ColourRule";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";
import type { GridCell } from "../contract/generated/GridCell";
import type { GridCounts } from "../contract/generated/GridCounts";
import type { GridPage } from "../contract/generated/GridPage";
import type { GridState } from "../contract/generated/GridState";
import type { MarkState } from "../contract/generated/MarkState";
import type { PageOptions } from "../contract/generated/PageOptions";
import { BLOCK_ROWS, BlockCache, MAX_BLOCKS, blockOf, blocksCovering } from "./blocks";

/** One row as the grid draws it. */
export interface GridRow {
  id: number;
  mark: MarkState | null;
  cells: GridCell[];
  /** Index into the page options' rules of the first rule the row matches (Task 21). */
  rule: number | null;
  /** Find mode: the search selects this row (Task 18). */
  hit: boolean;
}

/** `JSON.stringify` with object keys sorted recursively (arrays keep their order): equal values, equal keys. */
export function stableKey(value: unknown): string {
  const sorted = (v: unknown): unknown => {
    if (Array.isArray(v)) return v.map(sorted);
    if (v !== null && typeof v === "object") {
      return Object.fromEntries(
        Object.entries(v as Record<string, unknown>)
          .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
          .map(([k, x]) => [k, sorted(x)]),
      );
    }
    return v;
  };
  return JSON.stringify(sorted(value));
}

/** One Copy rows run: its own fetches, cancelled together when the view changes or a newer copy starts. */
export interface CopySession {
  /** False once the grid's state changed, the tab closed or a newer copy began: nothing may be written then. */
  alive(): boolean;
  /** True when a newer copy (not a change of view) ended this one. */
  replaced(): boolean;
  /** Block `b` of the current state (held or fetched on `tab:<id>:copy:<offset>`); null when no longer alive. */
  page(block: number): Promise<GridPage | null>;
  /** A cell's whole stored text through `cell_value`; null when no longer alive. */
  value(rowId: number, column: string): Promise<string | null>;
}

const isStale = (e: unknown) => isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case");

/** Same names in the same order and the same zoning: the header need not rebuild. */
const sameColumns = (a: ColumnMeta[], b: ColumnMeta[]) =>
  a.length === b.length && a.every((c, i) => c.name === b[i].name && c.time_semantics === b[i].time_semantics && c.sql_type === b[i].sql_type);

export class GridStore {
  private version = 0;
  private revision = 0;
  private readonly listeners = new Set<() => void>();
  private cache = new BlockCache<GridPage>(MAX_BLOCKS);
  private inflight = new Map<number, string>();
  private countsQuery: string | null = null;
  private failed = new Set<number>();
  private copyIds = new Set<string>();
  private copyEpoch = 0;
  private state: GridState | null = null;
  private options: PageOptions = { zone: "UTC", rules: [] };
  private enabled = false;
  private first = 0;
  private last = 0;

  /** The key of the state the cache holds rows for; `useGridData` hands out no row while it differs from the render's. */
  cacheKey: string | null = null;
  /** The columns of the latest page; kept across versions so the header does not blink. */
  columns: ColumnMeta[] | null = null;
  counts: GridCounts | null = null;
  /** The zone the latest page's `display` text is in. */
  zone: string | null = null;
  private onError: (e: unknown) => void = () => {};

  setErrorHandler(f: (e: unknown) => void): void {
    this.onError = f;
  }

  constructor(readonly tabId: string) {}

  subscribe = (l: () => void): (() => void) => {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  };
  getRevision = (): number => this.revision;
  private notify(): void {
    this.revision += 1;
    for (const l of [...this.listeners]) l();
  }

  /** A new version for `state` and `options`: cancels what is running, forgets what is held, fetches what is wanted. */
  reset(state: GridState, options: PageOptions, key: string): void {
    this.state = state;
    this.options = options;
    this.cacheKey = key;
    this.restart();
  }

  /** The same state again (the case changed under it). */
  refresh(): void {
    if (this.state) this.restart();
  }

  private restart(): void {
    this.cancelAll();
    this.version += 1;
    this.cache.clear();
    this.failed.clear();
    this.counts = null;
    this.notify();
    this.pump();
  }

  /** Reads happen only while the tab is shown. */
  setEnabled(enabled: boolean): void {
    this.enabled = enabled;
    if (enabled) this.pump();
  }

  /** The tab is going away (or being replaced by a strict-mode remount): cancel everything, answer nothing. */
  stop(): void {
    this.cancelAll();
    this.version += 1;
  }

  private cancelAll(): void {
    const ids = [...this.inflight.values()];
    if (this.countsQuery !== null) ids.push(this.countsQuery);
    ids.push(...this.copyIds);
    this.copyIds.clear();
    this.inflight.clear();
    this.countsQuery = null;
    if (ids.length > 0) queryCancel(ids).catch(() => {});
  }

  /** The rows the screen draws, inclusive. Counts the blocks as drawn, then fetches what is missing. */
  want(first: number, last: number): void {
    this.first = Math.max(0, first);
    this.last = Math.max(this.first, last);
    for (const b of blocksCovering(this.first, this.last)) this.cache.touch(b);
    this.pump();
  }

  private pump(): void {
    if (!this.enabled || !this.state) return;
    const wanted = blocksCovering(this.first, this.last);
    // Blocks nobody draws any more are not worth the database's time.
    const drop = [...this.inflight.entries()].filter(([b]) => !wanted.includes(b));
    if (drop.length > 0) {
      for (const [b] of drop) this.inflight.delete(b);
      queryCancel(drop.map(([, id]) => id)).catch(() => {});
    }
    if (this.counts === null && this.countsQuery === null) this.fetchCounts();
    for (const b of wanted) {
      if (!this.cache.has(b) && !this.inflight.has(b) && !this.failed.has(b)) this.fetchBlock(b);
    }
  }

  private fetchBlock(block: number): void {
    const state = this.state!;
    const version = this.version;
    const id = newQueryId();
    const offset = block * BLOCK_ROWS;
    this.inflight.set(block, id);
    gridPage(state, BLOCK_ROWS, offset, this.options, `tab:${this.tabId}:page:${offset}`, id).then(
      (page) => {
        if (version !== this.version) return; // an answer for an older state is never drawn
        if (this.inflight.get(block) === id) this.inflight.delete(block);
        else return; // cancelled while it was on its way
        this.cache.set(block, page, new Set(blocksCovering(this.first, this.last)));
        if (!this.columns || !sameColumns(this.columns, page.columns)) this.columns = page.columns;
        this.zone = page.zone;
        this.notify();
      },
      (e: unknown) => {
        if (version !== this.version) return;
        if (this.inflight.get(block) !== id) return; // cancelled (and maybe asked again): whatever it says is obsolete
        this.inflight.delete(block);
        if (isStale(e)) return;
        this.failed.add(block);
        this.onError(e);
        this.notify();
      },
    );
  }

  private fetchCounts(): void {
    const version = this.version;
    const id = newQueryId();
    this.countsQuery = id;
    gridCounts(this.state!, `tab:${this.tabId}:counts`, id).then(
      (counts) => {
        if (version !== this.version) return;
        this.countsQuery = null;
        this.counts = counts;
        this.notify();
      },
      (e: unknown) => {
        if (version !== this.version) return;
        this.countsQuery = null;
        if (!isStale(e)) this.onError(e);
      },
    );
  }

  /**
   * Starts a Copy rows run. Its queries get ids the store knows, so a state change, a tab close or a newer copy
   * cancels them; every answer is checked against the version it started under before it is handed back.
   */
  beginCopy(): CopySession {
    const older = [...this.copyIds];
    this.copyIds.clear();
    if (older.length > 0) queryCancel(older).catch(() => {});
    const epoch = ++this.copyEpoch;
    const version = this.version;
    const state = this.state;
    const options = this.options;
    const alive = () => this.version === version && this.copyEpoch === epoch;
    const run = async <T,>(start: (id: string) => Promise<T>): Promise<T | null> => {
      const id = newQueryId();
      this.copyIds.add(id);
      try {
        const r = await start(id);
        return alive() ? r : null;
      } catch (e) {
        if (!alive()) return null;
        throw e;
      } finally {
        this.copyIds.delete(id);
      }
    };
    return {
      alive,
      replaced: () => this.copyEpoch !== epoch && this.version === version,
      page: async (block) => {
        if (!alive() || !state) return null;
        const held = this.cache.peek(block);
        if (held) return held;
        const offset = block * BLOCK_ROWS;
        return run((id) => gridPage(state, BLOCK_ROWS, offset, options, `tab:${this.tabId}:copy:${offset}`, id));
      },
      value: async (rowId, column) => {
        if (!alive()) return null;
        const v = await run((id) => cellValue(rowId, column, `tab:${this.tabId}:copy:value`, id));
        return v === null ? null : (v.text ?? "");
      },
    };
  }

  /** Row `index` of the current state, or null while its block is not here. */
  rowAt(index: number): GridRow | null {
    const page = this.cache.peek(blockOf(index));
    const k = index - blockOf(index) * BLOCK_ROWS;
    if (!page || index < 0 || k >= page.row_ids.length) return null;
    return {
      id: page.row_ids[k], mark: page.marks[k] ?? null, cells: page.cells[k], rule: page.row_rule[k] ?? null,
      hit: page.search_hits[k] ?? false,
    };
  }

  /** The text of every loaded cell of `column` (for best fit); nothing while the column is not among the fetched ones. */
  loadedTexts(column: string): string[] {
    const out: string[] = [];
    for (const b of this.cache.keys()) {
      const page = this.cache.peek(b)!;
      const at = page.columns.findIndex((c) => c.name === column);
      if (at < 0) continue;
      for (const row of page.cells) {
        const cell = row[at];
        if (cell) out.push(cell.display ?? cell.text ?? "");
      }
    }
    return out;
  }

  /** How many rows the scrollbar spans: the count when it is here, else what the loaded blocks show. */
  totalRows(): number {
    if (this.counts) return this.counts.rows;
    let n = 0;
    for (const b of this.cache.keys()) n = Math.max(n, b * BLOCK_ROWS + this.cache.peek(b)!.row_ids.length);
    return n;
  }
}

const NO_RULES: ColourRule[] = [];

export interface GridDataArgs {
  tabId: string;
  /** Must keep its identity between renders unless it changes: a new object is a new query. */
  state: GridState;
  zone: string;
  rules?: ColourRule[];
  /** False while the tab is hidden: no read starts. */
  enabled: boolean;
  onError(e: unknown): void;
}

export interface GridData {
  columns: ColumnMeta[] | null;
  counts: GridCounts | null;
  total: number;
  zone: string | null;
  rowAt(index: number): GridRow | null;
  want(first: number, last: number): void;
  refresh(): void;
  /** See `GridStore.loadedTexts`. */
  loadedTexts(column: string): string[];
  /** See `GridStore.beginCopy`. */
  beginCopy(): CopySession;
  /** Rises whenever something drawn may have changed. */
  revision: number;
}

/**
 * Resets are keyed on `stableKey([state, zone, rules])` (JSON with sorted keys), so an equal-but-new `state` object is not a
 * new query. The key is computed in render, and `rowAt` / `counts` answer nothing while the store's cache belongs to
 * another key: a changed state never paints the old state's rows, not even for the frame before the reset effect runs.
 */
export function useGridData({ tabId, state, zone, rules = NO_RULES, enabled, onError }: GridDataArgs): GridData {
  const [store] = useState(() => new GridStore(tabId));
  const key = useMemo(() => stableKey([state, zone, rules]), [state, zone, rules]);
  const latest = useRef({ state, zone, rules });
  useEffect(() => {
    latest.current = { state, zone, rules };
  });
  useEffect(() => {
    store.setErrorHandler(onError);
  }, [store, onError]);
  useEffect(() => {
    const l = latest.current;
    store.reset(l.state, { zone: l.zone, rules: l.rules }, key);
    return () => store.stop();
  }, [store, key]);
  useEffect(() => {
    store.setEnabled(enabled);
  }, [store, enabled]);
  const revision = useSyncExternalStore(store.subscribe, store.getRevision);
  const fresh = store.cacheKey === key;
  const want = useCallback((first: number, last: number) => store.want(first, last), [store]);
  const refresh = useCallback(() => store.refresh(), [store]);
  const beginCopy = useCallback(() => store.beginCopy(), [store]);
  const loadedTexts = useCallback((column: string) => (store.cacheKey === key ? store.loadedTexts(column) : []), [store, key]);
  const rowAt = useCallback((i: number) => (store.cacheKey === key ? store.rowAt(i) : null), [store, key]);
  return {
    columns: store.columns, counts: fresh ? store.counts : null, total: store.totalRows(), zone: store.zone, rowAt, want,
    refresh, beginCopy, loadedTexts, revision,
  };
}
