// The tab strip: the Overview, the four pinned case sheets with their counts, then one tab per open dataset. Arrow keys, Home and End move between tabs, Delete closes
// the focused dataset tab. Roving tabindex: only the active tab is in the tab order.
import { useRef, type KeyboardEvent } from "react";
import { displaySafe } from "../text";
import { isPinned, type TabsState } from "./tabs";

export const tabDomId = (id: string) => `tab-${id}`;
export const panelDomId = (id: string) => `panel-${id}`;

export default function TabStrip({
  state, title, count, onSelect, onClose,
}: {
  state: TabsState;
  title(tabId: string): string;
  /** The count a pinned sheet shows beside its name (`null` until it is read). */
  count(tabId: string): string | null;
  onSelect(id: string): void;
  onClose(id: string): void;
}) {
  const refs = useRef(new Map<string, HTMLButtonElement>());
  const go = (id: string) => {
    onSelect(id);
    refs.current.get(id)?.focus();
  };
  const onKey = (e: KeyboardEvent, ix: number) => {
    const n = state.tabs.length;
    const id = state.tabs[ix].id;
    if (e.key === "ArrowRight") go(state.tabs[(ix + 1) % n].id);
    else if (e.key === "ArrowLeft") go(state.tabs[(ix - 1 + n) % n].id);
    else if (e.key === "Home") go(state.tabs[0].id);
    else if (e.key === "End") go(state.tabs[n - 1].id);
    else if (e.key === "Delete" && !isPinned(state.tabs[ix])) onClose(id);
    else return;
    e.preventDefault();
  };
  return (
    <div className="tab-strip" role="tablist" aria-label="Open tabs">
      {state.tabs.map((t, ix) => {
        const selected = t.id === state.active;
        const name = displaySafe(title(t.id));
        const n = count(t.id);
        return (
          <span key={t.id} className={selected ? "tab tab-active" : "tab"}>
            <button
              type="button"
              role="tab"
              id={tabDomId(t.id)}
              aria-selected={selected}
              aria-controls={panelDomId(t.id)}
              tabIndex={selected ? 0 : -1}
              ref={(el) => {
                if (el) refs.current.set(t.id, el);
                else refs.current.delete(t.id);
              }}
              onClick={() => onSelect(t.id)}
              onKeyDown={(e) => onKey(e, ix)}
            >
              {name}
              {n !== null && <>{" "}<span className="tab-count">{n}</span></>}
            </button>
            {!isPinned(t) && (
              <button type="button" className="tab-close" tabIndex={-1} aria-label={`Close ${name}`} onClick={() => onClose(t.id)}>
                ×
              </button>
            )}
          </span>
        );
      })}
    </div>
  );
}
