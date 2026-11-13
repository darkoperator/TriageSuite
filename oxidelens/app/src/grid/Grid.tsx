// The dataset grid, mounted in a dataset tab's body. It owns the tab's `GridState` and hands it to `useGridData`; it
// draws what the store holds through TanStack Virtual (a window of rows) with TanStack Table keeping the column model
// and sort state. Selection, the roving cell focus, copy, the cell menu and the value viewer live here.
//
// Extension points for later tasks: edit `state` through `changeState` (columns 16, chips 17, search 18, groups 19);
// `rules` go to `useGridData`; row detail (22) reads `selection` / `focus`; the header and body take more columns of
// behaviour through their props.
import { getCoreRowModel, useReactTable, type ColumnDef, type SortingState } from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type MouseEvent, type ReactNode,
} from "react";
import {
  columns as readColumns, defaultColourRules, gridEmptyColumns, gridExport, gridPage, gridRowOffset, isCmdError, newQueryId, viewSave,
} from "../api";
import { pickCsvSavePath } from "../dialogs";
import ApplyWarnings, { type ViewNotice } from "../views/ApplyWarnings";
import ExportDialog from "../views/ExportDialog";
import SaveViewDialog from "../views/SaveViewDialog";
import type { Chip } from "../contract/generated/Chip";
import type { ColourRule } from "../contract/generated/ColourRule";
import type { ColumnLayout } from "../contract/generated/ColumnLayout";
import type { GridPage } from "../contract/generated/GridPage";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";
import type { GridState } from "../contract/generated/GridState";
import type { SearchState } from "../contract/generated/SearchState";
import RowDetail from "../detail/RowDetail";
import { useShell } from "../shell/context";
import { displaySafe } from "../text";
import type { TabBodyProps } from "../shell/slots";
import AdvancedEditor from "./AdvancedEditor";
import CellMenu from "./CellMenu";
import ChipBar from "./ChipBar";
import {
  MARK_COLUMN, MAX_CHIPS, cellChip, filterColumns, filterRowChip, filterRowId, groupChips, hash, removeChip, toggleChip, upsertChip,
} from "./chips";
import ColourRules from "./ColourRules";
import ColumnManager from "./ColumnManager";
import { cellText, rowsAsTsv, writeClipboard } from "./copy";
import FilterPanel from "./FilterPanel";
import FilterRow from "./FilterRow";
import GridBody from "./GridBody";
import GroupBar from "./GroupBar";
import GroupBody, { keyText } from "./GroupRow";
import { MAX_GROUP_BY, useGroupTree, type Key } from "./groupTree";
import GridHeader, { MARK_WIDTH } from "./GridHeader";
import { initialGridState, nextSort } from "./gridState";
import HeaderMenu from "./HeaderMenu";
import Histogram from "./Histogram";
import HostScope from "./HostScope";
import {
  bestFitWidth, fetchedColumns, hide, materialise, moveColumn, resolve, setWidth, setWidths, show, showAll, togglePin,
} from "./layout";
import { measureText } from "./measure";
import { BLOCK_ROWS, blockOf } from "./blocks";
import { filterFromChips, newRule, rulesStatus } from "./ruleClasses";
import { MAX_COPY_ROWS, contains, emptySelection, indices, selectOnly, selectRange, size, toggle, type Selection } from "./selection";
import SearchBox from "./SearchBox";
import { stableKey, useGridData } from "./useGridData";
import { useMatches } from "./useMatches";
import ValueViewer from "./ValueViewer";
import ZonePicker from "./ZonePicker";
import { rowHeightFor } from "../theme/appearance";
import "./grid.css";

const NO_ROWS: unknown[] = [];
/** A layout change is saved this long after the last one. */
const LAYOUT_SAVE_MS = 250;
const fmt = (n: number) => n.toLocaleString("en-US");
const plural = (n: number, one: string, many: string) => (n === 1 ? one : many);

const NO_RULES: ColourRule[] = [];

let revealCount = 0;
/** A token no other reveal request of this window has: a counter and some randomness (never a clock). */
const revealToken = () => `${(revealCount++).toString(36)}${Math.random().toString(36).slice(2, 8)}`;

interface Focus {
  row: number;
  col: number;
}

export default function Grid({ tab, active, scrollOffset, onScroll }: TabBodyProps): ReactNode {
  const {
    summary, settings, change, setGridStatus, reportError, saveLayout, saveColourRules, openScoped, setHistogram, setRightPane, pendingRow, clearPendingRow,
    pendingApply, clearPendingApply, exportJob, exportRunning, beginExport, endExport,
  } = useShell();
  const relation = tab.relation ?? "";

  const [state, setState] = useState<GridState>(() => tab.seed?.state ?? initialGridState(relation));
  // The group bar: the columns grouped by, outermost first. It is not part of `GridState` (the engine takes it per call).
  const [by, setBy] = useState<string[]>(() => tab.seed?.by ?? []);
  const grouped = by.length > 0;
  const [selection, setSelection] = useState<Selection>(emptySelection);
  const [focus, setFocus] = useState<Focus | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [viewer, setViewer] = useState<{ rowId: number; column: string } | null>(null);
  const [allMetas, setAllMetas] = useState<ColumnMeta[] | null>(null);
  const [headerMenu, setHeaderMenu] = useState<{ x: number; y: number; column: string } | null>(null);
  const [managerOpen, setManagerOpen] = useState(false);
  const [emptyBusy, setEmptyBusy] = useState(false);
  const [panel, setPanel] = useState<{ column: string; x: number; y: number; opener?: HTMLElement | null } | null>(null);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [clearToken, setClearToken] = useState(0);
  // The column layout: what the settings hold for the relation, and what the analyst does here. Opening writes nothing;
  // a change is saved `LAYOUT_SAVE_MS` after the last one (see `edit`). When the settings' layout for the relation
  // changes to one this tab is not showing (a view applied, or an edit, in another tab of the relation), the tab adopts it,
  // so its next edit builds on it; the latest write wins for every tab.
  const settingsLayout = settings.layouts[relation] ?? null;
  const [layout, setLayout] = useState<ColumnLayout | null>(settingsLayout);
  const layoutRef = useRef<ColumnLayout | null>(layout);
  const saveTimer = useRef<number | undefined>(undefined);
  const savePending = useRef(false);

  const rootRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);

  // Hiding a column changes what the engine fetches (so the state changes and the grid refetches); order, width and
  // pinning are drawn from the layout alone and never reach the state.
  const resolved = useMemo(() => (allMetas ? resolve(layout, allMetas) : null), [layout, allMetas]);
  const fetched = useMemo(() => (resolved && allMetas ? fetchedColumns(resolved, allMetas) : null), [resolved, allMetas]);
  const shownState = useMemo<GridState>(() => ({ ...state, columns: fetched }), [state, fetched]);

  // Colour rules: this relation's saved list, or (no saved entry) the engine's defaults, read once. Until one of them is
  // known no page is asked for, so the first rows are never drawn untinted and then tinted. An edit stores the whole list.
  // What the shell's settings hold is what is shown: an edit is applied to that latest list and goes through the settings
  // queue, so another tab on the relation sees it, and a rejected write falls back to the confirmed list.
  const savedRules: ColourRule[] | null = settings.colour_rules[relation] ?? null;
  const [defaultRules, setDefaultRules] = useState<ColourRule[] | null>(null);
  const [rulesOpen, setRulesOpen] = useState(false);
  useEffect(() => {
    if (savedRules !== null || defaultRules !== null) return;
    let live = true;
    defaultColourRules(relation, `tab:${tab.id}:rules`).then(
      (r) => live && setDefaultRules(r),
      (e: unknown) => {
        if (!live) return;
        if (!(isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case"))) reportError(e);
        setDefaultRules([]);
      },
    );
    return () => {
      live = false;
    };
  }, [savedRules, defaultRules, relation, tab.id, reportError]);
  const rules = useMemo(() => savedRules ?? defaultRules ?? NO_RULES, [savedRules, defaultRules]);
  const rulesReady = savedRules !== null || defaultRules !== null;
  const setRules = (next: ColourRule[]) => {
    saveColourRules(relation, next);
  };
  const resetRules = () => {
    setDefaultRules(null);
    saveColourRules(relation, null);
  };

  const data = useGridData({
    tabId: tab.id, state: shownState, zone: summary.display_zone, rules, enabled: active && allMetas !== null && rulesReady && !grouped,
    onError: reportError,
  });
  const { want, refresh, rowAt, beginCopy, loadedTexts } = data;
  const tree = useGroupTree({
    tabId: tab.id, state: shownState, by, zone: summary.display_zone, enabled: active && allMetas !== null && grouped, onError: reportError,
  });
  const rowCount = grouped ? tree.items.length : data.total;

  // ---- the layout: one edit function, saved after a pause -------------------------------------------------------------
  const flushLayout = useCallback(() => {
    window.clearTimeout(saveTimer.current);
    if (!savePending.current) return;
    savePending.current = false;
    saveLayout(relation, layoutRef.current);
  }, [relation, saveLayout]);
  useEffect(() => flushLayout, [flushLayout]); // closing the tab does not lose a change still waiting

  const scheduleSave = useCallback(() => {
    savePending.current = true;
    window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(flushLayout, LAYOUT_SAVE_MS);
  }, [flushLayout]);
  const edit = useCallback(
    (apply: (l: ColumnLayout) => ColumnLayout) => {
      if (!allMetas) return;
      const next = apply(materialise(layoutRef.current, allMetas));
      layoutRef.current = next;
      setLayout(next);
      scheduleSave();
    },
    [allMetas, scheduleSave],
  );
  const resetLayout = () => {
    layoutRef.current = null;
    setLayout(null);
    setFocus(null);
    scheduleSave();
  };
  useEffect(() => {
    // An edit of this tab still waiting to be saved is about to be the latest write; it is not dropped for an older one.
    if (savePending.current) return;
    if (stableKey(settingsLayout) === stableKey(layoutRef.current)) return; // a reply to this tab's own write, or the same
    layoutRef.current = settingsLayout;
    setLayout(settingsLayout);
    setFocus(null);
  }, [settingsLayout]);

  /** Every edit of what the grid shows goes through here: the selection's row positions mean nothing afterwards. */
  const changeState = useCallback((edit: (s: GridState) => GridState) => {
    setState(edit);
    setSelection(emptySelection);
    setMenu(null);
  }, []);

  // ---- filters: every layer is a chip in `state.chips` ---------------------------------------------------------------
  const chipsRef = useRef(state.chips);
  useEffect(() => {
    chipsRef.current = state.chips;
  });
  /** Edits the chip list; more than `MAX_CHIPS` is refused with a message and nothing changes. */
  const applyChips = useCallback(
    (edit: (chips: Chip[]) => Chip[]) => {
      const next = edit(chipsRef.current);
      if (next.length > MAX_CHIPS) {
        reportError(new Error(`at most ${MAX_CHIPS} filters: remove one first`));
        return;
      }
      chipsRef.current = next; // the next edit in this tick starts from this one, not from the last render
      changeState((s) => ({ ...s, chips: next }));
    },
    [changeState, reportError],
  );
  // ---- search: applied only once the engine accepts it, so a refusal leaves the results on screen ---------------------
  const stateRef = useRef(state);
  useEffect(() => {
    stateRef.current = state;
  });
  const searchSeq = useRef(0);
  const [searchError, setSearchError] = useState<string | null>(null);
  const applySearch = useCallback(
    async (next: SearchState) => {
      const seq = ++searchSeq.current;
      const current = stateRef.current;
      if (stableKey(next) === stableKey(current.search)) {
        setSearchError(null);
        return;
      }
      if (next.text !== "") {
        try {
          // The same one-row read as a chip check: the engine parses the text and says if it cannot.
          await gridPage({ ...current, search: next }, 1, 0, { zone: summary.display_zone, rules: [] }, `tab:${tab.id}:search`);
        } catch (e) {
          if (seq !== searchSeq.current || (isCmdError(e) && (e.code === "superseded" || e.code === "interrupted"))) return;
          if (isCmdError(e) && e.code === "invalid") setSearchError(e.message);
          else reportError(e);
          return;
        }
      }
      if (seq !== searchSeq.current) return;
      setSearchError(null);
      changeState((s) => ({ ...s, search: next }));
    },
    [changeState, reportError, summary.display_zone, tab.id],
  );
  const findActive = state.search.mode === "find" && state.search.text !== "";
  const matches = useMatches(tab.id, state, findActive, change.generation, reportError);

  const onFilterRowText = useCallback(
    (column: string, text: string) =>
      applyChips((chips) => (text === "" ? removeChip(chips, filterRowId(column)) : upsertChip(chips, filterRowChip(column, text)))),
    [applyChips],
  );
  /** Asks the engine whether the grid would take `chip` (one row is read); its message when it would not. */
  const validate = useCallback(
    async (chip: Chip): Promise<string | null> => {
      const chips = upsertChip(chipsRef.current, chip);
      if (chips.length > MAX_CHIPS) return `at most ${MAX_CHIPS} filters: remove one first`;
      try {
        await gridPage({ ...state, chips }, 1, 0, { zone: summary.display_zone, rules: [] }, `tab:${tab.id}:validate`);
        return null;
      } catch (e) {
        if (isCmdError(e)) {
          if (e.code === "superseded" || e.code === "interrupted") return "The check was interrupted: try again.";
          return e.message;
        }
        return e instanceof Error ? e.message : String(e);
      }
    },
    [state, summary.display_zone, tab.id],
  );
  const savePreset = useCallback(
    async (name: string): Promise<string | null> => {
      try {
        // A preset is chips only: everything else of a view is left empty, which is what the engine requires.
        await viewSave(name, "preset", relation, {
          chips: state.chips,
          search: { text: "", regex: false, case_sensitive: false, mode: "filter" },
          hosts: [], group_by: [], sort: [], layout: null, colour_rules: [],
        });
        return null;
      } catch (e) {
        return isCmdError(e) ? e.message : e instanceof Error ? e.message : String(e);
      }
    },
    [relation, state.chips],
  );
  const filtered = useMemo(
    () => new Set(state.chips.filter((c) => c.enabled).flatMap((c) => filterColumns(c.filter))),
    [state.chips],
  );

  // ---- columns ----------------------------------------------------------------------------------------------------
  const meta = useMemo(() => new Map<string, ColumnMeta>((allMetas ?? []).map((c) => [c.name, c])), [allMetas]);
  const columnDefs = useMemo<ColumnDef<unknown>[]>(
    () => (resolved?.shown ?? []).map((c) => ({ id: c.name, header: c.name, size: resolved!.width(c.name) })),
    [resolved],
  );
  const sorting = useMemo<SortingState>(() => state.sort.map((k) => ({ id: k.column, desc: k.descending })), [state.sort]);
  // TanStack Table's functions are not memoizable; the grid only reads the column model from it during render.
  // eslint-disable-next-line react-hooks/incompatible-library
  const table = useReactTable({
    data: NO_ROWS, columns: columnDefs, state: { sorting }, manualSorting: true, getCoreRowModel: getCoreRowModel(),
  });
  const leaves = table.getVisibleLeafColumns();
  const names = leaves.map((c) => c.id);
  const widths = leaves.map((c) => c.getSize());
  const width = MARK_WIDTH + widths.reduce((a, b) => a + b, 0);
  // Pages hold the columns in catalog order; the grid draws them in layout order.
  const pageColumns = data.columns;
  const cellIdx = names.map((n) => pageColumns?.findIndex((c) => c.name === n) ?? -1);
  const pinLeft: (number | null)[] = [];
  let offset = MARK_WIDTH;
  names.forEach((n, i) => {
    pinLeft.push(resolved?.pinned.has(n) ? offset : null);
    offset += widths[i];
  });

  // How many columns the relation has, for the status line (the grid's own page only carries the shown ones).
  const generation = change.generation;
  useEffect(() => {
    let live = true;
    readColumns(relation, `tab:${tab.id}:columns`).then(
      (c) => live && setAllMetas(c),
      (e: unknown) => {
        if (live && !(isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case"))) reportError(e);
      },
    );
    return () => {
      live = false;
    };
  }, [relation, tab.id, generation, reportError]);

  // A write to the case (a mark, a load, a zone) re-reads the rows; filters and scroll stay. The selection, its anchor
  // and the focused cell are row offsets, and a load or a supersede can put other rows at the same offsets, so they are
  // cleared with the re-read: Copy rows can never take rows the analyst did not pick. (The row detail is kept: it holds
  // a row id.) A change to the saved views alone re-reads no rows and keeps them.
  const treeRefresh = tree.refresh;
  const seenGeneration = useRef(change.generation);
  // Rises with every re-read of the rows; the histogram strip re-reads with it.
  const [dataVersion, setDataVersion] = useState(0);
  useEffect(() => {
    if (change.generation === seenGeneration.current) return;
    seenGeneration.current = change.generation;
    if (change.kinds.some((k) => k !== "views")) {
      setSelection(emptySelection);
      setFocus(null);
      setMenu(null);
      setDataVersion((v) => v + 1);
      refresh();
      treeRefresh();
    }
  }, [change, refresh, treeRefresh]);

  // ---- virtual rows -----------------------------------------------------------------------------------------------
  const rowHeight = rowHeightFor(settings.font_size);
  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    overscan: 12,
    initialOffset: scrollOffset,
  });
  // The virtualiser keeps the sizes it measured; a new row height (the font-size setting) must drop them.
  const measureRows = virtualizer.measure;
  useEffect(() => {
    measureRows();
  }, [rowHeight, measureRows]);
  const items = virtualizer.getVirtualItems();
  const firstShown = items.length > 0 ? items[0].index : 0;
  const lastShown = items.length > 0 ? items[items.length - 1].index : 0;
  useEffect(() => {
    if (items.length > 0 && !grouped) want(firstShown, lastShown);
  }, [firstShown, lastShown, items.length, want, grouped]);

  // ---- scroll: restore on mount and on showing the tab, report as it moves ----------------------------------------
  const lastOffset = useRef(scrollOffset);
  const restored = useRef(scrollOffset === 0);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!active || !el) return;
    // The scrollbar only spans the saved offset once the row count is known; until then a restore would be clamped.
    if (!restored.current && rowCount * rowHeight < lastOffset.current + el.clientHeight) return;
    restored.current = true;
    el.scrollTop = lastOffset.current;
  }, [active, rowCount, rowHeight]);

  // ---- the status line --------------------------------------------------------------------------------------------
  const shownColumns = names.length;
  const totalColumns = allMetas?.length ?? shownColumns;
  const counts = data.counts;
  const selectedCount = size(selection);
  const rulesText = rulesStatus(rules, savedRules === null);
  const groupedBy = by.map((c) => displaySafe(c === MARK_COLUMN ? "Mark" : c)).join(" › ");
  const statusNode = useMemo(
    () =>
      counts === null && !grouped ? null : (
        <>
          {grouped
            ? `Grouped by ${groupedBy} · ${fmt(shownColumns)} of ${fmt(totalColumns)} columns · `
            : `${fmt(counts!.rows)} of ${fmt(counts!.total)} rows · ${fmt(selectedCount)} selected · ${fmt(shownColumns)} of ${fmt(totalColumns)} columns · `}
          <button type="button" className="status-link" aria-haspopup="dialog" onClick={() => setManagerOpen((o) => !o)}>
            {`${fmt(totalColumns - shownColumns)} hidden ▾`}
          </button>
          {!grouped && ` · ${fmt(counts!.failed_values)} ${plural(counts!.failed_values, "value", "values")} failed to convert`}
          {rulesText !== null && ` · ${rulesText}`}
        </>
      ),
    [counts, grouped, groupedBy, selectedCount, shownColumns, totalColumns, rulesText],
  );
  useEffect(() => {
    if (!active) return;
    setGridStatus(statusNode);
    return () => setGridStatus(null);
  }, [active, statusNode, setGridStatus]);

  // ---- the row detail: what the right pane shows while this tab is the active one ----------------------------------------
  // A single selection (a plain click) or Enter on the focused row shows that row; grouped lists have no rows to select.
  const [detailRow, setDetailRow] = useState<number | null>(null);
  const showDetailAt = (index: number) => {
    const row = rowAt(index);
    if (row) setDetailRow(row.id);
  };
  const detailNode = useMemo(
    () =>
      detailRow === null ? null : (
        <RowDetail rowId={detailRow} onFilter={(chip) => applyChips((chips) => [...chips, chip])} onShowRow={setDetailRow} />
      ),
    [detailRow, applyChips],
  );
  useEffect(() => {
    if (!active) return;
    setRightPane(detailNode);
    return () => setRightPane(null);
  }, [active, detailNode, setRightPane]);

  // ---- revealing a row: the merged tab's *open* link, or a tab opened to show one -----------------------------------------
  // The row is shown in the right pane; where the engine places it in this tab's order, the grid scrolls to it and focuses
  // it. A row the tab's filters leave out is said so, with a way to see it in an unfiltered tab. One reveal at a time: a
  // newer one, or any change of the tab's state while the engine is answering, drops the answer.
  const [hidden, setHidden] = useState<number | null>(null);
  // Where to scroll, for the state it was computed under and the request that asked: either changing drops it.
  const [scrollTarget, setScrollTarget] = useState<{ offset: number; key: string; seq: number } | null>(null);
  const revealSeq = useRef(0);
  const stateKey = useMemo(() => stableKey(state), [state]);
  const reveal = useCallback(
    async (rowId: number) => {
      setDetailRow(rowId);
      setHidden(null);
      setScrollTarget(null);
      const seq = ++revealSeq.current; // a newer request supersedes any older one still answering or waiting
      if (by.length > 0) return; // a grouped list has no row positions
      const asked = stateRef.current;
      const key = stableKey(asked);
      try {
        const offset = await gridRowOffset(asked, rowId, `tab:${tab.id}:reveal`);
        if (seq !== revealSeq.current || stableKey(stateRef.current) !== key) return;
        if (offset === null) setHidden(rowId);
        else setScrollTarget({ offset, key, seq });
      } catch (e) {
        if (seq !== revealSeq.current) return;
        if (!(isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case"))) reportError(e);
      }
    },
    [by.length, tab.id, reportError],
  );
  useEffect(() => {
    if (pendingRow && pendingRow.tabId === tab.id) {
      clearPendingRow?.();
      void reveal(pendingRow.rowId);
    }
  }, [pendingRow, tab.id, clearPendingRow, reveal]);
  // A tab opened to show a row asks once, when it first mounts (every request opens its own tab).
  const revealed = useRef(false);
  useEffect(() => {
    if (tab.reveal !== undefined && !revealed.current) {
      revealed.current = true;
      void reveal(tab.reveal);
    }
  }, [tab.reveal, reveal]);
  // The scroll waits for the tab to be shown and the row count to cover the row, and happens only if the target is still
  // the one asked for under the tab's present state.
  useEffect(() => {
    if (scrollTarget === null) return;
    if (scrollTarget.seq !== revealSeq.current || scrollTarget.key !== stateKey) {
      setScrollTarget(null);
      return;
    }
    if (!active || rowCount <= scrollTarget.offset) return;
    virtualizer.scrollToIndex(scrollTarget.offset, { align: "center" });
    setFocus({ row: scrollTarget.offset, col: 0 });
    setSelection(selectOnly(scrollTarget.offset));
    setScrollTarget(null);
  }, [scrollTarget, stateKey, active, rowCount, virtualizer]);
  /** The row is hidden by this tab's filters: the relation opens in a new, unfiltered tab that scrolls to the row. */
  const openUnfiltered = (rowId: number) => {
    // Every request opens a tab of its own (a fresh token in its id), so it is unfiltered whatever the earlier ones were
    // changed to, and its one reveal is not already spent.
    const seed = { state: initialGridState(relation), by: [] };
    openScoped({
      id: `reveal:${hash(stableKey([relation, rowId, revealToken()]))}`, kind: "dataset", relation, title: `${relation} · row ${rowId}`, seed, reveal: rowId,
    });
    setHidden(null);
  };

  // ---- selection and focus ----------------------------------------------------------------------------------------
  const onCellClick = (e: MouseEvent, index: number, col: number) => {
    setFocus({ row: index, col });
    setMenu(null);
    if (e.shiftKey) setSelection((s) => selectRange(s, index));
    else if (e.ctrlKey || e.metaKey) setSelection((s) => toggle(s, index));
    else {
      setSelection(selectOnly(index));
      showDetailAt(index);
    }
  };

  const onCellContextMenu = (e: MouseEvent, index: number, col: number) => {
    e.preventDefault();
    setFocus({ row: index, col });
    if (!contains(selection, index)) setSelection(selectOnly(index));
    setMenu({ x: e.clientX, y: e.clientY });
  };

  const moveFocus = (dr: number, dc: number) => {
    if (data.total === 0 || names.length === 0) return;
    const base = focus ?? { row: 0, col: 0 };
    const row = Math.min(data.total - 1, Math.max(0, base.row + dr));
    const col = Math.min(names.length - 1, Math.max(0, base.col + dc));
    setFocus({ row, col });
    virtualizer.scrollToIndex(row, { align: "auto" });
  };

  /** F3 / Shift+F3 / ▲▼: the next or previous match after where the analyst is, scrolled into view and focused. */
  const stepMatch = (direction: 1 | -1) => {
    if (!findActive) return;
    // Where the focus has been moved away from the current match, stepping goes on from the focus.
    const anchor = focus && focus.row !== matches.current ? focus.row : null;
    void matches.step(direction, anchor).then((offset) => {
      if (offset === null) return;
      setFocus((f) => ({ row: offset, col: f?.col ?? 0 }));
      virtualizer.scrollToIndex(offset, { align: "center" });
    });
  };

  // ---- copy and the viewer ----------------------------------------------------------------------------------------
  const viewChanged = (session: { replaced(): boolean }) => {
    if (!session.replaced()) reportError(new Error("the view changed; copy again"));
  };

  const guarded = useCallback(
    (work: Promise<void>) =>
      work.catch((e: unknown) => {
        if (!(isCmdError(e) && (e.code === "superseded" || e.code === "interrupted"))) reportError(e);
      }),
    [reportError],
  );

  const copyCell = () => {
    if (!focus) return;
    const row = rowAt(focus.row);
    const cell = row?.cells[cellIdx[focus.col]];
    if (!row || !cell) return;
    const session = beginCopy();
    void guarded(
      (async () => {
        const text = cell.clipped ? await session.value(row.id, names[focus.col]) : cellText(cell);
        if (text === null || !session.alive()) return viewChanged(session);
        await writeClipboard(text);
      })(),
    );
  };

  const copyRows = () => {
    const picked = size(selection) > 0 ? selection : focus ? selectOnly(focus.row) : null;
    if (!picked) return;
    if (size(picked) > MAX_COPY_ROWS) {
      reportError(new Error(`select at most ${fmt(MAX_COPY_ROWS)} rows to copy`));
      return;
    }
    const session = beginCopy();
    void guarded(
      (async () => {
        const wanted = [...new Set([...indices(picked)].map(blockOf))];
        const pages = new Map<number, GridPage>();
        for (const b of wanted) {
          const page = await session.page(b); // the blocks not in memory are fetched for this copy
          if (!page) return viewChanged(session);
          pages.set(b, page);
        }
        const lines: string[][] = [];
        for (const i of indices(picked)) {
          const page = pages.get(blockOf(i));
          const k = i - blockOf(i) * BLOCK_ROWS;
          if (!page || k >= page.row_ids.length) continue;
          const line: string[] = [];
          for (const name of names) {
            const at = page.columns.findIndex((c) => c.name === name);
            const cell = at < 0 ? null : page.cells[k][at];
            if (cell === null) line.push("");
            else if (!cell.clipped) line.push(cellText(cell));
            else {
              const whole = await session.value(page.row_ids[k], name);
              if (whole === null) return viewChanged(session);
              line.push(whole);
            }
          }
          lines.push(line);
        }
        if (!session.alive()) return viewChanged(session); // checked again just before the clipboard is touched
        await writeClipboard(rowsAsTsv(names, lines));
      })(),
    );
  };

  const showValue = () => {
    if (!focus) return;
    const row = rowAt(focus.row);
    if (row && names[focus.col] !== undefined) setViewer({ rowId: row.id, column: names[focus.col] });
  };

  /** Filter in / Filter out on the focused cell: case-sensitive `eq` / `ne` on its stored text (a clipped one is read whole). */
  const cellFilter = (op: "eq" | "ne") => {
    if (!focus) return;
    const row = rowAt(focus.row);
    const column = names[focus.col];
    const cell = row?.cells[cellIdx[focus.col]];
    if (!row || !cell || column === undefined) return;
    const session = beginCopy();
    void guarded(
      (async () => {
        const text = cell.clipped ? await session.value(row.id, column) : cell.text;
        if (!session.alive()) return viewChanged(session);
        applyChips((chips) => [...chips, cellChip(column, op, text)]);
      })(),
    );
  };

  const closeViewer = useCallback(() => {
    setViewer(null);
    gridRef.current?.focus();
  }, []);
  const closePanel = useCallback(() => {
    setPanel(null);
    gridRef.current?.focus();
  }, []);
  const closeMenu = useCallback(() => {
    setMenu(null);
    gridRef.current?.focus();
  }, []);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const t = e.target as HTMLElement;
    if (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable) return;
    // The shortcuts below work on the flat list's offsets; the group list has its own (buttons, native Space and Enter).
    if (grouped) return;
    const height = scrollRef.current?.clientHeight ?? 0;
    const page = height > 0 ? Math.max(1, Math.floor(height / rowHeight) - 1) : 10;
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === "c") {
      e.preventDefault();
      if (e.shiftKey) copyRows();
      else copyCell();
      return;
    }
    switch (e.key) {
      case "F3": e.preventDefault(); stepMatch(e.shiftKey ? -1 : 1); break;
      case " ":
        e.preventDefault();
        showValue();
        break;
      case "Enter":
        if (focus && t.tagName !== "BUTTON") {
          e.preventDefault();
          showDetailAt(focus.row);
        }
        break;
      case "ArrowDown": e.preventDefault(); moveFocus(1, 0); break;
      case "ArrowUp": e.preventDefault(); moveFocus(-1, 0); break;
      case "ArrowRight": e.preventDefault(); moveFocus(0, 1); break;
      case "ArrowLeft": e.preventDefault(); moveFocus(0, -1); break;
      case "PageDown": e.preventDefault(); moveFocus(page, 0); break;
      case "PageUp": e.preventDefault(); moveFocus(-page, 0); break;
      case "Home": e.preventDefault(); moveFocus(mod ? -data.total : 0, -names.length); break;
      case "End": e.preventDefault(); moveFocus(mod ? data.total : 0, names.length); break;
      default:
    }
  };

  const onSort = (column: string, additive: boolean) =>
    changeState((s) => ({ ...s, sort: nextSort(s.sort, column, additive) }));

  // ---- column actions ---------------------------------------------------------------------------------------------
  const [restore, setRestore] = useState<{ column: string; token: number } | null>(null);
  const onHeaderMenu = (column: string, x: number, y: number) => {
    setMenu(null);
    setHeaderMenu({ x, y, column });
  };
  const headerMenuColumn = headerMenu?.column;
  const closeHeaderMenu = useCallback(() => {
    setHeaderMenu(null);
    if (headerMenuColumn !== undefined) setRestore((r) => ({ column: headerMenuColumn, token: (r?.token ?? 0) + 1 }));
  }, [headerMenuColumn]);
  const hideColumns = (hideNames: readonly string[]) => {
    if (!resolved) return;
    const left = resolved.shown.filter((c) => !hideNames.includes(c.name));
    // The grid never shows nothing: if everything would go, the first shown column stays.
    const keep = left.length === 0 && resolved.shown.length > 0 ? [resolved.shown[0].name] : [];
    edit((l) => hide(l, hideNames.filter((n) => !keep.includes(n))));
    setFocus(null);
  };
  const toggleColumn = (name: string) => {
    if (!resolved) return;
    if (resolved.hidden.has(name)) edit((l) => show(l, [name]));
    else hideColumns([name]);
  };
  const fit = (fitNames: readonly string[]) => {
    const fitted: Record<string, number> = {};
    for (const n of fitNames) {
      const texts = [...new Set(loadedTexts(n).map((t) => displaySafe(t)))];
      fitted[n] = bestFitWidth(texts, displaySafe(n), (t, f) => measureText(t, f, settings.font_size));
    }
    edit((l) => setWidths(l, fitted));
  };
  const hideEmpty = () => {
    setEmptyBusy(true);
    void guarded(
      gridEmptyColumns({ ...state, columns: null }, `tab:${tab.id}:empty`).then((empty) => {
        hideColumns(empty);
      }),
    ).finally(() => setEmptyBusy(false));
  };

  // ---- grouping ---------------------------------------------------------------------------------------------------
  const changeBy = useCallback((next: string[]) => {
    setBy(next);
    setSelection(emptySelection);
    setFocus(null);
    setMenu(null);
  }, []);
  const addGroup = (column: string) => {
    if (by.includes(column)) return;
    if (by.length >= MAX_GROUP_BY) {
      reportError(new Error(`group by at most ${MAX_GROUP_BY} columns: remove one first`));
      return;
    }
    changeBy([...by, column]);
  };
  /** Double-click on a group: a new tab with this state, the group's chips and the levels below it still grouped. */
  const openGroup = (path: Key[]) => {
    const chips = groupChips(by, path, state.chips);
    if (state.chips.length + chips.length > MAX_CHIPS) {
      reportError(new Error(`at most ${MAX_CHIPS} filters: remove one first`));
      return;
    }
    const seed = { state: { ...state, chips: [...state.chips, ...chips] }, by: by.slice(path.length) };
    const clip = (t: string) => (t.length > 40 ? t.slice(0, 39) + "…" : t);
    const title = `${relation} · ${path.map((k, i) => clip(keyText(by[i], k, false))).join(" · ")}`;
    openScoped({ id: `scoped:${hash(stableKey([relation, seed]))}`, kind: "dataset", relation, title, seed });
  };

  // ---- saved views and export -------------------------------------------------------------------------------------
  const [notice, setNotice] = useState<ViewNotice | null>(null);
  const [saveOpen, setSaveOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  // A tab that is hidden keeps no popover open: it would still be there on return, and could start an export the toolbar
  // forbids (switching tabs by key does not press the mouse, so the popover's own outside-press close does not run).
  if (!active && (saveOpen || exportOpen)) {
    setSaveOpen(false);
    setExportOpen(false);
  }
  const saveButton = useRef<HTMLButtonElement>(null);
  const exportButton = useRef<HTMLButtonElement>(null);

  // A view the left pane applied to this tab: the engine's answer replaces the state and the group-by. Its layout and
  // colour rules, when it has any, are written for this relation through the shell's settings queue (they are per-machine
  // settings, and the analyst chose to apply the view); the confirmation says so.
  useEffect(() => {
    if (!pendingApply || pendingApply.tabId !== tab.id) return;
    clearPendingApply?.();
    const { applied, name } = pendingApply;
    changeState(() => applied.state);
    changeBy(applied.group_by);
    setClearToken((t) => t + 1);
    const wrote: string[] = [];
    if (applied.layout !== null) {
      window.clearTimeout(saveTimer.current);
      savePending.current = false;
      layoutRef.current = applied.layout;
      setLayout(applied.layout);
      saveLayout(relation, applied.layout);
      wrote.push("column layout");
    }
    if (applied.colour_rules.length > 0) {
      saveColourRules(relation, applied.colour_rules);
      wrote.push("colour rules");
    }
    setNotice({
      notes: [`Applied “${name}”.${wrote.length > 0 ? ` Its ${wrote.join(" and ")} ${wrote.length > 1 ? "were" : "was"} saved for ${relation} on this machine.` : ""}`],
      errors: [],
      warnings: applied.warnings,
    });
  }, [pendingApply, tab.id, clearPendingApply, changeState, changeBy, saveLayout, saveColourRules, relation]);

  /** Saves what the tab shows as a view: chips, search, hosts, group-by, sort, the layout and the colour rules. */
  const saveView = async (name: string, pattern: string): Promise<string | null> => {
    try {
      await viewSave(name, "view", pattern, {
        chips: state.chips, search: state.search, hosts: state.hosts, group_by: by, sort: state.sort,
        layout: layoutRef.current, colour_rules: rules,
      });
      setNotice({ notes: [`Saved view “${name}”.`], errors: [], warnings: [] });
      return null;
    } catch (e) {
      return isCmdError(e) ? e.message : e instanceof Error ? e.message : String(e);
    }
  };

  /** Asks for the file, then writes the rows as a job the status bar can cancel. */
  const runExport = async (zoneTimes: boolean) => {
    setExportOpen(false);
    // One export at a time, and the shell is where that is decided: a dialog left open in another tab cannot get round it.
    const refuse = () => setNotice({ notes: [], errors: ["An export is running."], warnings: [] });
    if (exportRunning?.()) return refuse();
    let path: string | null;
    try {
      path = await pickCsvSavePath();
    } catch (e) {
      reportError(e);
      return;
    }
    if (path === null) return;
    const queryId = newQueryId();
    if (beginExport && !beginExport({ queryId, zoneTimes })) return refuse();
    try {
      const report = await gridExport(shownState, path, zoneTimes ? summary.display_zone : null, `tab:${tab.id}:export`, queryId);
      setNotice({ notes: [`Exported ${fmt(report.rows)} ${plural(report.rows, "row", "rows")} to ${report.path}.`], errors: [], warnings: [] });
    } catch (e) {
      if (isCmdError(e) && (e.code === "interrupted" || e.code === "superseded")) {
        setNotice({ notes: ["Export cancelled."], errors: [], warnings: [] });
      } else if (isCmdError(e)) {
        setNotice({ notes: [], errors: [`Export failed (${e.code}): ${e.message}`], warnings: [] });
      } else {
        reportError(e);
      }
    } finally {
      endExport?.(queryId); // frees the slot only while it is this job's
    }
  };

  return (
    <div ref={rootRef} className="grid" style={{ "--cell-size": `${settings.font_size}px` } as CSSProperties}>
      <div className="grid-toolbar">
        <SearchBox
          search={state.search}
          error={searchError}
          find={findActive ? { count: matches.count, position: matches.position } : null}
          onApply={(next) => void applySearch(next)}
          onStep={stepMatch}
        />
        <HostScope hosts={summary.hosts} chosen={state.hosts} onChange={(hosts) => changeState((s) => ({ ...s, hosts }))} />
        <ZonePicker zone={summary.display_zone} tabId={tab.id} onError={reportError} />
        <button
          type="button"
          className="scope-button"
          aria-pressed={settings.histogram}
          onClick={() => setHistogram(!settings.histogram)}
        >
          Histogram
        </button>
        <button type="button" className="scope-button" aria-haspopup="dialog" aria-expanded={rulesOpen} title="Colour rules" onClick={() => setRulesOpen(true)}>
          {`Colours ${rules.filter((r) => r.enabled).length}`}
        </button>
        <button ref={saveButton} type="button" className="scope-button" aria-haspopup="dialog" aria-expanded={saveOpen} onClick={() => setSaveOpen((o) => !o)}>
          Save view…
        </button>
        <button ref={exportButton} type="button" className="scope-button" aria-haspopup="dialog" aria-expanded={exportOpen} disabled={exportJob != null} onClick={() => setExportOpen((o) => !o)}>
          Export…
        </button>
        {exportJob != null && <span className="chip-note">An export is running.</span>}
        {saveOpen && <SaveViewDialog anchor={saveButton} relation={relation} onSave={saveView} onClose={() => setSaveOpen(false)} />}
        {exportOpen && <ExportDialog anchor={exportButton} zone={summary.display_zone} onStart={(z) => void runExport(z)} onClose={() => setExportOpen(false)} />}
      </div>
      {notice !== null && <ApplyWarnings notice={notice} onDismiss={() => setNotice(null)} />}
      <ChipBar
        search={state.search}
        onClearSearch={() => void applySearch({ ...state.search, text: "" })}
        chips={state.chips}
        columns={[...(allMetas ?? []).map((c) => ({ name: c.name, label: c.name })), { name: MARK_COLUMN, label: "Mark" }]}
        onToggle={(id) => applyChips((chips) => toggleChip(chips, id))}
        onRemove={(id) => applyChips((chips) => removeChip(chips, id))}
        onClear={() => {
          setClearToken((t) => t + 1);
          applyChips(() => []);
        }}
        onPlus={(column, x, y, opener) => setPanel({ column, x, y, opener })}
        onAdvanced={() => setAdvancedOpen(true)}
        onSavePreset={savePreset}
      />
      {settings.histogram && allMetas !== null && (
        <Histogram
          tabId={tab.id}
          relation={relation}
          columns={allMetas}
          primaryTime={summary.relations.find((r) => r.name === relation)?.primary_time ?? null}
          state={state}
          zone={summary.display_zone}
          version={dataVersion}
          enabled={active}
          applyChips={applyChips}
          onError={reportError}
        />
      )}
      {hidden !== null && (
        <div className="grid-notice" role="status">
          <span>This row is hidden by the current filters.</span>
          <button type="button" onClick={() => openUnfiltered(hidden)}>{`Open ${displaySafe(relation)} unfiltered`}</button>
          <button type="button" aria-label="Dismiss" onClick={() => setHidden(null)}>×</button>
        </div>
      )}
      <GroupBar
        by={by}
        onAdd={addGroup}
        onChange={changeBy}
        onExpandAll={tree.expandAll}
        onCollapseAll={tree.collapseAll}
      />
      <div
        ref={scrollRef}
        className="grid-scroll"
        data-testid="grid-scroll"
        onScroll={(e) => {
          if (!active || !restored.current) return;
          lastOffset.current = e.currentTarget.scrollTop;
          onScroll(e.currentTarget.scrollTop);
        }}
      >
        <div
          ref={gridRef}
          role="grid"
          tabIndex={0}
          aria-label="Rows"
          aria-rowcount={rowCount + 1}
          aria-colcount={names.length + 1}
          aria-multiselectable="true"
          className="grid-inner"
          style={{ width }}
          onKeyDown={onKeyDown}
        >
          <GridHeader
            table={table}
            meta={meta}
            width={width}
            pinLeft={pinLeft}
            onSort={onSort}
            onMenu={onHeaderMenu}
            restore={restore}
            onResize={(c, w) => edit((l) => setWidth(l, c, w))}
            onBestFit={(c) => fit([c])}
            onMove={(c, t) => edit((l) => moveColumn(l, c, t))}
            onFilter={(column, x, y) => setPanel({ column, x, y })}
            filtered={filtered}
          />
          <FilterRow names={names} widths={widths} pinLeft={pinLeft} width={width} chips={state.chips} clearToken={clearToken} onText={onFilterRowText} />
          {grouped ? (
            <GroupBody
              virtualizer={virtualizer}
              items={tree.items}
              by={by}
              names={names}
              widths={widths}
              pinLeft={pinLeft}
              width={width}
              rowHeight={rowHeight}
              onToggle={tree.toggle}
              onOpen={openGroup}
              onMore={tree.more}
            />
          ) : (
          <GridBody
            virtualizer={virtualizer}
            rowAt={rowAt}
            widths={widths}
            cellIdx={cellIdx}
            colNames={names}
            rules={rules}
            ruleColumns={data.ruleColumns}
            pinLeft={pinLeft}
            width={width}
            rowHeight={rowHeight}
            isSelected={(i) => contains(selection, i)}
            focus={focus}
            onCellClick={onCellClick}
            onCellContextMenu={onCellContextMenu}
          />
          )}
        </div>
      </div>
      {menu && (
        <CellMenu
          x={menu.x}
          y={menu.y}
          onCopyCell={copyCell}
          onCopyRows={copyRows}
          onShowValue={showValue}
          onFilterIn={() => cellFilter("eq")}
          onFilterOut={() => cellFilter("ne")}
          onClose={closeMenu}
        />
      )}
      {headerMenu && (
        <HeaderMenu
          x={headerMenu.x}
          y={headerMenu.y}
          pinned={resolved?.pinned.has(headerMenu.column) ?? false}
          canHide={(resolved?.shown.length ?? 0) > 1}
          onSort={(descending) => changeState((s) => ({ ...s, sort: [{ column: headerMenu.column, descending }] }))}
          onGroupBy={() => addGroup(headerMenu.column)}
          onFilter={() => setPanel({ column: headerMenu.column, x: headerMenu.x, y: headerMenu.y })}
          onPin={() => edit((l) => togglePin(l, headerMenu.column))}
          onBestFit={() => fit([headerMenu.column])}
          onHide={() => hideColumns([headerMenu.column])}
          onClose={closeHeaderMenu}
        />
      )}
      {managerOpen && resolved && (
        <ColumnManager
          all={resolved.all}
          hidden={resolved.hidden}
          pinned={resolved.pinned}
          busy={emptyBusy}
          onToggle={toggleColumn}
          onPin={(c) => edit((l) => togglePin(l, c))}
          onMove={(c, t) => edit((l) => moveColumn(l, c, t))}
          onShowAll={() => edit(showAll)}
          onHideEmpty={hideEmpty}
          onBestFit={() => fit(resolved.shown.map((c) => c.name))}
          onReset={resetLayout}
          onClose={() => setManagerOpen(false)}
        />
      )}
      {panel && (
        <FilterPanel
          tabId={tab.id}
          relation={relation}
          column={panel.column}
          meta={meta.get(panel.column)}
          state={state}
          zone={summary.display_zone}
          x={panel.x}
          y={panel.y}
          applyChips={applyChips}
          validate={validate}
          onError={reportError}
          returnFocus={panel.opener}
          onClose={closePanel}
        />
      )}
      {advancedOpen && allMetas && (
        <AdvancedEditor
          columns={allMetas}
          validate={validate}
          onAdd={(chip) => applyChips((chips) => [...chips, chip])}
          onClose={() => {
            setAdvancedOpen(false);
            gridRef.current?.focus();
          }}
        />
      )}
      {rulesOpen && (
        <ColourRules
          rules={rules}
          usingDefaults={savedRules === null}
          errors={data.ruleErrors}
          canFromFilter={filterFromChips(state.chips) !== null}
          onChange={setRules}
          onFromFilter={() => {
            const filter = filterFromChips(state.chips);
            if (filter) setRules([...rules, newRule(filter, rules)]);
          }}
          onReset={resetRules}
          onClose={() => setRulesOpen(false)}
        />
      )}
      {viewer && (
        <ValueViewer tabId={tab.id} rowId={viewer.rowId} column={viewer.column} onClose={closeViewer} onError={reportError} />
      )}
    </div>
  );
}
