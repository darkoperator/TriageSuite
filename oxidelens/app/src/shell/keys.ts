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

/** The keys live in step 3, in the order the key strip shows them (F and K arrive in step 4). */
export const KEY_STRIP: ReadonlyArray<readonly [string, string]> = [
  ["S", "suspicious"],
  ["U", "clear mark"],
  ["M", "malicious → timeline"],
  ["T", "tag"],
  ["N", "note"],
  ["A", "attach"],
  ["I", "add as IOC"],
  ["Space", "full value"],
  ["Enter", "row detail"],
  ["Ctrl+C", "copy"],
  ["Ctrl+Shift+C", "copy rows"],
  ["Ctrl+[ Ctrl+]", "panes"],
];

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
