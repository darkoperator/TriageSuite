// The grouped grid's data: a flattened tree. `GroupStore` holds, per *path* (the keys of the enclosing levels, whole keys,
// `null` for an empty cell), the groups of the next level and, for an expanded leaf-level group, its rows. `flatten`
// turns what is held plus what is expanded into the list the virtual list draws: group header rows, the data rows of an
// expanded leaf group, a "more" row where a page is not the end, and a status row while something loads or failed.
//
// Fetching is driven by what is expanded and nothing else (`pump` walks the expanded tree and starts what is missing, at
// most `MAX_PARALLEL` at a time). Rules, the same as the flat grid's store (`useGridData.ts`):
//  - a *version* is bumped by every new state, zone or grouping, by `refresh` and by `stop`; every in-flight id is
//    cancelled (`query_cancel`) and everything held is dropped. An answer is dropped when its version is not current or
//    when its in-flight entry is no longer its own id (checked after the await, never before);
//  - what is expanded survives a version (a filter change re-reads the open groups); a change of the grouping columns
//    clears it, and so does Collapse all;
//  - a failure that is not superseded/interrupted marks the path failed for this version (no retry loop) and `onError`
//    hears it; superseded/interrupted answers are never errors and never drawn;
//  - `useGroupTree` keys the store on the canonical key of (state, by, zone) computed in render, and lists nothing while
//    the store's key is another one, so the first render under a new state paints no old group.
import { useCallback, useLayoutEffect, useMemo, useState, useSyncExternalStore } from "react";
import { gridGroups, gridPage, isCmdError, newQueryId, queryCancel } from "../api";
import type { GridPage } from "../contract/generated/GridPage";
import type { GridState } from "../contract/generated/GridState";
import type { GroupView } from "../contract/generated/GroupView";
import type { PageOptions } from "../contract/generated/PageOptions";
import { MAX_CHIPS, groupChips } from "./chips";
import { stableKey } from "./useGridData";

export const MAX_GROUP_BY = 4;
/** Groups and rows are read this many at a time. */
export const GROUP_PAGE = 200;
/** Expand all opens at most this many of the first level's groups. */
export const MAX_EXPAND_ALL = 200;
const MAX_PARALLEL = 6;

/** A group's identity at its level: the whole key (`full_key ?? key`); an empty cell is `null`. */
export type Key = string | null;
export const keyOf = (g: GroupView): Key => {
  const k = g.full_key ?? g.key;
  return k === "" ? null : k;
};
export const pathId = (path: readonly Key[]): string => JSON.stringify(path);

interface Level {
  path: Key[];
  groups: GroupView[];
  more: boolean;
}
interface Leaf {
  path: Key[];
  pages: GridPage[];
  more: boolean;
}

export type Item =
  | { kind: "group"; id: string; depth: number; path: Key[]; group: GroupView; open: boolean }
  | { kind: "row"; id: string; depth: number; page: GridPage; k: number }
  | { kind: "more-groups"; id: string; depth: number; path: Key[]; busy: boolean }
  | { kind: "more-rows"; id: string; depth: number; path: Key[]; busy: boolean }
  | { kind: "status"; id: string; depth: number; text: string };

const isStale = (e: unknown) => isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case");
const describe = (e: unknown) => (isCmdError(e) ? e.message : e instanceof Error ? e.message : String(e));
const under = (parent: readonly Key[], path: readonly Key[]) => parent.length <= path.length && parent.every((k, i) => k === path[i]);
const rowsHeld = (l: Leaf | undefined) => (l ? l.pages.reduce((n, p) => n + p.row_ids.length, 0) : 0);

interface Flight {
  id: string;
  path: Key[];
}

export class GroupStore {
  private version = 0;
  private revision = 0;
  private readonly listeners = new Set<() => void>();
  private state: GridState | null = null;
  private by: string[] = [];
  private options: PageOptions = { zone: "UTC", rules: [] };
  private enabled = false;
  private readonly expanded = new Map<string, Key[]>();
  private levels = new Map<string, Level>();
  private leaves = new Map<string, Leaf>();
  private inflight = new Map<string, Flight>();
  private failed = new Map<string, string>();
  private slots = new Map<string, number>();
  private onError: (e: unknown) => void = () => {};
  /** The key of the (state, by, zone) the held groups belong to. */
  cacheKey: string | null = null;

  constructor(readonly tabId: string) {}

  setErrorHandler(f: (e: unknown) => void): void {
    this.onError = f;
  }
  subscribe = (l: () => void): (() => void) => {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  };
  getRevision = (): number => this.revision;
  private notify(): void {
    this.revision += 1;
    for (const l of [...this.listeners]) l();
  }

  /** A new version for these arguments. A different grouping closes everything. */
  reset(state: GridState, by: string[], zone: string, key: string): void {
    if (stableKey(by) !== stableKey(this.by)) this.expanded.clear();
    this.state = state;
    this.by = by;
    this.options = { zone, rules: [] };
    this.cacheKey = key;
    this.restart();
  }

  /** The same arguments again (the case changed under them). */
  refresh(): void {
    if (this.state) this.restart();
  }

  private restart(): void {
    this.cancel(() => true);
    this.version += 1;
    this.levels.clear();
    this.leaves.clear();
    this.failed.clear();
    this.notify();
    this.pump();
  }

  setEnabled(enabled: boolean): void {
    this.enabled = enabled;
    if (enabled) this.pump();
  }

  stop(): void {
    this.cancel(() => true);
    this.version += 1;
  }

  /** Cancels the in-flight reads that `pick` names, forgetting them so their answers are dropped on arrival. */
  private cancel(pick: (f: Flight, key: string) => boolean): void {
    const ids: string[] = [];
    for (const [k, f] of [...this.inflight]) {
      if (!pick(f, k)) continue;
      ids.push(f.id);
      this.inflight.delete(k);
    }
    if (ids.length > 0) queryCancel(ids).catch(() => {});
  }

  private slot(kind: "groups" | "rows", pid: string): string {
    const k = `${kind}:${pid}`;
    let n = this.slots.get(k);
    if (n === undefined) {
      n = this.slots.size;
      this.slots.set(k, n);
    }
    return `tab:${this.tabId}:${kind}:${n}`;
  }

  // ---- what the analyst does ---------------------------------------------------------------------------------------
  toggle(path: Key[]): void {
    const pid = pathId(path);
    if (this.expanded.has(pid)) {
      this.expanded.delete(pid);
      this.dropUnder(path);
    } else {
      this.expanded.set(pid, path);
    }
    this.notify();
    this.pump();
  }

  /** Opens the first level's groups held so far, at most `MAX_EXPAND_ALL` of them. */
  expandAll(): void {
    const root = this.levels.get(pathId([]));
    if (!root) return;
    for (const g of root.groups.slice(0, MAX_EXPAND_ALL)) {
      const path = [keyOf(g)];
      this.expanded.set(pathId(path), path);
    }
    this.notify();
    this.pump();
  }

  /** Closes everything and drops every fetched row and nested level; the first level stays. */
  collapseAll(): void {
    this.expanded.clear();
    this.cancel((f) => f.path.length > 0);
    const root = pathId([]);
    for (const k of [...this.levels.keys()]) if (k !== root) this.levels.delete(k);
    this.leaves.clear();
    for (const k of [...this.failed.keys()]) if (k !== `g:${root}`) this.failed.delete(k);
    this.notify();
  }

  /** The "… more groups" / "… more rows" row: reads the next page of that path. */
  more(kind: "groups" | "rows", path: Key[]): void {
    if (!this.enabled || !this.state) return;
    const pid = pathId(path);
    if (kind === "groups") {
      const l = this.levels.get(pid);
      if (!l || !l.more) return;
      this.failed.delete(`g:${pid}`);
      this.fetchGroups(path, l.groups.length);
    } else {
      const l = this.leaves.get(pid);
      if (!l || !l.more) return;
      this.failed.delete(`r:${pid}`);
      this.fetchRows(path, rowsHeld(l));
    }
    this.notify();
  }

  private dropUnder(path: readonly Key[]): void {
    this.cancel((f) => under(path, f.path));
    for (const [k, l] of [...this.levels]) if (under(path, l.path)) this.levels.delete(k);
    for (const [k, l] of [...this.leaves]) if (under(path, l.path)) this.leaves.delete(k);
    for (const k of [...this.failed.keys()]) if (under(path, JSON.parse(k.slice(2)) as Key[])) this.failed.delete(k);
    for (const [k, p] of [...this.expanded]) if (under(path, p)) this.expanded.delete(k);
  }

  // ---- fetching ----------------------------------------------------------------------------------------------------
  /** Starts what the expanded tree is missing, in the order it is drawn, within the parallel limit. */
  private pump(): void {
    if (!this.enabled || !this.state || this.by.length === 0) return;
    const want: { kind: "g" | "r"; path: Key[] }[] = [];
    const walk = (path: Key[]): void => {
      const level = this.levels.get(pathId(path));
      if (!level) {
        want.push({ kind: "g", path });
        return;
      }
      for (const g of level.groups) {
        const child = [...path, keyOf(g)];
        if (!this.expanded.has(pathId(child))) continue;
        if (child.length < this.by.length) walk(child);
        else if (!this.leaves.has(pathId(child))) want.push({ kind: "r", path: child });
      }
    };
    walk([]);
    for (const w of want) {
      if (this.inflight.size >= MAX_PARALLEL) return;
      const k = `${w.kind}:${pathId(w.path)}`;
      if (this.inflight.has(k) || this.failed.has(k)) continue;
      if (w.kind === "g") this.fetchGroups(w.path, 0);
      else this.fetchRows(w.path, 0);
    }
  }

  private fetchGroups(path: Key[], offset: number): void {
    const state = this.state!;
    const version = this.version;
    const pid = pathId(path);
    const k = `g:${pid}`;
    if (this.inflight.has(k)) return;
    const id = newQueryId();
    this.inflight.set(k, { id, path });
    gridGroups(state, this.by, path, GROUP_PAGE, offset, this.options.zone, this.slot("groups", pid), id).then(
      (listing) => {
        if (version !== this.version) return; // an answer for an older state is never drawn
        if (this.inflight.get(k)?.id !== id) return; // cancelled while it was on its way
        this.inflight.delete(k);
        const held = this.levels.get(pid);
        if (offset > 0 && (!held || held.groups.length !== offset)) return;
        this.levels.set(pid, { path, groups: [...(offset > 0 ? held!.groups : []), ...listing.items], more: listing.truncated });
        this.notify();
        this.pump();
      },
      (e: unknown) => this.failedRead(version, k, id, e),
    );
  }

  private fetchRows(path: Key[], offset: number): void {
    const state = this.state!;
    const version = this.version;
    const pid = pathId(path);
    const k = `r:${pid}`;
    if (this.inflight.has(k)) return;
    const chips = [...state.chips, ...groupChips(this.by, path, state.chips)];
    if (chips.length > MAX_CHIPS) {
      const why = `at most ${MAX_CHIPS} filters: remove one first`;
      this.failed.set(k, why);
      this.onError(new Error(why));
      this.notify();
      return;
    }
    const id = newQueryId();
    this.inflight.set(k, { id, path });
    gridPage({ ...state, chips }, GROUP_PAGE, offset, this.options, this.slot("rows", pid), id).then(
      (page) => {
        if (version !== this.version) return;
        if (this.inflight.get(k)?.id !== id) return;
        this.inflight.delete(k);
        const held = this.leaves.get(pid);
        if (offset !== rowsHeld(held)) return;
        this.leaves.set(pid, { path, pages: [...(held?.pages ?? []), page], more: page.truncated });
        this.notify();
        this.pump();
      },
      (e: unknown) => this.failedRead(version, k, id, e),
    );
  }

  private failedRead(version: number, k: string, id: string, e: unknown): void {
    if (version !== this.version) return;
    if (this.inflight.get(k)?.id !== id) return; // cancelled (and maybe asked again): whatever it says is obsolete
    this.inflight.delete(k);
    if (isStale(e)) return;
    this.failed.set(k, describe(e));
    this.onError(e);
    this.notify();
    this.pump(); // the slot is free for the next path
  }

  // ---- the flattened tree ------------------------------------------------------------------------------------------
  flatten(): Item[] {
    const out: Item[] = [];
    const note = (id: string, depth: number, k: string, loadingToo: boolean) => {
      const why = this.failed.get(k);
      if (why !== undefined) out.push({ kind: "status", id: `s:${id}`, depth, text: `could not read: ${why}` });
      else if (loadingToo) out.push({ kind: "status", id: `s:${id}`, depth, text: "loading…" });
    };
    const walk = (path: Key[]): void => {
      const depth = path.length;
      const pid = pathId(path);
      const level = this.levels.get(pid);
      if (!level) {
        note(`g${pid}`, depth, `g:${pid}`, true);
        return;
      }
      if (level.groups.length === 0 && depth === 0) out.push({ kind: "status", id: "s:none", depth, text: "no rows" });
      for (const g of level.groups) {
        const child = [...path, keyOf(g)];
        const cid = pathId(child);
        const open = this.expanded.has(cid);
        out.push({ kind: "group", id: `g:${cid}`, depth, path: child, group: g, open });
        if (!open) continue;
        if (child.length < this.by.length) {
          walk(child);
          continue;
        }
        const leaf = this.leaves.get(cid);
        if (!leaf) {
          note(`r${cid}`, depth + 1, `r:${cid}`, true);
          continue;
        }
        let n = 0;
        for (const page of leaf.pages) {
          for (let k = 0; k < page.row_ids.length; k++) out.push({ kind: "row", id: `r:${cid}:${n++}`, depth: depth + 1, page, k });
        }
        if (leaf.more) out.push({ kind: "more-rows", id: `mr:${cid}`, depth: depth + 1, path: child, busy: this.inflight.has(`r:${cid}`) });
        note(`r${cid}`, depth + 1, `r:${cid}`, false);
      }
      if (level.more) out.push({ kind: "more-groups", id: `mg:${pid}`, depth, path, busy: this.inflight.has(`g:${pid}`) });
      note(`g${pid}`, depth, `g:${pid}`, false);
    };
    if (this.by.length > 0) walk([]);
    return out;
  }
}

export interface GroupTreeArgs {
  tabId: string;
  /** Must keep its identity between renders unless it changes (a new object with equal content is not a new query). */
  state: GridState;
  /** Empty means the plain grid: nothing is read. */
  by: string[];
  zone: string;
  enabled: boolean;
  onError(e: unknown): void;
}

export interface GroupTree {
  items: Item[];
  toggle(path: Key[]): void;
  expandAll(): void;
  collapseAll(): void;
  more(kind: "groups" | "rows", path: Key[]): void;
  refresh(): void;
}

export function useGroupTree({ tabId, state, by, zone, enabled, onError }: GroupTreeArgs): GroupTree {
  const [store] = useState(() => new GroupStore(tabId));
  const key = useMemo(() => stableKey([state, by, zone]), [state, by, zone]);
  // Layout effects: the store belongs to the committed key before anything else can act on it.
  useLayoutEffect(() => {
    store.setErrorHandler(onError);
  }, [store, onError]);
  useLayoutEffect(() => {
    store.reset(state, by, zone, key);
    return () => store.stop();
    // `state`, `by` and `zone` are what `key` says: an equal-but-new object is not a new query.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [store, key]);
  useLayoutEffect(() => {
    store.setEnabled(enabled);
  }, [store, enabled]);
  const revision = useSyncExternalStore(store.subscribe, store.getRevision);
  const fresh = store.cacheKey === key;
  // eslint-disable-next-line react-hooks/exhaustive-deps -- `revision` says the store changed
  const items = useMemo(() => (fresh ? store.flatten() : []), [store, fresh, revision]);
  return {
    items,
    toggle: useCallback((p: Key[]) => store.toggle(p), [store]),
    expandAll: useCallback(() => store.expandAll(), [store]),
    collapseAll: useCallback(() => store.collapseAll(), [store]),
    more: useCallback((k: "groups" | "rows", p: Key[]) => store.more(k, p), [store]),
    refresh: useCallback(() => store.refresh(), [store]),
  };
}
