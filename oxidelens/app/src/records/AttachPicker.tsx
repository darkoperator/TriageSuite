// A: the rows the selection was resolved to, attached as evidence to an entry the analyst picks. The entries are the
// Timeline's (`entries_list`), the most recently created or changed first (the engine's `recent` order); typing narrows them through the engine's text filter, one read at a time on one slot, and
// only the answer for what the box holds now is drawn.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { entriesList, newQueryId, queryCancel, rowsAttach } from "../api";
import type { Entry } from "../contract/generated/Entry";
import { displaySafe } from "../text";
import { isStale, messageOf, rowRefusals } from "./outcomes";
import RecordDialog from "./RecordDialog";

const fmt = (n: number) => n.toLocaleString("en-US");
/** How many entries the picker lists at once. */
const LISTED = 50;

export default function AttachPicker({
  rows, report, onAttached, onClose,
}: {
  rows: number[];
  report(e: unknown): void;
  /** The rows are now evidence on `entryId` (the rows the engine did not take have been reported). */
  onAttached(entryId: number): void;
  onClose(): void;
}): ReactNode {
  const [text, setText] = useState("");
  const [listed, setListed] = useState<{ text: string; items: Entry[]; truncated: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textRef = useRef(text);
  useEffect(() => {
    textRef.current = text;
  });

  useEffect(() => {
    const asked = text;
    const id = newQueryId();
    let settled = false;
    const query = { status: [], host: null, tactic: null, text: asked === "" ? null : asked, row: null, order: "recent" as const };
    entriesList(query, LISTED, 0, "dialog:attach", id).then(
      (page) => {
        settled = true;
        if (textRef.current === asked) setListed({ text: asked, items: page.items, truncated: page.truncated });
      },
      (e: unknown) => {
        settled = true;
        if (textRef.current === asked && !isStale(e)) setError(messageOf(e));
      },
    );
    return () => {
      if (!settled) void queryCancel([id]).catch(() => {});
    };
  }, [text]);

  const pick = async (entry: Entry) => {
    if (busy) return;
    setBusy(true);
    try {
      const outcomes = await rowsAttach(entry.id, rows);
      const refused = rowRefusals(outcomes);
      if (refused !== null) report(new Error(refused));
      onAttached(entry.id);
      onClose();
    } catch (e) {
      setBusy(false);
      if (!isStale(e)) setError(messageOf(e));
    }
  };

  const shown = listed?.text === text ? listed : null;
  return (
    <RecordDialog label="Attach to an entry" onClose={onClose} error={error}>
      <p className="form-hint">{`${fmt(rows.length)} ${rows.length === 1 ? "row" : "rows"} as evidence`}</p>
      <input type="search" aria-label="Filter entries" placeholder="Title or description" value={text} onChange={(e) => setText(e.target.value)} />
      <div className="record-list" role="listbox" aria-label="Entries">
        {(shown?.items ?? []).map((e) => (
          <button key={e.id} type="button" role="option" aria-selected={false} className="record-option" disabled={busy} onClick={() => void pick(e)}>
            <span className="record-id">{`E-${e.id}`}</span>
            <span className="record-title">{displaySafe(e.title)}</span>
            <span className="record-meta">{e.needs_time ? "needs a time" : displaySafe(e.time ?? "")}</span>
          </button>
        ))}
      </div>
      {shown === null && <p className="form-hint">Loading…</p>}
      {shown !== null && shown.items.length === 0 && <p className="form-hint">No entries match.</p>}
      {shown?.truncated && <p className="form-hint">{`The first ${LISTED}: type to narrow them.`}</p>}
      <div className="form-buttons">
        <button type="button" onClick={onClose}>Cancel</button>
      </div>
    </RecordDialog>
  );
}
