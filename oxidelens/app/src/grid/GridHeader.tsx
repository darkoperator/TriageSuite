// The header row: the fixed mark column, then one cell per visible column. A click sorts (ascending, descending, off),
// shift-click adds a secondary key. A header drags to a new place, its right edge drags to resize (double-click fits
// it to its text), a right-click opens the column menu. Pinned columns stick left of the scrolling area. A timestamp
// column whose zone is not known carries the `?tz` badge.
import type { Table } from "@tanstack/react-table";
import { useEffect, useRef, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";
import { displaySafe } from "../text";
import { COLUMN_DRAG } from "./GroupBar";

export const MARK_WIDTH = 28;

/** Timestamp columns not known to be UTC are shown as written. */
export const needsZoneBadge = (c: ColumnMeta | undefined) =>
  c !== undefined && c.time_semantics !== null && c.time_semantics !== "utc";

export interface GridHeaderProps {
  table: Table<unknown>;
  meta: ReadonlyMap<string, ColumnMeta>;
  width: number;
  /** Per visible column: the `left` offset of a pinned column, else null. */
  pinLeft: readonly (number | null)[];
  onSort(column: string, additive: boolean): void;
  /** Open the column menu at a point (a right-click, or the header's corner for Shift+F10 / the menu key). */
  onMenu(column: string, x: number, y: number): void;
  /** Set when a menu closed: put focus back on that column's header (the first one if it is gone). `token` makes each request new. */
  restore: { column: string; token: number } | null;
  /** The column's width while its edge is dragged (the layout clamps it). */
  onResize(column: string, width: number): void;
  onBestFit(column: string): void;
  /** `column` was dropped on `target`. */
  onMove(column: string, target: string): void;
  /** The ▾ of a column (or of the Mark cell, as `_mark`) was pressed: open its filter panel at this point. */
  onFilter(column: string, x: number, y: number): void;
  /** Columns a filter is applied to (they show their ▾ lit). */
  filtered: ReadonlySet<string>;
}

export default function GridHeader(p: GridHeaderProps): ReactNode {
  const { table, meta, width, pinLeft } = p;
  const leaves = table.getVisibleLeafColumns();
  const multi = table.getState().sorting.length > 1;
  const dragging = useRef<string | null>(null);
  // The header row is one tab stop: the current header has tabIndex 0, Left/Right move between headers.
  const cells = useRef(new Map<string, HTMLElement>());
  const [current, setCurrent] = useState<string | null>(null);
  const ids = leaves.map((c) => c.id);
  const tabStop = current !== null && ids.includes(current) ? current : (ids[0] ?? null);
  const { restore } = p;
  useEffect(() => {
    if (!restore) return;
    const id = ids.includes(restore.column) ? restore.column : ids[0];
    if (id === undefined) return;
    cells.current.get(id)?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- only a new request moves focus
  }, [restore?.token]);

  const onHeaderKey = (e: KeyboardEvent<HTMLDivElement>, id: string, i: number) => {
    if (e.target !== e.currentTarget) return;
    // The header sits inside the grid, whose handler moves the cell focus: the keys handled here stop here.
    if (["ArrowRight", "ArrowLeft", "Enter", "ContextMenu"].includes(e.key) || (e.key === "F10" && e.shiftKey)) e.stopPropagation();
    if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
      e.preventDefault();
      const to = ids[Math.min(ids.length - 1, Math.max(0, i + (e.key === "ArrowRight" ? 1 : -1)))];
      setCurrent(to);
      cells.current.get(to)?.focus();
    } else if (e.key === "Enter") {
      e.preventDefault();
      p.onSort(id, e.shiftKey);
    } else if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
      e.preventDefault();
      const r = e.currentTarget.getBoundingClientRect();
      p.onMenu(id, r.left, r.bottom);
    }
  };

  const startResize = (e: MouseEvent, id: string, start: number) => {
    e.preventDefault(); // no text selection, and the header does not start a drag
    e.stopPropagation();
    const x0 = e.clientX;
    const move = (m: globalThis.MouseEvent) => p.onResize(id, start + m.clientX - x0);
    const up = (m: globalThis.MouseEvent) => {
      document.removeEventListener("mousemove", move);
      document.removeEventListener("mouseup", up);
      p.onResize(id, start + m.clientX - x0);
    };
    document.addEventListener("mousemove", move);
    document.addEventListener("mouseup", up);
  };

  return (
    <div className="grid-header" role="row" style={{ width }}>
      <div
        role="columnheader"
        aria-colindex={1}
        aria-label="Mark"
        data-fixed="true"
        className="hcell hcell-mark"
        style={{ width: MARK_WIDTH }}
      >
        <FilterButton column="_mark" label="Mark" active={p.filtered.has("_mark")} onFilter={p.onFilter} />
      </div>
      {leaves.map((col, i) => {
        const sorted = col.getIsSorted();
        const left = pinLeft[i];
        return (
          <div
            key={col.id}
            role="columnheader"
            aria-colindex={i + 2}
            aria-sort={sorted === "asc" ? "ascending" : sorted === "desc" ? "descending" : undefined}
            className={left === null || left === undefined ? "hcell" : "hcell hcell-pinned"}
            data-pinned={left === null || left === undefined ? undefined : "true"}
            style={{ width: col.getSize(), left: left ?? undefined }}
            ref={(el) => {
              if (el) cells.current.set(col.id, el);
              else cells.current.delete(col.id);
            }}
            tabIndex={col.id === tabStop ? 0 : -1}
            onFocus={(e) => e.target === e.currentTarget && setCurrent(col.id)}
            onKeyDown={(e) => onHeaderKey(e, col.id, i)}
            draggable
            onDragStart={(e) => {
              dragging.current = col.id;
              e.dataTransfer?.setData("text/plain", "");
              e.dataTransfer?.setData(COLUMN_DRAG, col.id);
            }}
            onDragOver={(e) => {
              if (dragging.current !== null) e.preventDefault();
            }}
            onDrop={(e) => {
              e.preventDefault();
              const from = dragging.current;
              dragging.current = null;
              if (from !== null && from !== col.id) p.onMove(from, col.id);
            }}
            onDragEnd={() => (dragging.current = null)}
            onClick={(e) => p.onSort(col.id, e.shiftKey)}
            onContextMenu={(e) => {
              e.preventDefault();
              p.onMenu(col.id, e.clientX, e.clientY);
            }}
          >
            <span className="col-name">{displaySafe(col.id)}</span>
            {needsZoneBadge(meta.get(col.id)) && (
              <span className="tz-badge" title="Zone unknown: shown as written">?tz</span>
            )}
            <FilterButton column={col.id} label={displaySafe(col.id)} active={p.filtered.has(col.id)} onFilter={p.onFilter} />
            {sorted !== false && (
              <span className="sort-ind" aria-hidden="true">
                {sorted === "asc" ? "▲" : "▼"}
                {multi ? col.getSortIndex() + 1 : ""}
              </span>
            )}
            {/* eslint-disable-next-line jsx-a11y/click-events-have-key-events, jsx-a11y/no-noninteractive-element-interactions -- onClick only stops the header's sort; dragging is pointer-only and the keyboard path to a width is Best fit (header menu, column manager) */}
            <div
              role="separator"
              aria-orientation="vertical"
              aria-label={`Resize ${displaySafe(col.id)}`}
              className="col-grip"
              draggable={false}
              onClick={(e) => e.stopPropagation()}
              onMouseDown={(e) => startResize(e, col.id, col.getSize())}
              onDoubleClick={(e) => {
                e.stopPropagation();
                p.onBestFit(col.id);
              }}
            />
          </div>
        );
      })}
    </div>
  );
}

/** The ▾ that opens a column's filter panel. It is not a tab stop (the header row is one); Filter… in the header menu is the keyboard way. */
function FilterButton(p: { column: string; label: string; active: boolean; onFilter(column: string, x: number, y: number): void }): ReactNode {
  return (
    <button
      type="button"
      className="col-filter"
      tabIndex={-1}
      data-active={p.active ? "true" : undefined}
      aria-label={`Filter ${p.label}`}
      draggable={false}
      onClick={(e) => {
        e.stopPropagation();
        const r = e.currentTarget.getBoundingClientRect();
        p.onFilter(p.column, r.left, r.bottom);
      }}
    >
      ▾
    </button>
  );
}
