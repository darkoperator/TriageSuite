// The Timeline sheet (screen 08): the entries the analyst built from evidence, as the engine lists them (by time, the
// undated "needs a time" ones last), narrowed by its text, status, host and tactic filters and grouped by host, tactic
// or status, as a table or as Host lanes. The selected entry is edited in the right pane (`EntryEditor`).
//
// Nothing here sorts, filters or converts a time: the filters go to `entries_list`, the times are the engine's text in
// the case's display zone (`time_shown`), the glyph is the mark the entry gives its rows (`mark`), the Account column is
// the first account it links (`links`). Grouping only splits the engine's answer, in its order, by a field it gives.
//
// The Host filter is a picker, as the link picker is: typing asks the engine for the host records whose name holds the
// text (the closest first), 200 at a time with *Show more*, so every host in the case can be picked.
//
// Keys on the table (inactive while a text field has focus): ↑/↓ move the selection, N new entry, Enter edit (the
// editor's first field), R mark reviewed (a refusal is shown in the editor, never in a dialog), Del remove (the engine
// keeps the evidence rows in their datasets).
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { entriesList, entryDelete, recordsList, tactics as readTactics } from "../api";
import type { CaseRecord } from "../contract/generated/CaseRecord";
import type { Entry } from "../contract/generated/Entry";
import type { EntryQuery } from "../contract/generated/EntryQuery";
import type { EntryStatus } from "../contract/generated/EntryStatus";
import ZonePicker from "../grid/ZonePicker";
import { useShell } from "../shell/context";
import { isTypingTarget } from "../shell/keys";
import type { TabBodyProps } from "../shell/slots";
import { displaySafe } from "../text";
import EntryEditor, { NewEntryForm, useReview } from "./EntryEditor";
import HostLanes, { NO_HOST } from "./HostLanes";
import MarkGlyph from "./MarkGlyph";
import { messageOf, reportFailure } from "./outcomes";
import RecordDialog from "./RecordDialog";
import { useCaseRead } from "./useCaseRead";
import { usePagedRead } from "./usePagedRead";
import "./records.css";

/** The most entries one read lists (the engine's `MAX_LIMIT`). */
const LIMIT = 1000;
const SLOT = "sheet:timeline";
const STATUSES: readonly EntryStatus[] = ["draft", "unreviewed", "reviewed"];
type GroupBy = "" | "host" | "tactic" | "status";
const fmt = (n: number) => n.toLocaleString("en-US");

/** The groups of the engine's answer, each in its order: the group keys in the order they first appear. */
function groupsOf(items: Entry[], by: GroupBy): { key: string; label: string; entries: Entry[] }[] {
  if (by === "") {
    // The engine lists the undated last; they get their own heading.
    const dated = items.filter((e) => !e.needs_time);
    const undated = items.filter((e) => e.needs_time);
    const out = [{ key: "", label: "", entries: dated }];
    if (undated.length > 0) out.push({ key: "undated", label: "Undated · needs a time", entries: undated });
    return out;
  }
  const keysOf = (e: Entry): string[] =>
    by === "host" ? (e.hosts.length === 0 ? [NO_HOST] : e.hosts) : by === "tactic" ? [e.tactic ?? "no tactic"] : [e.status];
  const groups = new Map<string, Entry[]>();
  for (const e of items) for (const k of keysOf(e)) groups.set(k, [...(groups.get(k) ?? []), e]);
  return [...groups].map(([key, entries]) => ({ key, label: key, entries }));
}

function StatusText({ entry }: { entry: Entry }): ReactNode {
  if (entry.agent_added && entry.status === "unreviewed") return <span className="status-agent">agent · review</span>;
  return <span className={entry.status === "reviewed" ? "status-reviewed" : "status-other"}>{entry.status}</span>;
}

export default function TimelineSheet({ tab, active }: TabBodyProps): ReactNode {
  const { summary, change, reportError, setRightPane, pendingSelect, clearPendingSelect } = useShell();
  const [text, setText] = useState("");
  const [status, setStatus] = useState<EntryStatus | "">("");
  const [host, setHost] = useState("");
  const [tactic, setTactic] = useState("");
  const [groupBy, setGroupBy] = useState<GroupBy>("");
  const [view, setView] = useState<"table" | "lanes">("table");
  const [selected, setSelected] = useState<number | null>(null);
  const [creating, setCreating] = useState(false);
  const [focusSeq, setFocusSeq] = useState(0);
  const [pickingHost, setPickingHost] = useState(false);
  const review = useReview();

  const query: EntryQuery = useMemo(
    () => ({ status: status === "" ? [] : [status], host: host === "" ? null : host, tactic: tactic === "" ? null : tactic, text: text === "" ? null : text, row: null, order: "timeline" }),
    [status, host, tactic, text],
  );
  // A hidden sheet reads nothing; shown again, it reads for the case as it is then. The list is read a page of 1,000
  // at a time: *Show more* asks for one page more, and every page shown is read again, one after another, on a write
  // (the last answer stays up meanwhile). Other filters start again from one page.
  const base = active ? JSON.stringify([summary.case_id, query]) : null;
  const [pages, setPages] = useState<{ base: string | null; n: number }>({ base, n: 1 });
  const shownPages = pages.base === base ? pages.n : 1;
  if (pages.base !== base) setPages({ base, n: 1 });
  const listed = useCaseRead(
    base,
    `${change.generation}:${shownPages}`,
    async (next) => {
      const items: Entry[] = [];
      for (let page = 0; page < shownPages; page++) {
        const got = await entriesList(query, LIMIT, page * LIMIT, SLOT, next());
        items.push(...got.items);
        if (!got.truncated) return { items, truncated: false };
      }
      return { items, truncated: true };
    },
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
  useEffect(() => {
    if (listed?.error !== undefined) reportFailure(listed.error, reportError);
  }, [listed, reportError]);

  const items = useMemo(() => listed?.value?.items ?? [], [listed]);
  const groups = useMemo(() => groupsOf(items, groupBy), [items, groupBy]);
  // The entries in the order the table draws them, each once: what ↑ and ↓ step through.
  const order = useMemo(() => [...new Set(groups.flatMap((g) => g.entries.map((e) => e.id)))], [groups]);

  // ---- the right pane and the status bar, while this tab is shown ------------------------------------------------------
  const table = useRef<HTMLTableElement>(null);
  const body = useRef<HTMLDivElement>(null);
  // Switching views moves the focus into the view shown (the table, or the lanes: the first glyph, else the lanes
  // themselves), so the keys keep working where the toggle left off. Not on the first render.
  const shownView = useRef(view);
  useEffect(() => {
    if (shownView.current === view) return;
    shownView.current = view;
    const b = body.current;
    if (!b) return;
    const target = view === "table" ? b.querySelector<HTMLElement>("table") : (b.querySelector<HTMLElement>(".lane-glyph") ?? b.querySelector<HTMLElement>(".host-lanes"));
    target?.focus();
  }, [view]);
  const select = useCallback((id: number | null) => {
    setSelected(id);
    setCreating(false);
  }, []);
  // An entry another screen asked for (*open →* on a record's linked entry): selected as the request arrives, which the
  // shell then forgets.
  const [taken, setTaken] = useState<typeof pendingSelect>(null);
  if (pendingSelect && pendingSelect.sheet === "timeline" && pendingSelect !== taken) {
    setTaken(pendingSelect);
    setSelected(pendingSelect.id);
    setCreating(false);
  }
  useEffect(() => {
    if (pendingSelect?.sheet === "timeline") clearPendingSelect?.();
  }, [pendingSelect, clearPendingSelect]);
  const paneNode = useMemo(() => {
    if (creating) {
      return (
        <NewEntryForm onCreated={(id) => select(id)} onCancel={() => setCreating(false)} />
      );
    }
    if (selected === null) return <p className="pane-empty">Select an entry to edit it, or press N for a new one.</p>;
    return <EntryEditor key={selected} entryId={selected} slot={`${SLOT}:editor`} review={review} focusSeq={focusSeq} />;
  }, [creating, selected, review, focusSeq, select]);
  useEffect(() => {
    if (!active) return;
    setRightPane(paneNode);
    return () => setRightPane(null);
  }, [active, paneNode, setRightPane]);
  const shown = listed?.value ? listed.value.items.length : null;
  const statusText =
    shown === null ? null : `${fmt(shown)} ${shown === 1 ? "entry" : "entries"} shown${listed?.value?.truncated ? " · more in the case" : ""}`;

  // The selected row has the focus while the table does, so the keys act on what is shown as selected.
  useEffect(() => {
    const t = table.current;
    if (!t || selected === null || !t.contains(document.activeElement)) return;
    t.querySelector<HTMLElement>(`tr[data-entry="${selected}"]`)?.focus();
  }, [selected]);

  const remove = async (id: number) => {
    const at = order.indexOf(id);
    try {
      await entryDelete(id);
      setSelected((s) => (s === id ? (order[at + 1] ?? order[at - 1] ?? null) : s));
    } catch (e) {
      reportFailure(e, reportError);
    }
  };

  // The sheet's keys, one path for the table and the Host lanes: they act on the entry whose row or glyph has the focus
  // (`data-entry`), else on the selection. ↑ and ↓ step through the table's rows.
  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (e.ctrlKey || e.altKey || e.metaKey || isTypingTarget(e.target)) return;
    const focused = e.target instanceof HTMLElement ? e.target.closest("[data-entry]")?.getAttribute("data-entry") : null;
    const target = focused ? Number(focused) : selected;
    const at = target === null ? -1 : order.indexOf(target);
    if (view === "table" && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      const next = e.key === "ArrowDown" ? Math.min(order.length - 1, at + 1) : Math.max(0, at - 1);
      if (order[next] !== undefined) select(order[next]);
    } else if (e.key === "n" || e.key === "N") {
      setCreating(true);
    } else if (e.key === "Enter") {
      if (target === null) return;
      select(target);
      setFocusSeq((s) => s + 1);
    } else if (e.key === "r" || e.key === "R") {
      if (target === null) return;
      select(target);
      review.run(target);
    } else if (e.key === "Delete") {
      if (target !== null) void remove(target);
    } else {
      return;
    }
    e.preventDefault();
  };

  const zone = summary.display_zone;
  return (
    <div className="timeline-sheet">
      <div className="sheet-bar">
        <input type="search" className="sheet-search" aria-label="Search entries" placeholder="search entries" value={text} onChange={(e) => setText(e.target.value)} />
        <select aria-label="Status" value={status} onChange={(e) => setStatus(e.target.value as EntryStatus | "")}>
          <option value="">Status: all</option>
          {STATUSES.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </select>
        <button type="button" className="sheet-filter" aria-haspopup="dialog" aria-expanded={pickingHost} onClick={() => setPickingHost(true)}>
          {`Host: ${host === "" ? "all" : displaySafe(host)}`}
        </button>
        {pickingHost && (
          <HostPicker
            onPick={(h) => {
              setHost(h);
              setPickingHost(false);
            }}
            onClose={() => setPickingHost(false)}
          />
        )}
        <select aria-label="Tactic" value={tactic} onChange={(e) => setTactic(e.target.value)}>
          <option value="">Tactic: all</option>
          {tactics.map((t) => (
            <option key={t} value={t}>
              {displaySafe(t)}
            </option>
          ))}
        </select>
        <select aria-label="Group by" value={groupBy} onChange={(e) => setGroupBy(e.target.value as GroupBy)}>
          <option value="">Group by: none</option>
          <option value="host">host</option>
          <option value="tactic">tactic</option>
          <option value="status">status</option>
        </select>
        <ZonePicker zone={zone} tabId={tab.id} onError={reportError} />
        <span className="sheet-bar-gap" />
        <div className="view-toggle">
          <button type="button" aria-pressed={view === "table"} onClick={() => setView("table")}>
            Table
          </button>
          <button type="button" aria-pressed={view === "lanes"} onClick={() => setView("lanes")}>
            Host lanes
          </button>
        </div>
      </div>
      {/* Both views sit in one container that holds the sheet's keys; the view switch moves the focus into it. */}
      <div ref={body} className="sheet-body" role="group" aria-label="Entries" tabIndex={-1} onKeyDown={onKeyDown}>
      {view === "lanes" ? (
        <HostLanes entries={items} selected={selected} onSelect={select} />
      ) : (
        <div className="sheet-scroll">
          <table ref={table} className="sheet-table" role="grid" aria-label="Timeline entries" tabIndex={0}>
            <thead>
              <tr>
                <th aria-label="Mark" className="col-mark" />
                <th>{`Time (${displaySafe(zone)})`}</th>
                <th>Host</th>
                <th>What happened</th>
                <th>Account</th>
                <th>ATT&amp;CK tactic</th>
                <th className="col-num">Rows</th>
                <th>Status</th>
              </tr>
            </thead>
            <tbody>
              {groups.map((g) => [
                g.label !== "" && (
                  <tr key={`group:${g.key}`} className="sheet-group" data-group={g.key}>
                    <td role="gridcell" colSpan={8}>{`${displaySafe(g.label)} · ${fmt(g.entries.length)}`}</td>
                  </tr>
                ),
                ...g.entries.map((e) => {
                  const account = e.links.find((l) => l.kind === "account");
                  return (
                    <tr
                      key={`${g.key}:${e.id}`}
                      data-entry={e.id}
                      className={selected === e.id ? "sheet-row sheet-row-selected" : "sheet-row"}
                      aria-selected={selected === e.id}
                      tabIndex={-1}
                      onClick={() => select(e.id)}
                    >
                      <td role="gridcell" className="col-mark">
                        <MarkGlyph mark={e.mark} />
                      </td>
                      <td role="gridcell" className="col-time">{e.needs_time ? <span className="badge badge-warn">needs a time</span> : displaySafe(e.time_shown ?? e.time ?? "")}</td>
                      <td role="gridcell">{displaySafe(e.hosts.join(", "))}</td>
                      <td role="gridcell" className="col-title">{displaySafe(e.title)}</td>
                      <td role="gridcell">{account ? displaySafe(account.label) : ""}</td>
                      <td role="gridcell">{e.tactic === null ? "" : displaySafe(e.tactic)}</td>
                      <td role="gridcell" className="col-num">{fmt(e.evidence_rows)}</td>
                      <td role="gridcell">
                        <StatusText entry={e} />
                      </td>
                    </tr>
                  );
                }),
              ])}
            </tbody>
          </table>
          {listed === null && <p className="pane-empty">Reading the entries…</p>}
          {listed?.value?.items.length === 0 && <p className="pane-empty">No entries. Press M on a row, or N here, to make one.</p>}
        </div>
      )}
      </div>
      {statusText !== null && (
        <div className="sheet-foot">
          <span>{statusText}</span>
          {listed?.value?.truncated && (
            <button type="button" className="link-button" onClick={() => setPages({ base, n: shownPages + 1 })}>
              Show more
            </button>
          )}
        </div>
      )}
    </div>
  );
}

// ---- the Host filter ----------------------------------------------------------------------------------------------------
/** How many host records the Host filter asks for at once. */
const HOSTS_LISTED = 200;

/** The Host filter's picker: *All hosts*, then the case's host records 200 at a time, narrowed and ranked by the engine
 *  as the analyst types (`records_list` with the text), *Show more* reading the next 200, so every host is reachable. A
 *  read is for one text: an answer for an older one is dropped and the read in flight cancelled (`usePagedRead`). A
 *  click picks one; Escape closes it with the filter as it was; focus goes back to the Host button. */
function HostPicker({ onPick, onClose }: { onPick(host: string): void; onClose(): void }): ReactNode {
  const { change, reportError } = useShell();
  const [text, setText] = useState("");
  const { read, showMore } = usePagedRead<CaseRecord>(
    JSON.stringify(["hosts", text]),
    change.generation,
    HOSTS_LISTED,
    (limit, offset, q) => recordsList({ kind: "host", text: text === "" ? null : text }, limit, offset, `${SLOT}:hosts`, q),
    reportError,
  );
  const shown = read?.value;
  const names = (shown?.items ?? []).flatMap((r) => (r.kind === "host" ? [{ id: r.id, name: r.name }] : []));
  const list = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  // *Show more* goes once the last hosts are listed: the focus it had goes to the first host it added, else the search.
  const adding = useRef<number | null>(null);
  useEffect(() => {
    const from = adding.current;
    adding.current = null;
    if (from === null || read === null) return;
    const dialog = list.current?.closest('[role="dialog"]');
    if (!dialog || dialog.contains(document.activeElement)) return;
    (list.current?.querySelectorAll<HTMLElement>('[role="option"]')[from + 1] ?? search.current)?.focus();
  }, [read]);
  const more = () => {
    if (shown === undefined) return;
    adding.current = shown.items.length;
    showMore();
  };
  return (
    <RecordDialog label="Filter by host" onClose={onClose} error={read?.error !== undefined ? messageOf(read.error) : null}>
      <input ref={search} type="search" aria-label="Filter hosts" placeholder="Type to find a host" value={text} onChange={(e) => setText(e.target.value)} />
      <div ref={list} className="record-list" role="listbox" aria-label="Hosts">
        <button type="button" role="option" aria-selected={false} className="record-option" onClick={() => onPick("")}>
          <span className="record-title">All hosts</span>
        </button>
        {names.map((h) => (
          <button key={h.id} type="button" role="option" aria-selected={false} className="record-option" onClick={() => onPick(h.name)}>
            <span className="record-title">{displaySafe(h.name)}</span>
          </button>
        ))}
      </div>
      {read === null && <p className="form-hint">Loading…</p>}
      {shown?.items.length === 0 && <p className="form-hint">{text === "" ? "The case has no host records yet." : "None match."}</p>}
      {shown !== undefined && shown.items.length > 0 && (
        <p className="form-hint">
          <span>{`${fmt(shown.items.length)} listed${!shown.truncated ? "" : text === "" ? " · more in the case" : " · more match"}`}</span>
          {shown.truncated && (
            <>
              {" "}
              <button type="button" className="link-button" onClick={more}>
                Show more
              </button>
            </>
          )}
        </p>
      )}
      <div className="form-buttons">
        <button type="button" onClick={onClose}>Cancel</button>
      </div>
    </RecordDialog>
  );
}
