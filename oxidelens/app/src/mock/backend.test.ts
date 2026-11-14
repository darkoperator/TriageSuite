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
import { MOCK_COMMANDS, mockCall, mockOn, setMockLatency, setMockMarks } from "./backend";
import { HOSTILE, HAYABUSA_COLUMNS, HAYABUSA_FIRST_ID, EVENT_COLUMNS, EVENT_ROWS, eventRows, hayabusaRows } from "./generated";

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
