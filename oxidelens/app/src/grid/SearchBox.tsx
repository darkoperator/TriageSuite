// The search box: Timeline Explorer syntax in the text (the engine parses it), the Rx and Aa toggles, the filter | find
// switch and, in find mode, `k of n` with ▲ ▼. Enter or 400 ms without typing applies; a toggle applies at once. The
// box keeps what the analyst typed; what the grid shows (`search`) changes only when the grid accepts it, so an
// engine refusal leaves the results as they were and its message sits under the box.
import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import type { SearchState } from "../contract/generated/SearchState";
import { displaySafe } from "../text";

/** Quiet time before typing applies. */
export const SEARCH_IDLE_MS = 400;

const keyOf = (s: SearchState) => JSON.stringify([s.text, s.regex, s.case_sensitive, s.mode]);

export interface SearchBoxProps {
  /** What the grid shows now. */
  search: SearchState;
  /** The engine's message for the last text it refused; null when none. */
  error: string | null;
  /** Find mode with a text: how many rows match (null until known) and which one the analyst is on (0 = none yet). */
  find: { count: number | null; position: number } | null;
  onApply(next: SearchState): void;
  onStep(direction: 1 | -1): void;
}

export default function SearchBox({ search, error, find, onApply, onStep }: SearchBoxProps): ReactNode {
  const [draft, setDraft] = useState(search);
  const [seen, setSeen] = useState(keyOf(search));
  const [sent, setSent] = useState<string | null>(null);
  const [adopted, setAdopted] = useState(0);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);

  // Something other than this box changed the search (the pill's ×, a saved view): the box follows it. What this box
  // itself sent is not echoed back over a newer keystroke.
  const propKey = keyOf(search);
  if (propKey !== seen) {
    setSeen(propKey);
    if (propKey !== sent) {
      setDraft(search);
      setSent(null);
      setAdopted((n) => n + 1);
    }
  }
  // An older draft's pending idle apply must not land over the value just adopted.
  useEffect(() => {
    if (adopted > 0) window.clearTimeout(timer.current);
  }, [adopted]);

  const submit = (next: SearchState) => {
    window.clearTimeout(timer.current);
    setDraft(next);
    setSent(keyOf(next));
    onApply(next);
  };

  const onType = (text: string) => {
    const next = { ...draft, text };
    setDraft(next);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => submit(next), SEARCH_IDLE_MS);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      submit(draft);
    } else if (e.key === "F3") {
      // A function key types nothing, so stepping from the box is safe; every other key stays with the box.
      e.preventDefault();
      if (find) onStep(e.shiftKey ? -1 : 1);
    }
  };

  const none = find === null || find.count === null || find.count === 0;
  return (
    <div className="search-box">
      <div className="search-row">
        <input
          type="search"
          aria-label="Search"
          placeholder="Search…"
          spellCheck={false}
          autoComplete="off"
          value={draft.text}
          onChange={(e) => onType(e.target.value)}
          onKeyDown={onKeyDown}
        />
        <button
          type="button"
          className="search-toggle"
          aria-label="Regular expression"
          aria-pressed={draft.regex}
          title="Regular expression"
          onClick={() => submit({ ...draft, regex: !draft.regex })}
        >
          Rx
        </button>
        <button
          type="button"
          className="search-toggle"
          aria-label="Match case"
          aria-pressed={draft.case_sensitive}
          title="Match case"
          onClick={() => submit({ ...draft, case_sensitive: !draft.case_sensitive })}
        >
          Aa
        </button>
        <div className="search-mode" role="radiogroup" aria-label="Search mode">
          {(["filter", "find"] as const).map((m) => (
            <button
              key={m}
              type="button"
              role="radio"
              aria-checked={draft.mode === m}
              aria-label={m}
              onClick={() => submit({ ...draft, mode: m })}
            >
              {m}
            </button>
          ))}
        </div>
        {find && (
          <span className="search-find">
            <span data-testid="match-position">{`${find.position} of ${find.count === null ? "…" : find.count}`}</span>
            <button type="button" className="search-step" aria-label="Previous match" disabled={none} onClick={() => onStep(-1)}>
              ▲
            </button>
            <button type="button" className="search-step" aria-label="Next match" disabled={none} onClick={() => onStep(1)}>
              ▼
            </button>
          </span>
        )}
      </div>
      {error !== null && (
        <div className="search-error" role="alert">
          {displaySafe(error)}
        </div>
      )}
    </div>
  );
}
