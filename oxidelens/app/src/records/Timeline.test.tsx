// The Timeline sheet and the entry editor: the engine's order and fields drawn as given (undated last, the mark, the
// time in the case's zone, the first linked account), the filters and grouping, the Host lanes, the editor's review,
// evidence, links and history, the candidates after M, and the sheet's keys. Against the mock backend through the real
// `api.ts`, in the whole shell, so a write's `case-changed` re-reads the sheet and the pane as the app does.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Candidate } from "../contract/generated/Candidate";
import type { CaseRecord } from "../contract/generated/CaseRecord";
import type { Entry } from "../contract/generated/Entry";
import type { EntryQuery } from "../contract/generated/EntryQuery";
import type { GridState } from "../contract/generated/GridState";
import type { Evidence } from "../contract/generated/Evidence";
import { mockCall } from "../mock/backend";
import { HOSTILE_RECORD } from "../mock/generated";
import { epochMicros } from "../mock/records";
import Shell from "../shell/Shell";
import { callsOf, overrideCommand } from "../test/backend";

vi.mock("../grid/measure", () => ({ measureText: vi.fn(() => 50) }));
vi.mock("../dialogs", () => ({ pickNewCasePath: vi.fn(), pickCaseFile: vi.fn(), pickRunFolder: vi.fn(), pickCsvFile: vi.fn() }));

beforeEach(() => {
  localStorage.clear();
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 440 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
});
afterEach(() => vi.restoreAllMocks());

// ---- helpers ------------------------------------------------------------------------------------------------------------
/** An entry as the engine answers one; `over` sets what a test needs. */
function entryOf(over: Partial<Entry> & { id: number }): Entry {
  return {
    time: null, time_source: null, needs_time: true, title: `Entry ${over.id}`, description: "", tactic: null, technique_ids: [],
    status: "draft", created_by: "analyst:analyst01", created_at: "2026-01-10T12:00:00.000000Z", modified_by: "analyst:analyst01",
    modified_at: "2026-01-10T12:00:00.000000Z", reviewed_by: null, agent_added: false, evidence_rows: 0, hosts: [], time_shown: null,
    links: [], mark: null, time_us: null, ...over,
  };
}
const dated = (id: number, utc: string, over: Partial<Entry> = {}): Entry =>
  entryOf({
    id, time: utc, time_source: "row", needs_time: false, time_shown: utc.replace("T", " ").replace("Z", ""), time_us: epochMicros(utc), ...over,
  });

const rightPane = () => screen.getByRole("complementary", { name: "Row detail" });
const sheet = () => screen.getByRole("grid", { name: "Timeline entries" });
/** The entry rows of the table, in the order drawn. */
const entryRows = () => within(sheet()).getAllByRole("row").filter((r) => r.getAttribute("data-entry") !== null);
const idsDrawn = () => entryRows().map((r) => Number(r.getAttribute("data-entry")));
const rowOfEntry = (id: number) => entryRows().find((r) => r.getAttribute("data-entry") === String(id))!;
/** The cell of entry row `row` under the header named `name`. */
const cellOf = (row: HTMLElement, name: string) => {
  const headers = within(sheet()).getAllByRole("columnheader");
  const at = headers.findIndex((h) => h.textContent === name || h.textContent?.startsWith(`${name} (`));
  if (at < 0) throw new Error(`no header ${name}`);
  return within(row).getAllByRole("gridcell")[at];
};
const lastQuery = (): EntryQuery => (callsOf("entries_list").at(-1)!.args as { query: EntryQuery }).query;
const sheetQueries = () =>
  callsOf("entries_list")
    .filter((c) => (c.args as { slot: string }).slot === "sheet:timeline")
    .map((c) => (c.args as { query: EntryQuery }).query);

/** The shell with the recent case open and the Timeline tab shown, its entries drawn. */
async function openTimeline(expectRows = true) {
  const user = userEvent.setup();
  render(<Shell />);
  await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
  await screen.findByRole("tab", { name: "Overview" });
  await user.click(screen.getByRole("tab", { name: /^Timeline/ }));
  await screen.findByRole("grid", { name: "Timeline entries" });
  if (expectRows) await waitFor(() => expect(entryRows().length).toBeGreaterThan(0));
  return user;
}

/** Selects entry `id` in the table; the editor for it is in the right pane. */
async function select(user: ReturnType<typeof userEvent.setup>, id: number) {
  await user.click(within(rowOfEntry(id)).getAllByRole("gridcell")[3]);
  return within(rightPane()).findByRole("region", { name: `Entry E-${id}` });
}

/** Opens the Host filter, waits for `listed` (when given) to be its options, and picks `name`. */
async function pickHost(user: ReturnType<typeof userEvent.setup>, name: string, listed?: string[]) {
  await user.click(screen.getByRole("button", { name: /^Host: / }));
  const dialog = await screen.findByRole("dialog", { name: "Filter by host" });
  const options = () => within(dialog).getAllByRole("option").map((o) => o.textContent);
  if (listed) await waitFor(() => expect(options()).toEqual(listed));
  await user.click(await within(dialog).findByRole("option", { name }));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Filter by host" })).toBeNull());
}

/** A promise a test settles by hand: holds a backend answer back. */
function held<T>() {
  let release!: (v: T) => void;
  const promise = new Promise<T>((r) => (release = r));
  return { promise, release };
}

// ---- the table ----------------------------------------------------------------------------------------------------------
describe("the Timeline sheet", () => {
  it("entries_sort_with_undated_last", async () => {
    // The engine's order, drawn as given: a later time first here, so a sheet that re-sorted by time or id is caught.
    const items = [
      dated(9, "2026-01-05T02:20:00.000000Z"),
      dated(3, "2026-01-05T02:14:07.000000Z"),
      entryOf({ id: 1, title: "Undated one" }),
      entryOf({ id: 7, title: "Undated two" }),
    ];
    overrideCommand("entries_list", async () => ({ items, truncated: false }));
    await openTimeline();
    expect(idsDrawn()).toEqual([9, 3, 1, 7]);
    // The undated sit under their own group heading, after every dated entry, each badged.
    const all = within(sheet()).getAllByRole("row");
    const heading = all.findIndex((r) => r.textContent?.startsWith("Undated"));
    expect(heading).toBeGreaterThan(all.indexOf(rowOfEntry(3)));
    expect(heading).toBeLessThan(all.indexOf(rowOfEntry(1)));
    expect(all[heading].textContent).toContain("2");
    expect(cellOf(rowOfEntry(1), "Time").textContent).toBe("needs a time");
    expect(cellOf(rowOfEntry(7), "Time").textContent).toBe("needs a time");
    // A dated entry shows the engine's text for the time in the case's zone, and the header names the zone.
    expect(cellOf(rowOfEntry(9), "Time").textContent).toBe("2026-01-05 02:20:00.000000");
    expect(within(sheet()).getAllByRole("columnheader")[1].textContent).toBe("Time (UTC)");
  });

  it("entries_made_out_of_time_order_list_by_time_with_undated_last", async () => {
    // The mock lists as the engine does (by time, the undated last, then by id; the engine's own order is pinned in
    // `record_entry.rs`). Entries typed out of time order show in time order: a list by id or by creation is caught.
    await openTimeline();
    expect(idsDrawn()).toEqual([1, 2, 3, 4]);
    const typed = (title: string, time: string) =>
      mockCall("entry_new", { edit: { title, description: null, time: { set: time }, tactic: null, technique_ids: null } });
    await act(async () => {
      await typed("Late", "2026-01-05T09:00:00.000000Z");
      await typed("Early", "2026-01-04T00:00:00.000000Z");
    });
    // E-6 (early) before the seeded E-1 and E-2 (from rows of 2026-01-05 02:1x), E-5 (late) after them, the undated
    // E-3 and E-4 last.
    await waitFor(() => expect(idsDrawn()).toEqual([6, 1, 2, 5, 3, 4]));
    const times = [6, 1, 2, 5].map((id) => cellOf(rowOfEntry(id), "Time").textContent!);
    expect([...times].sort()).toEqual(times);
  });

  it("the_timeline_pages_past_a_thousand", async () => {
    const all = Array.from({ length: 1001 }, (_, i) => dated(i + 1, `2026-01-05T02:${String(Math.floor(i / 60) % 60).padStart(2, "0")}:${String(i % 60).padStart(2, "0")}.000000Z`));
    overrideCommand("entries_list", async (a) => {
      const { limit, offset } = a as { limit: number; offset: number };
      return { items: all.slice(offset, offset + limit), truncated: offset + limit < all.length };
    });
    const user = await openTimeline();
    await waitFor(() => expect(entryRows()).toHaveLength(1000));
    const more = screen.getByRole("button", { name: "Show more" });
    expect(screen.getByText("1,000 entries shown · more in the case")).toBeInTheDocument();
    await user.click(more);
    await waitFor(() => expect(entryRows()).toHaveLength(1001));
    expect(sheetQueries().length).toBeGreaterThan(1);
    expect(callsOf("entries_list").some((c) => (c.args as { offset: number }).offset === 1000)).toBe(true);
    expect(screen.getByText("1,001 entries shown")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Show more" })).toBeNull();
    expect(idsDrawn().at(-1)).toBe(1001);
  });

  it("filters_and_grouping_reach_entries_list", async () => {
    const user = await openTimeline();
    expect(lastQuery()).toEqual({ status: [], host: null, tactic: null, text: null, row: null, order: "timeline" });
    await user.type(screen.getByRole("searchbox", { name: "Search entries" }), "logon");
    await waitFor(() => expect(lastQuery().text).toBe("logon"));
    await user.selectOptions(screen.getByRole("combobox", { name: "Status" }), "reviewed");
    await waitFor(() => expect(lastQuery().status).toEqual(["reviewed"]));
    // The hosts and tactics offered are the engine's: the live host records, and its fourteen tactics.
    await pickHost(user, "WS01", ["All hosts", "SRV01", "WS01"]);
    expect(screen.getByRole("button", { name: "Host: WS01" })).toHaveFocus();
    const tactic = screen.getByRole("combobox", { name: "Tactic" });
    await waitFor(() => expect(within(tactic).getAllByRole("option")).toHaveLength(15));
    // What the engine answers for the last filter: an entry with two hosts, one with none.
    const items = [
      dated(1, "2026-01-05T02:14:07.000000Z", { hosts: ["WS01"] }),
      dated(2, "2026-01-05T02:15:00.000000Z", { hosts: ["SRV01", "WS01"] }),
      dated(3, "2026-01-05T02:16:00.000000Z", { hosts: [] }),
    ];
    overrideCommand("entries_list", async () => ({ items, truncated: false }));
    await user.selectOptions(tactic, "Credential Access");
    await waitFor(() =>
      expect(lastQuery()).toEqual({ status: ["reviewed"], host: "WS01", tactic: "Credential Access", text: "logon", row: null, order: "timeline" }),
    );
    await waitFor(() => expect(idsDrawn()).toEqual([1, 2, 3]));

    // Grouping splits the engine's answer for these filters, in its own order, with no read of its own: an entry
    // under each of its hosts, the groups in the order their keys first appear.
    const before = sheetQueries().length;
    await user.selectOptions(screen.getByRole("combobox", { name: "Group by" }), "host");
    await waitFor(() => expect(within(sheet()).getAllByRole("row").filter((r) => r.getAttribute("data-group") !== null)).toHaveLength(3));
    const groups = within(sheet()).getAllByRole("row").filter((r) => r.getAttribute("data-group") !== null);
    expect(groups.map((g) => g.getAttribute("data-group"))).toEqual(["WS01", "SRV01", "no host"]);
    // The whole answer is shown: each header counts its group.
    expect(groups.map((g) => g.textContent)).toEqual(["WS01 · 2", "SRV01 · 1", "no host · 1"]);
    expect(idsDrawn()).toEqual([1, 2, 2, 3]);
    expect(sheetQueries()).toHaveLength(before);
  });

  it("a_group_header_says_when_more_pages_follow", async () => {
    // The engine says more entries follow these three: each header counts what is shown, and says so.
    const items = [
      dated(1, "2026-01-05T02:14:07.000000Z", { hosts: ["WS01"] }),
      dated(2, "2026-01-05T02:15:00.000000Z", { hosts: ["SRV01", "WS01"] }),
      entryOf({ id: 3, title: "Undated", hosts: ["WS01"] }),
    ];
    overrideCommand("entries_list", async () => ({ items, truncated: true }));
    const user = await openTimeline();
    const headers = () => within(sheet()).getAllByRole("row").filter((r) => r.getAttribute("data-group") !== null).map((g) => g.textContent);
    // Ungrouped, the undated heading counts what is shown too.
    await waitFor(() => expect(headers()).toEqual(["Undated · needs a time · 1+ shown"]));
    await user.selectOptions(screen.getByRole("combobox", { name: "Group by" }), "host");
    await waitFor(() => expect(headers()).toEqual(["WS01 · 3+ shown", "SRV01 · 1+ shown"]));
  });

  it("the_host_filter_reaches_a_host_past_the_first_thousand", async () => {
    // 1,500 host records: the filter asks the engine for them by the typed text, 200 at a time, so the last is reachable.
    // Each made from the mock's first host record, read once the case is open.
    let real: CaseRecord | null = null;
    const names = Array.from({ length: 1500 }, (_, i) => `HOST-${String(i).padStart(4, "0")}`);
    overrideCommand("records_list", async (a) => {
      const { query, limit, offset } = a as { query: { kind: string; text?: string | null }; limit: number; offset: number };
      if (query.kind !== "host") return mockCall("records_list", a as Record<string, unknown>);
      real ??= ((await mockCall("records_list", { ...(a as Record<string, unknown>), limit: 1, offset: 0, query: { kind: "host" } })) as { items: CaseRecord[] }).items[0];
      const template = real;
      const matching = names.filter((n) => !query.text || n.toLowerCase().includes(query.text.toLowerCase()));
      return {
        items: matching.slice(offset, offset + limit).map((name, i) => ({ ...template, id: 10_000 + offset + i, name })),
        truncated: offset + limit < matching.length,
      };
    });
    const user = await openTimeline();
    await user.click(screen.getByRole("button", { name: "Host: all" }));
    const dialog = await screen.findByRole("dialog", { name: "Filter by host" });
    // Untyped: the first 200, saying more follow; Show more reads the next 200 from the engine.
    await waitFor(() => expect(within(dialog).getAllByRole("option")).toHaveLength(201));
    expect(dialog.textContent).toContain("200 listed · more in the case");
    await user.click(within(dialog).getByRole("button", { name: "Show more" }));
    await waitFor(() => expect(within(dialog).getAllByRole("option")).toHaveLength(401));
    const hostReads = () => callsOf("records_list").map((c) => c.args as { query: { kind: string; text: string | null }; limit: number; offset: number; slot: string }).filter((c) => c.slot === "sheet:timeline:hosts");
    expect(hostReads().at(-1)).toMatchObject({ limit: 200, offset: 200 });
    // Typed: the engine narrows them, and host 1,400 is one click away.
    await user.type(within(dialog).getByRole("searchbox", { name: "Filter hosts" }), "host-1400");
    await waitFor(() => expect(within(dialog).getAllByRole("option").map((o) => o.textContent)).toEqual(["All hosts", "HOST-1400"]));
    expect(hostReads().at(-1)?.query).toEqual({ kind: "host", text: "host-1400" });
    await user.click(within(dialog).getByRole("option", { name: "HOST-1400" }));
    await waitFor(() => expect(lastQuery().host).toBe("HOST-1400"));
    expect(screen.getByRole("button", { name: "Host: HOST-1400" })).toBeInTheDocument();
    // All hosts clears it.
    await pickHost(user, "All hosts");
    await waitFor(() => expect(lastQuery().host).toBeNull());
    expect(screen.getByRole("button", { name: "Host: all" })).toBeInTheDocument();
  });

  it("agent_entries_say_agent_review", async () => {
    const user = await openTimeline();
    // E-2 is the agent's, unreviewed: its status reads `agent · review` in the agent colour, its glyph is the agent's.
    const agentRow = rowOfEntry(2);
    const status = cellOf(agentRow, "Status");
    expect(status.textContent).toBe("agent · review");
    expect(status.querySelector(".status-agent")).not.toBeNull();
    expect(within(cellOf(agentRow, "")).getByRole("img", { name: "agent: malicious" })).toBeInTheDocument();
    // E-1 is the analyst's, reviewed: Malicious, and its status says so in no agent colour.
    expect(cellOf(rowOfEntry(1), "Status").textContent).toBe("reviewed");
    expect(cellOf(rowOfEntry(1), "Status").querySelector(".status-agent")).toBeNull();
    expect(within(cellOf(rowOfEntry(1), "")).getByRole("img", { name: "malicious" })).toBeInTheDocument();
    // The first linked account is the Account column; the editor names the agent.
    expect(cellOf(rowOfEntry(1), "Account").textContent).toBe("ACME\\jdoe");
    const editor = await select(user, 2);
    await waitFor(() => expect(editor.textContent).toContain("added by agent: triage-bot"));
  });

  it("hostile_titles_render_as_text", async () => {
    const items = [
      dated(1, "2026-01-05T02:14:07.000000Z", { title: HOSTILE_RECORD[1], hosts: [HOSTILE_RECORD[2]], tactic: null }),
      entryOf({
        id: 2, title: HOSTILE_RECORD[0], description: HOSTILE_RECORD[4],
        links: [{ kind: "account", id: 1, label: HOSTILE_RECORD[2] }, { kind: "ioc", id: 2, label: HOSTILE_RECORD[1] }],
      }),
    ];
    overrideCommand("entries_list", async () => ({ items, truncated: false }));
    overrideCommand("entry_get", async (a) => items.find((e) => e.id === (a as { id: number }).id));
    const user = await openTimeline();
    expect(sheet().querySelector("img")).toBeNull();
    expect(cellOf(rowOfEntry(1), "What happened").textContent).toBe("<img src=x onerror=1>");
    expect(cellOf(rowOfEntry(1), "Host").textContent).toBe("invoice\\u{202E}fdp.exe");
    expect(cellOf(rowOfEntry(2), "What happened").textContent).toBe('=HYPERLINK("http://x")');
    expect(cellOf(rowOfEntry(2), "Account").textContent).toBe("invoice\\u{202E}fdp.exe");
    const editor = await select(user, 2);
    const linked = await within(editor).findByRole("region", { name: "Linked records" });
    expect(linked.textContent).toContain("invoice\\u{202E}fdp.exe");
    expect(linked.textContent).toContain("<img src=x onerror=1>");
    expect(document.querySelector("img")).toBeNull();
  });

  it("a_late_entries_answer_is_not_drawn", async () => {
    const user = await openTimeline();
    const old = held<unknown>();
    overrideCommand("entries_list", (a) => {
      const q = (a as { query: EntryQuery }).query;
      if (q.text === "a") return old.promise;
      return Promise.resolve({ items: [dated(42, "2026-01-05T02:14:07.000000Z", { title: "The answer for ab" })], truncated: false });
    });
    const box = screen.getByRole("searchbox", { name: "Search entries" });
    await user.type(box, "a");
    await waitFor(() => expect(sheetQueries().at(-1)!.text).toBe("a"));
    await user.type(box, "b");
    await waitFor(() => expect(idsDrawn()).toEqual([42]));
    // The read for "a" was cancelled when "ab" was asked, and its answer, released now, is dropped.
    expect(callsOf("query_cancel").length).toBeGreaterThan(0);
    await act(async () => old.release({ items: [dated(41, "2026-01-05T02:14:07.000000Z", { title: "Stale" })], truncated: false }));
    expect(idsDrawn()).toEqual([42]);
  });
});

// ---- the editor ---------------------------------------------------------------------------------------------------------
describe("the entry editor", () => {
  it("mark_reviewed_on_an_undated_entry_shows_why", async () => {
    const user = await openTimeline();
    // E-3 is the analyst's undated draft: the engine refuses to review it, and its reason is shown in place.
    const editor = await select(user, 3);
    await user.click(await within(editor).findByRole("button", { name: "Mark reviewed" }));
    const why = await within(editor).findByRole("status", { name: "Review" });
    await waitFor(() => expect(why.textContent).toBe("E-3 needs a time before it can be reviewed"));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(callsOf("entry_status").map((c) => c.args)).toEqual([{ id: 3, status: "reviewed" }]);
    // R on the table does the same, with no dialog either.
    rowOfEntry(3).focus();
    await user.keyboard("r");
    await waitFor(() => expect(callsOf("entry_status")).toHaveLength(2));
    expect(screen.queryByRole("dialog")).toBeNull();
    // A refusal for an agent's entry (an agent may not review) is shown the same way, without its code.
    overrideCommand("entry_status", () => Promise.reject({ code: "refused", message: "only an analyst reviews entries" }));
    const agents = await select(user, 2);
    await user.click(await within(agents).findByRole("button", { name: "Mark reviewed" }));
    await waitFor(() => expect(within(agents).getByRole("status", { name: "Review" }).textContent).toBe("only an analyst reviews entries"));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("evidence_opens_its_row_in_the_dataset", async () => {
    const user = await openTimeline();
    const editor = await select(user, 1);
    const evidence = await within(editor).findByRole("region", { name: "Evidence" });
    await waitFor(() => expect(evidence.textContent).toContain("row unchanged"));
    expect(evidence.textContent).toContain("mock_events");
    await user.click(within(evidence).getByRole("button", { name: "open in dataset, scrolled to the row" }));
    // The dataset tab opens and asks the engine where row 7 sits in its order, to scroll to it.
    await screen.findByRole("tab", { name: /mock_events/, selected: true });
    await waitFor(() => expect(callsOf("grid_row_offset").map((c) => (c.args as { rowId: number }).rowId)).toContain(7));
    const asked = callsOf("grid_row_offset").at(-1)!.args as { state: GridState };
    expect(asked.state.relation).toBe("mock_events");
  });

  it("evidence_is_read_a_page_at_a_time", async () => {
    // 150 evidence rows: the editor reads 100, says more follow, and Show more reads the next page from the engine.
    const rows: Evidence[] = Array.from({ length: 150 }, (_, i) => ({
      row_id: 1000 + i, relation: "mock_events", host: "WS01", standing: "unchanged", summary: `evidence row ${1000 + i}`,
      attached_by: "analyst:analyst01", attached_at: "2026-01-10T12:00:00.000000Z",
    }));
    overrideCommand("evidence_list", async (a) => {
      const { limit, offset } = a as { limit: number; offset: number };
      return { items: rows.slice(offset, offset + limit), truncated: offset + limit < rows.length };
    });
    const user = await openTimeline();
    const editor = await select(user, 1);
    const evidence = await within(editor).findByRole("region", { name: "Evidence" });
    await waitFor(() => expect(within(evidence).getAllByRole("listitem")).toHaveLength(100));
    const paged = () => callsOf("evidence_list").map((c) => c.args as { limit: number; offset: number });
    expect(paged().every((p) => p.limit === 100)).toBe(true);
    expect(paged().map((p) => p.offset)).not.toContain(100);
    expect(evidence.textContent).not.toContain("evidence row 1100");
    await user.click(within(evidence).getByRole("button", { name: "Show more" }));
    await waitFor(() => expect(within(evidence).getAllByRole("listitem")).toHaveLength(150));
    expect(paged().at(-1)).toMatchObject({ limit: 100, offset: 100 });
    expect(evidence.textContent).toContain("evidence row 1149");
    expect(within(evidence).queryByRole("button", { name: "Show more" })).toBeNull();
  });

  it("history_is_read_when_opened", async () => {
    const user = await openTimeline();
    const editor = await select(user, 1);
    expect(callsOf("history_list")).toHaveLength(0);
    await user.click(await within(editor).findByRole("button", { name: /^History/ }));
    await waitFor(() => expect(callsOf("history_list").map((c) => c.args)).toContainEqual(expect.objectContaining({ targetKind: "entry", targetId: "1" })));
    await waitFor(() => expect(within(editor).getByRole("list", { name: "History" }).textContent).toContain("entry_create"));
  });

  it("a_late_editor_answer_is_not_drawn", async () => {
    const user = await openTimeline();
    const old = held<unknown>();
    const real = (a: unknown) => import("../mock/backend").then((m) => m.mockCall("entry_get", a as Record<string, unknown>));
    overrideCommand("entry_get", (a) => ((a as { id: number }).id === 1 ? old.promise : real(a)));
    await user.click(within(rowOfEntry(1)).getAllByRole("gridcell")[3]);
    await within(rightPane()).findByRole("region", { name: "Entry E-1" });
    const editor = await select(user, 3);
    await waitFor(() => expect(within(editor).getByRole("textbox", { name: "Title" })).toHaveValue(HOSTILE_RECORD[1]));
    await act(async () => old.release(entryOf({ id: 1, title: "Stale answer" })));
    expect(within(rightPane()).queryByRole("region", { name: "Entry E-1" })).toBeNull();
    expect(within(rightPane()).getByRole("textbox", { name: "Title" })).toHaveValue(HOSTILE_RECORD[1]);
  });

  it("edits_save_only_the_changed_fields", async () => {
    const user = await openTimeline();
    const editor = await select(user, 3);
    const title = await within(editor).findByRole("textbox", { name: "Title" });
    await user.clear(title);
    await user.type(title, "Log wipe on WS01");
    await user.type(within(editor).getByRole("textbox", { name: "Time (UTC)" }), "2026-01-05T02:30:00Z");
    await user.click(within(editor).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(callsOf("entry_edit")).toHaveLength(1));
    expect(callsOf("entry_edit")[0].args).toEqual({
      id: 3,
      edit: { title: "Log wipe on WS01", description: null, time: { set: "2026-01-05T02:30:00Z" }, tactic: null, technique_ids: null },
    });
  });
});

// ---- keys ---------------------------------------------------------------------------------------------------------------
describe("the Timeline's keys", () => {
  it("n_makes_an_entry_and_del_removes_one", async () => {
    const user = await openTimeline();
    expect(screen.getByLabelText("Keys").textContent).toBe(
      "N new entry · Enter edit · R mark reviewed · Del remove (keeps evidence rows) · Ctrl+[ Ctrl+] panes",
    );
    rowOfEntry(1).focus();
    await user.keyboard("n");
    const form = await within(rightPane()).findByRole("form", { name: "New entry" });
    const title = within(form).getByRole("textbox", { name: "Title" });
    expect(title).toHaveFocus();
    await user.type(title, "Lateral movement to SRV01");
    await user.click(within(form).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(callsOf("entry_new")).toHaveLength(1));
    await within(rightPane()).findByRole("region", { name: "Entry E-5" });
    await waitFor(() => expect(idsDrawn()).toContain(5));
    // Enter moves to the editor's first field; Del removes the entry, and its evidence rows stay in their datasets.
    rowOfEntry(1).focus();
    await user.keyboard("{ArrowDown}");
    expect(rowOfEntry(2)).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(within(rightPane()).getByRole("textbox", { name: "Title" })).toHaveFocus());
    rowOfEntry(2).focus();
    await user.keyboard("{Delete}");
    await waitFor(() => expect(callsOf("entry_delete").map((c) => c.args)).toEqual([{ id: 2 }]));
    await waitFor(() => expect(idsDrawn()).not.toContain(2));
    // The next entry is selected; its editor does not take the focus an earlier Enter asked for.
    const next = await within(rightPane()).findByRole("region", { name: "Entry E-3" });
    await within(next).findByRole("textbox", { name: "Title" });
    expect(within(next).getByRole("textbox", { name: "Title" })).not.toHaveFocus();
    expect(callsOf("evidence_detach")).toHaveLength(0);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("keys_do_nothing_while_typing", async () => {
    const user = await openTimeline();
    await user.type(screen.getByRole("searchbox", { name: "Search entries" }), "nr");
    expect(callsOf("entry_new")).toHaveLength(0);
    expect(callsOf("entry_status")).toHaveLength(0);
    expect(within(rightPane()).queryByRole("form", { name: "New entry" })).toBeNull();
  });
});

// ---- Host lanes ---------------------------------------------------------------------------------------------------------
describe("Host lanes", () => {
  it("host_lanes_place_entries_by_host", async () => {
    const items = [
      // Uneven gaps: one minute, then three. Positions follow the time, not the rank.
      dated(1, "2026-01-05T02:14:00.000000Z", { hosts: ["WS01"], mark: "malicious" }),
      dated(2, "2026-01-05T02:15:00.000000Z", { hosts: ["SRV01"], mark: "agent_malicious" }),
      dated(3, "2026-01-05T02:18:00.000000Z", { hosts: ["SRV01", "WS01"], mark: "malicious" }),
      entryOf({ id: 4, hosts: ["WS01"] }),
    ];
    overrideCommand("entries_list", async () => ({ items, truncated: false }));
    const user = await openTimeline();
    await user.click(screen.getByRole("button", { name: "Host lanes" }));
    expect(screen.getByRole("button", { name: "Host lanes" })).toHaveAttribute("aria-pressed", "true");
    const lanes = await screen.findByRole("region", { name: "Host lanes" });
    const lane = (host: string) => within(lanes).getByRole("group", { name: `Host ${host}` });
    // One lane per host, in the order the hosts first appear; each entry drawn in each of its hosts' lanes.
    expect(within(lanes).getAllByRole("group").map((g) => g.getAttribute("aria-label"))).toEqual(["Host WS01", "Host SRV01"]);
    const ids = (host: string) => within(lane(host)).getAllByRole("button").map((b) => b.getAttribute("data-entry"));
    expect(ids("WS01")).toEqual(["1", "3"]);
    expect(ids("SRV01")).toEqual(["2", "3"]);
    // On the shared time axis (the engine's `time_us`): the first at the start, the last at the end, one minute of
    // four at a quarter, the same entry level in every lane. The undated one is not drawn, and is counted under it.
    const left = (host: string, id: string) =>
      within(lane(host)).getAllByRole("button").find((b) => b.getAttribute("data-entry") === id)!.style.left;
    expect([left("WS01", "1"), left("SRV01", "2"), left("WS01", "3"), left("SRV01", "3")]).toEqual(["0%", "25%", "100%", "100%"]);
    expect(within(lanes).queryAllByRole("button").some((b) => b.getAttribute("data-entry") === "4")).toBe(false);
    expect(lanes.textContent).toContain("1 undated entry, not drawn");
    // A glyph opens its entry in the editor.
    await user.click(within(lane("SRV01")).getAllByRole("button")[0]);
    await within(rightPane()).findByRole("region", { name: "Entry E-2" });
  });

  it("switching_to_lanes_moves_the_keys_there", async () => {
    const user = await openTimeline();
    // The switch leaves the focus where the analyst put it: the keys still reach the sheet.
    await user.click(screen.getByRole("button", { name: "Host lanes" }));
    const lanes = await screen.findByRole("region", { name: "Host lanes" });
    expect(lanes.contains(document.activeElement)).toBe(true);
    await user.keyboard("n");
    await within(rightPane()).findByRole("form", { name: "New entry" });
    // And back: the table takes the focus.
    await user.click(within(rightPane()).getByRole("button", { name: "Cancel" }));
    await user.click(screen.getByRole("button", { name: "Table" }));
    await waitFor(() => expect(sheet()).toHaveFocus());
  });

  it("the_lanes_show_where_the_focus_is", () => {
    // With every entry undated the focus lands on the lanes themselves: they draw a focus indicator, never none.
    const css = readFileSync("src/records/records.css", "utf8");
    const rule = /\.host-lanes:focus(?:-visible)?\s*\{([^}]*)\}/.exec(css);
    expect(rule).not.toBeNull();
    expect(rule![1]).toMatch(/outline:\s*\d+px solid/);
    expect(css).not.toMatch(/\.host-lanes:focus\s*\{\s*outline:\s*none/);
  });

  it("n_works_in_lanes_with_only_undated_entries", async () => {
    const items = [entryOf({ id: 1, hosts: ["WS01"] }), entryOf({ id: 2 })];
    overrideCommand("entries_list", async () => ({ items, truncated: false }));
    const user = await openTimeline();
    await user.click(screen.getByRole("button", { name: "Host lanes" }));
    const lanes = await screen.findByRole("region", { name: "Host lanes" });
    expect(within(lanes).queryAllByRole("button")).toHaveLength(0);
    expect(lanes.textContent).toContain("2 undated entries, not drawn");
    await user.keyboard("n");
    await within(rightPane()).findByRole("form", { name: "New entry" });
  });

  it("the_sheet_keys_work_in_host_lanes", async () => {
    const items = [
      dated(1, "2026-01-05T02:14:00.000000Z", { hosts: ["WS01"] }),
      dated(2, "2026-01-05T02:15:00.000000Z", { hosts: ["SRV01"] }),
    ];
    overrideCommand("entries_list", async () => ({ items, truncated: false }));
    const user = await openTimeline();
    await user.click(screen.getByRole("button", { name: "Host lanes" }));
    const lanes = await screen.findByRole("region", { name: "Host lanes" });
    const glyph = (id: number) => within(lanes).getAllByRole("button").find((b) => b.getAttribute("data-entry") === String(id))!;
    await user.click(glyph(2));
    await within(rightPane()).findByRole("region", { name: "Entry E-2" });
    // R reviews the entry the lanes have selected; its refusal or success is the engine's.
    glyph(2).focus();
    await user.keyboard("r");
    await waitFor(() => expect(callsOf("entry_status").map((c) => c.args)).toEqual([{ id: 2, status: "reviewed" }]));
    // N opens the new-entry form.
    glyph(2).focus();
    await user.keyboard("n");
    const form = await within(rightPane()).findByRole("form", { name: "New entry" });
    await user.click(within(form).getByRole("button", { name: "Cancel" }));
    // Enter on a glyph takes it, and Del removes the focused entry.
    glyph(1).focus();
    await user.keyboard("{Enter}");
    await within(rightPane()).findByRole("region", { name: "Entry E-1" });
    glyph(1).focus();
    await user.keyboard("{Delete}");
    await waitFor(() => expect(callsOf("entry_delete").map((c) => c.args)).toEqual([{ id: 1 }]));
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

// ---- the candidates after M ---------------------------------------------------------------------------------------------
/** The shell on mock_events, M pressed on its row at index 3: the editor of the new entry E-5. */
async function promoteRow3() {
  const user = userEvent.setup();
  render(<Shell />);
  await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
  await screen.findByRole("tab", { name: "Overview" });
  await user.click(within(screen.getByRole("complementary", { name: "Case" })).getByRole("button", { name: /^mock_events/ }));
  await screen.findByRole("tab", { name: /mock_events/, selected: true });
  const grid = await screen.findByRole("grid", { name: "Rows" });
  await waitFor(() => expect(within(grid).getAllByRole("row").length).toBeGreaterThan(10));
  const row = within(grid).getAllByRole("row").find((r) => r.getAttribute("data-row-index") === "3")!;
  await user.click(within(row).getAllByRole("gridcell")[2]);
  await user.keyboard("m");
  return { user, editor: await within(rightPane()).findByRole("region", { name: "Entry E-5" }) };
}

describe("linking a record by hand", () => {
  it("the_link_picker_finds_a_record_past_the_first_page_by_typing", async () => {
    // The case has more hosts than the picker lists at once; the one wanted is not among them, and typing asks the
    // engine for it. An answer for an older text is dropped.
    const first = Array.from({ length: 200 }, (_, i) => ({ kind: "host", id: 1000 + i, name: `HOST${String(i).padStart(3, "0")}`, role: "", note: "" }));
    const old = held<unknown>();
    overrideCommand("records_list", (a) => {
      const q = (a as { query: { kind: string; text?: string | null } }).query;
      if (q.kind !== "host") return import("../mock/backend").then((m) => m.mockCall("records_list", a as Record<string, unknown>));
      if (!q.text) return Promise.resolve({ items: first, truncated: true });
      if (q.text === "WS9") return old.promise;
      return Promise.resolve({ items: [{ kind: "host", id: 4242, name: "WS999", role: "", note: "" }], truncated: false });
    });
    overrideCommand("record_link", async () => null);
    const user = await openTimeline();
    const editor = await select(user, 1);
    await user.click(await within(editor).findByRole("button", { name: "+ link a host, account or IOC" }));
    const dialog = await screen.findByRole("dialog", { name: "Link a record" });
    const list = within(dialog).getByRole("listbox", { name: "Records" });
    await waitFor(() => expect(within(list).getAllByRole("option")).toHaveLength(200));
    expect(within(list).queryByRole("option", { name: "WS999" })).toBeNull();
    const box = within(dialog).getByRole("searchbox", { name: "Filter records" });
    await user.type(box, "WS9");
    await user.type(box, "9");
    const found = await within(dialog).findByRole("option", { name: "WS999" });
    await act(async () => old.release({ items: [{ kind: "host", id: 7, name: "STALE", role: "", note: "" }], truncated: false }));
    expect(within(dialog).queryByRole("option", { name: "STALE" })).toBeNull();
    const asked = callsOf("records_list").map((c) => (c.args as { query: { text?: string | null } }).query.text ?? null);
    expect(asked).toContain("WS99");
    await user.click(found);
    await waitFor(() => expect(callsOf("record_link").map((c) => c.args)).toEqual([{ entryId: 1, kind: "host", recordId: 4242 }]));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("typing_a_full_name_lists_that_record_first", async () => {
    // 200 hosts hold "ZZZ" and sort before the host named "ZZZ": the engine ranks the exact name first, and the picker
    // shows its answer in that order.
    const user = await openTimeline();
    await act(async () => {
      for (let i = 0; i < 200; i++) {
        await mockCall("record_upsert", { spec: { kind: "host", id: null, name: `A-ZZZ-${String(i).padStart(3, "0")}`, role: "", note: "" } });
      }
      await mockCall("record_upsert", { spec: { kind: "host", id: null, name: "ZZZ", role: "", note: "" } });
    });
    const editor = await select(user, 1);
    await user.click(await within(editor).findByRole("button", { name: "+ link a host, account or IOC" }));
    const dialog = await screen.findByRole("dialog", { name: "Link a record" });
    const list = within(dialog).getByRole("listbox", { name: "Records" });
    await user.type(within(dialog).getByRole("searchbox", { name: "Filter records" }), "ZZZ");
    await waitFor(() => expect(within(list).getAllByRole("option")[0].textContent).toBe("ZZZ"));
    expect(within(list).getAllByRole("option")).toHaveLength(200);
  });

  it("closing_the_link_picker_cancels_its_read", async () => {
    overrideCommand("records_list", (a) => {
      const q = (a as { query: { kind: string } }).query;
      if (q.kind === "host" && (a as { slot: string }).slot === "dialog:link") return new Promise(() => {});
      return import("../mock/backend").then((m) => m.mockCall("records_list", a as Record<string, unknown>));
    });
    const user = await openTimeline();
    const editor = await select(user, 1);
    await user.click(await within(editor).findByRole("button", { name: "+ link a host, account or IOC" }));
    const dialog = await screen.findByRole("dialog", { name: "Link a record" });
    const pending = callsOf("records_list").filter((c) => (c.args as { slot: string }).slot === "dialog:link").at(-1)!;
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(callsOf("query_cancel").flatMap((c) => (c.args as { queryIds: string[] }).queryIds)).toContain((pending.args as { queryId: string }).queryId),
    );
  });

  /** The picker's own reads of the records, in the order asked. */
  const pickerReads = () =>
    callsOf("records_list")
      .map((c) => c.args as { slot: string; queryId: string; offset: number; query: { kind: string; text?: string | null } })
      .filter((a) => a.slot === "dialog:link");
  const cancelled = () => callsOf("query_cancel").flatMap((c) => (c.args as { queryIds: string[] }).queryIds);
  /** 200 accounts named `svc_scan` in domains that hold "ZED", and one in the domain ZED itself, which sorts last. */
  async function manyAccounts(): Promise<number> {
    const spec = (domain: string) => ({ kind: "account", id: null, name: "svc_scan", domain, sid: null, description: "", privilege: "unknown", note: "" });
    let last = 0;
    await act(async () => {
      for (let i = 0; i < 200; i++) await mockCall("record_upsert", { spec: spec(`A-ZED-${String(i).padStart(3, "0")}`) });
      last = (await mockCall<{ id: number }>("record_upsert", { spec: spec("ZED") })).id;
    });
    return last;
  }
  /** The picker open on entry E-1, listing accounts. */
  async function pickAccounts(user: ReturnType<typeof userEvent.setup>) {
    const editor = await select(user, 1);
    await user.click(await within(editor).findByRole("button", { name: "+ link a host, account or IOC" }));
    const dialog = await screen.findByRole("dialog", { name: "Link a record" });
    await user.selectOptions(within(dialog).getByRole("combobox", { name: "Kind" }), "account");
    return { dialog, list: within(dialog).getByRole("listbox", { name: "Records" }), box: within(dialog).getByRole("searchbox", { name: "Filter records" }) };
  }

  it("show_more_reaches_the_record_past_the_first_200", async () => {
    // 201 accounts share the name typed, so they rank equally and the one in the last domain is the 201st. The picker
    // lists 200 and says so; *Show more* asks for the next 200 from there, and only those, and adds them to the list.
    const user = await openTimeline();
    const wanted = await manyAccounts();
    const { dialog, list, box } = await pickAccounts(user);
    await user.type(box, "svc_scan");
    await waitFor(() => expect(pickerReads().some((a) => a.query.text === "svc_scan")).toBe(true));
    await waitFor(() => expect(within(list).getAllByRole("option")).toHaveLength(200));
    expect(within(list).queryByRole("option", { name: "ZED\\svc_scan" })).toBeNull();
    expect(within(dialog).getByText("200 listed · more match")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Show more" }));
    await waitFor(() => expect(within(list).getAllByRole("option")).toHaveLength(201));
    const options = within(list).getAllByRole("option");
    expect(options.slice(0, 200).map((o) => o.textContent)).toEqual(Array.from({ length: 200 }, (_, i) => `A-ZED-${String(i).padStart(3, "0")}\\svc_scan`));
    expect(options[200].textContent).toBe("ZED\\svc_scan");
    expect(pickerReads().filter((a) => a.query.text === "svc_scan").map((a) => a.offset)).toEqual([0, 200]);
    // Every match is listed now: the count says so, there is no more to ask for, and the focus *Show more* had is on
    // the first record it added, not lost outside the dialog.
    expect(within(dialog).getByText("201 listed")).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Show more" })).toBeNull();
    expect(options[200]).toHaveFocus();
    await user.click(options[200]);
    await waitFor(() => expect(callsOf("record_link").map((c) => c.args)).toEqual([{ entryId: 1, kind: "account", recordId: wanted }]));
  });

  it("typing_a_domain_and_name_lists_that_account_first", async () => {
    // The engine ranks an account by its `DOMAIN\\name` as well as its name: the domain typed in full is ahead of the
    // 200 domains that only hold it, and domain and name together find the one account.
    const user = await openTimeline();
    await manyAccounts();
    const { list, box } = await pickAccounts(user);
    await user.type(box, "zed");
    await waitFor(() => expect(within(list).getAllByRole("option")[0].textContent).toBe("ZED\\svc_scan"));
    expect(within(list).getAllByRole("option")).toHaveLength(200);
    await user.type(box, "\\SVC_scan");
    await waitFor(() => expect(within(list).getAllByRole("option").map((o) => o.textContent)).toEqual(["ZED\\svc_scan"]));
    expect(pickerReads().at(-1)!.query.text).toBe("zed\\SVC_scan");
  });

  /** A case of more than 200 hosts whose second page the test holds back; a typed text finds one host. */
  function heldSecondPage() {
    const first = Array.from({ length: 200 }, (_, i) => ({ kind: "host", id: 1000 + i, name: `HOST${String(i).padStart(3, "0")}`, role: "", note: "" }));
    const second = held<unknown>();
    overrideCommand("records_list", (a) => {
      const { slot, offset, query } = a as { slot: string; offset: number; query: { text?: string | null } };
      if (slot !== "dialog:link") return import("../mock/backend").then((m) => m.mockCall("records_list", a as Record<string, unknown>));
      if (query.text) return Promise.resolve({ items: [{ kind: "host", id: 4242, name: "WS999", role: "", note: "" }], truncated: false });
      return offset === 0 ? Promise.resolve({ items: first, truncated: true }) : second.promise;
    });
    return second;
  }
  /** The picker open on entry E-1 with 200 hosts listed and *Show more* pressed: its read of the next 200, held. */
  async function showMoreHeld(user: ReturnType<typeof userEvent.setup>) {
    const editor = await select(user, 1);
    await user.click(await within(editor).findByRole("button", { name: "+ link a host, account or IOC" }));
    const dialog = await screen.findByRole("dialog", { name: "Link a record" });
    const list = within(dialog).getByRole("listbox", { name: "Records" });
    await waitFor(() => expect(within(list).getAllByRole("option")).toHaveLength(200));
    await user.click(within(dialog).getByRole("button", { name: "Show more" }));
    await waitFor(() => expect(pickerReads().some((a) => a.offset === 200)).toBe(true));
    return { dialog, list, pending: pickerReads().find((a) => a.offset === 200)! };
  }

  it("a_text_change_drops_a_held_show_more_answer", async () => {
    const second = heldSecondPage();
    const user = await openTimeline();
    const { dialog, list, pending } = await showMoreHeld(user);
    // The first 200 stay listed while the next are read.
    expect(within(list).getAllByRole("option")).toHaveLength(200);
    await user.type(within(dialog).getByRole("searchbox", { name: "Filter records" }), "W");
    await within(list).findByRole("option", { name: "WS999" });
    // The read of the next 200 was for the text before: it is cancelled, and its answer is not added to this list.
    expect(cancelled()).toContain(pending.queryId);
    await act(async () => second.release({ items: [{ kind: "host", id: 7, name: "STALE", role: "", note: "" }], truncated: false }));
    expect(within(list).getAllByRole("option").map((o) => o.textContent)).toEqual(["WS999"]);
    expect(within(dialog).getByText("1 listed")).toBeInTheDocument();
  });

  it("a_write_while_show_more_reads_starts_every_page_again", async () => {
    // The 200 listed were read before the write, so they are not added to: both pages are read again for the case as
    // it is now (one host more at the front, every page moved by one), and the held answer from before is dropped.
    const host = (i: number) => ({ kind: "host", id: 1000 + i, name: `HOST${String(i).padStart(3, "0")}`, role: "", note: "" });
    const hosts = (from: number, n: number) => Array.from({ length: n }, (_, i) => host(from + i));
    const before = held<unknown>();
    let written = false;
    overrideCommand("records_list", (a) => {
      const { slot, offset } = a as { slot: string; offset: number };
      if (slot !== "dialog:link") return import("../mock/backend").then((m) => m.mockCall("records_list", a as Record<string, unknown>));
      if (!written) return offset === 0 ? Promise.resolve({ items: hosts(0, 200), truncated: true }) : before.promise;
      return Promise.resolve(
        offset === 0 ? { items: [{ kind: "host", id: 9, name: "AAA-NEW", role: "", note: "" }, ...hosts(0, 199)], truncated: true } : { items: hosts(199, 2), truncated: false },
      );
    });
    const user = await openTimeline();
    const { list, pending } = await showMoreHeld(user);
    written = true;
    await act(async () => {
      await mockCall("record_upsert", { spec: { kind: "host", id: null, name: "AAA-NEW", role: "", note: "" } });
    });
    await waitFor(() => expect(within(list).getAllByRole("option")).toHaveLength(202));
    const names = () => within(list).getAllByRole("option").map((o) => o.textContent);
    expect(names()).toEqual(["AAA-NEW", ...hosts(0, 201).map((h) => h.name)]);
    expect(pickerReads().map((a) => a.offset)).toEqual([0, 200, 0, 200]);
    expect(cancelled()).toContain(pending.queryId);
    await act(async () => before.release({ items: [{ kind: "host", id: 7, name: "STALE", role: "", note: "" }], truncated: false }));
    expect(names()).toHaveLength(202);
    expect(names()).not.toContain("STALE");
  });

  it("closing_the_picker_cancels_a_show_more_read", async () => {
    heldSecondPage();
    const user = await openTimeline();
    const { dialog, pending } = await showMoreHeld(user);
    expect(cancelled()).not.toContain(pending.queryId);
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(cancelled()).toContain(pending.queryId));
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("the candidates after M", () => {
  it("candidates_scan_every_evidence_row", async () => {
    // 25 evidence rows; only the last names an account. Every row is read, one at a time, and its offer is shown.
    const rows: Evidence[] = Array.from({ length: 25 }, (_, i) => ({
      row_id: 100 + i, relation: "mock_events", host: "WS01", standing: "unchanged", summary: `row ${100 + i}`,
      attached_by: "analyst:analyst01", attached_at: "2026-01-10T12:00:00.000000Z",
    }));
    overrideCommand("evidence_list", async () => ({ items: rows, truncated: false }));
    overrideCommand("candidates", async (a) =>
      (a as { rowId: number }).rowId === 124
        ? [{ kind: "account", ioc_type: null, value: "late_user", domain: null, sid: null, column: "UserName", existing_id: null }]
        : [],
    );
    const { editor } = await promoteRow3();
    const panel = await within(editor).findByRole("region", { name: "Candidates" });
    expect(await within(panel).findByRole("button", { name: "Create and link account late_user" })).toBeInTheDocument();
    expect(callsOf("candidates").map((c) => (c.args as { rowId: number }).rowId)).toEqual(rows.map((r) => r.row_id));
  });

  it("candidates_link_on_one_click", async () => {
    const offers: Candidate[] = [
      { kind: "account", ioc_type: null, value: "jdoe", domain: "ACME", sid: null, column: "UserName", existing_id: 1 },
      { kind: "ioc", ioc_type: "ip", value: "198.51.100.9", domain: null, sid: null, column: "SourceIp", existing_id: null },
    ];
    overrideCommand("candidates", async () => offers);
    const user = userEvent.setup();
    render(<Shell />);
    await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
    await screen.findByRole("tab", { name: "Overview" });
    await user.click(within(screen.getByRole("complementary", { name: "Case" })).getByRole("button", { name: /^mock_events/ }));
    await screen.findByRole("tab", { name: /mock_events/, selected: true });
    const grid = await screen.findByRole("grid", { name: "Rows" });
    await waitFor(() => expect(within(grid).getAllByRole("row").length).toBeGreaterThan(10));
    const row = within(grid).getAllByRole("row").find((r) => r.getAttribute("data-row-index") === "3")!;
    await user.click(within(row).getAllByRole("gridcell")[2]);
    await user.keyboard("m");
    const editor = await within(rightPane()).findByRole("region", { name: "Entry E-5" });
    const panel = await within(editor).findByRole("region", { name: "Candidates" });
    // An account the case has: one click links it, nothing is created.
    await user.click(await within(panel).findByRole("button", { name: "Link account ACME\\jdoe" }));
    await waitFor(() => expect(callsOf("record_link").map((c) => c.args)).toEqual([{ entryId: 5, kind: "account", recordId: 1 }]));
    expect(callsOf("record_upsert")).toHaveLength(0);
    // An IOC it has not: one click creates it, then links the new record.
    await user.click(within(panel).getByRole("button", { name: "Create and link IOC ip 198.51.100.9" }));
    await waitFor(() => expect(callsOf("record_link")).toHaveLength(2));
    expect(callsOf("record_upsert").map((c) => c.args)).toEqual([
      { spec: { kind: "ioc", id: null, ioc_type: "ip", value: "198.51.100.9", tool: null, attack_ids: [], note: "" } },
    ]);
    expect(callsOf("record_link")[1].args).toEqual({ entryId: 5, kind: "ioc", recordId: 4 });
    expect(screen.queryByRole("dialog")).toBeNull();
    // The links show in the editor once the case re-reads.
    const linked = within(editor).getByRole("region", { name: "Linked records" });
    await waitFor(() => expect(linked.textContent).toContain("ip 198.51.100.9"));
    expect(linked.textContent).toContain("ACME\\jdoe");
  });
});
