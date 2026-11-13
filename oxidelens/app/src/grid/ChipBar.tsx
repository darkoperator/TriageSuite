// The chip bar above the grid: one chip per filter (label, an on/off switch that dims and disables it, a remove ×),
// `+ filter` (pick a column, open its panel), Advanced…, Save preset… and Clear all. Labels are untrusted display text.
import { useCallback, useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import type { Chip } from "../contract/generated/Chip";
import type { SearchState } from "../contract/generated/SearchState";
import { displaySafe } from "../text";

export interface ChipBarProps {
  chips: readonly Chip[];
  /** Every column a filter can name, in the order to offer them (the Mark column is `_mark`, shown as "Mark"). */
  columns: readonly { name: string; label: string }[];
  /** The grid's search, shown as a pill before the chips when it has a text (it is `GridState.search`, not a chip). */
  search?: SearchState;
  onClearSearch?(): void;
  onToggle(id: string): void;
  onRemove(id: string): void;
  onClear(): void;
  /** A column was chosen from `+ filter`: where its panel opens, and the button to give focus back to. */
  onPlus(column: string, x: number, y: number, opener: HTMLElement | null): void;
  onAdvanced(): void;
  /** Save the chips as a preset named `name`; resolves to a message when the engine refuses it. */
  onSavePreset(name: string): Promise<string | null>;
}

export default function ChipBar(p: ChipBarProps): ReactNode {
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const plusButton = useRef<HTMLButtonElement>(null);
  const [saving, setSaving] = useState<{ x: number; y: number } | null>(null);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);

  const closeMenu = useCallback(() => setMenu(null), []);

  const save = async () => {
    setBusy(true);
    const message = await p.onSavePreset(name);
    setBusy(false);
    if (message !== null) {
      setError(message);
      return;
    }
    setSaving(null);
    setName("");
    setError(null);
    setSaved(true);
  };

  return (
    <div className="chip-bar" role="region" aria-label="Filters">
      {p.search && p.search.text !== "" && (
        <span className="search-pill" title={`search: ${displaySafe(p.search.text)}`}>
          <span className="chip-label">{`search: ${displaySafe(p.search.text)}`}</span>
          <span className="chip-flags">
            {[p.search.regex ? "Rx" : "", p.search.case_sensitive ? "Aa" : "", p.search.mode === "find" ? "find" : ""].filter(Boolean).join(" ")}
          </span>
          <button type="button" className="chip-remove" aria-label="Clear search" onClick={() => p.onClearSearch?.()}>
            ×
          </button>
        </span>
      )}
      <ul className="chips" aria-label="Active filters">
        {p.chips.map((c) => {
          const label = displaySafe(c.label);
          return (
            <li key={c.id} className="chip" data-enabled={c.enabled ? "true" : "false"} title={label}>
              <button
                type="button"
                role="switch"
                aria-checked={c.enabled}
                aria-label={`Use filter ${label}`}
                className="chip-switch"
                onClick={() => p.onToggle(c.id)}
              >
                <span aria-hidden="true">{c.enabled ? "●" : "○"}</span>
              </button>
              <span className="chip-label">{label}</span>
              <button type="button" className="chip-remove" aria-label={`Remove filter ${label}`} onClick={() => p.onRemove(c.id)}>
                ×
              </button>
            </li>
          );
        })}
      </ul>
      <button
        ref={plusButton}
        type="button"
        className="chip-action"
        aria-haspopup="menu"
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          setMenu({ x: r.left, y: r.bottom });
        }}
      >
        + filter
      </button>
      <button type="button" className="chip-action" onClick={p.onAdvanced}>Advanced…</button>
      <button
        type="button"
        className="chip-action"
        disabled={p.chips.length === 0}
        aria-haspopup="dialog"
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          setSaved(false);
          setError(null);
          setSaving({ x: r.left, y: r.bottom });
        }}
      >
        Save preset…
      </button>
      <button type="button" className="chip-action" disabled={p.chips.length === 0} onClick={p.onClear}>Clear all</button>
      {saved && <span className="chip-note" role="status">Preset saved</span>}
      {menu && <ColumnPicker x={menu.x} y={menu.y} columns={p.columns} onPick={(c) => { closeMenu(); p.onPlus(c, menu.x, menu.y, plusButton.current); }} onClose={closeMenu} />}
      {saving && (
        <div
          className="preset-form"
          role="dialog"
          aria-label="Save preset"
          style={{ left: saving.x, top: saving.y }}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.stopPropagation();
              setSaving(null);
            }
          }}
        >
          <input
            type="text"
            aria-label="Preset name"
            value={name}
            autoFocus
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !busy) void save();
            }}
          />
          <button type="button" disabled={busy} onClick={() => void save()}>Save</button>
          <button type="button" onClick={() => setSaving(null)}>Cancel</button>
          {error !== null && <div role="alert" className="form-error">{displaySafe(error)}</div>}
        </div>
      )}
    </div>
  );
}

/** The `+ filter` list: every column, as menu items; Escape or an outside press closes it. */
function ColumnPicker(p: {
  x: number; y: number; columns: readonly { name: string; label: string }[]; onPick(column: string): void; onClose(): void;
}): ReactNode {
  const box = useRef<HTMLDivElement>(null);
  const { onClose } = p;
  useEffect(() => {
    box.current?.querySelector<HTMLElement>("[role='menuitem']")?.focus();
    const away = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", away);
    return () => document.removeEventListener("mousedown", away);
  }, [onClose]);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const items = [...(box.current?.querySelectorAll<HTMLElement>("[role='menuitem']") ?? [])];
      const at = items.indexOf(document.activeElement as HTMLElement);
      items[(at + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
    }
  };
  return (
    <div ref={box} className="cell-menu column-picker" role="menu" aria-label="Filter column" style={{ left: p.x, top: p.y }} onKeyDown={onKeyDown}>
      {p.columns.map((c) => (
        <button key={c.name} type="button" role="menuitem" onClick={() => p.onPick(c.name)}>{displaySafe(c.label)}</button>
      ))}
    </div>
  );
}
