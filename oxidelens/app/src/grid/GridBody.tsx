// The body: only the rows the virtualiser says are near the viewport exist in the DOM. A row whose block has not
// arrived yet is drawn empty. Cells show `display ?? text` through `displaySafe`; nothing from a cell is ever markup,
// a class name or a style.
import type { Virtualizer } from "@tanstack/react-virtual";
import type { MouseEvent, ReactNode } from "react";
import type { GridCell } from "../contract/generated/GridCell";
import { displaySafe } from "../text";
import { MARK_WIDTH } from "./GridHeader";
import type { GridRow } from "./useGridData";

export interface GridBodyProps {
  virtualizer: Virtualizer<HTMLDivElement, Element>;
  rowAt(index: number): GridRow | null;
  widths: number[];
  /** Per visible column: its index in the page's cells (columns are drawn in layout order, pages hold catalog order). */
  cellIdx: number[];
  /** Per visible column: the `left` offset of a pinned column, else null. */
  pinLeft: (number | null)[];
  width: number;
  rowHeight: number;
  isSelected(index: number): boolean;
  focus: { row: number; col: number } | null;
  onCellClick(e: MouseEvent, index: number, col: number): void;
  onCellContextMenu(e: MouseEvent, index: number, col: number): void;
}

export function CellText({ cell }: { cell: GridCell }): ReactNode {
  const text = displaySafe(cell.display ?? cell.text ?? "");
  if (cell.failed) {
    return (
      <span className="cell-text cell-failed" title="did not convert" data-clipped={cell.clipped ? "true" : undefined}>
        {text}
        {cell.clipped ? "…" : ""}
      </span>
    );
  }
  if (cell.clipped) {
    return (
      <span className="cell-text" data-clipped="true">
        {text}
        {"…"}
      </span>
    );
  }
  return <span className="cell-text">{text}</span>;
}

export default function GridBody(p: GridBodyProps): ReactNode {
  return (
    <div className="grid-body" style={{ height: p.virtualizer.getTotalSize(), width: p.width }}>
      {p.virtualizer.getVirtualItems().map((v) => {
        const row = p.rowAt(v.index);
        const isSelected = row !== null && p.isSelected(v.index);
        const classes = ["grid-row", v.index % 2 === 1 ? "row-odd" : "", isSelected ? "row-selected" : "", row?.hit ? "row-hit" : ""];
        return (
          <div
            key={v.index}
            role="row"
            data-row-index={v.index}
            aria-rowindex={v.index + 2}
            aria-selected={row ? isSelected : undefined}
            className={classes.filter(Boolean).join(" ")}
            style={{ height: p.rowHeight, transform: `translateY(${v.start}px)`, width: p.width }}
          >
            <div
              role="gridcell"
              aria-colindex={1}
              className="cell cell-mark"
              style={{ width: MARK_WIDTH }}
              onClick={(e) => row && p.onCellClick(e, v.index, p.focus?.col ?? 0)}
            >
              {row?.mark === "suspicious" && (
                <span className="mark-suspicious" role="img" aria-label="suspicious">
                  ●
                </span>
              )}
            </div>
            {p.widths.map((w, c) => {
              const cell = p.cellIdx[c] >= 0 ? row?.cells[p.cellIdx[c]] : undefined;
              const left = p.pinLeft[c];
              const focused = p.focus !== null && p.focus.row === v.index && p.focus.col === c;
              return (
                <div
                  key={c}
                  role="gridcell"
                  aria-colindex={c + 2}
                  className={left === null ? "cell" : "cell cell-pinned"}
                  data-pinned={left === null ? undefined : "true"}
                  data-focused={focused ? "true" : undefined}
                  style={{ width: w, left: left ?? undefined }}
                  onClick={(e) => row && p.onCellClick(e, v.index, c)}
                  onContextMenu={(e) => row && p.onCellContextMenu(e, v.index, c)}
                >
                  {cell && <CellText cell={cell} />}
                </div>
              );
            })}
          </div>
        );
      })}
    </div>
  );
}
