// The value viewer: one cell's whole text (`cell_value`), and when the engine split it into fields one field per line
// with its key. A modal dialog that traps focus; Escape and Close dismiss it, Copy puts the whole stored text on the
// clipboard. Every string goes through `displaySafe` for display; Copy sends the original.
import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { cellValue, isCmdError } from "../api";
import type { CellValue } from "../contract/generated/CellValue";
import { displaySafe, displaySafeMultiline } from "../text";
import { writeClipboard } from "./copy";

export interface ValueViewerProps {
  tabId: string;
  rowId: number;
  column: string;
  onClose(): void;
  onError(e: unknown): void;
}

const FOCUSABLE = "button:not(:disabled), [href], input, select, textarea, [tabindex]:not([tabindex='-1'])";

export default function ValueViewer({ tabId, rowId, column, onClose, onError }: ValueViewerProps): ReactNode {
  const [value, setValue] = useState<CellValue | null>(null);
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let live = true;
    cellValue(rowId, column, `tab:${tabId}:value`).then(
      (v) => live && setValue(v),
      (e: unknown) => {
        if (!live) return;
        if (!(isCmdError(e) && (e.code === "superseded" || e.code === "interrupted"))) onError(e);
        onClose();
      },
    );
    return () => {
      live = false;
    };
  }, [tabId, rowId, column, onError, onClose]);

  useEffect(() => {
    box.current?.focus();
  }, []);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
      return;
    }
    if (e.key !== "Tab" || !box.current) return;
    const items = [...box.current.querySelectorAll<HTMLElement>(FOCUSABLE)];
    if (items.length === 0) {
      e.preventDefault();
      return;
    }
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement;
    if (e.shiftKey && (active === first || active === box.current)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && active === last) {
      e.preventDefault();
      first.focus();
    }
  };

  const split = value !== null && value.parts.length > 1;
  return (
    <div className="viewer-scrim">
      <div
        ref={box}
        className="viewer"
        role="dialog"
        aria-modal="true"
        aria-label={`Value of ${displaySafe(column)}`}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <div className="viewer-title">{displaySafe(column)}</div>
        <div className="viewer-body">
          {value === null ? (
            <span className="viewer-loading">Loading…</span>
          ) : split ? (
            value.parts.map((p, i) => (
              <div key={i} className="viewer-line">
                {p.key !== null ? `${displaySafe(p.key)}: ${displaySafe(p.value)}` : displaySafe(p.value)}
              </div>
            ))
          ) : (
            <pre className="viewer-text">{displaySafeMultiline(value.text ?? "")}</pre>
          )}
        </div>
        <div className="viewer-buttons">
          <button type="button" disabled={value === null} onClick={() => void writeClipboard(value?.text ?? "").catch(onError)}>
            Copy
          </button>
          <button type="button" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
