// The right-click menu of a cell. Filter in / Filter out are disabled when the grid hands no handler; entries for later
// steps' keys are not shown. Escape, an outside press or any choice closes it.
import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";

export interface CellMenuProps {
  x: number;
  y: number;
  onCopyCell(): void;
  onCopyRows(): void;
  onShowValue(): void;
  /** Without a handler the entry is disabled. */
  onFilterIn?: () => void;
  onFilterOut?: () => void;
  onClose(): void;
}

export default function CellMenu(p: CellMenuProps): ReactNode {
  const box = useRef<HTMLDivElement>(null);
  const { onClose } = p;

  useEffect(() => {
    box.current?.querySelector<HTMLElement>("[role='menuitem']:not(:disabled)")?.focus();
    const away = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", away);
    return () => document.removeEventListener("mousedown", away);
  }, [onClose]);

  const choose = (f?: () => void) => () => {
    onClose();
    f?.();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const items = [...(box.current?.querySelectorAll<HTMLElement>("[role='menuitem']:not(:disabled)") ?? [])];
      const at = items.indexOf(document.activeElement as HTMLElement);
      const next = (at + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length;
      items[next]?.focus();
    }
  };

  return (
    <div ref={box} className="cell-menu" role="menu" tabIndex={-1} style={{ left: p.x, top: p.y }} onKeyDown={onKeyDown}>
      <button type="button" role="menuitem" onClick={choose(p.onCopyCell)}>Copy cell</button>
      <button type="button" role="menuitem" onClick={choose(p.onCopyRows)}>Copy rows (with headers)</button>
      <button type="button" role="menuitem" onClick={choose(p.onShowValue)}>Show full value (Space)</button>
      <button type="button" role="menuitem" disabled={!p.onFilterIn} onClick={choose(p.onFilterIn)}>Filter in</button>
      <button type="button" role="menuitem" disabled={!p.onFilterOut} onClick={choose(p.onFilterOut)}>Filter out</button>
    </div>
  );
}
