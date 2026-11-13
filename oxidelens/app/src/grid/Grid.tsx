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
import { columns as readColumns, gridEmptyColumns, gridPage, isCmdError, viewSave } from "../api";
import type { Chip } from "../contract/generated/Chip";
import type { ColumnLayout } from "../contract/generated/ColumnLayout";
import type { GridPage } from "../contract/generated/GridPage";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";
import type { GridState } from "../contract/generated/GridState";
import { useShell } from "../shell/context";
import { displaySafe } from "../text";
import type { TabBodyProps } from "../shell/slots";
import AdvancedEditor from "./AdvancedEditor";
import CellMenu from "./CellMenu";
import ChipBar from "./ChipBar";
import {
  MARK_COLUMN, MAX_CHIPS, cellChip, filterColumns, filterRowChip, filterRowId, removeChip, toggleChip, upsertChip,
} from "./chips";
import ColumnManager from "./ColumnManager";
import { cellText, rowsAsTsv, writeClipboard } from "./copy";
import FilterPanel from "./FilterPanel";
import FilterRow from "./FilterRow";
import GridBody from "./GridBody";
import GridHeader, { MARK_WIDTH } from "./GridHeader";
import { initialGridState, nextSort } from "./gridState";
import HeaderMenu from "./HeaderMenu";
import {
  bestFitWidth, fetchedColumns, hide, materialise, moveColumn, resolve, setWidth, setWidths, show, showAll, togglePin,
} from "./layout";
import { measureText } from "./measure";
import { BLOCK_ROWS, blockOf } from "./blocks";
import { MAX_COPY_ROWS, contains, emptySelection, indices, selectOnly, selectRange, size, toggle, type Selection } from "./selection";
import { useGridData } from "./useGridData";
import ValueViewer from "./ValueViewer";
import "./grid.css";

const NO_ROWS: unknown[] = [];
/** A layout change is saved this long after the last one. */
const LAYOUT_SAVE_MS = 250;
const fmt = (n: number) => n.toLocaleString("en-US");
const plural = (n: number, one: string, many: string) => (n === 1 ? one : many);

interface Focus {
  row: number;
  col: number;
}

export default function Grid({ tab, active, scrollOffset, onScroll }: TabBodyProps): ReactNode {
  const { summary, settings, change, setGridStatus, reportError, saveLayout } = useShell();
  const relation = tab.relation ?? "";

  const [state, setState] = useState<GridState>(() => initialGridState(relation));
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
  // The column layout: what the settings held when the grid opened, then what the analyst does. Opening writes nothing;
  // a change is saved `LAYOUT_SAVE_MS` after the last one (see `edit`).
  const [layout, setLayout] = useState<ColumnLayout | null>(() => settings.layouts[relation] ?? null);
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

  const data = useGridData({
    tabId: tab.id, state: shownState, zone: summary.display_zone, enabled: active && allMetas !== null, onError: reportError,
  });
  const { want, refresh, rowAt, beginCopy, loadedTexts } = data;

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

  // A write to the case (a mark, a load, a zone) re-reads the rows; filters, selection and scroll stay.
  const seenGeneration = useRef(change.generation);
  useEffect(() => {
    if (change.generation === seenGeneration.current) return;
    seenGeneration.current = change.generation;
    if (change.kinds.some((k) => k !== "views")) refresh();
  }, [change, refresh]);

  // ---- virtual rows -----------------------------------------------------------------------------------------------
  const rowHeight = settings.font_size + 10;
  const virtualizer = useVirtualizer({
    count: data.total,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    overscan: 12,
    initialOffset: scrollOffset,
  });
  const items = virtualizer.getVirtualItems();
  const firstShown = items.length > 0 ? items[0].index : 0;
  const lastShown = items.length > 0 ? items[items.length - 1].index : 0;
  useEffect(() => {
    if (items.length > 0) want(firstShown, lastShown);
  }, [firstShown, lastShown, items.length, want]);

  // ---- scroll: restore on mount and on showing the tab, report as it moves ----------------------------------------
  const lastOffset = useRef(scrollOffset);
  const restored = useRef(scrollOffset === 0);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!active || !el) return;
    // The scrollbar only spans the saved offset once the row count is known; until then a restore would be clamped.
    if (!restored.current && data.total * rowHeight < lastOffset.current + el.clientHeight) return;
    restored.current = true;
    el.scrollTop = lastOffset.current;
  }, [active, data.total, rowHeight]);

  // ---- the status line --------------------------------------------------------------------------------------------
  const shownColumns = names.length;
  const totalColumns = allMetas?.length ?? shownColumns;
  const counts = data.counts;
  const selectedCount = size(selection);
  const statusNode = useMemo(
    () =>
      counts === null ? null : (
        <>
          {`${fmt(counts.rows)} of ${fmt(counts.total)} rows · ${fmt(selectedCount)} selected · ${fmt(shownColumns)} of ${fmt(totalColumns)} columns · `}
          <button type="button" className="status-link" aria-haspopup="dialog" onClick={() => setManagerOpen((o) => !o)}>
            {`${fmt(totalColumns - shownColumns)} hidden ▾`}
          </button>
          {` · ${fmt(counts.failed_values)} ${plural(counts.failed_values, "value", "values")} failed to convert`}
        </>
      ),
    [counts, selectedCount, shownColumns, totalColumns],
  );
  useEffect(() => {
    if (!active) return;
    setGridStatus(statusNode);
    return () => setGridStatus(null);
  }, [active, statusNode, setGridStatus]);

  // ---- selection and focus ----------------------------------------------------------------------------------------
  const onCellClick = (e: MouseEvent, index: number, col: number) => {
    setFocus({ row: index, col });
    setMenu(null);
    if (e.shiftKey) setSelection((s) => selectRange(s, index));
    else if (e.ctrlKey || e.metaKey) setSelection((s) => toggle(s, index));
    else setSelection(selectOnly(index));
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
      case " ":
        e.preventDefault();
        showValue();
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

  return (
    <div ref={rootRef} className="grid" style={{ "--cell-size": `${settings.font_size}px` } as CSSProperties}>
      <ChipBar
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
          aria-rowcount={data.total + 1}
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
          <GridBody
            virtualizer={virtualizer}
            rowAt={rowAt}
            widths={widths}
            cellIdx={cellIdx}
            pinLeft={pinLeft}
            width={width}
            rowHeight={rowHeight}
            isSelected={(i) => contains(selection, i)}
            focus={focus}
            onCellClick={onCellClick}
            onCellContextMenu={onCellContextMenu}
          />
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
      {viewer && (
        <ValueViewer tabId={tab.id} rowId={viewer.rowId} column={viewer.column} onClose={closeViewer} onError={reportError} />
      )}
    </div>
  );
}
