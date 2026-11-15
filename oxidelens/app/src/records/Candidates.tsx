// The candidates panel the entry editor shows after M (or A): the accounts and IOCs the engine finds in the entry's
// evidence rows (`candidates`, one row at a time on one slot), each offered for one click. A record the case already has
// (`existing_id`) is linked; one it has not is created (`record_upsert`) and then linked. Nothing is added unless the
// analyst clicks, and a refusal is shown here, in place, never in a dialog.
import { useState, type ReactNode } from "react";
import { candidates as readCandidates, isCmdError, recordLink, recordUpsert } from "../api";
import type { Candidate } from "../contract/generated/Candidate";
import type { EntryLink } from "../contract/generated/EntryLink";
import type { RecordSpec } from "../contract/generated/RecordSpec";
import { useShell } from "../shell/context";
import { displaySafe } from "../text";
import { isStale, messageOf } from "./outcomes";
import { useCaseRead } from "./useCaseRead";

/** The candidate as one line: an account's `domain\name`, an IOC's type and value. Text from evidence. */
export const candidateText = (c: Candidate): string =>
  c.kind === "account" ? (c.domain !== null ? `${c.domain}\\${c.value}` : c.value) : `${c.ioc_type ?? "other"} ${c.value}`;
const kindWord = (c: Candidate): string => (c.kind === "account" ? "account" : "IOC");

/** What `record_upsert` makes for a candidate the case does not have yet: its values as the engine read them. */
function specOf(c: Candidate): RecordSpec {
  if (c.kind === "account") {
    return { kind: "account", id: null, name: c.value, domain: c.domain, sid: c.sid, description: "", privilege: "unknown", note: "" };
  }
  return { kind: "ioc", id: null, ioc_type: c.ioc_type ?? "other", value: c.value, tool: null, attack_ids: [], note: "" };
}

export default function Candidates({
  entryId, rows, links, slot,
}: {
  entryId: number;
  /** The entry's evidence rows, in the engine's order. */
  rows: number[];
  /** What the entry links now: a candidate already among them says so instead of offering itself. */
  links: EntryLink[];
  slot: string;
}): ReactNode {
  const { change, reportError } = useShell();
  // Every evidence row is read, one after another; a new key cancels the one in flight and starts no more.
  const base = JSON.stringify([entryId, rows]);
  const read = useCaseRead(
    base,
    change.generation,
    async (nextId) => {
      // One row at a time on one slot, each read under its own query id. A row the case no longer has offers nothing.
      const found: Candidate[] = [];
      for (const row of rows) {
        try {
          found.push(...(await readCandidates(row, slot, nextId())));
        } catch (e) {
          if (!(isCmdError(e) && e.code === "not_found")) throw e;
        }
      }
      return found;
    },
    reportError,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ entryId: number; text: string } | null>(null);

  const take = async (c: Candidate) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const id = c.existing_id ?? (await recordUpsert(specOf(c))).id;
      await recordLink(entryId, c.kind, id);
    } catch (e) {
      if (!isStale(e)) setError({ entryId, text: messageOf(e) });
    } finally {
      setBusy(false);
    }
  };

  const offers = read?.value ?? [];
  const linked = (c: Candidate) => c.existing_id !== null && links.some((l) => l.kind === c.kind && l.id === c.existing_id);
  return (
    <section className="editor-section" aria-label="Candidates">
      <h3 className="editor-heading">Candidates · found in the evidence</h3>
      {read === null && <p className="form-hint">Reading the evidence…</p>}
      {read?.error !== undefined && <p className="form-error">{displaySafe(messageOf(read.error))}</p>}
      {read?.value !== undefined && offers.length === 0 && <p className="form-hint">No account or IOC found in the evidence.</p>}
      <ul className="editor-list">
        {offers.map((c, i) => {
          const text = displaySafe(candidateText(c));
          return (
            <li key={`${i}:${c.kind}:${c.column}`} className="editor-item">
              <span className="record-meta">{displaySafe(c.column)}</span>
              {linked(c) ? (
                <span>{`${kindWord(c)} ${text} · linked ✓`}</span>
              ) : (
                <button type="button" className="link-button" disabled={busy} onClick={() => void take(c)}>
                  {c.existing_id !== null ? `Link ${kindWord(c)} ${text}` : `Create and link ${kindWord(c)} ${text}`}
                </button>
              )}
            </li>
          );
        })}
      </ul>
      {error !== null && error.entryId === entryId && <p className="form-error">{displaySafe(error.text)}</p>}
    </section>
  );
}
