import { describe, expect, it } from "vitest";
import apiSource from "../api.ts?raw";
import { COMMANDS } from "../contract/generated/commands";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { GridPage } from "../contract/generated/GridPage";
import type { GridState } from "../contract/generated/GridState";
import type { Histogram } from "../contract/generated/Histogram";
import type { Listing } from "../contract/generated/Listing";
import type { GroupView } from "../contract/generated/GroupView";
import type { ValueCount } from "../contract/generated/ValueCount";
import type { MergedView } from "../contract/generated/MergedView";
import type { Command, CommandArgs } from "../contract/generated/commands";
import type { CaseRecord } from "../contract/generated/CaseRecord";
import type { Entry } from "../contract/generated/Entry";
import type { Evidence } from "../contract/generated/Evidence";
import type { PromoteOutcome } from "../contract/generated/PromoteOutcome";
import { MOCK_COMMANDS, mockCall, mockOn, resetMock, setMockLatency, setMockMarks } from "./backend";
import {
  HOSTILE, HOSTILE_RECORD, HAYABUSA_COLUMNS, HAYABUSA_FIRST_ID, EVENT_COLUMNS, EVENT_ROWS, eventRows, hayabusaRows,
} from "./generated";
import { hostsSeenOrder } from "./records";

const apiCommands = [...apiSource.matchAll(/\bcall(?:<[^(]*>)?\(\s*"([a-z_]+)"/g)].map((m) => m[1]);

const state = (over: Partial<GridState> = {}): GridState => ({
  relation: "mock_events",
  chips: [],
  search: { text: "", regex: false, case_sensitive: false, mode: "filter" },
  hosts: [],
  sort: [],
  columns: null,
  ...over,
});
const opts = { zone: "UTC", rules: [] };
const q = (slot: string, queryId = `q-${Math.random()}`) => ({ slot, queryId });
const rejection = async (p: Promise<unknown>) => p.then(() => null, (e: unknown) => e);

describe("mock backend", () => {
  it("answers exactly the commands api.ts calls, each a command of the contract", () => {
    expect(apiCommands.length).toBe(COMMANDS.length);
    expect([...new Set(apiCommands)].sort()).toEqual([...MOCK_COMMANDS].sort());
    for (const cmd of MOCK_COMMANDS) expect(COMMANDS as readonly string[]).toContain(cmd);
    expect(new Set(MOCK_COMMANDS).size).toBe(COMMANDS.length);
  });

  it("refuses an unknown command and every read before a case is open", async () => {
    expect(await rejection(mockCall("nope"))).toMatchObject({ code: "internal" });
    expect(await rejection(mockCall("case_summary", q("s")))).toMatchObject({ code: "no_case" });
    expect(await rejection(mockCall("grid_page", { ...q("g"), state: state(), limit: 5, offset: 0, options: opts }))).toMatchObject({
      code: "no_case",
    });
  });

  it("opens a case that lists the mock relations", async () => {
    const summary = await mockCall<CaseSummary>("case_open", { path: "/cases/acme.case.duckdb" });
    expect(summary.relations.map((r) => r.name)).toEqual(["mock_events", "mock_hayabusa"]);
    expect(summary.relations[0].rows).toBe(EVENT_ROWS);
    expect(summary.relations[0].primary_time).toBe("TimeCreated");
    expect(await mockCall("case_close")).toBeNull();
    expect(await rejection(mockCall("case_summary", q("s")))).toMatchObject({ code: "no_case" });
  });
});

describe("the marked worklist", () => {
  it("orders the marked rows of every relation by time, then pages them", async () => {
    await mockCall<CaseSummary>("case_open", { path: "/cases/acme.case.duckdb" });
    // Events 1, 2 and 5 fall at 02:14:03, 02:14:25 and 02:15:07; the first two Hayabusa rows at 02:14:00 and
    // 02:19:12, so the two relations interleave.
    const [h1, h2] = [HAYABUSA_FIRST_ID, HAYABUSA_FIRST_ID + 1];
    setMockMarks([[1, "malicious"], [2, "suspicious"], [5, "agent_malicious"], [h1, "suspicious"], [h2, "malicious"]]);
    const page = (offset: number, limit: number) =>
      mockCall<Listing<MergedView>>("merged_page", {
        ...q("m"), source: { marked: { marks: ["suspicious", "malicious", "agent_malicious"] } }, limit, offset, zone: "UTC",
      });
    const all = await page(0, 10);
    expect(all.items.map((v) => v.row.row_id)).toEqual([h1, 1, 2, 5, h2]);
    expect(all.truncated).toBe(false);
    // A page boundary inside the interleaving.
    const second = await page(2, 2);
    expect(second.items.map((v) => v.row.row_id)).toEqual([2, 5]);
    expect(second.truncated).toBe(true);
    const only = await mockCall<Listing<MergedView>>("merged_page", {
      ...q("m"), source: { marked: { marks: ["malicious"] } }, limit: 10, offset: 0, zone: "UTC",
    });
    expect(only.items.map((v) => [v.row.row_id, v.row.mark])).toEqual([[1, "malicious"], [h2, "malicious"]]);
    await mockCall("case_close");
  });
});

describe("the generated data", () => {
  it("is 5,000 rows of 40 columns with a utc and an unknown time column, the same every time", () => {
    expect(EVENT_COLUMNS.length).toBe(40);
    const rows = eventRows();
    expect(rows.length).toBe(5000);
    expect(rows.every((r) => r.length === 40)).toBe(true);
    expect(EVENT_COLUMNS.find((c) => c.name === "TimeCreated")).toMatchObject({ time_semantics: "utc", sql_type: "TIMESTAMP" });
    expect(EVENT_COLUMNS.find((c) => c.name === "LocalTime")).toMatchObject({ time_semantics: "unknown" });
    expect(eventRows()).toEqual(rows);
  });

  it("carries the hostile values", () => {
    const all = new Set(eventRows().flat());
    for (const value of Object.values(HOSTILE)) expect(all.has(value), value.slice(0, 30)).toBe(true);
    expect(HOSTILE.html).toBe("<img src=x onerror=alert(1)>");
    expect(HOSTILE.bidi).toContain("\u202E");
    expect(HOSTILE.joiner).toContain("\u200D");
    expect(HOSTILE.bell).toContain("\u0007");
    expect(HOSTILE.formula).toBe('=HYPERLINK("http://x")');
    expect(HOSTILE.long.length).toBe(5000);
  });

  it("has a Hayabusa-like relation", () => {
    expect(HAYABUSA_COLUMNS.map((c) => c.name)).toEqual(expect.arrayContaining(["Timestamp", "Level", "RuleTitle", "Details"]));
    expect(hayabusaRows().every((r) => r.length === HAYABUSA_COLUMNS.length)).toBe(true);
  });
});

describe("the grid commands", () => {
  async function open() {
    await mockCall("case_open", { path: "/cases/acme.case.duckdb" });
  }

  it("pages: display text for utc columns, raw text for the unknown one, the hostile text unchanged", async () => {
    await open();
    const page = await mockCall<GridPage>("grid_page", { ...q("grid"), state: state(), limit: 50, offset: 0, options: opts });
    expect(page.cells.length).toBe(50);
    expect(page.row_ids.length).toBe(50);
    expect(page.columns.length).toBe(40);
    expect(page.truncated).toBe(true);
    const t = page.columns.findIndex((c) => c.name === "TimeCreated");
    const l = page.columns.findIndex((c) => c.name === "LocalTime");
    expect(page.cells[0][t].display).toMatch(/^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d\.\d{7}$/);
    expect(page.cells[0][l].display).toBeNull();
    const shown = page.cells.flat().map((c) => c.text);
    expect(shown).toContain(HOSTILE.html);
  });

  it("filters by chips, search and hosts, sorts, and counts the same way", async () => {
    await open();
    const chip = {
      id: "c1", label: "EventId = 4624", enabled: true, origin: "cell" as const,
      filter: { column: "EventId", op: "eq" as const, value: "4624", case_sensitive: false },
    };
    const s = state({ chips: [chip], sort: [{ column: "EventId", descending: true }] });
    const counts = await mockCall<{ rows: number; total: number }>("grid_counts", { ...q("c"), state: s });
    expect(counts.total).toBe(EVENT_ROWS);
    expect(counts.rows).toBeGreaterThan(0);
    expect(counts.rows).toBeLessThan(EVENT_ROWS);
    const page = await mockCall<GridPage>("grid_page", { ...q("g"), state: s, limit: 10, offset: 0, options: opts });
    const idx = page.columns.findIndex((c) => c.name === "EventId");
    expect(page.cells.every((r) => r[idx].text === "4624")).toBe(true);
    const off = await mockCall<{ rows: number }>("grid_counts", { ...q("c"), state: state({ chips: [{ ...chip, enabled: false }] }) });
    expect(off.rows).toBe(EVENT_ROWS);
    const found = await mockCall<{ rows: number }>("grid_counts", {
      ...q("c"), state: state({ search: { text: "onerror=alert", regex: false, case_sensitive: false, mode: "filter" } }),
    });
    expect(found.rows).toBeGreaterThan(0);
    expect(found.rows).toBeLessThan(20);
  });

  it("groups, lists values and buckets a histogram", async () => {
    await open();
    const groups = await mockCall<Listing<GroupView>>("grid_groups", {
      ...q("grp"), state: state(), by: ["Channel"], parent: [], limit: 50, offset: 0, zone: "UTC",
    });
    expect(groups.items.reduce((n, g) => n + g.count, 0)).toBe(EVENT_ROWS);
    const values = await mockCall<Listing<ValueCount>>("grid_values", {
      ...q("v"), state: state(), column: "Channel", exceptChips: [], contains: null, order: "commonest", limit: 10, offset: 0,
    });
    expect(values.items[0].count).toBeGreaterThanOrEqual(values.items[1].count);
    const hist = await mockCall<Histogram>("grid_histogram", { ...q("h"), state: state(), column: null, maxBuckets: 60, zone: "UTC" });
    expect(hist.column).toBe("TimeCreated");
    expect(hist.buckets.reduce((n, b) => n + b.count, 0) + hist.untimed).toBe(EVENT_ROWS);
    expect(hist.buckets.length).toBeLessThanOrEqual(60);
  });

  it("reads one row in full with its provenance", async () => {
    await open();
    const view = await mockCall<{ detail: { row_id: number }; fields: unknown[]; provenance: { host: string | null } }>("row_view", {
      ...q("d"), rowId: 3, zone: "UTC",
    });
    expect(view.detail.row_id).toBe(3);
    expect(view.fields.length).toBe(40);
    const cell = await mockCall<{ text: string }>("cell_value", { ...q("d"), rowId: 1, column: "Message" });
    expect(cell.text.length).toBeGreaterThan(0);
  });
});

describe("queries", () => {
  it("a newer query on a slot supersedes the older; query_cancel interrupts by id", async () => {
    setMockLatency(5);
    await mockCall("case_open", { path: "/cases/acme.case.duckdb" });
    const call = (queryId: string) => mockCall("grid_counts", { slot: "counts", queryId, state: state() });
    const first = rejection(call("a"));
    const second = call("b");
    expect(await first).toMatchObject({ code: "superseded" });
    await expect(second).resolves.toBeTruthy();
    const third = rejection(call("c"));
    await mockCall("query_cancel", { queryIds: ["c"] });
    expect(await third).toMatchObject({ code: "interrupted" });
  });
});

describe("writes", () => {
  it("emit case-changed with a rising generation, and a job for a load, in the app's order", async () => {
    const seen: string[] = [];
    const changed: { kinds: string[]; generation: number }[] = [];
    mockOn("case-changed", (p) => {
      changed.push(p);
      seen.push(`changed ${p.kinds.join(",")}`);
    });
    mockOn("job-started", (p) => seen.push(`start ${p.kind}`));
    mockOn("job-finished", (p) => seen.push(`end ${p.kind} ${p.ok}`));
    await mockCall("case_open", { path: "/cases/acme.case.duckdb" });
    const saved = await mockCall<{ id: number; name: string }>("view_save", {
      name: "Logons", kind: "view", pattern: "mock_*",
      doc: { chips: [], search: { text: "", regex: false, case_sensitive: false, mode: "filter" }, hosts: [], group_by: [], sort: [], layout: null, colour_rules: [] },
    });
    expect(saved.name).toBe("Logons");
    await mockCall("load_run", { path: "/runs/run-1" });
    expect(seen).toEqual(["changed views", "start load_run", "end load_run true", "changed sources"]);
    const gens = changed.map((c) => c.generation);
    expect(gens).toEqual([...gens].sort((x, y) => x - y));
    expect(new Set(gens).size).toBe(gens.length);
  });

  it("answers no_case to a write before a case is open, announcing no job", async () => {
    const seen: string[] = [];
    mockOn("job-started", (p) => seen.push(p.kind));
    expect(await rejection(mockCall("load_run", { path: "/runs/run-1" }))).toMatchObject({ code: "no_case" });
    expect(await rejection(mockCall("load_file", { path: "/f.csv" }))).toMatchObject({ code: "no_case" });
    expect(await rejection(mockCall("view_delete", { id: 1 }))).toMatchObject({ code: "no_case" });
    expect(seen).toEqual([]);
  });

  it("rejects a second write that arrives while one runs with busy", async () => {
    setMockLatency(5);
    await mockCall("case_open", { path: "/cases/acme.case.duckdb" });
    const running = mockCall("load_run", { path: "/runs/run-1" });
    expect(await rejection(mockCall("load_run", { path: "/runs/run-2" }))).toMatchObject({ code: "busy" });
    await running;
  });
});

// Arguments for every command, of its contract's shape: the compiler holds the table to `CommandArgs` and to every
// command, so a new command without a sample fails `npm run build`.
const noEntryEdit = { title: null, description: null, time: null, tactic: null, technique_ids: null };
const SAMPLES: { [C in Command]: CommandArgs[C] } = {
  case_new: { path: "/cases/new.duckdb", name: "WS01 triage" },
  case_open: { path: "/cases/acme.case.duckdb" },
  case_close: {},
  case_summary: q("s"),
  load_run: { path: "/runs/run-1" },
  load_file: { path: "/f.csv" },
  columns: { ...q("c"), relation: "mock_events" },
  sessions: q("s"),
  host_zones: q("z"),
  zone_names: {},
  ioc_guess: { value: "203.0.113.7" },
  tactics: {},
  display_zone_set: { zone: "UTC" },
  grid_page: { ...q("g"), state: state(), limit: 5, offset: 0, options: opts },
  grid_counts: { ...q("g"), state: state() },
  grid_groups: { ...q("g"), state: state(), by: ["Channel"], parent: [], limit: 5, offset: 0, zone: "UTC" },
  grid_histogram: { ...q("g"), state: state(), column: null, maxBuckets: 10, zone: "UTC" },
  grid_values: { ...q("g"), state: state(), column: "Channel", exceptChips: [], contains: null, order: "commonest", limit: 5, offset: 0 },
  grid_empty_columns: { ...q("g"), state: state() },
  grid_matches: { ...q("g"), state: state(), from: null, backward: false, limit: 5 },
  grid_match_count: { ...q("g"), state: state() },
  grid_row_offset: { ...q("g"), state: state(), rowId: 3 },
  grid_row_ids: { ...q("g"), state: state(), ranges: [[0, 2]] },
  grid_row_positions: { ...q("g"), state: state(), ids: [1, 2] },
  grid_export: { ...q("e"), state: state(), path: "/out/events.csv", zoneTimes: null },
  date_range_chip: { ...q("d"), relation: "mock_events", column: "TimeCreated", from: "2026-01-05", to: null, zone: "UTC" },
  default_colour_rules: { ...q("r"), relation: "mock_hayabusa" },
  row_view: { ...q("r"), rowId: 3, zone: "UTC" },
  cell_value: { ...q("r"), rowId: 3, column: "Message" },
  around_totals: { ...q("r"), rowId: 3, windows: [60] },
  merged_page: { ...q("m"), source: { marked: { marks: ["suspicious"] } }, limit: 5, offset: 0, zone: "UTC" },
  views_list: { ...q("v"), relation: null, limit: 5, offset: 0 },
  views_matching: { ...q("v"), limit: 5, offset: 0 },
  view_save: {
    name: "Logons", kind: "view", pattern: "mock_*",
    doc: { chips: [], search: state().search, hosts: [], group_by: [], sort: [], layout: null, colour_rules: [] },
  },
  view_delete: { id: 1 },
  view_apply: { ...q("v"), id: 1, relation: "mock_events" },
  view_export: { ...q("v"), id: 1, path: "/out/view.oxlview.json" },
  view_import: { path: "/in/view.oxlview.json" },
  mark_set: { rows: [3, 4] },
  mark_clear: { rows: [5] },
  tag_add: { rows: [3], tag: "triage" },
  tag_remove: { rows: [5], tag: "seen" },
  tags_list: { ...q("t"), limit: 10, offset: 0 },
  note_add: { rowId: 3, text: HOSTILE.html },
  note_delete: { id: 1 },
  notes_list: { ...q("n"), rowId: 5, limit: 10, offset: 0 },
  entries_list: { ...q("tl"), query: { status: [], host: null, tactic: null, text: null, row: null, order: null }, limit: 50, offset: 0 },
  entry_get: { ...q("e"), id: 2 },
  entry_new: { edit: { ...noEntryEdit, title: "Lateral movement" } },
  entry_edit: { id: 3, edit: { ...noEntryEdit, time: { set: "2026-01-05T03:00:00Z" } } },
  entry_status: { id: 1, status: "draft" },
  entry_delete: { id: 3 },
  rows_promote: { rows: [3] },
  rows_attach: { entryId: 3, rows: [4] },
  evidence_detach: { entryId: 1, rowId: 7 },
  evidence_list: { ...q("ev"), entryId: 1, limit: 10, offset: 0 },
  records_list: { ...q("rl"), query: { kind: "host" }, limit: 10, offset: 0 },
  record_get: { ...q("rg"), kind: "ioc", id: 1 },
  record_upsert: { spec: { kind: "host", id: null, name: "SRV02", role: "", note: "" } },
  record_delete: { kind: "ioc", id: 2 },
  record_merge: { kind: "host", fromId: 2, intoId: 1 },
  record_status: { kind: "account", id: 2, status: "reviewed" },
  record_link: { entryId: 3, kind: "ioc", recordId: 1 },
  record_unlink: { entryId: 1, kind: "account", recordId: 1 },
  record_entries: { ...q("re"), kind: "host", id: 1, limit: 10, offset: 0 },
  candidates: { ...q("cand"), rowId: 3 },
  ioc_sightings: { ...q("sightings"), id: 1 },
  logon_summary: { ...q("logons"), id: 1 },
  history_list: { ...q("h"), targetKind: "entry", targetId: "1", limit: 10, offset: 0 },
  export_timeline: { ...q("x"), path: "/out/timeline.csv" },
  export_workbook: { ...q("x"), path: "/out/record.xlsx" },
  export_report: { ...q("x"), path: "/out/record.md" },
  settings_get: {},
  settings_update: {
    settings: {
      version: 1, theme: "dark", font_size: 12, left_pane: "open", right_pane: "open", histogram: true,
      analyst: "analyst01", recent: [], layouts: {}, colour_rules: {},
    },
  },
  recent_clear: {},
  recent_remove: { path: "/cases/acme.case.duckdb" },
  query_cancel: { queryIds: ["nothing"] },
};

describe("the case record", () => {
  async function open() {
    await mockCall("case_open", { path: "/cases/acme.case.duckdb" });
  }

  it("mock_answers_every_api_call", async () => {
    expect(Object.keys(SAMPLES).sort()).toEqual([...COMMANDS].sort());
    for (const cmd of COMMANDS) {
      // Each command on a freshly opened case, so no answer depends on another's write.
      resetMock();
      setMockLatency(0);
      await open();
      const answer = await mockCall(cmd, SAMPLES[cmd] as Record<string, unknown>).then(
        (value) => ({ ok: true as const, value }),
        (error: unknown) => ({ ok: false as const, error }),
      );
      expect(answer, cmd).toMatchObject({ ok: true });
    }
  });

  it("holds an agent entry, an undated entry, records of every kind and the hostile record values", async () => {
    await open();
    const entries = await mockCall<Listing<Entry>>("entries_list", SAMPLES.entries_list);
    const agent = entries.items.find((e) => e.agent_added);
    expect(agent).toMatchObject({ status: "unreviewed", created_by: expect.stringMatching(/^agent:/) });
    const undated = entries.items.filter((e) => e.needs_time);
    expect(undated.length).toBeGreaterThan(0);
    // The engine's order: dated by time, the undated last.
    expect(entries.items.slice(-undated.length).every((e) => e.needs_time && e.time === null)).toBe(true);
    const texts: string[] = entries.items.flatMap((e) => [e.title, e.description]);
    for (const kind of ["host", "account", "ioc"] as const) {
      const query = kind === "ioc" ? { kind, ioc_type: null, tool: null, text: null } : { kind };
      const listed = await mockCall<Listing<CaseRecord>>("records_list", { ...q("rl"), query, limit: 50, offset: 0 });
      expect(listed.items.length, kind).toBeGreaterThan(0);
      expect(listed.items.every((r) => r.kind === kind)).toBe(true);
      for (const r of listed.items) texts.push(...Object.values(r).filter((v): v is string => typeof v === "string"));
    }
    for (const value of HOSTILE_RECORD) expect(texts, value.slice(0, 30)).toContain(value);
    const evidence = await mockCall<Listing<Evidence>>("evidence_list", SAMPLES.evidence_list);
    expect(evidence.items.length).toBeGreaterThan(0);
    // Paged as the engine pages it: a page of one past the first row holds the second, or nothing.
    const all = evidence.items.map((e) => e.row_id);
    const second = await mockCall<Listing<Evidence>>("evidence_list", { ...SAMPLES.evidence_list, limit: 1, offset: 1 });
    expect(second.items.map((e) => e.row_id)).toEqual(all.slice(1, 2));
    expect(second.truncated).toBe(all.length > 2);
    const first = await mockCall<Listing<Evidence>>("evidence_list", { ...SAMPLES.evidence_list, limit: 1, offset: 0 });
    expect(first.truncated).toBe(all.length > 1);
  });

  it("M makes an entry and marks the row, emitting case-changed; refusals are the engine's codes", async () => {
    const changed: string[][] = [];
    mockOn("case-changed", (p) => changed.push(p.kinds));
    await open();
    const [outcome] = await mockCall<PromoteOutcome[]>("rows_promote", { rows: [3] });
    expect(outcome).toMatchObject({ row_id: 3, created: true });
    const entry = await mockCall<Entry>("entry_get", { ...q("e"), id: outcome.entry_id });
    expect(entry).toMatchObject({ evidence_rows: 1, time_source: "row", needs_time: false });
    const page = await mockCall<GridPage>("grid_page", { ...q("g"), state: state(), limit: 5, offset: 0, options: opts });
    expect(page.marks[page.row_ids.indexOf(3)]).toBe("malicious");
    expect(changed).toEqual([["entries", "marks", "records"]]);
    expect(await rejection(mockCall("entry_get", { ...q("e"), id: 999 }))).toMatchObject({ code: "not_found" });
    // The undated entry cannot be reviewed until it has a time.
    expect(await rejection(mockCall("entry_status", { id: 4, status: "reviewed" }))).toMatchObject({ code: "refused" });
  });

  it("an export is a job, and ioc_sightings is a cancellable read", async () => {
    setMockLatency(5);
    await open();
    const jobs: string[] = [];
    mockOn("job-started", (p) => jobs.push(`start ${p.kind} ${p.label}`));
    mockOn("job-finished", (p) => jobs.push(`end ${p.kind} ${p.ok}`));
    await mockCall("export_report", SAMPLES.export_report);
    expect(jobs).toEqual(["start export record.md", "end export true"]);
    const sweep = rejection(mockCall("ioc_sightings", { slot: "sightings", queryId: "s1", id: 1 }));
    await mockCall("query_cancel", { queryIds: ["s1"] });
    expect(await sweep).toMatchObject({ code: "interrupted" });
  });
});

describe("the record order", () => {
  it("orders hosts seen by the count, then the names as the sheet shows them, as the engine does", async () => {
    resetMock();
    await mockCall("case_open", { path: "/cases/acme.case.duckdb" });
    const host = async (name: string) =>
      (await mockCall<CaseRecord>("record_upsert", { spec: { kind: "host", id: null, name, role: "", note: "" } })).id;
    const ioc = async (value: string) =>
      (await mockCall<CaseRecord>("record_upsert", { spec: { kind: "ioc", id: null, ioc_type: "file_name", value, tool: null, attack_ids: [], note: "" } })).id;
    const quote = await host('A"');
    const hash = await host("A#");
    const b = await host("B");
    const two = await ioc("seen-1.exe");
    const oneHash = await ioc("seen-2.exe");
    const none = await ioc("seen-3.exe");
    const oneQuote = await ioc("seen-4.exe");
    for (const [id, hosts] of [[two, [b, quote]], [oneHash, [hash]], [oneQuote, [quote]]] as const) {
      const e = await mockCall<Entry>("entry_new", { edit: { title: "seen", description: null, time: null, tactic: null, technique_ids: null } });
      await mockCall("record_link", { entryId: e.id, kind: "ioc", recordId: id });
      for (const h of hosts) await mockCall("record_link", { entryId: e.id, kind: "host", recordId: h });
    }
    const ids = async (descending: boolean) =>
      (await mockCall<Listing<CaseRecord>>("records_list", {
        ...q("r"), query: { kind: "ioc", ioc_type: null, tool: null, text: "seen-", order: { by: "hosts_seen", descending } }, limit: 10, offset: 0,
      })).items.map((r) => r.id);
    expect(await ids(false)).toEqual([none, oneQuote, oneHash, two]);
    expect(await ids(true)).toEqual([two, oneHash, oneQuote, none]);
  });

  it("compares the hosts seen count as a number, however many digits it has", () => {
    const hosts = (count: number) => Array.from({ length: count }, () => "H");
    // As text of a fixed width of six, 1,000,000 sorts before 999,999.
    expect(hostsSeenOrder(hosts(999_999), hosts(1_000_000))).toBeLessThan(0);
    expect(hostsSeenOrder(hosts(1_000_000), hosts(999_999))).toBeGreaterThan(0);
  });
});
