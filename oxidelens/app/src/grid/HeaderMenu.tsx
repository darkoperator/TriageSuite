// The right-click menu of a column header. Group by is shown but disabled until the task that owns it hands the grid a handler. Escape, an outside press or any choice closes it.
import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";

export interface HeaderMenuProps {
  x: number;
  y: number;
  pinned: boolean;
  /** Hiding the last shown column is refused. */
  canHide: boolean;
  onSort(descending: boolean): void;
  onGroupBy?: () => void;
  onFilter?: () => void;
  onPin(): void;
  onBestFit(): void;
  onHide(): void;
  onClose(): void;
}

export default function HeaderMenu(p: HeaderMenuProps): ReactNode {
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
    <div ref={box} className="cell-menu" role="menu" aria-label="Column" tabIndex={-1} style={{ left: p.x, top: p.y }} onKeyDown={onKeyDown}>
      <button type="button" role="menuitem" onClick={choose(() => p.onSort(false))}>Sort ascending</button>
      <button type="button" role="menuitem" onClick={choose(() => p.onSort(true))}>Sort descending</button>
      <button type="button" role="menuitem" disabled={!p.onGroupBy} onClick={choose(p.onGroupBy)}>Group by this column</button>
      <button type="button" role="menuitem" disabled={!p.onFilter} onClick={choose(p.onFilter)}>Filter…</button>
      <button type="button" role="menuitem" onClick={choose(p.onPin)}>{p.pinned ? "Unpin" : "Pin left"}</button>
      <button type="button" role="menuitem" onClick={choose(p.onBestFit)}>Best fit</button>
      <button type="button" role="menuitem" disabled={!p.canHide} onClick={choose(p.onHide)}>Hide</button>
    </div>
  );
}
