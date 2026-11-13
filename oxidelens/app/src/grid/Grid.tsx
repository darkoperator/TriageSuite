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
import { columns as readColumns, isCmdError } from "../api";
import type { GridPage } from "../contract/generated/GridPage";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";
import type { GridState } from "../contract/generated/GridState";
import { useShell } from "../shell/context";
import type { TabBodyProps } from "../shell/slots";
import CellMenu from "./CellMenu";
import { cellText, rowsAsTsv, writeClipboard } from "./copy";
import GridBody from "./GridBody";
import GridHeader, { MARK_WIDTH, columnWidth } from "./GridHeader";
import { initialGridState, nextSort } from "./gridState";
import { BLOCK_ROWS, blockOf } from "./blocks";
import { MAX_COPY_ROWS, contains, emptySelection, indices, selectOnly, selectRange, size, toggle, type Selection } from "./selection";
import { useGridData } from "./useGridData";
import ValueViewer from "./ValueViewer";
import "./grid.css";

const NO_ROWS: unknown[] = [];
const fmt = (n: number) => n.toLocaleString("en-US");
const plural = (n: number, one: string, many: string) => (n === 1 ? one : many);

interface Focus {
  row: number;
  col: number;
}

export default function Grid({ tab, active, scrollOffset, onScroll }: TabBodyProps): ReactNode {
  const { summary, settings, change, setGridStatus, reportError } = useShell();
  const relation = tab.relation ?? "";

  const [state, setState] = useState<GridState>(() => initialGridState(relation));
  const [selection, setSelection] = useState<Selection>(emptySelection);
  const [focus, setFocus] = useState<Focus | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [viewer, setViewer] = useState<{ rowId: number; column: string } | null>(null);
  const [catalogued, setCatalogued] = useState<number | null>(null);

  const rootRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);

  const data = useGridData({ tabId: tab.id, state, zone: summary.display_zone, enabled: active, onError: reportError });
  const { want, refresh, rowAt, beginCopy } = data;

  /** Every edit of what the grid shows goes through here: the selection's row positions mean nothing afterwards. */
  const changeState = useCallback((edit: (s: GridState) => GridState) => {
    setState(edit);
    setSelection(emptySelection);
    setMenu(null);
  }, []);

  // ---- columns ----------------------------------------------------------------------------------------------------
  const metas = data.columns;
  const meta = useMemo(() => new Map<string, ColumnMeta>((metas ?? []).map((c) => [c.name, c])), [metas]);
  const columnDefs = useMemo<ColumnDef<unknown>[]>(
    () => (metas ?? []).map((c) => ({ id: c.name, header: c.name, size: columnWidth(c) })),
    [metas],
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

  // How many columns the relation has, for the status line (the grid's own page only carries the shown ones).
  const generation = change.generation;
  useEffect(() => {
    let live = true;
    readColumns(relation, `tab:${tab.id}:columns`).then(
      (c) => live && setCatalogued(c.length),
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
  const totalColumns = Math.max(catalogued ?? shownColumns, shownColumns);
  const counts = data.counts;
  const selectedCount = size(selection);
  const statusText =
    counts === null
      ? null
      : `${fmt(counts.rows)} of ${fmt(counts.total)} rows · ${fmt(selectedCount)} selected · ${fmt(shownColumns)} of ${fmt(totalColumns)} columns · ${fmt(totalColumns - shownColumns)} hidden · ${fmt(counts.failed_values)} ${plural(counts.failed_values, "value", "values")} failed to convert`;
  useEffect(() => {
    if (!active) return;
    setGridStatus(statusText);
    return () => setGridStatus(null);
  }, [active, statusText, setGridStatus]);

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
    const cell = row?.cells[focus.col];
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

  const closeViewer = useCallback(() => {
    setViewer(null);
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

  return (
    <div ref={rootRef} className="grid" style={{ "--cell-size": `${settings.font_size}px` } as CSSProperties}>
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
          <GridHeader table={table} meta={meta} width={width} onSort={onSort} />
          <GridBody
            virtualizer={virtualizer}
            rowAt={rowAt}
            widths={widths}
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
        <CellMenu x={menu.x} y={menu.y} onCopyCell={copyCell} onCopyRows={copyRows} onShowValue={showValue} onClose={closeMenu} />
      )}
      {viewer && (
        <ValueViewer tabId={tab.id} rowId={viewer.rowId} column={viewer.column} onClose={closeViewer} onError={reportError} />
      )}
    </div>
  );
}
