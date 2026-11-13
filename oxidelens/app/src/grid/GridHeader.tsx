// The header row: the fixed mark column, then one cell per visible column. A click sorts (ascending, descending, off),
// shift-click adds a secondary key. A timestamp column whose zone is not known carries the `?tz` badge.
import type { Table } from "@tanstack/react-table";
import type { ReactNode } from "react";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";
import { displaySafe } from "../text";

export const MARK_WIDTH = 28;

/** A default width by what the column holds; column management (Task 16) lets the analyst change it. */
export function columnWidth(c: ColumnMeta): number {
  if (c.sql_type === "TIMESTAMP") return 200;
  if (c.sql_type === "BIGINT" || c.sql_type === "UBIGINT") return 100;
  return 160;
}

/** Timestamp columns not known to be UTC are shown as written. */
export const needsZoneBadge = (c: ColumnMeta | undefined) =>
  c !== undefined && c.time_semantics !== null && c.time_semantics !== "utc";

export interface GridHeaderProps {
  table: Table<unknown>;
  meta: ReadonlyMap<string, ColumnMeta>;
  width: number;
  onSort(column: string, additive: boolean): void;
}

export default function GridHeader({ table, meta, width, onSort }: GridHeaderProps): ReactNode {
  const leaves = table.getVisibleLeafColumns();
  const multi = table.getState().sorting.length > 1;
  return (
    <div className="grid-header" role="row" style={{ width }}>
      <div
        role="columnheader"
        aria-colindex={1}
        aria-label="Mark"
        data-fixed="true"
        className="hcell hcell-mark"
        style={{ width: MARK_WIDTH }}
      />
      {leaves.map((col, i) => {
        const sorted = col.getIsSorted();
        return (
          <div
            key={col.id}
            role="columnheader"
            aria-colindex={i + 2}
            aria-sort={sorted === "asc" ? "ascending" : sorted === "desc" ? "descending" : undefined}
            className="hcell"
            style={{ width: col.getSize() }}
            onClick={(e) => onSort(col.id, e.shiftKey)}
          >
            <span className="col-name">{displaySafe(col.id)}</span>
            {needsZoneBadge(meta.get(col.id)) && (
              <span className="tz-badge" title="Zone unknown: shown as written">?tz</span>
            )}
            {sorted !== false && (
              <span className="sort-ind" aria-hidden="true">
                {sorted === "asc" ? "▲" : "▼"}
                {multi ? col.getSortIndex() + 1 : ""}
              </span>
            )}
          </div>
        );
      })}
    </div>
  );
}
