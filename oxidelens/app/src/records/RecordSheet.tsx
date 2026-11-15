// What the Hosts, Accounts and IOCs sheets share (screens 09 and 10, without what D21 leaves out): a grid over the
// case's records of one kind as `records_list` answers them, its computed columns (entries, first and last seen, hosts
// seen) as the engine gives them, a filter row whose boxes go to the engine's query, the engine's order by a column,
// grouping, and the record editor in the right pane for the selected record.
//
// Nothing here filters, sorts, counts or converts: the filter row narrows through `records_list`, a header click asks it
// for an order (ascending, descending, then its own again) and the list is read again from its first page, the computed
// columns are the engine's, a time is the engine's text in the case's display zone. Grouping splits the engine's order,
// in it, by a field the record gives.
//
// Keys on the grid (inactive while a text field has focus): ↑/↓ move the selection, Enter focuses the editor's first
// field.
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { recordsList } from "../api";
import type { CaseRecord } from "../contract/generated/CaseRecord";
import type { RecordKind } from "../contract/generated/RecordKind";
import type { RecordOrder } from "../contract/generated/RecordOrder";
import type { RecordOrderBy } from "../contract/generated/RecordOrderBy";
import type { RecordQuery } from "../contract/generated/RecordQuery";
import { useShell } from "../shell/context";
import { isTypingTarget } from "../shell/keys";
import type { SheetKind } from "../shell/tabs";
import { displaySafe } from "../text";
import { reportFailure } from "./outcomes";
import RecordEditor from "./RecordEditor";
import { usePagedRead } from "./usePagedRead";
import "./records.css";

/** The most records one read lists (the engine's `MAX_LIMIT`). */
const LIMIT = 1000;
export const fmt = (n: number) => n.toLocaleString("en-US");

/** One column of a record grid. */
export interface Column<R> {
  /** The header's text. */
  header: string;
  /** The header names the case's display zone: the column shows a time in it. */
  zoned?: boolean;
  /** What the cell draws (through `displaySafe` for case text). */
  cell(r: R): ReactNode;
  /** What the engine orders by when the header is clicked; a column without one has no sort. */
  order?: RecordOrderBy;
  /** The engine query field the filter row's box under this column sets. */
  filter?: "text" | "tool";
  num?: boolean;
}

/** A field the sheet can group by. */
export interface GroupOption<R> {
  key: string;
  label: string;
  of(r: R): string;
}

/** The filter row's boxes, as the engine's query takes them. */
export interface RowFilters {
  text: string | null;
  tool: string | null;
}

/** The status column: `agent · review` (in the agent colour) for an agent's unreviewed record, else its status. */
export function RecordStatusText({ record }: { record: Pick<CaseRecord, "agent_added" | "status"> }): ReactNode {
  if (record.agent_added && record.status === "unreviewed") return <span className="status-agent">agent · review</span>;
  return <span className={record.status === "reviewed" ? "status-reviewed" : "status-other"}>{record.status}</span>;
}

/** The engine's text for a time, or nothing. */
export const timeText = (t: string | null): string => (t === null ? "" : displaySafe(t));

export default function RecordSheet<R extends CaseRecord>({
  active, kind, sheet, label, noun, columns, groups, query, queryKey = "", strip, bar, extra,
}: {
  active: boolean;
  kind: RecordKind;
  sheet: SheetKind;
  /** The grid's accessible name ("Host records"). */
  label: string;
  /** The record's name, one and many ("host", "hosts"). */
  noun: [string, string];
  columns: Column<R>[];
  groups: GroupOption<R>[];
  /** The engine's query for the filter row's boxes and the header's order (and the sheet's own narrowing, named by
   *  `queryKey`). */
  query(f: RowFilters, order: RecordOrder | null): RecordQuery;
  queryKey?: string;
  /** Above the bar (the IOC type strip). */
  strip?: ReactNode;
  /** At the end of the bar (Sweep all). */
  bar?: ReactNode;
  /** What the editor shows below the record's fields (sightings, the logon summary). */
  extra?(record: R): ReactNode;
}): ReactNode {
  const { summary, change, reportError, setRightPane, pendingSelect, clearPendingSelect } = useShell();
  const [filters, setFilters] = useState<RowFilters>({ text: null, tool: null });
  const [groupBy, setGroupBy] = useState("");
  const [sortBy, setSortBy] = useState<RecordOrder | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [focusSeq, setFocusSeq] = useState(0);

  const q = useMemo(() => query(filters, sortBy), [query, filters, sortBy]);
  // A hidden sheet reads nothing; shown again, it reads the case as it is then. Another query (an order too) reads
  // again from the first page.
  const base = active ? JSON.stringify([summary.case_id, queryKey, q]) : null;
  const slot = `sheet:${sheet}`;
  const { read: listed, showMore } = usePagedRead<CaseRecord>(
    base,
    change.generation,
    LIMIT,
    (limit, offset, id) => recordsList(q, limit, offset, slot, id),
    reportError,
  );
  useEffect(() => {
    if (listed?.error !== undefined) reportFailure(listed.error, reportError);
  }, [listed, reportError]);

  const items = useMemo(() => (listed?.value?.items ?? []).filter((r): r is R => r.kind === kind), [listed, kind]);
  const group = groups.find((g) => g.key === groupBy) ?? null;
  const grouped = useMemo(() => {
    if (group === null) return [{ key: "", rows: items }];
    const out = new Map<string, R[]>();
    for (const r of items) {
      const k = group.of(r);
      out.set(k, [...(out.get(k) ?? []), r]);
    }
    return [...out].map(([key, rows]) => ({ key, rows }));
  }, [items, group]);
  const order = useMemo(() => grouped.flatMap((g) => g.rows.map((r) => r.id)), [grouped]);

  const select = useCallback((id: number | null) => setSelected(id), []);
  // A record another screen asked for (*open →* on an entry's linked record): selected as the request arrives, which
  // the shell then forgets.
  const [taken, setTaken] = useState<typeof pendingSelect>(null);
  if (pendingSelect && pendingSelect.sheet === sheet && pendingSelect !== taken) {
    setTaken(pendingSelect);
    setSelected(pendingSelect.id);
  }
  useEffect(() => {
    if (pendingSelect?.sheet === sheet) clearPendingSelect?.();
  }, [pendingSelect, sheet, clearPendingSelect]);

  const onDeleted = useCallback(
    (id: number) => {
      const at = order.indexOf(id);
      setSelected((s) => (s === id ? (order[at + 1] ?? order[at - 1] ?? null) : s));
    },
    [order],
  );
  const onMerged = useCallback((into: number) => setSelected(into), []);
  const paneNode = useMemo(() => {
    if (selected === null) return <p className="pane-empty">{`Select a ${noun[0]} to see and edit it.`}</p>;
    return (
      <RecordEditor<R>
        key={`${kind}:${selected}`}
        kind={kind}
        id={selected}
        slot={`${slot}:editor`}
        focusSeq={focusSeq}
        onDeleted={onDeleted}
        onMerged={onMerged}
        extra={extra}
      />
    );
  }, [selected, kind, slot, focusSeq, onDeleted, onMerged, extra, noun]);
  useEffect(() => {
    if (!active) return;
    setRightPane(paneNode);
    return () => setRightPane(null);
  }, [active, paneNode, setRightPane]);

  // The selected row has the focus while the table does, so the keys act on what is shown as selected.
  const table = useRef<HTMLTableElement>(null);
  useEffect(() => {
    const t = table.current;
    if (!t || selected === null) return;
    const row = t.querySelector<HTMLElement>(`tr[data-record="${selected}"]`);
    if (t.contains(document.activeElement)) row?.focus();
    else row?.scrollIntoView?.({ block: "nearest" });
  }, [selected, listed]);

  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (e.ctrlKey || e.altKey || e.metaKey || isTypingTarget(e.target)) return;
    const at = selected === null ? -1 : order.indexOf(selected);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const next = e.key === "ArrowDown" ? Math.min(order.length - 1, at + 1) : Math.max(0, at - 1);
      if (order[next] !== undefined) select(order[next]);
    } else if (e.key === "Enter") {
      if (selected === null) return;
      setFocusSeq((s) => s + 1);
    } else {
      return;
    }
    e.preventDefault();
  };

  const clickHeader = (by: RecordOrderBy) =>
    setSortBy((o) => (o === null || o.by !== by ? { by, descending: false } : o.descending ? null : { by, descending: true }));
  const setFilter = (field: "text" | "tool", value: string) => setFilters((f) => ({ ...f, [field]: value === "" ? null : value }));

  const n = listed?.value ? items.length : null;
  const statusText = n === null ? null : `${fmt(n)} ${n === 1 ? noun[0] : noun[1]} shown${listed?.value?.truncated ? " · more in the case" : ""}`;
  return (
    <div className="timeline-sheet record-sheet">
      {strip}
      <div className="sheet-bar">
        {groups.length > 0 && (
          <select aria-label="Group by" value={groupBy} onChange={(e) => setGroupBy(e.target.value)}>
            <option value="">Group by: none</option>
            {groups.map((g) => (
              <option key={g.key} value={g.key}>
                {g.label}
              </option>
            ))}
          </select>
        )}
        <span className="sheet-bar-gap" />
        {bar}
      </div>
      <div className="sheet-body" role="group" aria-label={noun[1]} tabIndex={-1} onKeyDown={onKeyDown}>
        <div className="sheet-scroll">
          <table ref={table} className="sheet-table" role="grid" aria-label={label} tabIndex={0}>
            <thead>
              <tr>
                {columns.map((c) => {
                  const text = c.zoned ? `${c.header} (${displaySafe(summary.display_zone)})` : c.header;
                  const by = c.order;
                  if (by === undefined) {
                    return (
                      <th key={c.header} className={c.num ? "col-num" : undefined}>
                        {text}
                      </th>
                    );
                  }
                  const dir = sortBy?.by === by ? (sortBy.descending ? "descending" : "ascending") : null;
                  return (
                    <th key={c.header} className={c.num ? "col-num" : undefined} aria-sort={dir ?? "none"}>
                      <button type="button" className="sheet-sort" onClick={() => clickHeader(by)}>
                        {text}
                        <span aria-hidden="true">{dir === "descending" ? " ▾" : dir === "ascending" ? " ▴" : ""}</span>
                      </button>
                    </th>
                  );
                })}
              </tr>
              <tr className="sheet-filter-row">
                {columns.map((c) => (
                  <td key={c.header}>
                    {c.filter !== undefined && (
                      <input
                        type="text"
                        className="sheet-search"
                        aria-label={`Filter ${c.header}`}
                        placeholder="filter"
                        value={filters[c.filter] ?? ""}
                        onChange={(e) => setFilter(c.filter!, e.target.value)}
                      />
                    )}
                  </td>
                ))}
              </tr>
            </thead>
            <tbody>
              {grouped.map((g) => [
                group !== null && (
                  <tr key={`group:${g.key}`} className="sheet-group" data-group={g.key}>
                    <td role="gridcell" colSpan={columns.length}>{`${displaySafe(g.key)} · ${fmt(g.rows.length)}`}</td>
                  </tr>
                ),
                ...g.rows.map((r) => (
                  <tr
                    key={`${g.key}:${r.id}`}
                    data-record={r.id}
                    className={selected === r.id ? "sheet-row sheet-row-selected" : "sheet-row"}
                    aria-selected={selected === r.id}
                    tabIndex={-1}
                    onClick={() => select(r.id)}
                  >
                    {columns.map((c) => (
                      <td key={c.header} role="gridcell" className={c.num ? "col-num" : undefined}>
                        {c.cell(r)}
                      </td>
                    ))}
                  </tr>
                )),
              ])}
            </tbody>
          </table>
          {listed === null && <p className="pane-empty">{`Reading the ${noun[1]}…`}</p>}
          {listed?.value?.items.length === 0 && <p className="pane-empty">{`No ${noun[1]} here.`}</p>}
        </div>
      </div>
      {statusText !== null && (
        <div className="sheet-foot">
          <span>{statusText}</span>
          {listed?.value?.truncated && (
            <button type="button" className="link-button" onClick={showMore}>
              Show more
            </button>
          )}
        </div>
      )}
    </div>
  );
}
