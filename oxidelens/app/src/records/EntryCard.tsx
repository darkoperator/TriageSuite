// One timeline entry in the right pane: what M made (or found), or an entry a row's detail links to. The Timeline sheet
// is where it is edited; this shows its id, title, time (or "needs a time"), status, who added it, its hosts and its
// evidence count.
//
// Read: `entry_get` on slot `detail:entry`, for one key (entry, case generation); an answer is drawn only while the
// pane still shows that entry, and a new generation keeps the last answer up while the re-read runs.
import { useEffect, useState, type ReactNode } from "react";
import { entryGet, isCmdError, newQueryId, queryCancel } from "../api";
import type { Entry } from "../contract/generated/Entry";
import { useShell } from "../shell/context";
import { displaySafe, displaySafeMultiline } from "../text";
import { addedBy } from "./EntryEditor";
import { isStale } from "./outcomes";
import "./records.css";


export default function EntryCard({ entryId, onBack }: { entryId: number; onBack?: (() => void) | null }): ReactNode {
  const { change, reportError } = useShell();
  const [held, setHeld] = useState<{ id: number; entry: Entry } | null>(null);
  const [gone, setGone] = useState<number | null>(null);

  useEffect(() => {
    let live = true;
    const id = newQueryId();
    let settled = false;
    entryGet(entryId, "detail:entry", id).then(
      (entry) => {
        settled = true;
        if (!live) return;
        setHeld({ id: entryId, entry });
        setGone(null);
      },
      (e: unknown) => {
        settled = true;
        if (!live || isStale(e)) return;
        if (isCmdError(e) && e.code === "not_found") {
          // Deleted since it was shown: the old answer is dropped, not kept up.
          setHeld(null);
          setGone(entryId);
        }
        else reportError(e);
      },
    );
    return () => {
      live = false;
      if (!settled) void queryCancel([id]).catch(() => {});
    };
  }, [entryId, change.generation, reportError]);

  const entry = held?.id === entryId ? held.entry : null;
  const label = `Entry E-${entryId}`;
  return (
    <section className="entry-card" aria-label={label}>
      <header className="detail-head">
        <div className="detail-title">
          <span className="record-id">{`E-${entryId}`}</span>
          {entry && <span className="detail-rel">{displaySafe(entry.title)}</span>}
        </div>
        {onBack && (
          <button type="button" className="detail-toggle" onClick={onBack}>
            Back to the row
          </button>
        )}
      </header>
      {gone === entryId && entry === null && <p className="pane-empty">This entry was deleted.</p>}
      {entry === null && gone !== entryId && <p className="pane-empty">Loading…</p>}
      {entry && (
        <dl className="entry-facts">
          <dt>Time</dt>
          <dd>
            {entry.time === null ? <span className="badge badge-warn">needs a time</span> : displaySafe(entry.time)}
          </dd>
          <dt>Status</dt>
          <dd>{entry.status}</dd>
          <dt>Added by</dt>
          <dd>{addedBy(entry)}</dd>
          {entry.reviewed_by !== null && (
            <>
              <dt>Reviewed</dt>
              <dd>{`reviewed by ${displaySafe(entry.reviewed_by)}`}</dd>
            </>
          )}
          <dt>Hosts</dt>
          <dd>{entry.hosts.length === 0 ? "none" : entry.hosts.map((h) => displaySafe(h)).join(", ")}</dd>
          <dt>Evidence</dt>
          <dd>{`${entry.evidence_rows.toLocaleString("en-US")} ${entry.evidence_rows === 1 ? "row" : "rows"}`}</dd>
          {entry.description !== "" && (
            <>
              <dt>Description</dt>
              <dd className="entry-description">{displaySafeMultiline(entry.description)}</dd>
            </>
          )}
        </dl>
      )}
    </section>
  );
}
