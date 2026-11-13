// The body of a grouped grid: the same virtual list as the flat body, but each row is one of the flattened tree's items
// (see `groupTree.ts`): a group header (key, count, first and last time in the zone, suspicious count), a data row of an
// expanded leaf group, a "more" row or a status row. Text from the case goes through `displaySafe`; the indent is a
// number from the item's depth, never a value from the data.
import type { Virtualizer } from "@tanstack/react-virtual";
import type { ReactNode } from "react";
import { displaySafe } from "../text";
import { CellText } from "./GridBody";
import { MARK_WIDTH } from "./GridHeader";
import { MARK_COLUMN } from "./chips";
import { GROUP_PAGE, type Item, type Key } from "./groupTree";

const fmt = (n: number) => n.toLocaleString("en-US");
const INDENT = 16;

/** How a group's key reads: an empty cell is `(empty)` (no mark is `(no mark)`), a cut key ends in an ellipsis. */
export function keyText(column: string, key: string | null, clipped: boolean): string {
  if (key === null || key === "") return column === MARK_COLUMN ? "(no mark)" : "(empty)";
  return displaySafe(key) + (clipped ? "…" : "");
}

export interface GroupBodyProps {
  virtualizer: Virtualizer<HTMLDivElement, Element>;
  items: readonly Item[];
  by: readonly string[];
  /** The visible columns, in the order drawn, their widths and pinned offsets. */
  names: readonly string[];
  widths: readonly number[];
  pinLeft: readonly (number | null)[];
  width: number;
  rowHeight: number;
  onToggle(path: Key[]): void;
  onOpen(path: Key[]): void;
  onMore(kind: "groups" | "rows", path: Key[]): void;
}

export default function GroupBody(p: GroupBodyProps): ReactNode {
  return (
    <div className="grid-body" style={{ height: p.virtualizer.getTotalSize(), width: p.width }}>
      {p.virtualizer.getVirtualItems().map((v) => {
        const item = p.items[v.index];
        if (!item) return null;
        const style = { height: p.rowHeight, transform: `translateY(${v.start}px)`, width: p.width };
        const odd = v.index % 2 === 1 ? " row-odd" : "";
        const indent = { paddingLeft: 8 + item.depth * INDENT };
        switch (item.kind) {
          case "group": {
            const g = item.group;
            const column = p.by[item.depth] ?? "";
            const name = column === MARK_COLUMN ? "Mark" : column;
            return (
              <div
                key={item.id}
                role="row"
                data-kind="group"
                data-depth={item.depth}
                aria-level={item.depth + 1}
                aria-expanded={item.open}
                className={`grid-row group-row${odd}`}
                style={style}
                onDoubleClick={() => p.onOpen(item.path)}
              >
                <div role="gridcell" className="group-cell" style={{ ...indent, width: p.width }}>
                  <button
                    type="button"
                    className="group-toggle"
                    aria-expanded={item.open}
                    aria-label={`${item.open ? "Collapse" : "Expand"} ${displaySafe(name)} ${keyText(column, g.key, g.key_clipped)}`}
                    onClick={() => p.onToggle(item.path)}
                  >
                    {item.open ? "▾" : "▸"}
                  </button>
                  <span className="group-col">{displaySafe(name)}</span>
                  <span className="group-value" title={g.key_clipped ? "The key is cut here; the whole key is used to open the group" : undefined}>
                    {keyText(column, g.key, g.key_clipped)}
                  </span>
                  <span className="group-count">{`${fmt(g.count)} ${g.count === 1 ? "row" : "rows"}`}</span>
                  {g.first !== null && (
                    <span className="group-span">
                      {displaySafe(g.first)}
                      {g.last !== null && g.last !== g.first ? ` – ${displaySafe(g.last)}` : ""}
                    </span>
                  )}
                  {g.suspicious > 0 && <span className="group-suspicious">{`${fmt(g.suspicious)} suspicious`}</span>}
                </div>
              </div>
            );
          }
          case "row": {
            const mark = item.page.marks[item.k] ?? null;
            const cells = item.page.cells[item.k];
            return (
              <div
                key={item.id}
                role="row"
                data-kind="row"
                data-depth={item.depth}
                className={`grid-row${odd}`}
                style={style}
              >
                <div role="gridcell" className="cell cell-mark" style={{ width: MARK_WIDTH }}>
                  {mark === "suspicious" && (
                    <span className="mark-suspicious" role="img" aria-label="suspicious">
                      ●
                    </span>
                  )}
                </div>
                {p.widths.map((w, c) => {
                  const at = item.page.columns.findIndex((col) => col.name === p.names[c]);
                  const cell = at >= 0 ? cells[at] : undefined;
                  const left = p.pinLeft[c];
                  return (
                    <div
                      key={c}
                      role="gridcell"
                      className={left === null ? "cell" : "cell cell-pinned"}
                      style={{ width: w, left: left ?? undefined }}
                    >
                      {cell && <CellText cell={cell} />}
                    </div>
                  );
                })}
              </div>
            );
          }
          case "more-groups":
          case "more-rows": {
            const groups = item.kind === "more-groups";
            return (
              <div key={item.id} role="row" data-kind="more" className={`grid-row${odd}`} style={style}>
                <div role="gridcell" className="group-cell" style={{ ...indent, width: p.width }}>
                  <button
                    type="button"
                    className="group-more"
                    disabled={item.busy}
                    onClick={() => p.onMore(groups ? "groups" : "rows", item.path)}
                  >
                    {item.busy ? "loading…" : `… more ${groups ? "groups" : "rows"} (next ${GROUP_PAGE})`}
                  </button>
                </div>
              </div>
            );
          }
          case "status":
            return (
              <div key={item.id} role="row" data-kind="status" className={`grid-row${odd}`} style={style}>
                <div role="gridcell" className="group-cell group-status" style={{ ...indent, width: p.width }}>
                  {displaySafe(item.text)}
                </div>
              </div>
            );
        }
      })}
    </div>
  );
}
