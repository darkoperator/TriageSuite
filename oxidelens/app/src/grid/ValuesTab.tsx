// The Values tab of the column filter panel: the column's distinct values with counts from `grid_values`, ticks that
// apply as they change (an `in` chip, or `not_in` with the Exclude switch), a search box (`contains`, a literal
// case-insensitive substring, 200 ms debounce) and an order. The counts leave out this column's own Values chip, so
// ticking a value never makes the others vanish; they respect every other filter.
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { gridValues, isCmdError } from "../api";
import type { Chip } from "../contract/generated/Chip";
import type { GridState } from "../contract/generated/GridState";
import type { ValueCount } from "../contract/generated/ValueCount";
import type { ValueOrder } from "../contract/generated/ValueOrder";
import { displaySafe } from "../text";
import { readValuesChip, removeChip, upsertChip, valuesChip, valuesId, type ValuesMode } from "./chips";
import { stableKey } from "./useGridData";

export const VALUES_DEBOUNCE_MS = 200;
const VALUES_LIMIT = 300;
const fmt = (n: number) => n.toLocaleString("en-US");

export interface ValuesTabProps {
  tabId: string;
  column: string;
  state: GridState;
  /** Edit the grid's chip list. */
  applyChips(edit: (chips: Chip[]) => Chip[]): void;
  onError(e: unknown): void;
}

export default function ValuesTab(p: ValuesTabProps): ReactNode {
  const { tabId, column, state, applyChips, onError } = p;
  const own = valuesId(column);
  const ownChip = state.chips.find((c) => c.id === own);
  const read = ownChip ? readValuesChip(ownChip) : null;
  const [localMode, setLocalMode] = useState<ValuesMode>("include");
  const mode = read?.mode ?? localMode;
  const ticked = useMemo(() => read?.values ?? [], [read]);

  const [typed, setTyped] = useState("");
  const [contains, setContains] = useState("");
  const [order, setOrder] = useState<ValueOrder>("commonest");
  useEffect(() => {
    const t = window.setTimeout(() => setContains(typed), VALUES_DEBOUNCE_MS);
    return () => window.clearTimeout(t);
  }, [typed]);

  const [rows, setRows] = useState<ValueCount[] | null>(null);
  const [more, setMore] = useState(false);
  const stateRef = useRef(state);
  useEffect(() => {
    stateRef.current = state;
  });
  const except = ownChip ? [own] : [];
  // A refetch happens when something this list depends on changes: the other filters, the search, the order, or whether
  // this column's own chip exists (its counts leave it out). Ticking inside an existing chip does not refetch.
  const key = stableKey([
    { chips: state.chips.filter((c) => c.id !== own), search: state.search, hosts: state.hosts }, column, except, contains, order,
  ]);
  useEffect(() => {
    let live = true;
    gridValues(
      { ...stateRef.current, columns: null }, column, except, contains === "" ? null : contains, order, VALUES_LIMIT, 0,
      `tab:${tabId}:values`,
    ).then(
      (l) => {
        if (!live) return;
        setRows(l.items);
        setMore(l.truncated);
      },
      (e: unknown) => {
        if (live && !(isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case"))) onError(e);
      },
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `key` names everything the fetch reads
  }, [key]);

  const apply = (values: (string | null)[], nextMode: ValuesMode) =>
    applyChips((chips) => (values.length === 0 ? removeChip(chips, own) : upsertChip(chips, valuesChip(column, values, nextMode))));

  const toggle = (v: string | null) =>
    apply(ticked.includes(v) ? ticked.filter((t) => t !== v) : [...ticked, v], mode);

  const setMode = (m: ValuesMode) => {
    setLocalMode(m);
    if (ticked.length > 0) apply(ticked, m);
  };

  return (
    <div className="ft-values">
      <div className="ft-bar">
        <input
          type="search"
          role="searchbox"
          aria-label="Search values"
          placeholder="Search values"
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
        />
        <select aria-label="Order" value={order} onChange={(e) => setOrder(e.target.value as ValueOrder)}>
          <option value="commonest">Most frequent</option>
          <option value="rarest">Least frequent</option>
          <option value="value">By value</option>
        </select>
      </div>
      <div className="ft-mode" role="radiogroup" aria-label="Ticked values">
        {(["include", "exclude"] as const).map((m) => (
          <label key={m}>
            <input type="radio" name={`mode-${tabId}`} checked={mode === m} onChange={() => setMode(m)} />
            {m === "include" ? "Include" : "Exclude"}
          </label>
        ))}
      </div>
      <ul className="ft-list" aria-label="Values">
        {rows === null && <li className="ft-note">Loading…</li>}
        {rows?.length === 0 && <li className="ft-note">No values</li>}
        {rows?.map((r) => {
          const text = r.value === null ? "(empty)" : displaySafe(r.value);
          return (
            <li key={r.value === null ? "\u0000empty" : r.value} className="ft-item">
              <label className={r.value === null ? "ft-value ft-empty" : "ft-value"}>
                <input
                  type="checkbox"
                  checked={ticked.includes(r.value)}
                  disabled={r.clipped}
                  title={r.clipped ? "Too long to filter by value" : undefined}
                  onChange={() => toggle(r.value)}
                />
                <span className="ft-text">{text}{r.clipped ? "…" : ""}</span>
              </label>
              <span className="ft-count" data-testid="value-count">{fmt(r.count)}</span>
            </li>
          );
        })}
      </ul>
      {more && <div className="ft-note">More values than shown: search to narrow the list.</div>}
      <div className="ft-note">Counts respect the other filters.</div>
    </div>
  );
}
