// The row detail pane: one row, every field in full. Times are the engine's display text (zone and offset); each value
// has Filter in / Filter out (chips on the row's own tab) and Copy (the stored text); *Around this row* shows pivot
// counts and opens a merged tab; then provenance, the raw record, the row's mark (with S, U and M as buttons), its tags
// (add, remove), its notes (add, delete) and the live entries it is evidence on.
//
// Reads: `row_view` on slot `detail`, `around_totals` on slot `detail:around` and the linked entries (`entries_list`
// narrowed to the row) on slot `detail:entries`. A read is for one key (row, zone, case generation); an answer is drawn
// only while the screen still has that key, so a slow old row never replaces a new one, and leaving a key cancels what
// is still running. Writes go to the engine as typed; a refusal is shown in the status bar with the engine's reason, and
// the `case-changed` that follows a write re-reads the pane.
import { useCallback, useEffect, useState, type ReactNode } from "react";
import {
  aroundTotals, entriesList, isCmdError, markClear, markSet, newQueryId, noteAdd, noteDelete, queryCancel, rowsPromote, rowView, tagAdd,
  tagRemove,
} from "../api";
import type { Chip } from "../contract/generated/Chip";
import type { Entry } from "../contract/generated/Entry";
import type { RowView } from "../contract/generated/RowView";
import type { WindowTotal } from "../contract/generated/WindowTotal";
import { cellChip, hash } from "../grid/chips";
import { writeClipboard } from "../grid/copy";
import { stableKey } from "../grid/useGridData";
import { useShell } from "../shell/context";
import type { Tab } from "../shell/tabs";
import { displaySafe, displaySafeMultiline } from "../text";
import MarkGlyph, { MARK_NAME } from "../records/MarkGlyph";
import { promotedEntry, promoteRefusals, reportFailure, rowRefusals } from "../records/outcomes";
import PivotButtons, { PIVOT_WINDOWS, windowLabel } from "./PivotButtons";
import Provenance from "./Provenance";
import "./detail.css";

export interface RowDetailProps {
  rowId: number;
  onFilter(chip: Chip): void;
  /** Shows another row of the case (the current row of a superseded one). */
  onShowRow(rowId: number): void;
  /** Shows a timeline entry in the pane: the one M made, or one the row is evidence on. */
  onShowEntry?(entryId: number): void;
}

/** How many linked entries the pane lists. */
const LINKED = 50;

const obsolete = (e: unknown) => isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case");

interface ViewRecord {
  /** What the answer is about apart from the case generation: a re-read after `case-changed` keeps drawing it. */
  about: string;
  view: RowView;
}
interface EntriesRecord {
  about: string;
  items: Entry[];
  truncated: boolean;
}
interface AroundRecord {
  about: string;
  totals: WindowTotal[] | "untimed";
}

export default function RowDetail({ rowId, onFilter, onShowRow, onShowEntry }: RowDetailProps): ReactNode {
  const { summary, change, openScoped, reportError } = useShell();
  const zone = summary.display_zone;
  const key = `${rowId}|${zone}|${change.generation}`;
  const about = `${rowId}|${zone}`;
  const aroundKey = `${rowId}|${change.generation}`;
  const [record, setRecord] = useState<ViewRecord | null>(null);
  const [aroundRecord, setAroundRecord] = useState<AroundRecord | null>(null);
  const [hideEmpty, setHideEmpty] = useState(false);
  const [rawOpen, setRawOpen] = useState(false);
  const [linked, setLinked] = useState<EntriesRecord | null>(null);
  const [newTag, setNewTag] = useState("");
  const [newNote, setNewNote] = useState("");
  const [writing, setWriting] = useState(false);

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

  // The live entries the row is evidence on.
  useEffect(() => {
    let live = true;
    const id = newQueryId();
    let settled = false;
    const query = { status: [], host: null, tactic: null, text: null, row: rowId };
    entriesList(query, LINKED, 0, "detail:entries", id).then(
      (page) => {
        settled = true;
        if (live) setLinked({ about: String(rowId), items: page.items, truncated: page.truncated });
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
  }, [aroundKey, rowId, reportError]);

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
  const entries = linked?.about === String(rowId) ? linked : null;

  /** One write of the pane's at a time; what the engine refuses goes to the status bar. */
  const write = async (work: () => Promise<string | null | void>) => {
    if (writing) return;
    setWriting(true);
    try {
      const refused = await work();
      if (typeof refused === "string") reportError(new Error(refused));
    } catch (e) {
      reportFailure(e, reportError);
    } finally {
      setWriting(false);
    }
  };
  const promote = () =>
    write(async () => {
      const outcomes = await rowsPromote([rowId]);
      const entry = promotedEntry(outcomes);
      if (entry !== null) onShowEntry?.(entry);
      return promoteRefusals(outcomes);
    });
  const addTag = () =>
    write(async () => {
      const refused = rowRefusals(await tagAdd([rowId], newTag));
      setNewTag("");
      return refused;
    });
  const addNote = () =>
    write(async () => {
      await noteAdd(rowId, newNote);
      setNewNote("");
    });

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

      <section className="detail-section" aria-label="Marks">
        <h3>Mark</h3>
        <p className="detail-mark-line">
          {detail.mark === null ? (
            <span className="detail-note">Not marked.</span>
          ) : (
            <>
              <MarkGlyph mark={detail.mark} />
              {` ${MARK_NAME[detail.mark]}`}
              {detail.marked_by !== null && <span className="detail-note">{` · ${displaySafe(detail.marked_by)}`}</span>}
            </>
          )}
        </p>
        <div className="detail-raw-actions">
          <button type="button" disabled={writing} onClick={() => void write(async () => rowRefusals(await markSet([rowId])))}>Mark suspicious (S)</button>
          <button type="button" disabled={writing} onClick={() => void write(async () => rowRefusals(await markClear([rowId])))}>Clear mark (U)</button>
          <button type="button" disabled={writing} onClick={() => void promote()}>Make an entry (M)</button>
        </div>
      </section>

      <section className="detail-section" aria-label="Tags">
        <h3>Tags</h3>
        {detail.tags.length === 0 ? (
          <p className="detail-note">No tags.</p>
        ) : (
          <div className="tags">
            {detail.tags.map((t) => (
              <span key={t} className="tag">
                {displaySafe(t)}
                <button
                  type="button"
                  className="tag-remove"
                  aria-label={`Remove tag ${displaySafe(t)}`}
                  disabled={writing}
                  onClick={() => void write(async () => rowRefusals(await tagRemove([rowId], t)))}
                >
                  ×
                </button>
              </span>
            ))}
          </div>
        )}
        <div className="detail-add">
          <input
            type="text"
            aria-label="New tag"
            value={newTag}
            maxLength={200}
            onChange={(e) => setNewTag(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && newTag !== "") void addTag();
            }}
          />
          <button type="button" disabled={writing || newTag === ""} onClick={() => void addTag()}>Add tag</button>
        </div>
      </section>

      <section className="detail-section" aria-label="Notes">
        <h3>Notes</h3>
        {detail.notes.length === 0 ? (
          <p className="detail-note">No notes.</p>
        ) : (
          detail.notes.map((n) => (
            <div key={n.id} className="note">
              <div className="note-meta">
                {`${displaySafe(n.written_by)} · ${displaySafe(n.written_at)}`}
                <button type="button" className="note-delete" aria-label="Delete note" disabled={writing} onClick={() => void write(() => noteDelete(n.id))}>
                  Delete
                </button>
              </div>
              <div className="note-text">{displaySafeMultiline(n.text)}</div>
            </div>
          ))
        )}
        <div className="detail-add">
          <textarea aria-label="New note" rows={3} value={newNote} onChange={(e) => setNewNote(e.target.value)} />
          <button type="button" disabled={writing || newNote === ""} onClick={() => void addNote()}>Add note</button>
        </div>
      </section>

      <section className="detail-section" aria-label="Linked entries">
        <h3>Linked entries</h3>
        {entries === null ? (
          <p className="detail-note">Loading…</p>
        ) : entries.items.length === 0 ? (
          <p className="detail-note">Not evidence on any entry.</p>
        ) : (
          <ul className="detail-entries" role="list">
            {entries.items.map((e) => (
              <li key={e.id}>
                <button type="button" className="status-link" onClick={() => onShowEntry?.(e.id)}>
                  {`E-${e.id} · ${displaySafe(e.title)} · ${e.status}`}
                </button>
              </li>
            ))}
          </ul>
        )}
        {entries?.truncated && <p className="detail-note">{`The first ${LINKED} entries.`}</p>}
      </section>
    </div>
  );
}
