// Keyboard handling of the shell. Keys do nothing while the analyst is typing: a text input, a text area, a select or
// an editable element has the focus.
import { useEffect, useRef } from "react";

export function isTypingTarget(t: EventTarget | null): boolean {
  if (!(t instanceof HTMLElement)) return false;
  if (t.isContentEditable || t.tagName === "TEXTAREA" || t.tagName === "SELECT") return true;
  if (t.tagName !== "INPUT") return false;
  const type = (t as HTMLInputElement).type;
  return !["button", "checkbox", "radio", "submit", "reset", "range", "color", "file", "image"].includes(type);
}

type KeyLine = ReadonlyArray<readonly [string, string]>;

/** The row keys of a grid (F and K arrive in step 4); a merged tab has no cell to take I from. */
const ROW_KEYS: KeyLine = [
  ["S", "suspicious"],
  ["U", "clear mark"],
  ["M", "malicious → timeline"],
  ["T", "tag"],
  ["N", "note"],
  ["A", "attach"],
];
const CELL_KEYS: KeyLine = [
  ["I", "add as IOC"],
  ["Space", "full value"],
  ["Enter", "row detail"],
  ["Ctrl+C", "copy"],
  ["Ctrl+Shift+C", "copy rows"],
];
const PANE_KEYS: KeyLine = [["Ctrl+[ Ctrl+]", "panes"]];

/** The keys live on a tab of `kind`, in the order the key strip shows them: only what works there. A grouped grid
 *  (`grouped`) takes the row keys and I on its focused data row, and none of the flat grid's cell keys. */
export function keyStrip(kind: "overview" | "dataset" | "merged" | "grouped" | null): KeyLine {
  if (kind === "dataset") return [...ROW_KEYS, ...CELL_KEYS, ...PANE_KEYS];
  if (kind === "grouped") return [...ROW_KEYS, CELL_KEYS[0], ...PANE_KEYS];
  if (kind === "merged") return [...ROW_KEYS, ...PANE_KEYS];
  return PANE_KEYS;
}

export interface ShellKeys {
  toggleLeft(): void;
  toggleRight(): void;
}

/** Ctrl+[ folds or unfolds the left pane, Ctrl+] the right one. */
export function useShellKeys(keys: ShellKeys, enabled: boolean): void {
  const ref = useRef(keys);
  useEffect(() => {
    ref.current = keys;
  });
  useEffect(() => {
    if (!enabled) return;
    const onKey = (e: KeyboardEvent) => {
      if (!e.ctrlKey || e.altKey || e.metaKey || e.shiftKey || isTypingTarget(e.target)) return;
      if (e.key === "[") ref.current.toggleLeft();
      else if (e.key === "]") ref.current.toggleRight();
      else return;
      e.preventDefault();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [enabled]);
}
