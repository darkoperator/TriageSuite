// The filter row: a box under every column header. Typing makes or replaces that column's filter-row chip (`contains`,
// case-insensitive) after `FILTER_ROW_MS` of quiet; emptying a box removes it. What a box shows is what the analyst has
// typed since the chip last changed, else the chip's own text, so removing the chip elsewhere empties the box.
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { Chip } from "../contract/generated/Chip";
import { displaySafe } from "../text";
import { filterRowId, filterRowText } from "./chips";
import { MARK_WIDTH } from "./GridHeader";

export const FILTER_ROW_MS = 300;

export interface FilterRowProps {
  names: readonly string[];
  widths: readonly number[];
  pinLeft: readonly (number | null)[];
  width: number;
  chips: readonly Chip[];
  /** Changes each time Clear all is pressed: every pending edit is dropped. */
  clearToken: number;
  /** The box of `column` settled on `text` (empty removes the chip). */
  onText(column: string, text: string): void;
}

export default function FilterRow(p: FilterRowProps): ReactNode {
  const [drafts, setDrafts] = useState<Readonly<Record<string, string>>>({});
  const timers = useRef(new Map<string, number>());
  // The timer fires later than the render that set it: it calls whatever handler is current then.
  const onText = useRef(p.onText);
  useEffect(() => {
    onText.current = p.onText;
  });
  useEffect(() => {
    const live = timers.current;
    return () => live.forEach((t) => window.clearTimeout(t));
  }, []);

  // A filter-row chip taken away (its ×) takes that box's pending edit with it (Clear all uses `clearToken`). Otherwise the timer would put the chip straight back.
  const seen = useRef<readonly Chip[]>(p.chips);
  useEffect(() => {
    const before = seen.current;
    seen.current = p.chips;
    const now = new Set(p.chips.map((c) => c.id));
    const gone = new Set(before.filter((c) => !now.has(c.id)).map((c) => c.id));
    if (gone.size === 0) return;
    const cancel = (column: string) => {
      window.clearTimeout(timers.current.get(column));
      timers.current.delete(column);
    };
    const columns = [...timers.current.keys()].filter((c) => gone.has(filterRowId(c)));
    columns.forEach(cancel);
    setDrafts((d) => {
      const rest = { ...d };
      for (const c of Object.keys(rest)) if (gone.has(filterRowId(c))) delete rest[c];
      return rest;
    });
  }, [p.chips]);

  // Clear all says so itself: the chip count reaching zero is also what removing the last chip does.
  const cleared = useRef(p.clearToken);
  useEffect(() => {
    if (cleared.current === p.clearToken) return;
    cleared.current = p.clearToken;
    timers.current.forEach((t) => window.clearTimeout(t));
    timers.current.clear();
    setDrafts({});
  }, [p.clearToken]);

  const type = (column: string, text: string) => {
    setDrafts((d) => ({ ...d, [column]: text }));
    window.clearTimeout(timers.current.get(column));
    timers.current.set(
      column,
      window.setTimeout(() => {
        timers.current.delete(column);
        onText.current(column, text);
        setDrafts((d) => {
          const { [column]: _settled, ...rest } = d;
          void _settled;
          return rest;
        });
      }, FILTER_ROW_MS),
    );
  };

  return (
    <div className="filter-row" role="row" style={{ width: p.width }}>
      <div role="gridcell" className="fcell fcell-mark" style={{ width: MARK_WIDTH }} />
      {p.names.map((name, i) => {
        const left = p.pinLeft[i];
        return (
          <div
            key={name}
            role="gridcell"
            className={left === null || left === undefined ? "fcell" : "fcell fcell-pinned"}
            style={{ width: p.widths[i], left: left ?? undefined }}
          >
            <input
              type="text"
              className="fbox"
              aria-label={`Match ${displaySafe(name)}`}
              placeholder="filter"
              spellCheck={false}
              value={drafts[name] ?? filterRowText(p.chips, name)}
              onChange={(e) => type(name, e.target.value)}
            />
          </div>
        );
      })}
    </div>
  );
}
