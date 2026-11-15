// The entry editor in the right pane: one timeline entry's time (UTC, with where it came from), title, description,
// ATT&CK tactic and technique ids, status with *Mark reviewed*, the records it links (each opens its sheet; more are
// linked from a picker), its evidence rows (each re-checked by the engine, each opening its dataset scrolled to the
// row), its history (read when opened), and after M the candidates found in its evidence.
//
// Reads are by the late-answer rules (`useCaseRead`): the entry, its evidence and its history each for one key (entry,
// case generation), on slots under `slot`. Every refusal the engine gives is shown in place, never in a dialog; a review
// refusal (an undated entry, an agent) is shown under the status with the engine's own words.
import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from "react";
import {
  entryEdit, entryGet, entryNew, entryStatus, evidenceList, historyList, isCmdError, recordLink, recordsList, recordUnlink, tactics as readTactics,
} from "../api";
import type { CaseRecord } from "../contract/generated/CaseRecord";
import type { Entry } from "../contract/generated/Entry";
import type { EntryChanges } from "../contract/generated/EntryChanges";
import type { EvidenceStanding } from "../contract/generated/EvidenceStanding";
import type { RecordKind } from "../contract/generated/RecordKind";
import type { RecordQuery } from "../contract/generated/RecordQuery";
import { useDialogFocus } from "../grid/dialogFocus";
import { useShell } from "../shell/context";
import { displaySafe } from "../text";
import Candidates from "./Candidates";
import { isStale, messageOf, reportFailure } from "./outcomes";
import RecordDialog from "./RecordDialog";
import { useCaseRead } from "./useCaseRead";
import "./records.css";

const AGENT = "agent:";
/** Who added it: "added by agent: <name>" for an agent's entry, reviewed or not (`agent_added` says so), else the
 *  author as stored. The entry card shows the same. */
export function addedBy(entry: Entry): string {
  if (!entry.agent_added) return displaySafe(entry.created_by);
  const name = entry.created_by.startsWith(AGENT) ? entry.created_by.slice(AGENT.length) : entry.created_by;
  return `added by agent: ${displaySafe(name)}`;
}

export const KIND_NAME: Record<RecordKind, string> = { host: "Host", account: "Account", ioc: "IOC" };
const STANDING: Record<EvidenceStanding, string> = {
  unchanged: "✓ row unchanged",
  changed: "row changed since it was attached",
  superseded: "from a superseded run",
  source_removed: "source removed",
};
const SOURCE: Record<string, string> = { row: "taken from the row", typed: "typed by hand" };
/** How many history rows are read at once. */
const HISTORY = 50;

// ---- review ------------------------------------------------------------------------------------------------------------
/** The last *Mark reviewed* refusal, for the entry it was for. */
export interface Review {
  note: { id: number; text: string } | null;
  run(id: number): void;
}

/** *Mark reviewed* (button or R): the engine decides; its refusal is kept to be shown in place. */
export function useReview(): Review {
  const [note, setNote] = useState<{ id: number; text: string } | null>(null);
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);
  const run = useCallback((id: number) => {
    setNote(null);
    entryStatus(id, "reviewed").then(
      () => {},
      (e: unknown) => {
        if (live.current && !isStale(e)) setNote({ id, text: messageOf(e) });
      },
    );
  }, []);
  return useMemo(() => ({ note, run }), [note, run]);
}

// ---- the form's fields -------------------------------------------------------------------------------------------------
interface Fields {
  title: string;
  description: string;
  time: string;
  tactic: string;
  techniques: string;
}
const fieldsOf = (e: Entry): Fields => ({
  title: e.title, description: e.description, time: e.time ?? "", tactic: e.tactic ?? "", techniques: e.technique_ids.join(" "),
});
/** The analyst's technique ids as typed, split at spaces and commas; the engine checks each. */
const techniqueList = (text: string): string[] => text.split(/[\s,]+/).filter((t) => t !== "");

/** Only what the analyst changed; `null` leaves a field as it is (a time the evidence gave stays the row's). */
function changesOf(e: Entry, f: Fields): EntryChanges | null {
  const was = fieldsOf(e);
  const text = (now: string, before: string) => (now === before ? null : now === "" ? ("clear" as const) : { set: now });
  const changes: EntryChanges = {
    title: f.title === was.title ? null : f.title,
    description: f.description === was.description ? null : f.description,
    time: text(f.time, was.time),
    tactic: text(f.tactic, was.tactic),
    technique_ids: f.techniques === was.techniques ? null : techniqueList(f.techniques),
  };
  return Object.values(changes).every((v) => v === null) ? null : changes;
}

// ---- the editor ---------------------------------------------------------------------------------------------------------
export default function EntryEditor({
  entryId, slot = "editor", candidates = false, review, focusSeq = 0, onBack,
}: {
  entryId: number;
  /** The slot prefix its reads run on. */
  slot?: string;
  /** Show the candidates found in its evidence (after M or A). */
  candidates?: boolean;
  /** The sheet's review state, so R on the sheet and the button share one refusal; the editor keeps its own without. */
  review?: Review;
  /** Rises when the sheet asks the editor to take focus (Enter): the title field is focused. */
  focusSeq?: number;
  onBack?: (() => void) | null;
}): ReactNode {
  const { change, reportError, openRow, openSheet } = useShell();
  const gen = change.generation;
  const own = useReview();
  const reviewing = review ?? own;

  const read = useCaseRead<Entry>(`entry:${entryId}`, gen, (next) => entryGet(entryId, slot, next()), reportError);
  const evidence = useCaseRead(`evidence:${entryId}`, gen, (next) => evidenceList(entryId, `${slot}:evidence`, next()), reportError);
  const [historyOpen, setHistoryOpen] = useState(false);
  const history = useCaseRead(
    historyOpen ? `history:${entryId}` : null,
    gen,
    (next) => historyList("entry", String(entryId), HISTORY, 0, `${slot}:history`, next()),
    reportError,
  );
  const [tactics, setTactics] = useState<string[]>([]);
  useEffect(() => {
    let live = true;
    readTactics().then((t) => live && setTactics(t), (e: unknown) => live && reportFailure(e, reportError));
    return () => {
      live = false;
    };
  }, [reportError]);

  const entry = read?.value ?? null;
  const gone = read?.error !== undefined && isCmdError(read.error) && read.error.code === "not_found";
  useEffect(() => {
    if (read?.error !== undefined && !gone) reportFailure(read.error, reportError);
  }, [read, gone, reportError]);

  // The form holds the entry's values until the analyst types in it (`dirty`); a re-read of the entry (another write,
  // or this form's own save) gives an untouched form the new values. Another entry always starts from its own.
  const [form, setForm] = useState<{ id: number; at: string; dirty: boolean; fields: Fields } | null>(null);
  if (entry && (form === null || form.id !== entry.id || (!form.dirty && form.at !== entry.modified_at))) {
    setForm({ id: entry.id, at: entry.modified_at, dirty: false, fields: fieldsOf(entry) });
  }
  const fields = entry && form?.id === entry.id ? form.fields : null;
  const set = (k: keyof Fields) => (value: string) => {
    if (entry && form?.id === entry.id) setForm({ ...form, dirty: true, fields: { ...form.fields, [k]: value } });
  };
  const [saveError, setSaveError] = useState<{ id: number; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const save = async (ev: FormEvent) => {
    ev.preventDefault();
    if (!entry || !fields || saving) return;
    const changes = changesOf(entry, fields);
    if (changes === null) return;
    setSaving(true);
    setSaveError(null);
    try {
      await entryEdit(entry.id, changes);
      // Saved: the re-read the write announces gives the form the stored values.
      setForm((f) => (f !== null && f.id === entry.id ? { ...f, dirty: false } : f));
    } catch (e) {
      if (!isStale(e)) setSaveError({ id: entry.id, text: messageOf(e) });
    } finally {
      setSaving(false);
    }
  };

  // Enter on the sheet focuses the title: only a request made while this editor is shown (an editor opened on another
  // entry after an earlier Enter does not take the focus), served once the entry is there.
  const titleRef = useRef<HTMLInputElement>(null);
  const servedSeq = useRef(focusSeq);
  const loaded = entry !== null;
  useEffect(() => {
    if (!loaded || focusSeq === servedSeq.current) return;
    servedSeq.current = focusSeq;
    titleRef.current?.focus();
  }, [focusSeq, loaded]);

  const [linkError, setLinkError] = useState<{ id: number; text: string } | null>(null);
  const unlink = async (kind: RecordKind, id: number) => {
    setLinkError(null);
    try {
      await recordUnlink(entryId, kind, id);
    } catch (e) {
      if (!isStale(e)) setLinkError({ id: entryId, text: messageOf(e) });
    }
  };
  const [picking, setPicking] = useState(false);

  const label = `Entry E-${entryId}`;
  const rows = evidence?.value ?? [];
  return (
    <section className="entry-card entry-editor" aria-label={label}>
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
      {gone && <p className="pane-empty">This entry was deleted.</p>}
      {!gone && entry === null && <p className="pane-empty">Loading…</p>}
      {!gone && entry && fields && (
        <>
          <form className="editor-form" aria-label="Edit entry" onSubmit={(ev) => void save(ev)}>
            <label className="editor-field">
              <span>Time (UTC)</span>
              <input type="text" value={fields.time} placeholder="YYYY-MM-DDTHH:MM:SSZ" onChange={(ev) => set("time")(ev.target.value)} />
            </label>
            <p className="form-hint">
              {entry.needs_time ? <span className="badge badge-warn">needs a time</span> : (SOURCE[entry.time_source ?? ""] ?? null)}
            </p>
            <label className="editor-field">
              <span>Title</span>
              <input ref={titleRef} type="text" value={fields.title} onChange={(ev) => set("title")(ev.target.value)} />
            </label>
            <label className="editor-field">
              <span>Description</span>
              <textarea rows={3} value={fields.description} onChange={(ev) => set("description")(ev.target.value)} />
            </label>
            <label className="editor-field">
              <span>Tactic</span>
              <select value={fields.tactic} onChange={(ev) => set("tactic")(ev.target.value)}>
                <option value="">none</option>
                {/* The stored tactic is offered even before the list arrives, so the select never shows a wrong value. */}
                {[...new Set([...(fields.tactic === "" ? [] : [fields.tactic]), ...tactics])].map((t) => (
                  <option key={t} value={t}>
                    {displaySafe(t)}
                  </option>
                ))}
              </select>
            </label>
            <label className="editor-field">
              <span>Techniques</span>
              <input type="text" value={fields.techniques} placeholder="T1003.001 T1078" onChange={(ev) => set("techniques")(ev.target.value)} />
            </label>
            <div className="form-buttons">
              <button type="submit" disabled={saving || changesOf(entry, fields) === null}>
                Save
              </button>
            </div>
            {saveError !== null && saveError.id === entry.id && <p className="form-error">{displaySafe(saveError.text)}</p>}
          </form>

          <div className="editor-status">
            <span className="record-meta">status</span>
            <span>{entry.status}</span>
            {entry.reviewed_by !== null && <span className="record-meta">{`reviewed by ${displaySafe(entry.reviewed_by)}`}</span>}
            <span className="record-meta">{addedBy(entry)}</span>
            {entry.status !== "reviewed" && (
              <button type="button" className="link-button" onClick={() => reviewing.run(entry.id)}>
                Mark reviewed
              </button>
            )}
          </div>
          <p role="status" aria-label="Review" className="form-error">
            {reviewing.note !== null && reviewing.note.id === entry.id ? displaySafe(reviewing.note.text) : ""}
          </p>

          <section className="editor-section" aria-label="Linked records">
            <h3 className="editor-heading">Linked</h3>
            <ul className="editor-list">
              {entry.links.map((l) => (
                <li key={`${l.kind}:${l.id}`} className="editor-item">
                  <span className="editor-item-text">{`${KIND_NAME[l.kind]} · ${displaySafe(l.label)}`}</span>
                  {openSheet && (
                    <button type="button" className="link-button" aria-label={`Open ${KIND_NAME[l.kind]} ${displaySafe(l.label)}`} onClick={() => openSheet(l.kind)}>
                      open →
                    </button>
                  )}
                  <button type="button" className="link-button" aria-label={`Unlink ${KIND_NAME[l.kind]} ${displaySafe(l.label)}`} onClick={() => void unlink(l.kind, l.id)}>
                    unlink
                  </button>
                </li>
              ))}
            </ul>
            <button type="button" className="link-button" onClick={() => setPicking(true)}>
              + link a host, account or IOC
            </button>
            {linkError !== null && linkError.id === entryId && <p className="form-error">{displaySafe(linkError.text)}</p>}
          </section>

          <section className="editor-section" aria-label="Evidence">
            <h3 className="editor-heading">{`Evidence · ${entry.evidence_rows.toLocaleString("en-US")} ${entry.evidence_rows === 1 ? "row" : "rows"}`}</h3>
            {evidence === null && <p className="form-hint">Reading the evidence…</p>}
            <ul className="editor-list">
              {rows.map((ev) => (
                <li key={ev.row_id} className="editor-evidence">
                  <div className="editor-item">
                    <span className="editor-item-text">{`${displaySafe(ev.relation)}${ev.host === null ? "" : ` · ${displaySafe(ev.host)}`}`}</span>
                    <span className={ev.standing === "unchanged" ? "standing-ok" : "standing-warn"}>{STANDING[ev.standing]}</span>
                  </div>
                  <div className="editor-summary">{displaySafe(ev.summary)}</div>
                  {openRow && ev.standing !== "source_removed" && (
                    <button type="button" className="link-button" aria-label="open in dataset, scrolled to the row" onClick={() => openRow(ev.relation, ev.row_id)}>
                      open in dataset, scrolled to the row <span aria-hidden="true">→</span>
                    </button>
                  )}
                </li>
              ))}
            </ul>
          </section>

          {candidates && <Candidates entryId={entryId} rows={rows.map((r) => r.row_id)} links={entry.links} slot={`${slot}:candidates`} />}

          <section className="editor-section" aria-label="Entry history">
            <button type="button" className="link-button" aria-expanded={historyOpen} onClick={() => setHistoryOpen((o) => !o)}>
              {`History ${historyOpen ? "▾" : "▸"}`}
            </button>
            {historyOpen && history === null && <p className="form-hint">Reading the history…</p>}
            {historyOpen && history?.value && (
              <ul className="editor-list" aria-label="History">
                {history.value.items.map((h) => (
                  <li key={h.id} className="editor-item">
                    <span className="record-meta">{displaySafe(h.at)}</span>
                    <span>{displaySafe(h.action)}</span>
                    <span className="record-meta">{displaySafe(h.by)}</span>
                  </li>
                ))}
              </ul>
            )}
          </section>
        </>
      )}
      {picking && <LinkPicker entryId={entryId} onClose={() => setPicking(false)} />}
    </section>
  );
}

// ---- linking a record by hand -------------------------------------------------------------------------------------------
/** How many records the picker asks for at once. */
const LISTED = 200;
/** What the picker lists: the records of `pages` reads of 200, as the case was at its write `generation`. */
interface Listed {
  items: CaseRecord[];
  truncated: boolean;
  pages: number;
  generation: number;
}
/** The engine's query for `kind` narrowed by `text` (a host's name, an account's `DOMAIN\name`, an IOC's value, tool
 *  or note); empty text lists them all. */
const queryOf = (kind: RecordKind, text: string): RecordQuery => {
  const t = text === "" ? null : text;
  return kind === "ioc" ? { kind, ioc_type: null, tool: null, text: t } : { kind, text: t };
};
/** A record as one line in the picker: a host's name, an account's `domain\name`, an IOC's type and value. */
function recordText(r: CaseRecord): string {
  if (r.kind === "host") return r.name;
  if (r.kind === "account") return r.domain !== null ? `${r.domain}\\${r.name}` : r.name;
  return `${r.ioc_type} ${r.value}`;
}

/** *+ link a host, account or IOC*: the case's records of one kind, 200 at a time. Typing asks the engine for the ones
 *  whose text matches, the closest first, and *Show more* asks for the next 200 of them and adds those to the list,
 *  which says how many it holds, so every record is reachable. A read is for one (kind, text): an answer for an older
 *  one is dropped, and the read in flight is cancelled when they change or the picker closes. A click links one (focus
 *  goes back to the button). */
function LinkPicker({ entryId, onClose }: { entryId: number; onClose(): void }): ReactNode {
  const { change, reportError } = useShell();
  const [kind, setKind] = useState<RecordKind>("host");
  const [text, setText] = useState("");
  // *Show more* asks for one page more of this kind and text; another kind or text starts again from one page.
  const base = JSON.stringify([kind, text]);
  const [pages, setPages] = useState({ base, n: 1 });
  const shownPages = pages.base === base ? pages.n : 1;
  if (pages.base !== base) setPages({ base, n: 1 });
  const generation = change.generation;
  const listed = useCaseRead<Listed>(
    base,
    `${generation}:${shownPages}`,
    async (next, had) => {
      // The pages listed stay while the case is as it was when they were read, so *Show more* reads the next 200 only;
      // after a write every page shown is read again, one after another.
      const kept = had !== undefined && had.generation === generation && had.truncated && had.pages < shownPages ? had : null;
      const items = kept === null ? [] : [...kept.items];
      for (let page = kept === null ? 0 : kept.pages; page < shownPages; page++) {
        const got = await recordsList(queryOf(kind, text), LISTED, page * LISTED, "dialog:link", next());
        items.push(...got.items);
        if (!got.truncated) return { items, truncated: false, pages: page + 1, generation };
      }
      return { items, truncated: true, pages: shownPages, generation };
    },
    reportError,
  );
  const shown = listed?.value;
  const list = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  // *Show more* goes once the last records are listed (or its read fails). The focus it had then goes to the first
  // record it added, else to the search box: left on nothing, Tab and Escape would not reach the dialog.
  const adding = useRef<number | null>(null);
  useEffect(() => {
    const from = adding.current;
    adding.current = null;
    if (from === null || listed === null) return;
    const dialog = list.current?.closest('[role="dialog"]');
    if (!dialog || dialog.contains(document.activeElement)) return;
    (list.current?.querySelectorAll<HTMLElement>('[role="option"]')[from] ?? search.current)?.focus();
  }, [listed]);
  const showMore = () => {
    if (shown === undefined) return;
    adding.current = shown.items.length;
    setPages({ base, n: shown.pages + 1 });
  };
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const close = onClose;
  const pick = async (r: CaseRecord) => {
    if (busy) return;
    setBusy(true);
    try {
      await recordLink(entryId, r.kind, r.id);
      close();
    } catch (e) {
      setBusy(false);
      if (!isStale(e)) setError(messageOf(e));
    }
  };
  return (
    <RecordDialog label="Link a record" onClose={close} error={error ?? (listed?.error !== undefined ? messageOf(listed.error) : null)}>
      <label className="editor-field">
        <span>Kind</span>
        <select value={kind} onChange={(e) => setKind(e.target.value as RecordKind)}>
          <option value="host">Host</option>
          <option value="account">Account</option>
          <option value="ioc">IOC</option>
        </select>
      </label>
      <input ref={search} type="search" aria-label="Filter records" placeholder="Type to find a record" value={text} onChange={(e) => setText(e.target.value)} />
      <div ref={list} className="record-list" role="listbox" aria-label="Records">
        {(shown?.items ?? []).map((r) => (
          <button key={r.id} type="button" role="option" aria-selected={false} className="record-option" disabled={busy} onClick={() => void pick(r)}>
            <span className="record-title">{displaySafe(recordText(r))}</span>
          </button>
        ))}
      </div>
      {listed === null && <p className="form-hint">Loading…</p>}
      {shown?.items.length === 0 && <p className="form-hint">{text === "" ? "The case has none of these yet." : "None match."}</p>}
      {shown !== undefined && shown.items.length > 0 && (
        <p className="form-hint">
          <span>{`${shown.items.length.toLocaleString("en-US")} listed${!shown.truncated ? "" : text === "" ? " · more in the case" : " · more match"}`}</span>
          {shown.truncated && (
            <>
              {" "}
              <button type="button" className="link-button" onClick={showMore}>
                Show more
              </button>
            </>
          )}
        </p>
      )}
      <div className="form-buttons">
        <button type="button" onClick={close}>Cancel</button>
      </div>
    </RecordDialog>
  );
}

// ---- a new entry (N) ----------------------------------------------------------------------------------------------------
/** N on the Timeline: a title, then *Create*; the engine makes a draft with no time, and the editor opens on it. */
export function NewEntryForm({ onCreated, onCancel }: { onCreated(id: number): void; onCancel(): void }): ReactNode {
  const box = useRef<HTMLFormElement>(null);
  // Focus goes to the title, and back to what had it (the Timeline's row) when the form goes.
  useDialogFocus(box);
  const [title, setTitle] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);
  const create = async (ev: FormEvent) => {
    ev.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const made = await entryNew({ title, description: null, time: null, tactic: null, technique_ids: null });
      if (live.current) onCreated(made.id);
    } catch (e) {
      if (live.current) {
        setBusy(false);
        if (!isStale(e)) setError(messageOf(e));
      }
    }
  };
  return (
    <form
      ref={box}
      className="entry-card editor-form"
      aria-label="New entry"
      onSubmit={(ev) => void create(ev)}
      onKeyDown={(ev) => {
        if (ev.key === "Escape") {
          ev.preventDefault();
          onCancel();
        }
      }}
    >
      <h3 className="editor-heading">New entry</h3>
      <label className="editor-field">
        <span>Title</span>
        <input type="text" value={title} onChange={(ev) => setTitle(ev.target.value)} />
      </label>
      <div className="form-buttons">
        <button type="submit" disabled={busy}>
          Create
        </button>
        <button type="button" onClick={onCancel}>
          Cancel
        </button>
      </div>
      {error !== null && <p className="form-error">{displaySafe(error)}</p>}
    </form>
  );
}
