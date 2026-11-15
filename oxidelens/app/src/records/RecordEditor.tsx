// The record editor in the right pane: one host, account or IOC with every field it has, its status (*Mark reviewed*)
// and who added it ("added by agent: <name>"), what the engine computes from its linked entries (entries, first and
// last seen, hosts seen), the entries it is linked to (each opens in the Timeline), its history (read when opened),
// Delete and Merge into…, and below them what its sheet adds (an IOC's sightings, an account's logon summary).
//
// Reads are by the late-answer rules (`useCaseRead`, `usePagedRead`), each for one key (record, case generation), on
// slots under `slot`. Every refusal the engine gives (S3 ownership, S7's host guard, a value its type refuses) is shown
// in place with its own words and without the error code. Delete and Merge ask once, in a dialog; nothing else asks.
import { useEffect, useMemo, useRef, useState, type FormEvent, type ReactNode, type Ref } from "react";
import { historyList, isCmdError, recordDelete, recordEntries, recordGet, recordMerge, recordStatus, recordUpsert, recordsList } from "../api";
import type { CaseRecord } from "../contract/generated/CaseRecord";
import type { Entry } from "../contract/generated/Entry";
import type { HistoryRow } from "../contract/generated/HistoryRow";
import type { IocType } from "../contract/generated/IocType";
import type { Privilege } from "../contract/generated/Privilege";
import type { RecordKind } from "../contract/generated/RecordKind";
import type { RecordQuery } from "../contract/generated/RecordQuery";
import type { RecordSpec } from "../contract/generated/RecordSpec";
import { useShell } from "../shell/context";
import { displaySafe } from "../text";
import { attackIdsOf } from "./attackIds";
import { addedBy } from "./EntryEditor";
import { isStale, messageOf, reportFailure } from "./outcomes";
import RecordDialog from "./RecordDialog";
import { useCaseRead } from "./useCaseRead";
import { usePagedRead } from "./usePagedRead";
import "./records.css";

/** The id prefix of each kind (ruling S1). */
export const PREFIX: Record<RecordKind, string> = { host: "H", account: "A", ioc: "I" };
export const KIND_LABEL: Record<RecordKind, string> = { host: "Host", account: "Account", ioc: "IOC" };
const KIND_WORD: Record<RecordKind, string> = { host: "host", account: "account", ioc: "IOC" };
/** The IOC types (ruling S4) in the engine's order, as the screens name them. */
export const IOC_TYPES: readonly { type: IocType; label: string }[] = [
  { type: "ip", label: "IP" }, { type: "domain", label: "Domain" }, { type: "hostname", label: "Hostname" },
  { type: "url", label: "URL" }, { type: "email", label: "Email" }, { type: "md5", label: "MD5" },
  { type: "sha1", label: "SHA1" }, { type: "sha256", label: "SHA256" }, { type: "file_name", label: "File name" },
  { type: "file_path", label: "File path" }, { type: "registry_key", label: "Registry key" }, { type: "service", label: "Service" },
  { type: "scheduled_task", label: "Scheduled task" }, { type: "other", label: "Other" },
];
export const iocTypeLabel = (t: IocType): string => IOC_TYPES.find((x) => x.type === t)?.label ?? t;
export const PRIVILEGES: readonly { value: Privilege; label: string }[] = [
  { value: "standard", label: "standard" }, { value: "local_admin", label: "local admin" },
  { value: "domain_admin", label: "domain admin" }, { value: "service", label: "service" }, { value: "unknown", label: "unknown" },
];
export const privilegeLabel = (p: Privilege): string => PRIVILEGES.find((x) => x.value === p)?.label ?? p;

/** An account as one line: `DOMAIN\name`, or the name alone without a domain. */
export const accountText = (a: { name: string; domain: string | null }): string => (a.domain !== null ? `${a.domain}\\${a.name}` : a.name);
/** A record as one line: a host's name, an account's `DOMAIN\name`, an IOC's type and value. Raw: draw it through
 *  `displaySafe`. */
export function recordText(r: CaseRecord): string {
  if (r.kind === "host") return r.name;
  if (r.kind === "account") return accountText(r);
  return `${iocTypeLabel(r.ioc_type).toLowerCase()} ${r.value}`;
}
export const recordId = (kind: RecordKind, id: number) => `${PREFIX[kind]}-${id}`;

/** How many linked entries, history rows and merge candidates are read at once. */
const PAGE = 100;

// ---- the form's fields -------------------------------------------------------------------------------------------------
type Fields = Record<string, string>;
function fieldsOf(r: CaseRecord): Fields {
  if (r.kind === "host") return { name: r.name, role: r.role, note: r.note };
  if (r.kind === "account") {
    return { name: r.name, domain: r.domain ?? "", sid: r.sid ?? "", description: r.description, privilege: r.privilege, note: r.note };
  }
  return { ioc_type: r.ioc_type, value: r.value, tool: r.tool ?? "", attack: r.attack_ids.join(" "), note: r.note };
}
const orNull = (s: string) => (s === "" ? null : s);
/** Every field of the record as the analyst left it: the engine checks each. */
function specOf(r: CaseRecord, f: Fields): RecordSpec {
  if (r.kind === "host") return { kind: "host", id: r.id, name: f.name, role: f.role, note: f.note };
  if (r.kind === "account") {
    return {
      kind: "account", id: r.id, name: f.name, domain: orNull(f.domain), sid: orNull(f.sid), description: f.description,
      privilege: f.privilege as Privilege, note: f.note,
    };
  }
  return {
    kind: "ioc", id: r.id, ioc_type: f.ioc_type as IocType, value: f.value, tool: orNull(f.tool),
    attack_ids: attackIdsOf(f.attack), note: f.note,
  };
}
const changed = (a: Fields, b: Fields) => Object.keys(a).some((k) => a[k] !== b[k]);

function TextField({ label, value, onChange, inputRef, multiline = false }: {
  label: string;
  value: string;
  onChange(v: string): void;
  inputRef?: Ref<HTMLInputElement>;
  multiline?: boolean;
}): ReactNode {
  return (
    <label className="editor-field">
      <span>{label}</span>
      {multiline ? (
        <textarea rows={3} value={value} onChange={(e) => onChange(e.target.value)} />
      ) : (
        <input ref={inputRef} type="text" value={value} onChange={(e) => onChange(e.target.value)} />
      )}
    </label>
  );
}

// ---- the editor ---------------------------------------------------------------------------------------------------------
export default function RecordEditor<R extends CaseRecord>({
  kind, id, slot, focusSeq = 0, onDeleted, onMerged, extra,
}: {
  kind: RecordKind;
  id: number;
  /** The slot prefix its reads run on. */
  slot: string;
  /** Rises when the sheet asks the editor to take focus (Enter): the first field is focused. */
  focusSeq?: number;
  onDeleted(id: number): void;
  onMerged(into: number): void;
  extra?(record: R): ReactNode;
}): ReactNode {
  const { change, reportError, openEntry } = useShell();
  const gen = change.generation;
  const read = useCaseRead<CaseRecord>(`${kind}:${id}`, gen, (next) => recordGet(kind, id, slot, next()), reportError);
  const record = read?.value?.kind === kind && !read.value.deleted ? read.value : null;
  const gone = (read?.error !== undefined && isCmdError(read.error) && read.error.code === "not_found") || read?.value?.deleted === true;
  useEffect(() => {
    if (read?.error !== undefined && !gone) reportFailure(read.error, reportError);
  }, [read, gone, reportError]);

  // The form holds the record's values until the analyst types in it (`dirty`); a re-read (another write, or this
  // form's own save) gives an untouched form the new values.
  const [form, setForm] = useState<{ at: string; dirty: boolean; fields: Fields } | null>(null);
  if (record && (form === null || (!form.dirty && form.at !== record.modified_at))) {
    setForm({ at: record.modified_at, dirty: false, fields: fieldsOf(record) });
  }
  const fields = record && form ? form.fields : null;
  const set = (k: string) => (value: string) => {
    if (form) setForm({ ...form, dirty: true, fields: { ...form.fields, [k]: value } });
  };
  const [note, setNote] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);
  const save = async (ev: FormEvent) => {
    ev.preventDefault();
    if (!record || !fields || saving) return;
    setSaving(true);
    setNote(null);
    try {
      await recordUpsert(specOf(record, fields));
      if (live.current) setForm((f) => (f !== null ? { ...f, dirty: false } : f));
    } catch (e) {
      if (live.current && !isStale(e)) setNote(messageOf(e));
    } finally {
      if (live.current) setSaving(false);
    }
  };
  const review = async () => {
    setNote(null);
    try {
      await recordStatus(kind, id, "reviewed");
    } catch (e) {
      if (live.current && !isStale(e)) setNote(messageOf(e));
    }
  };

  // Enter on the sheet focuses the first field: only a request made while this editor is shown, once it is there.
  const firstRef = useRef<HTMLInputElement>(null);
  const servedSeq = useRef(focusSeq);
  const loaded = record !== null;
  useEffect(() => {
    if (!loaded || focusSeq === servedSeq.current) return;
    servedSeq.current = focusSeq;
    firstRef.current?.focus();
  }, [focusSeq, loaded]);

  const entries = usePagedRead<Entry>(
    `entries:${kind}:${id}`,
    gen,
    PAGE,
    (limit, offset, q) => recordEntries(kind, id, limit, offset, `${slot}:entries`, q),
    reportError,
  );
  const [historyOpen, setHistoryOpen] = useState(false);
  const history = usePagedRead<HistoryRow>(
    historyOpen ? `history:${kind}:${id}` : null,
    gen,
    PAGE,
    (limit, offset, q) => historyList(kind, String(id), limit, offset, `${slot}:history`, q),
    reportError,
  );
  const [asking, setAsking] = useState<"delete" | "merge" | null>(null);

  const name = recordId(kind, id);
  const linked = entries.read?.value;
  const hist = history.read?.value;
  return (
    <section className="entry-card entry-editor" aria-label={`${KIND_LABEL[kind]} ${name}`}>
      <header className="detail-head">
        <div className="detail-title">
          <span className="record-id">{name}</span>
          {record && <span className="detail-rel">{displaySafe(recordText(record))}</span>}
        </div>
      </header>
      {gone && <p className="pane-empty">{`This ${KIND_WORD[kind]} was deleted.`}</p>}
      {!gone && record === null && <p className="pane-empty">Loading…</p>}
      {!gone && record && fields && (
        <>
          <form className="editor-form" aria-label={`Edit ${KIND_WORD[kind]}`} onSubmit={(ev) => void save(ev)}>
            {record.kind === "host" && (
              <>
                <TextField label="Name" value={fields.name} onChange={set("name")} inputRef={firstRef} />
                <TextField label="Role" value={fields.role} onChange={set("role")} />
              </>
            )}
            {record.kind === "account" && (
              <>
                <TextField label="Name" value={fields.name} onChange={set("name")} inputRef={firstRef} />
                <TextField label="Domain" value={fields.domain} onChange={set("domain")} />
                <TextField label="SID" value={fields.sid} onChange={set("sid")} />
                <TextField label="Description" value={fields.description} onChange={set("description")} multiline />
                <label className="editor-field">
                  <span>Privilege</span>
                  <select value={fields.privilege} onChange={(e) => set("privilege")(e.target.value)}>
                    {PRIVILEGES.map((p) => (
                      <option key={p.value} value={p.value}>
                        {p.label}
                      </option>
                    ))}
                  </select>
                </label>
              </>
            )}
            {record.kind === "ioc" && (
              <>
                <label className="editor-field">
                  <span>Type</span>
                  <select value={fields.ioc_type} onChange={(e) => set("ioc_type")(e.target.value)}>
                    {IOC_TYPES.map((t) => (
                      <option key={t.type} value={t.type}>
                        {t.label}
                      </option>
                    ))}
                  </select>
                </label>
                <TextField label="Value" value={fields.value} onChange={set("value")} inputRef={firstRef} />
                <TextField label="Tool" value={fields.tool} onChange={set("tool")} />
                <TextField label="ATT&CK ids" value={fields.attack} onChange={set("attack")} />
              </>
            )}
            <TextField label="Note" value={fields.note} onChange={set("note")} multiline />
            <div className="form-buttons">
              <button type="submit" disabled={saving || !changed(fields, fieldsOf(record))}>
                Save
              </button>
            </div>
          </form>
          <p role="status" aria-label="Refusal" className="form-error">
            {note === null ? "" : displaySafe(note)}
          </p>

          <div className="editor-status">
            <span className="record-meta">status</span>
            <span>{record.status}</span>
            <span className="record-meta">{addedBy(record)}</span>
            {record.status !== "reviewed" && (
              <button type="button" className="link-button" onClick={() => void review()}>
                Mark reviewed
              </button>
            )}
          </div>

          <dl className="entry-facts" aria-label="From its entries">
            <dt>Entries</dt>
            <dd>{record.entries.toLocaleString("en-US")}</dd>
            <dt>First seen</dt>
            <dd>{record.first_seen_shown === null ? "none dated" : displaySafe(record.first_seen_shown)}</dd>
            <dt>Last seen</dt>
            <dd>{record.last_seen_shown === null ? "none dated" : displaySafe(record.last_seen_shown)}</dd>
            {record.kind !== "host" && (
              <>
                <dt>Hosts seen</dt>
                <dd>{record.hosts_seen.length === 0 ? "none" : displaySafe(record.hosts_seen.join(", "))}</dd>
              </>
            )}
          </dl>

          <div className="form-buttons">
            <button type="button" onClick={() => setAsking("merge")}>
              Merge into…
            </button>
            <button type="button" onClick={() => setAsking("delete")}>
              Delete
            </button>
          </div>

          {extra?.(record as R)}

          <section className="editor-section" aria-label="Linked entries">
            <h3 className="editor-heading">{`Linked entries · ${record.entries.toLocaleString("en-US")}`}</h3>
            {entries.read === null && <p className="form-hint">Reading the entries…</p>}
            <ul className="editor-list">
              {(linked?.items ?? []).map((e) => (
                <li key={e.id} className="editor-item">
                  <span className="record-id">{`E-${e.id}`}</span>
                  <span className="record-meta">{e.needs_time ? "needs a time" : displaySafe(e.time_shown ?? e.time ?? "")}</span>
                  <span className="editor-item-text">{displaySafe(e.title)}</span>
                  {openEntry && (
                    <button type="button" className="link-button" aria-label={`Open E-${e.id}`} onClick={() => openEntry(e.id)}>
                      open →
                    </button>
                  )}
                </li>
              ))}
            </ul>
            {linked?.truncated && (
              <button type="button" className="link-button" onClick={entries.showMore}>
                Show more
              </button>
            )}
            <ReadFailure read={entries.read} />
          </section>

          <section className="editor-section" aria-label="Record history">
            <button type="button" className="link-button" aria-expanded={historyOpen} onClick={() => setHistoryOpen((o) => !o)}>
              {`History ${historyOpen ? "▾" : "▸"}`}
            </button>
            {historyOpen && history.read === null && <p className="form-hint">Reading the history…</p>}
            {historyOpen && hist && (
              <ul className="editor-list" aria-label="History">
                {hist.items.map((h) => (
                  <li key={h.id} className="editor-item">
                    <span className="record-meta">{displaySafe(h.at)}</span>
                    <span>{displaySafe(h.action)}</span>
                    <span className="record-meta">{displaySafe(h.by)}</span>
                  </li>
                ))}
              </ul>
            )}
            {historyOpen && hist?.truncated && (
              <button type="button" className="link-button" onClick={history.showMore}>
                Show more
              </button>
            )}
            {historyOpen && <ReadFailure read={history.read} />}
          </section>
        </>
      )}
      {asking === "delete" && record && (
        <DeleteDialog record={record} onClose={() => setAsking(null)} onDone={() => onDeleted(id)} />
      )}
      {asking === "merge" && record && (
        <MergeDialog record={record} onClose={() => setAsking(null)} onDone={(into) => onMerged(into)} />
      )}
    </section>
  );
}

/** A section's failed read (a first page or a later one), shown in place: the engine's message for a refusal, else
 *  the code with it. A stale answer never reaches here (`useCaseRead` drops it). */
function ReadFailure({ read }: { read: { error?: unknown } | null }): ReactNode {
  if (read === null || read.error === undefined) return null;
  return (
    <p role="alert" className="form-error">
      {displaySafe(messageOf(read.error))}
    </p>
  );
}

// ---- Delete and Merge into… ---------------------------------------------------------------------------------------------
/** Delete asks once: the engine tombstones the record and removes its links; the entries stay. */
function DeleteDialog({ record, onClose, onDone }: { record: CaseRecord; onClose(): void; onDone(): void }): ReactNode {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const go = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await recordDelete(record.kind, record.id);
      onClose();
      onDone();
    } catch (e) {
      setBusy(false);
      if (!isStale(e)) setError(messageOf(e));
    }
  };
  const n = record.entries;
  return (
    <RecordDialog label={`Delete ${recordId(record.kind, record.id)}`} onClose={onClose} error={error}>
      <p className="form-hint">
        {`Delete ${KIND_WORD[record.kind]} ${displaySafe(recordText(record))}? Its links to ${n.toLocaleString("en-US")} ${n === 1 ? "entry are" : "entries are"} removed; the entries stay.`}
      </p>
      <div className="form-buttons">
        <button type="button" disabled={busy} onClick={() => void go()}>
          Delete
        </button>
        <button type="button" onClick={onClose}>
          Cancel
        </button>
      </div>
    </RecordDialog>
  );
}

const queryOf = (kind: RecordKind, text: string): RecordQuery => {
  const t = text === "" ? null : text;
  return kind === "ioc" ? { kind, ioc_type: null, tool: null, text: t } : { kind, text: t };
};

/** Merge into… asks once, for the record of the same kind it goes into: every entry linked to this one ends linked to
 *  that one, and this one is deleted. The records are listed 100 at a time (*Show more*), narrowed by typing. */
function MergeDialog({ record, onClose, onDone }: { record: CaseRecord; onClose(): void; onDone(into: number): void }): ReactNode {
  const { change, reportError } = useShell();
  const [text, setText] = useState("");
  const kind = record.kind;
  const { read, showMore } = usePagedRead<CaseRecord>(
    JSON.stringify([kind, text]),
    change.generation,
    PAGE,
    (limit, offset, q) => recordsList(queryOf(kind, text), limit, offset, "dialog:merge", q),
    reportError,
  );
  const options = useMemo(() => (read?.value?.items ?? []).filter((r) => !(r.kind === kind && r.id === record.id)), [read, kind, record.id]);
  const [into, setInto] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const go = async () => {
    if (busy || into === null) return;
    setBusy(true);
    setError(null);
    try {
      await recordMerge(kind, record.id, into);
      onClose();
      onDone(into);
    } catch (e) {
      setBusy(false);
      if (!isStale(e)) setError(messageOf(e));
    }
  };
  const target = options.find((r) => r.id === into) ?? null;
  return (
    <RecordDialog
      label={`Merge ${recordId(kind, record.id)} into…`}
      onClose={onClose}
      error={error ?? (read?.error !== undefined ? messageOf(read.error) : null)}
    >
      <input
        type="search"
        aria-label="Filter records"
        placeholder={`Type to find a ${KIND_WORD[kind]}`}
        value={text}
        onChange={(e) => {
          // The list changes: what was chosen may no longer be shown, so nothing is chosen.
          setText(e.target.value);
          setInto(null);
        }}
      />
      <div className="record-list" role="listbox" aria-label="Records">
        {options.map((r) => (
          <button
            key={r.id}
            type="button"
            role="option"
            aria-selected={into === r.id}
            className={into === r.id ? "record-option record-option-picked" : "record-option"}
            onClick={() => setInto(r.id)}
          >
            <span className="record-id">{recordId(r.kind, r.id)}</span>
            <span className="record-title">{displaySafe(recordText(r))}</span>
          </button>
        ))}
      </div>
      {read === null && <p className="form-hint">Loading…</p>}
      {read?.value !== undefined && options.length === 0 && <p className="form-hint">{text === "" ? `No other ${KIND_WORD[kind]} to merge into.` : "None match."}</p>}
      {read?.value?.truncated && (
        <button type="button" className="link-button" onClick={showMore}>
          Show more
        </button>
      )}
      {target !== null && (
        <p className="form-hint">
          {`${recordId(kind, record.id)} → ${recordId(kind, target.id)} ${displaySafe(recordText(target))}: its ${record.entries.toLocaleString("en-US")} entry links move, and ${recordId(kind, record.id)} is deleted.`}
        </p>
      )}
      <div className="form-buttons">
        <button type="button" disabled={busy || into === null} onClick={() => void go()}>
          Merge
        </button>
        <button type="button" onClick={onClose}>
          Cancel
        </button>
      </div>
    </RecordDialog>
  );
}
