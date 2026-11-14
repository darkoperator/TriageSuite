// The row detail pane: one row, every field in full. Times are the engine's display text (zone and offset); each value
// has Filter in / Filter out (chips on the row's own tab) and Copy (the stored text); *Around this row* shows pivot
// counts and opens a merged tab; then provenance, the raw record, and the row's tags and notes (read-only in step 2).
//
// Reads: `row_view` on slot `detail` and `around_totals` on slot `detail:around`. A read is for one key (row, zone, case
// generation); an answer is drawn only while the screen still has that key, so a slow old row never replaces a new one,
// and leaving a key cancels what is still running.
import { useCallback, useEffect, useState, type ReactNode } from "react";
import { aroundTotals, isCmdError, newQueryId, queryCancel, rowView } from "../api";
import type { Chip } from "../contract/generated/Chip";
import type { RowView } from "../contract/generated/RowView";
import type { WindowTotal } from "../contract/generated/WindowTotal";
import { cellChip, hash } from "../grid/chips";
import { writeClipboard } from "../grid/copy";
import { stableKey } from "../grid/useGridData";
import { useShell } from "../shell/context";
import type { Tab } from "../shell/tabs";
import { displaySafe, displaySafeMultiline } from "../text";
import PivotButtons, { PIVOT_WINDOWS, windowLabel } from "./PivotButtons";
import Provenance from "./Provenance";
import "./detail.css";

export interface RowDetailProps {
  rowId: number;
  onFilter(chip: Chip): void;
  /** Shows another row of the case (the current row of a superseded one). */
  onShowRow(rowId: number): void;
}

const obsolete = (e: unknown) => isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case");

interface ViewRecord {
  /** What the answer is about apart from the case generation: a re-read after `case-changed` keeps drawing it. */
  about: string;
  view: RowView;
}
interface AroundRecord {
  about: string;
  totals: WindowTotal[] | "untimed";
}

export default function RowDetail({ rowId, onFilter, onShowRow }: RowDetailProps): ReactNode {
  const { summary, change, openScoped, reportError } = useShell();
  const zone = summary.display_zone;
  const key = `${rowId}|${zone}|${change.generation}`;
  const about = `${rowId}|${zone}`;
  const aroundKey = `${rowId}|${change.generation}`;
  const [record, setRecord] = useState<ViewRecord | null>(null);
  const [aroundRecord, setAroundRecord] = useState<AroundRecord | null>(null);
  const [hideEmpty, setHideEmpty] = useState(false);
  const [rawOpen, setRawOpen] = useState(false);

  // The row itself.
  useEffect(() => {
    let live = true;
    const id = newQueryId();
    let settled = false;
    rowView(rowId, zone, "detail", id).then(
      (view) => {
        settled = true;
        if (live) setRecord({ about, view });
      },
      (e: unknown) => {
        settled = true;
        if (live && !obsolete(e)) reportError(e);
      },
    );
    return () => {
      live = false;
      if (!settled) void queryCancel([id]).catch(() => {});
    };
  }, [key, about, rowId, zone, reportError]);

  // The counts around it. A row with no time is the engine's `invalid`, which is what says so.
  useEffect(() => {
    let live = true;
    const id = newQueryId();
    let settled = false;
    aroundTotals(rowId, PIVOT_WINDOWS, "detail:around", id).then(
      (totals) => {
        settled = true;
        if (live) setAroundRecord({ about: String(rowId), totals });
      },
      (e: unknown) => {
        settled = true;
        if (!live) return;
        if (isCmdError(e) && e.code === "invalid") setAroundRecord({ about: String(rowId), totals: "untimed" });
        else if (!obsolete(e)) reportError(e);
      },
    );
    return () => {
      live = false;
      if (!settled) void queryCancel([id]).catch(() => {});
    };
  }, [aroundKey, rowId, reportError]);

  // What the screen draws is what was read for this very row and zone: another row or zone draws nothing old. A new case
  // generation (`case-changed`) keeps the last answer up while the re-read runs, and the new answer replaces it.
  const view = record?.about === about ? record.view : null;
  const totals = aroundRecord?.about === String(rowId) ? aroundRecord.totals : null;

  // The anchor is the engine's (`RowView.anchor`: host and R1 time), handed on unchanged.
  const anchor = view?.anchor ?? null;
  const blocked = !view ? "Loading" : anchor === null ? "This row has no host or time to pivot on" : null;

  const openMerged = (seconds: number) => {
    if (anchor === null) return;
    const source = { around: { host: anchor.host, at: anchor.at, window_seconds: seconds } };
    const tab: Tab = {
      id: `merged:${hash(stableKey(source))}`,
      kind: "merged",
      relation: null,
      title: `Around ${anchor.host} · ${windowLabel(seconds)}`,
      merged: { source },
    };
    openScoped(tab);
  };

  const copyText = useCallback(
    (text: string) => {
      void writeClipboard(text).catch(reportError);
    },
    [reportError],
  );

  if (view === null) return <p className="pane-empty">Loading row {rowId.toLocaleString("en-US")}…</p>;

  const { detail } = view;
  const shown = hideEmpty ? view.fields.filter((f) => f.text !== null && f.text !== "") : view.fields;
  const asJson = () => copyText(JSON.stringify(Object.fromEntries(view.fields.map((f) => [f.meta.name, f.text]))));

  return (
    <div className="detail">
      <header className="detail-head">
        <div className="detail-title">
          <span data-testid="detail-title" className="detail-rel">{displaySafe(view.summary ?? detail.relation)}</span>
          <span data-testid="detail-row" className="detail-rowid" title={displaySafe(detail.relation)}>{`row ${detail.row_id.toLocaleString("en-US")}`}</span>
          {detail.mark === "suspicious" && <span className="detail-mark">Suspicious</span>}
        </div>
        <button type="button" className="detail-toggle" aria-pressed={hideEmpty} onClick={() => setHideEmpty((h) => !h)}>
          {hideEmpty ? "Show all" : "Hide empty"}
        </button>
      </header>

      {detail.standing === "superseded" && (
        <div className="detail-banner" role="status">
          <span>Superseded: a later load of this host replaced this row.</span>
          {detail.current_row !== null && (
            <button type="button" onClick={() => onShowRow(detail.current_row!)}>{`Show the current row (${detail.current_row.toLocaleString("en-US")})`}</button>
          )}
        </div>
      )}
      {detail.standing === "removed" && (
        <div className="detail-banner" role="status">
          <span>Removed: the source that held this row was removed, so its fields are gone.</span>
          {detail.current_row !== null && (
            <button type="button" onClick={() => onShowRow(detail.current_row!)}>{`Show the current row (${detail.current_row.toLocaleString("en-US")})`}</button>
          )}
        </div>
      )}

      <dl className="detail-fields">
        {shown.map((f) => {
          const name = f.meta.name;
          const safe = displaySafe(name);
          const value = f.display ?? f.text;
          return (
            <div key={name} className="field" data-testid={`field-${name}`}>
              <dt>
                {safe}
                {f.meta.sql_type === "TIMESTAMP" && f.meta.time_semantics !== "utc" && (
                  <span className="badge" title="Shown as written: its zone is not known">?tz</span>
                )}
              </dt>
              <dd>
                {value === null ? (
                  <span className="field-null">NULL</span>
                ) : value === "" ? (
                  <span className="field-null">{'""'}</span>
                ) : (
                  <span className="field-value" title={f.display !== null && f.text !== null ? `Stored: ${displaySafe(f.text)}` : undefined}>
                    {displaySafeMultiline(value)}
                  </span>
                )}
                {f.failed && <span className="badge badge-warn" title="The text did not convert to the column's type">did not convert</span>}
                <span className="field-actions">
                  <button type="button" aria-label={`Filter in ${safe}`} title="Filter in" onClick={() => onFilter(cellChip(name, "eq", f.text))}>+</button>
                  <button type="button" aria-label={`Filter out ${safe}`} title="Filter out" onClick={() => onFilter(cellChip(name, "ne", f.text))}>−</button>
                  <button type="button" aria-label={`Copy ${safe}`} title="Copy the stored text" onClick={() => copyText(f.text ?? "")}>⧉</button>
                </span>
              </dd>
            </div>
          );
        })}
        {shown.length === 0 && <p className="detail-note">No fields to show.</p>}
      </dl>

      <PivotButtons totals={totals} blocked={blocked} onOpen={openMerged} />
      <Provenance provenance={view.provenance} />

      <section className="detail-section" aria-label="Raw record">
        <h3>Raw record</h3>
        <div className="detail-raw-actions">
          <button type="button" aria-expanded={rawOpen} onClick={() => setRawOpen((o) => !o)}>{rawOpen ? "Hide" : "Show"}</button>
          <button type="button" disabled={view.fields.length === 0} onClick={asJson}>Copy as JSON</button>
        </div>
        {rawOpen && (
          <pre className="detail-raw">{view.fields.map((f) => `${displaySafe(f.meta.name)}: ${f.text === null ? "" : displaySafeMultiline(f.text)}`).join("\n")}</pre>
        )}
      </section>

      <section className="detail-section" aria-label="Tags and notes">
        <h3>Tags</h3>
        {detail.tags.length === 0 ? (
          <p className="detail-note">No tags.</p>
        ) : (
          <div className="tags">{detail.tags.map((t) => <span key={t} className="tag">{displaySafe(t)}</span>)}</div>
        )}
        <h3>Notes</h3>
        {detail.notes.length === 0 ? (
          <p className="detail-note">No notes.</p>
        ) : (
          detail.notes.map((n) => (
            <div key={n.id} className="note">
              <div className="note-meta">{`${displaySafe(n.written_by)} · ${displaySafe(n.written_at)}`}</div>
              <div className="note-text">{displaySafeMultiline(n.text)}</div>
            </div>
          ))
        )}
      </section>
    </div>
  );
}
