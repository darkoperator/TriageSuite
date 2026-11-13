// The column filter panel (screen 02, left), opened from a header's ▾, the header menu's Filter…, or `+ filter`: tabs
// Text, Values and (for a TIMESTAMP column) Date range. The Mark pseudo-column has one tab with Suspicious and None.
// It builds chips and hands them to the grid; it holds no filter rules and never sees SQL.
import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import type { Chip } from "../contract/generated/Chip";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";
import type { GridState } from "../contract/generated/GridState";
import { displaySafe } from "../text";
import {
  MARK_COLUMN, MARK_LABEL, MARK_VALUES, markChip, markSelection, removeChip, upsertChip, MARK_CHIP_ID, type MarkValue,
} from "./chips";
import DateTab from "./DateTab";
import { useDialogFocus } from "./dialogFocus";
import TextTab from "./TextTab";
import ValuesTab from "./ValuesTab";

type TabName = "values" | "text" | "date";
const TAB_LABEL: Record<TabName, string> = { values: "Values", text: "Text", date: "Date range" };

export interface FilterPanelProps {
  tabId: string;
  relation: string;
  /** The column's name; `_mark` for the Mark pseudo-column. */
  column: string;
  meta: ColumnMeta | undefined;
  state: GridState;
  zone: string;
  x: number;
  y: number;
  applyChips(edit: (chips: Chip[]) => Chip[]): void;
  /** Asks the engine whether the grid would accept `chip`; resolves to its message when it would not. */
  validate(chip: Chip): Promise<string | null>;
  onError(e: unknown): void;
  /** The control that asked for the panel; focus goes back to it on close. */
  returnFocus?: HTMLElement | null;
  onClose(): void;
}

export default function FilterPanel(p: FilterPanelProps): ReactNode {
  const isMark = p.column === MARK_COLUMN;
  const tabs: TabName[] = isMark ? [] : p.meta?.sql_type === "TIMESTAMP" ? ["values", "text", "date"] : ["values", "text"];
  const [tab, setTab] = useState<TabName>("values");
  const box = useRef<HTMLDivElement>(null);
  const { onClose } = p;
  useDialogFocus(box, p.returnFocus);

  useEffect(() => {
    const away = (e: MouseEvent) => {
      const t = e.target as Element;
      // The ▾ button that opened this panel handles its own press; every other outside press closes it.
      if (box.current && !box.current.contains(t) && !t.closest?.(".col-filter")) onClose();
    };
    // Escape closes it wherever focus is (a menu that just closed may have handed focus back to the grid).
    const escape = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", escape);
    };
  }, [onClose]);

  const onTabKey = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
    e.preventDefault();
    const next = tabs[(i + (e.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length];
    setTab(next);
    box.current?.querySelector<HTMLElement>(`[data-tab='${next}']`)?.focus();
  };

  const title = isMark ? "Mark" : displaySafe(p.column);
  const left = Math.max(0, Math.min(p.x, (typeof window === "undefined" ? 1200 : window.innerWidth) - 340));
  return (
    <div ref={box} className="filter-panel" role="dialog" aria-label={`Filter ${title}`} style={{ left, top: p.y }}>
      <div className="fp-title">{title}</div>
      {tabs.length > 0 && (
        <div className="fp-tabs" role="tablist">
          {tabs.map((t, i) => (
            <button
              key={t}
              type="button"
              role="tab"
              data-tab={t}
              aria-selected={tab === t}
              tabIndex={tab === t ? 0 : -1}
              onClick={() => setTab(t)}
              onKeyDown={(e) => onTabKey(e, i)}
            >
              {TAB_LABEL[t]}
            </button>
          ))}
        </div>
      )}
      <div className="fp-body" role={tabs.length > 0 ? "tabpanel" : undefined}>
        {isMark && <MarkTab chips={p.state.chips} applyChips={p.applyChips} />}
        {tab === "values" && !isMark && (
          <ValuesTab tabId={p.tabId} column={p.column} state={p.state} applyChips={p.applyChips} onError={p.onError} />
        )}
        {tab === "text" && !isMark && (
          <TextTab
            column={p.column}
            typed={p.meta?.sql_type !== null && p.meta?.sql_type !== undefined}
            chips={p.state.chips}
            applyChips={p.applyChips}
            validate={p.validate}
            onDone={onClose}
          />
        )}
        {tab === "date" && !isMark && (
          <DateTab
            tabId={p.tabId}
            relation={p.relation}
            column={p.column}
            zone={p.zone}
            chips={p.state.chips}
            applyChips={p.applyChips}
            onDone={onClose}
          />
        )}
      </div>
    </div>
  );
}

/** Suspicious / None: one ticked filters to it, both or neither is no filter (so the chip goes). */
function MarkTab(p: { chips: readonly Chip[]; applyChips(edit: (chips: Chip[]) => Chip[]): void }): ReactNode {
  const ticked = markSelection(p.chips);
  const toggle = (v: MarkValue) => {
    const next = ticked.includes(v) ? ticked.filter((t) => t !== v) : [...ticked, v];
    const chip = markChip(next);
    p.applyChips((chips) => (chip ? upsertChip(chips, chip) : removeChip(chips, MARK_CHIP_ID)));
  };
  return (
    <ul className="ft-list" aria-label="Mark values">
      {MARK_VALUES.map((v) => (
        <li key={v} className="ft-item">
          <label className="ft-value">
            <input type="checkbox" checked={ticked.includes(v)} onChange={() => toggle(v)} />
            <span className="ft-text">{MARK_LABEL[v]}</span>
          </label>
        </li>
      ))}
    </ul>
  );
}
