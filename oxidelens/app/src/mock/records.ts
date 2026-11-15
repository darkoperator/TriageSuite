// DEV AND TEST ONLY: the mock backend's case record (timeline entries, their evidence, the host, account and IOC
// records, links, tags, notes and the history), for `backend.ts` to answer the record commands from. Seeded with an
// analyst's reviewed entry, an agent's unreviewed one, an analyst's undated draft and an agent's undated entry, records
// of every kind, and the engine fixtures' hostile record values (`HOSTILE_RECORD`) in titles, descriptions, notes and
// IOC values. All names are synthetic.
//
// Like the rest of the mock it holds no rules beyond what a screen needs to see answers of the right shape: a missing
// entry or record is `not_found`, an undated entry is not reviewed, an entry needs a title. Validation, ownership (S3)
// and canonical forms are the engine's; do not test them here.
import type { AccountRecord } from "../contract/generated/AccountRecord";
import type { CaseRecord } from "../contract/generated/CaseRecord";
import type { Candidate } from "../contract/generated/Candidate";
import type { Entry } from "../contract/generated/Entry";
import type { EntryChanges } from "../contract/generated/EntryChanges";
import type { EntryLink } from "../contract/generated/EntryLink";
import type { EntryQuery } from "../contract/generated/EntryQuery";
import type { EntryStatus } from "../contract/generated/EntryStatus";
import type { Evidence } from "../contract/generated/Evidence";
import type { HistoryRow } from "../contract/generated/HistoryRow";
import type { HostRecord } from "../contract/generated/HostRecord";
import type { IocRecord } from "../contract/generated/IocRecord";
import type { Listing } from "../contract/generated/Listing";
import type { LogonCount } from "../contract/generated/LogonCount";
import type { LogonSummary } from "../contract/generated/LogonSummary";
import type { MarkState } from "../contract/generated/MarkState";
import type { Note } from "../contract/generated/Note";
import type { PromoteOutcome } from "../contract/generated/PromoteOutcome";
import type { RecordKind } from "../contract/generated/RecordKind";
import type { RecordQuery } from "../contract/generated/RecordQuery";
import type { RecordSpec } from "../contract/generated/RecordSpec";
import type { RecordStatus } from "../contract/generated/RecordStatus";
import type { RowOutcome } from "../contract/generated/RowOutcome";
import type { Sightings } from "../contract/generated/Sightings";
import type { TagInfo } from "../contract/generated/TagInfo";
import type { TextChange } from "../contract/generated/TextChange";
import { HOSTILE, HOSTILE_RECORD } from "./generated";

const fail = (code: "not_found" | "invalid" | "refused", message: string): never => {
  throw { code, message };
};

const NOW = "2026-01-10T12:00:00.000000Z";
/** The agent the mock's agent items are recorded under. */
export const MOCK_AGENT = "agent:triage-bot";

/** The engine's `TACTICS` (`oxl-engine/src/record/mod.rs`), in its order, which the `tactics` command answers. */
export const TACTICS = [
  "Reconnaissance", "Resource Development", "Initial Access", "Execution", "Persistence", "Privilege Escalation",
  "Defense Evasion", "Credential Access", "Discovery", "Lateral Movement", "Collection", "Command and Control",
  "Exfiltration", "Impact",
] as const;

/** What the record reads about one row of the mock's data. */
export interface RowFacts {
  relation: string;
  host: string | null;
  /** The primary time as the engine writes an entry's time (six fractional digits), or null. */
  time: string | null;
  summary: string;
  /** The cells by column name, for candidates, sightings and logons. */
  cells: ReadonlyArray<readonly [string, string | null]>;
}

/** Every row of the mock's data, as `RowFacts`, by id; null for a row the mock does not have. */
export interface Rows {
  facts(rowId: number): RowFacts | null;
  all(): Iterable<[number, RowFacts]>;
}

interface Link {
  entry: number;
  kind: RecordKind;
  id: number;
}

/** Entries are kept without their derived `hosts`, `evidence_rows`, `time_shown`, `links` and `mark`, which `shown` fills in. */
type StoredEntry = Omit<Entry, "hosts" | "evidence_rows" | "time_shown" | "links" | "mark">;
type StoredRecord = Omit<HostRecord, "entries" | "first_seen" | "last_seen"> &
  Partial<Omit<AccountRecord, keyof HostRecord>> &
  Partial<Omit<IocRecord, keyof HostRecord>>;

export interface MockRecords {
  entries: Map<number, StoredEntry>;
  evidence: Map<number, Evidence[]>;
  hosts: HostRecord[];
  accounts: AccountRecord[];
  iocs: IocRecord[];
  links: Link[];
  tags: Map<number, Map<string, string>>;
  notes: Note[];
  history: HistoryRow[];
  /** Rows marked Suspicious by S, before evidence derives Malicious over them (S2). */
  suspicious: Set<number>;
  next: { entry: number; host: number; account: number; ioc: number; note: number; history: number };
}

const page = <T>(all: T[], limit: number, offset: number): Listing<T> => ({
  items: all.slice(offset, offset + limit),
  truncated: offset + limit < all.length,
});

const recordBase = (id: number, by: string, status: RecordStatus) => ({
  id, status, created_by: by, created_at: NOW, modified_by: by, modified_at: NOW,
  agent_added: by.startsWith("agent:"), deleted: false, entries: 0, first_seen: null, last_seen: null,
});

function entryBase(id: number, title: string, by: string, time: string | null, source: string | null): StoredEntry {
  return {
    id, time, time_source: time === null ? null : source, needs_time: time === null, title, description: "",
    tactic: null, technique_ids: [], status: by.startsWith("agent:") ? "unreviewed" : "draft", created_by: by,
    created_at: NOW, modified_by: by, modified_at: NOW, reviewed_by: null, agent_added: by.startsWith("agent:"),
  };
}

/** The seeded record. `analyst` is who the analyst's items are by. */
export function seedRecords(rows: Rows, analyst: string, suspicious: Iterable<number>): MockRecords {
  const r: MockRecords = {
    entries: new Map(), evidence: new Map(), hosts: [], accounts: [], iocs: [], links: [], tags: new Map(),
    notes: [], history: [], suspicious: new Set(suspicious),
    next: { entry: 1, host: 1, account: 1, ioc: 1, note: 1, history: 1 },
  };
  const host = (name: string, by: string, status: RecordStatus, note = "") => {
    r.hosts.push({ ...recordBase(r.next.host++, by, status), name, role: "", note });
  };
  host("WS01", analyst, "reviewed");
  host("SRV01", MOCK_AGENT, "unreviewed", HOSTILE_RECORD[2]);
  r.accounts.push(
    {
      ...recordBase(r.next.account++, analyst, "reviewed"), name: "jdoe", domain: "ACME", sid: null,
      description: HOSTILE_RECORD[0], privilege: "standard", note: "", hosts_seen: [],
    },
    {
      ...recordBase(r.next.account++, MOCK_AGENT, "unreviewed"), name: "svc_backup", domain: null, sid: null,
      description: "", privilege: "service", note: HOSTILE_RECORD[4], hosts_seen: [],
    },
  );
  const ioc = (ioc_type: IocRecord["ioc_type"], value: string, by: string, status: RecordStatus, note = "") => {
    r.iocs.push({
      ...recordBase(r.next.ioc++, by, status), ioc_type, value, tool: null, attack_ids: [], note, hosts_seen: [],
    });
  };
  ioc("ip", "203.0.113.7", analyst, "reviewed");
  ioc("other", HOSTILE_RECORD[1], MOCK_AGENT, "unreviewed");
  ioc("file_name", HOSTILE_RECORD[2], analyst, "reviewed", HOSTILE_RECORD[4]);

  // E-1: an analyst's, reviewed, from row 7; E-2: an agent's, from row 12; E-3 and E-4: undated.
  const reviewed = attachNew(r, rows, analyst, 7, "Failed logon burst from 203.0.113.7");
  const e1 = r.entries.get(reviewed)!;
  Object.assign(e1, { status: "reviewed", reviewed_by: analyst, tactic: "Credential Access", technique_ids: ["T1110"] });
  link(r, reviewed, "account", 1, analyst);
  link(r, reviewed, "ioc", 1, analyst);
  const agent = attachNew(r, rows, MOCK_AGENT, 12, HOSTILE_RECORD[0]);
  r.entries.get(agent)!.description = HOSTILE_RECORD[4];
  link(r, agent, "ioc", 2, MOCK_AGENT);
  const draft = entryBase(r.next.entry++, HOSTILE_RECORD[1], analyst, null, null);
  draft.description = HOSTILE_RECORD[3];
  r.entries.set(draft.id, draft);
  const wiped = entryBase(r.next.entry++, "Possible log wipe", MOCK_AGENT, null, null);
  wiped.description = HOSTILE.bidi;
  r.entries.set(wiped.id, wiped);
  for (const id of [reviewed, agent, draft.id, wiped.id]) addHistory(r, analyst, "entry", String(id), "entry_create");

  r.tags.set(5, new Map([["seen", analyst]]));
  r.notes.push({ id: r.next.note++, row_id: 5, text: HOSTILE.html, written_by: analyst, written_at: NOW });
  return r;
}

function addHistory(r: MockRecords, by: string, target_kind: string, target_id: string, action: string): void {
  r.history.push({ id: r.next.history++, at: NOW, by, target_kind, target_id, action, before: null, after: null });
}

/** The host record named `name` (ASCII case ignored), made when the case has none (S7). */
function ensureHost(r: MockRecords, name: string, by: string): number {
  const found = r.hosts.find((h) => !h.deleted && h.name.toLowerCase() === name.toLowerCase());
  if (found) return found.id;
  const id = r.next.host++;
  r.hosts.push({ ...recordBase(id, by, by.startsWith("agent:") ? "unreviewed" : "reviewed"), name, role: "", note: "" });
  return id;
}

function link(r: MockRecords, entry: number, kind: RecordKind, id: number, by: string): void {
  if (!r.links.some((l) => l.entry === entry && l.kind === kind && l.id === id)) {
    r.links.push({ entry, kind, id });
    addHistory(r, by, "link", `entry_${kind}:${entry}:${id}`, "link_add");
  }
}

/** Attach row `rowId` to `entry`, linking its host; false for a row the mock does not have. */
function attach(r: MockRecords, rows: Rows, by: string, entry: number, rowId: number): boolean {
  const facts = rows.facts(rowId);
  if (!facts) return false;
  const held = r.evidence.get(entry) ?? [];
  if (!held.some((e) => e.row_id === rowId)) {
    held.push({
      row_id: rowId, relation: facts.relation, host: facts.host, standing: "unchanged", summary: facts.summary,
      attached_by: by, attached_at: NOW,
    });
    r.evidence.set(entry, held);
    if (facts.host) link(r, entry, "host", ensureHost(r, facts.host, by), by);
  }
  return true;
}

/** A new entry with row `rowId` as its evidence, timed by the row; its id. */
function attachNew(r: MockRecords, rows: Rows, by: string, rowId: number, title?: string): number {
  const facts = rows.facts(rowId)!;
  const entry = entryBase(r.next.entry++, title ?? (facts.summary.slice(0, 120) || `Row ${rowId}`), by, facts.time, "row");
  r.entries.set(entry.id, entry);
  attach(r, rows, by, entry.id, rowId);
  return entry.id;
}

// --------------------------------------------------------------------------------------------------------- reads

const liveRecords = (r: MockRecords, kind: RecordKind): StoredRecord[] =>
  (kind === "host" ? r.hosts : kind === "account" ? r.accounts : r.iocs).filter((x) => !x.deleted) as StoredRecord[];

function stored(r: MockRecords, kind: RecordKind, id: number): StoredRecord {
  const all = (kind === "host" ? r.hosts : kind === "account" ? r.accounts : r.iocs) as StoredRecord[];
  return all.find((x) => x.id === id) ?? fail("not_found", `no ${kind} ${id}`);
}

export function shownEntry(r: MockRecords, e: StoredEntry): Entry {
  const hosts = r.links
    .filter((l) => l.entry === e.id && l.kind === "host")
    .map((l) => r.hosts.find((h) => h.id === l.id && !h.deleted)?.name)
    .filter((n): n is string => n !== undefined)
    .sort();
  // Hosts, then accounts, then IOCs, each in link order, as the engine lists them.
  const links: EntryLink[] = (["host", "account", "ioc"] as const).flatMap((kind) =>
    r.links
      .filter((l) => l.entry === e.id && l.kind === kind)
      .flatMap((l): EntryLink[] => {
        const x = liveRecords(r, kind).find((y) => y.id === l.id);
        if (!x) return [];
        const label = kind === "host" ? x.name : kind === "account" ? (x.domain ? `${x.domain}\\${x.name}` : x.name) : `${x.ioc_type} ${x.value}`;
        return [{ kind, id: x.id, label }];
      }),
  );
  // The mock shows every time in UTC (see backend.ts): the engine's display text of a UTC time, `T` and `Z` dropped.
  const time_shown = e.time === null ? null : e.time.replace("T", " ").replace(/Z$/, "");
  const evidence_rows = (r.evidence.get(e.id) ?? []).length;
  // The mark the entry gives its rows (S2): an agent's unreviewed entry Agent: malicious, any other Malicious.
  const mark: MarkState | null = evidence_rows === 0 ? null : e.agent_added && e.status !== "reviewed" ? "agent_malicious" : "malicious";
  return { ...e, hosts, evidence_rows, time_shown, links, mark };
}

function shownRecord(r: MockRecords, kind: RecordKind, x: StoredRecord): CaseRecord {
  const linked = r.links.filter((l) => l.kind === kind && l.id === x.id).map((l) => r.entries.get(l.entry)).filter((e) => e !== undefined);
  const times = linked.map((e) => e.time).filter((t): t is string => t !== null).sort();
  const hostsSeen = [...new Set(linked.flatMap((e) => shownEntry(r, e).hosts))].sort();
  const counted = { entries: linked.length, first_seen: times[0] ?? null, last_seen: times[times.length - 1] ?? null };
  if (kind === "host") return { kind, ...(x as HostRecord), ...counted };
  if (kind === "account") return { kind, ...(x as unknown as AccountRecord), ...counted, hosts_seen: hostsSeen };
  return { kind, ...(x as unknown as IocRecord), ...counted, hosts_seen: hostsSeen };
}

/** The Timeline's order: by time, the undated last, then by id. */
function timelineOrder(x: StoredEntry, y: StoredEntry): number {
  if (x.time !== y.time) return x.time === null ? 1 : y.time === null ? -1 : x.time < y.time ? -1 : 1;
  return x.id - y.id;
}

/** The attach picker's order: the most recently changed first, then the newest id. */
function recentOrder(x: StoredEntry, y: StoredEntry): number {
  if (x.modified_at !== y.modified_at) return x.modified_at < y.modified_at ? 1 : -1;
  return y.id - x.id;
}

export function entriesList(r: MockRecords, q: EntryQuery, limit: number, offset: number): Listing<Entry> {
  const text = q.text?.toLowerCase();
  const all = [...r.entries.values()]
    .map((e) => shownEntry(r, e))
    .filter((e) => q.status.length === 0 || q.status.includes(e.status))
    .filter((e) => !q.host || e.hosts.some((h) => h.toLowerCase() === q.host!.toLowerCase()))
    .filter((e) => !q.tactic || e.tactic === q.tactic)
    .filter((e) => !text || e.title.toLowerCase().includes(text) || e.description.toLowerCase().includes(text))
    .filter((e) => q.row === null || q.row === undefined || (r.evidence.get(e.id) ?? []).some((ev) => ev.row_id === q.row))
    .sort(q.order === "recent" ? recentOrder : timelineOrder);
  return page(all, limit, offset);
}

export function entryGet(r: MockRecords, id: number): Entry {
  const e = r.entries.get(id) ?? fail("not_found", `no entry E-${id}`);
  return shownEntry(r, e);
}

export function evidenceList(r: MockRecords, entry: number): Evidence[] {
  entryGet(r, entry);
  return r.evidence.get(entry) ?? [];
}

export function recordsList(r: MockRecords, q: RecordQuery, limit: number, offset: number): Listing<CaseRecord> {
  let all = liveRecords(r, q.kind);
  if (q.kind === "ioc") {
    const text = q.text?.toLowerCase();
    all = all
      .filter((x) => !q.ioc_type || x.ioc_type === q.ioc_type)
      .filter((x) => !q.tool || x.tool?.toLowerCase() === q.tool.toLowerCase())
      .filter((x) => !text || [x.value, x.tool, x.note].some((v) => v?.toLowerCase().includes(text)));
  }
  const key = (x: StoredRecord) => (x.name ?? x.value ?? "").toLowerCase();
  all = [...all].sort((x, y) => (key(x) < key(y) ? -1 : key(x) > key(y) ? 1 : x.id - y.id));
  return page(all.map((x) => shownRecord(r, q.kind, x)), limit, offset);
}

export const recordGet = (r: MockRecords, kind: RecordKind, id: number): CaseRecord => shownRecord(r, kind, stored(r, kind, id));

export function recordEntries(r: MockRecords, kind: RecordKind, id: number, limit: number, offset: number): Listing<Entry> {
  stored(r, kind, id);
  const linked = r.links.filter((l) => l.kind === kind && l.id === id).map((l) => r.entries.get(l.entry)).filter((e) => e !== undefined);
  return page(linked.sort(timelineOrder).map((e) => shownEntry(r, e)), limit, offset);
}

export function tagsList(r: MockRecords, limit: number, offset: number): Listing<TagInfo> {
  const byName = new Map<string, TagInfo>();
  for (const tags of r.tags.values()) {
    for (const [name, by] of tags) {
      const t = byName.get(name) ?? { name, rows: 0, created_by: by };
      t.rows += 1;
      byName.set(name, t);
    }
  }
  return page([...byName.values()].sort((x, y) => (x.name < y.name ? -1 : 1)), limit, offset);
}

/** A row's tags, sorted, as the row detail lists them. */
export const rowTags = (r: MockRecords, rowId: number): string[] => [...(r.tags.get(rowId)?.keys() ?? [])].sort();

/** The I key's guess, roughly as the engine's `guess_ioc_type` makes it (hashes, an IPv4 address, a URL, an email, a
 *  domain); the engine's own rule is tested in Rust. */
export function guessIocType(value: string): IocRecord["ioc_type"] | null {
  if (/^[0-9a-f]{64}$/i.test(value)) return "sha256";
  if (/^[0-9a-f]{40}$/i.test(value)) return "sha1";
  if (/^[0-9a-f]{32}$/i.test(value)) return "md5";
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(value)) return "ip";
  if (/^(https?|ftp):\/\/[^/\s]+/i.test(value)) return "url";
  if (/^[^@\s]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$/.test(value)) return "email";
  if (/^[A-Za-z0-9-]{1,63}(\.[A-Za-z0-9-]{1,63})+$/.test(value)) return "domain";
  return null;
}

export const notesList = (r: MockRecords, rowId: number, limit: number, offset: number): Listing<Note> =>
  page(r.notes.filter((n) => n.row_id === rowId), limit, offset);

export function historyList(r: MockRecords, kind: string | null, id: string | null, limit: number, offset: number): Listing<HistoryRow> {
  const all = r.history.filter((h) => (!kind || h.target_kind === kind) && (!id || h.target_id === id)).reverse();
  return page(all, limit, offset);
}

/** An account named in the `UserName` column, and an address in `SourceIp`, of row `rowId`. */
export function candidates(r: MockRecords, rows: Rows, rowId: number): Candidate[] {
  const facts = rows.facts(rowId) ?? fail("not_found", `no row ${rowId}`);
  const cell = (name: string) => facts.cells.find(([n]) => n === name)?.[1] ?? null;
  const out: Candidate[] = [];
  const user = cell("UserName");
  if (user) {
    const existing = r.accounts.find((a) => !a.deleted && a.name.toLowerCase() === user.toLowerCase());
    out.push({ kind: "account", ioc_type: null, value: user, domain: null, sid: null, column: "UserName", existing_id: existing?.id ?? null });
  }
  const ip = cell("SourceIp");
  if (ip) {
    const existing = r.iocs.find((i) => !i.deleted && i.ioc_type === "ip" && i.value === ip);
    out.push({ kind: "ioc", ioc_type: "ip", value: ip, domain: null, sid: null, column: "SourceIp", existing_id: existing?.id ?? null });
  }
  return out;
}

/** Cells whose text is the IOC's value, case ignored, across every row (S5's exact find). */
export function sightings(r: MockRecords, rows: Rows, id: number): Sightings {
  const ioc = r.iocs.find((i) => i.id === id && !i.deleted) ?? fail("not_found", `no ioc I-${id}`);
  const value = ioc.value.toLowerCase();
  const relations = new Map<string, number>();
  const hosts = new Set<string>();
  const times: string[] = [];
  let hits = 0;
  for (const [, facts] of rows.all()) {
    const n = facts.cells.filter(([, v]) => v !== null && v.toLowerCase() === value).length;
    if (n === 0) continue;
    hits += n;
    relations.set(facts.relation, (relations.get(facts.relation) ?? 0) + n);
    if (facts.host) hosts.add(facts.host);
    if (facts.time) times.push(facts.time);
  }
  times.sort();
  return { hits, relations: [...relations].sort(), hosts: [...hosts].sort(), first: times[0] ?? null, last: times[times.length - 1] ?? null };
}

/** Account `id`'s 4624 and 4625 rows by host and `LogonType`, from the mock's `UserName`, `EventId` and `LogonType`. */
export function logonSummary(r: MockRecords, rows: Rows, id: number): LogonSummary {
  const account = r.accounts.find((a) => a.id === id && !a.deleted) ?? fail("not_found", `no account A-${id}`);
  const counts = new Map<string, LogonCount>();
  let unparsed = 0;
  for (const [, facts] of rows.all()) {
    if (facts.relation !== "mock_events") continue;
    const cell = (name: string) => facts.cells.find(([n]) => n === name)?.[1] ?? null;
    const event = cell("EventId");
    if (event !== "4624" && event !== "4625") continue;
    const type = Number(cell("LogonType"));
    if (!Number.isInteger(type)) {
      unparsed += 1;
      continue;
    }
    if (cell("UserName")?.toLowerCase() !== account.name.toLowerCase()) continue;
    const outcome = event === "4624" ? "success" : "failure";
    const key = `${facts.host}|${type}|${outcome}`;
    const c = counts.get(key) ?? {
      outcome, logon_type: type, host: facts.host, count: 0,
      filter: {
        and: [
          { column: "UserName", op: "eq", value: account.name, case_sensitive: false },
          { column: "EventId", op: "eq", value: event, case_sensitive: true },
          { column: "LogonType", op: "eq", value: String(type), case_sensitive: true },
          { column: "_triage_host", op: "eq", value: facts.host, case_sensitive: true },
        ],
      },
    };
    c.count += 1;
    counts.set(key, c);
  }
  const sorted = [...counts.values()].sort(
    (x, y) => String(x.host).localeCompare(String(y.host)) || x.logon_type - y.logon_type || (x.outcome === "success" ? -1 : 1),
  );
  return { rows: sorted, unparsed, source: "mock_events" };
}

// -------------------------------------------------------------------------------------------------------- writes

/** The mark a row's evidence gives it (S2): Malicious on a live analyst's or reviewed entry, else Agent: malicious on
 *  an agent's unreviewed one, else Suspicious when S marked it; null for none. */
export function derivedMark(r: MockRecords, rowId: number): MarkState | null {
  let agent = false;
  for (const [entry, held] of r.evidence) {
    if (!held.some((e) => e.row_id === rowId)) continue;
    const e = r.entries.get(entry);
    if (!e) continue;
    if (!e.agent_added || e.status === "reviewed") return "malicious";
    agent = true;
  }
  if (agent) return "agent_malicious";
  return r.suspicious.has(rowId) ? "suspicious" : null;
}

/** The rows that are evidence on `entry`. */
export const evidenceRows = (r: MockRecords, entry: number): number[] => (r.evidence.get(entry) ?? []).map((e) => e.row_id);

/** The live analyst's (or reviewed) entry row `rowId` is evidence on, which S and U refuse it for (S2). */
function analystHolder(r: MockRecords, rowId: number): number | null {
  for (const [entry, held] of r.evidence) {
    const e = r.entries.get(entry);
    if (e && (!e.agent_added || e.status === "reviewed") && held.some((x) => x.row_id === rowId)) return entry;
  }
  return null;
}

export function markSet(r: MockRecords, rows: Rows, ids: number[], set: boolean): RowOutcome[] {
  return ids.map((row_id) => {
    if (!rows.facts(row_id)) return { row_id, done: false, reason: `no row ${row_id}` };
    const holder = analystHolder(r, row_id);
    if (holder !== null) return { row_id, done: false, reason: `evidence on entry E-${holder}; detach it there` };
    if (set) r.suspicious.add(row_id);
    else r.suspicious.delete(row_id);
    return { row_id, done: true, reason: null };
  });
}

export function tagRows(r: MockRecords, rows: Rows, ids: number[], tag: string, by: string, add: boolean): RowOutcome[] {
  return ids.map((row_id) => {
    if (!rows.facts(row_id)) return { row_id, done: false, reason: `no row ${row_id}` };
    const tags = r.tags.get(row_id) ?? new Map<string, string>();
    if (add) tags.set(tag.toLowerCase(), by);
    else tags.delete(tag.toLowerCase());
    r.tags.set(row_id, tags);
    return { row_id, done: true, reason: null };
  });
}

export function noteAdd(r: MockRecords, rows: Rows, rowId: number, text: string, by: string): Note {
  if (!rows.facts(rowId)) fail("not_found", `no row ${rowId}`);
  const note = { id: r.next.note++, row_id: rowId, text, written_by: by, written_at: NOW };
  r.notes.push(note);
  addHistory(r, by, "note", String(note.id), "note_add");
  return note;
}

export function noteDelete(r: MockRecords, id: number, by: string): void {
  if (!r.notes.some((n) => n.id === id)) fail("not_found", `no note ${id}`);
  r.notes = r.notes.filter((n) => n.id !== id);
  addHistory(r, by, "note", String(id), "note_delete");
}

const changeOf = (c: TextChange | null, old: string | null): string | null =>
  c === null ? old : c === "clear" ? null : c.set;

function applyChanges(e: StoredEntry, c: EntryChanges, by: string): void {
  if (c.title !== null) e.title = c.title;
  if (c.description !== null) e.description = c.description;
  if (c.time !== null) {
    e.time = changeOf(c.time, e.time);
    e.time_source = e.time === null ? null : "typed";
    e.needs_time = e.time === null;
  }
  e.tactic = changeOf(c.tactic, e.tactic);
  if (c.technique_ids !== null) e.technique_ids = [...new Set(c.technique_ids)];
  e.modified_by = by;
  e.modified_at = NOW;
}

export function entryNew(r: MockRecords, c: EntryChanges, by: string): Entry {
  if (c.title === null || c.title.trim() === "") return fail("invalid", "an entry needs a title");
  const e = entryBase(r.next.entry++, c.title, by, null, null);
  applyChanges(e, c, by);
  r.entries.set(e.id, e);
  addHistory(r, by, "entry", String(e.id), "entry_create");
  return shownEntry(r, e);
}

export function entryEdit(r: MockRecords, id: number, c: EntryChanges, by: string): Entry {
  const e = r.entries.get(id) ?? fail("not_found", `no entry E-${id}`);
  applyChanges(e, c, by);
  addHistory(r, by, "entry", String(id), "entry_edit");
  return shownEntry(r, e);
}

export function entryStatus(r: MockRecords, id: number, status: EntryStatus, by: string): Entry {
  const e = r.entries.get(id) ?? fail("not_found", `no entry E-${id}`);
  if (status === "reviewed" && e.time === null) fail("refused", `E-${id} needs a time before it can be reviewed`);
  e.status = status;
  e.reviewed_by = status === "reviewed" ? by : null;
  addHistory(r, by, "entry", String(id), "entry_status");
  return shownEntry(r, e);
}

export function entryDelete(r: MockRecords, id: number, by: string): void {
  if (!r.entries.delete(id)) fail("not_found", `no entry E-${id}`);
  r.links = r.links.filter((l) => l.entry !== id);
  addHistory(r, by, "entry", String(id), "entry_delete");
}

export function promote(r: MockRecords, rows: Rows, ids: number[], by: string): PromoteOutcome[] {
  return ids.map((row_id) => {
    if (!rows.facts(row_id)) return { row_id, entry_id: null, created: false, reason: `no row ${row_id}` };
    for (const [entry, held] of r.evidence) {
      if (r.entries.has(entry) && held.some((e) => e.row_id === row_id)) return { row_id, entry_id: entry, created: false, reason: null };
    }
    const entry = attachNew(r, rows, by, row_id);
    addHistory(r, by, "entry", String(entry), "entry_create");
    return { row_id, entry_id: entry, created: true, reason: null };
  });
}

export function attachRows(r: MockRecords, rows: Rows, entry: number, ids: number[], by: string): RowOutcome[] {
  entryGet(r, entry);
  return ids.map((row_id) =>
    attach(r, rows, by, entry, row_id) ? { row_id, done: true, reason: null } : { row_id, done: false, reason: `no row ${row_id}` },
  );
}

export function detach(r: MockRecords, entry: number, rowId: number, by: string): void {
  entryGet(r, entry);
  const held = r.evidence.get(entry) ?? [];
  if (!held.some((e) => e.row_id === rowId)) fail("not_found", `row ${rowId} is not evidence on E-${entry}`);
  r.evidence.set(entry, held.filter((e) => e.row_id !== rowId));
  addHistory(r, by, "evidence", `${entry}:${rowId}`, "evidence_detach");
}

export function recordUpsert(r: MockRecords, spec: RecordSpec, by: string): CaseRecord {
  const { kind, ...fields } = spec;
  const all = (kind === "host" ? r.hosts : kind === "account" ? r.accounts : r.iocs) as StoredRecord[];
  if (fields.id === null) {
    const id = r.next[kind]++;
    const seen = kind === "host" ? {} : { hosts_seen: [] };
    all.push({ ...recordBase(id, by, by.startsWith("agent:") ? "unreviewed" : "reviewed"), ...seen, ...fields, id } as StoredRecord);
    addHistory(r, by, kind, String(id), `${kind}_create`);
    return recordGet(r, kind, id);
  }
  const old = stored(r, kind, fields.id);
  Object.assign(old, fields, { modified_by: by, modified_at: NOW });
  addHistory(r, by, kind, String(fields.id), `${kind}_edit`);
  return recordGet(r, kind, fields.id);
}

export function recordDelete(r: MockRecords, kind: RecordKind, id: number, by: string): void {
  const x = stored(r, kind, id);
  if (x.deleted) fail("refused", `${kind} ${id} was deleted`);
  x.deleted = true;
  r.links = r.links.filter((l) => !(l.kind === kind && l.id === id));
  addHistory(r, by, kind, String(id), `${kind}_delete`);
}

export function recordMerge(r: MockRecords, kind: RecordKind, from: number, into: number, by: string): void {
  if (from === into) fail("invalid", "a record is not merged into itself");
  stored(r, kind, into);
  const x = stored(r, kind, from);
  for (const l of r.links.filter((l) => l.kind === kind && l.id === from)) link(r, l.entry, kind, into, by);
  r.links = r.links.filter((l) => !(l.kind === kind && l.id === from));
  x.deleted = true;
  addHistory(r, by, kind, String(from), `${kind}_merge`);
}

export function recordStatus(r: MockRecords, kind: RecordKind, id: number, status: RecordStatus, by: string): void {
  stored(r, kind, id).status = status;
  addHistory(r, by, kind, String(id), `${kind}_status`);
}

export function recordLink(r: MockRecords, entry: number, kind: RecordKind, id: number, by: string, add: boolean): void {
  entryGet(r, entry);
  stored(r, kind, id);
  if (add) return link(r, entry, kind, id, by);
  if (!r.links.some((l) => l.entry === entry && l.kind === kind && l.id === id)) fail("not_found", `E-${entry} is not linked to ${kind} ${id}`);
  r.links = r.links.filter((l) => !(l.entry === entry && l.kind === kind && l.id === id));
  addHistory(r, by, "link", `entry_${kind}:${entry}:${id}`, "link_remove");
}

/** Rows an export writes: every live entry and record. */
export const exportRows = (r: MockRecords): number =>
  r.entries.size + liveRecords(r, "host").length + liveRecords(r, "account").length + liveRecords(r, "ioc").length;
