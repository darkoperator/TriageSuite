// The group bar above the grid: the columns grouped by, in order (up to four), each removable and movable, with Expand
// all and Collapse all. A header dragged onto the bar joins it; so does Group by this column in the header menu and the
// Mark button. The bar holds no rules: it hands the new list to its owner, which refuses a fifth column.
import type { DragEvent, ReactNode } from "react";
import { displaySafe } from "../text";
import { MARK_COLUMN } from "./chips";

/** The drag data type a column header carries. */
export const COLUMN_DRAG = "application/x-oxl-column";

export interface GroupBarProps {
  by: readonly string[];
  /** Adds a column at the end (the owner refuses duplicates and a fifth). */
  onAdd(column: string): void;
  onChange(by: string[]): void;
  onExpandAll(): void;
  onCollapseAll(): void;
}

const label = (column: string) => (column === MARK_COLUMN ? "Mark" : column);

export default function GroupBar(p: GroupBarProps): ReactNode {
  const move = (i: number, d: -1 | 1) => {
    const next = [...p.by];
    [next[i], next[i + d]] = [next[i + d], next[i]];
    p.onChange(next);
  };
  const accepts = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes(COLUMN_DRAG);
  return (
    <div
      role="group"
      aria-label="Group by"
      className="group-bar"
      onDragOver={(e) => {
        if (accepts(e)) e.preventDefault();
      }}
      onDrop={(e) => {
        if (!accepts(e)) return;
        e.preventDefault();
        const column = e.dataTransfer.getData(COLUMN_DRAG);
        if (column !== "") p.onAdd(column);
      }}
    >
      {p.by.length === 0 ? (
        <span className="group-hint">Drag a column header here to group by it</span>
      ) : (
        <ol className="group-pills">
          {p.by.map((c, i) => (
            <li key={c} className="group-pill">
              {i > 0 && <span aria-hidden="true">›</span>}
              <span className="group-pill-name">{displaySafe(label(c))}</span>
              <button type="button" className="icon-button" aria-label={`Move ${displaySafe(label(c))} earlier`} disabled={i === 0} onClick={() => move(i, -1)}>
                ◂
              </button>
              <button type="button" className="icon-button" aria-label={`Move ${displaySafe(label(c))} later`} disabled={i === p.by.length - 1} onClick={() => move(i, 1)}>
                ▸
              </button>
              <button type="button" className="icon-button" aria-label={`Remove grouping ${displaySafe(label(c))}`} onClick={() => p.onChange(p.by.filter((x) => x !== c))}>
                ×
              </button>
            </li>
          ))}
        </ol>
      )}
      {!p.by.includes(MARK_COLUMN) && (
        <button type="button" className="group-action" aria-label="Group by Mark" onClick={() => p.onAdd(MARK_COLUMN)}>
          + Mark
        </button>
      )}
      {p.by.length > 0 && (
        <>
          <button type="button" className="group-action" onClick={p.onExpandAll}>
            Expand all
          </button>
          <button type="button" className="group-action" onClick={p.onCollapseAll}>
            Collapse all
          </button>
        </>
      )}
    </div>
  );
}
