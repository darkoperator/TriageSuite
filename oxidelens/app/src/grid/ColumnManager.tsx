// The column manager: every column of the dataset with show/hide, a drag handle, and pin; a find box; and Show all,
// Hide empty, Best fit and Reset. It only reports what the analyst chose; the grid applies it to the layout.
import { useRef, useState, type ReactNode } from "react";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";
import { displaySafe } from "../text";

export interface ColumnManagerProps {
  /** Every column in layout order (pinned first). */
  all: readonly ColumnMeta[];
  hidden: ReadonlySet<string>;
  pinned: ReadonlySet<string>;
  /** True while Hide empty waits for the engine. */
  busy: boolean;
  onToggle(name: string): void;
  onPin(name: string): void;
  onMove(name: string, target: string): void;
  onShowAll(): void;
  onHideEmpty(): void;
  onBestFit(): void;
  onReset(): void;
  onClose(): void;
}

export default function ColumnManager(p: ColumnManagerProps): ReactNode {
  const [find, setFind] = useState("");
  const dragging = useRef<string | null>(null);
  const needle = find.trim().toLowerCase();
  const rows = needle === "" ? p.all : p.all.filter((c) => c.name.toLowerCase().includes(needle));

  return (
    <div
      className="column-manager"
      role="dialog"
      aria-label="Columns"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          p.onClose();
        }
      }}
    >
      <div className="cm-head">
        <input
          type="search"
          aria-label="Find column"
          placeholder="Find column"
          value={find}
          onChange={(e) => setFind(e.target.value)}
        />
        <button type="button" className="icon-button" aria-label="Close columns" onClick={p.onClose}>×</button>
      </div>
      <ul className="cm-list">
        {rows.map((c) => {
          const shown = displaySafe(c.name);
          const isPinned = p.pinned.has(c.name);
          return (
            <li
              key={c.name}
              className="cm-item"
              draggable
              onDragStart={(e) => {
                dragging.current = c.name;
                e.dataTransfer?.setData("text/plain", "");
              }}
              onDragOver={(e) => {
                if (dragging.current !== null) e.preventDefault();
              }}
              onDragEnd={() => (dragging.current = null)}
              onDrop={(e) => {
                e.preventDefault();
                const from = dragging.current;
                dragging.current = null;
                if (from !== null && from !== c.name) p.onMove(from, c.name);
              }}
            >
              <span className="cm-handle" aria-hidden="true">⋮⋮</span>
              <label className="cm-name">
                <input
                  type="checkbox"
                  aria-label={`Show ${shown}`}
                  checked={!p.hidden.has(c.name)}
                  onChange={() => p.onToggle(c.name)}
                />
                <span>{shown}</span>
              </label>
              <button
                type="button"
                className="cm-pin"
                aria-label={`${isPinned ? "Unpin" : "Pin"} ${shown}`}
                aria-pressed={isPinned}
                onClick={() => p.onPin(c.name)}
              >
                ⇤
              </button>
            </li>
          );
        })}
        {rows.length === 0 && <li className="cm-none">No column matches.</li>}
      </ul>
      <div className="cm-foot">
        <button type="button" onClick={p.onShowAll}>Show all</button>
        <button type="button" disabled={p.busy} onClick={p.onHideEmpty}>Hide empty</button>
        <button type="button" onClick={p.onBestFit}>Best fit</button>
        <button type="button" onClick={p.onReset}>Reset</button>
      </div>
    </div>
  );
}
