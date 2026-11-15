// The Hosts, Accounts and IOCs sheets: the engine's records and their computed columns drawn as given, sort, the filter
// row and grouping (IOCs by tool is the tools view), the IOC type strip, the record editor (every field, linked entries,
// history, Delete and Merge into…), IOC sightings and Sweep all, and the account logon summary. Against the mock
// backend through the real `api.ts`, in the whole shell, so a write's `case-changed` re-reads the sheet as the app does.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AccountRecord } from "../contract/generated/AccountRecord";
import type { CaseRecord } from "../contract/generated/CaseRecord";
import type { FilterNode } from "../contract/generated/FilterNode";
import type { GridState } from "../contract/generated/GridState";
import type { HostRecord } from "../contract/generated/HostRecord";
import type { IocRecord } from "../contract/generated/IocRecord";
import type { LogonSummary } from "../contract/generated/LogonSummary";
import type { MergedSource } from "../contract/generated/MergedSource";
import type { RecordQuery } from "../contract/generated/RecordQuery";
import type { Sightings } from "../contract/generated/Sightings";
import { HOSTILE_RECORD } from "../mock/generated";
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
const base = {
  status: "reviewed" as const, created_by: "analyst:analyst01", created_at: "2026-01-10T12:00:00.000000Z", modified_by: "analyst:analyst01",
  modified_at: "2026-01-10T12:00:00.000000Z", agent_added: false, deleted: false, entries: 0, first_seen: null, last_seen: null, first_seen_shown: null,
  last_seen_shown: null,
};
const hostOf = (over: Partial<HostRecord> & { id: number; name: string }): CaseRecord => ({ kind: "host", ...base, role: "", note: "", ...over });
const accountOf = (over: Partial<AccountRecord> & { id: number; name: string }): CaseRecord => ({
  kind: "account", ...base, domain: null, sid: null, description: "", privilege: "standard", note: "", hosts_seen: [], ...over,
});
const iocOf = (over: Partial<IocRecord> & { id: number; value: string }): CaseRecord => ({
  kind: "ioc", ...base, ioc_type: "other", tool: null, attack_ids: [], note: "", hosts_seen: [], ...over,
});
const sightingsOf = (over: Partial<Sightings> = {}): Sightings => ({ hits: 0, relations: [], hosts: [], first: null, last: null, ...over });

type Sheet = "Hosts" | "Accounts" | "IOCs";
const GRID: Record<Sheet, string> = { Hosts: "Host records", Accounts: "Account records", IOCs: "IOC records" };
const rightPane = () => screen.getByRole("complementary", { name: "Row detail" });
const grid = (s: Sheet) => screen.getByRole("grid", { name: GRID[s] });
const recordRows = (s: Sheet) => within(grid(s)).getAllByRole("row").filter((r) => r.getAttribute("data-record") !== null);
const idsDrawn = (s: Sheet) => recordRows(s).map((r) => Number(r.getAttribute("data-record")));
const rowOf = (s: Sheet, id: number) => recordRows(s).find((r) => r.getAttribute("data-record") === String(id))!;
const cellOf = (s: Sheet, row: HTMLElement, name: string) => {
  const headers = within(grid(s)).getAllByRole("columnheader");
  const at = headers.findIndex((h) => h.textContent === name || h.textContent?.startsWith(name));
  if (at < 0) throw new Error(`no header ${name}`);
  return within(row).getAllByRole("gridcell")[at];
};
/** The `records_list` queries the sheet itself made (not the shell's counts, the pickers or the sweep). */
const sheetQueries = (s: Sheet) =>
  callsOf("records_list")
    .filter((c) => (c.args as { slot: string }).slot === `sheet:${s.toLowerCase()}`)
    .map((c) => (c.args as { query: RecordQuery }).query);

/** The shell with the recent case open and sheet `s` shown, its records drawn. */
async function openSheet(s: Sheet, expectRows = true) {
  const user = userEvent.setup();
  render(<Shell />);
  await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
  await screen.findByRole("tab", { name: "Overview" });
  await user.click(screen.getByRole("tab", { name: new RegExp(`^${s}`) }));
  await screen.findByRole("grid", { name: GRID[s] });
  if (expectRows) await waitFor(() => expect(recordRows(s).length).toBeGreaterThan(0));
  return user;
}

const PREFIX: Record<Sheet, string> = { Hosts: "Host H", Accounts: "Account A", IOCs: "IOC I" };
/** Selects record `id` of sheet `s`; its editor is in the right pane. */
async function select(user: ReturnType<typeof userEvent.setup>, s: Sheet, id: number) {
  await user.click(within(rowOf(s, id)).getAllByRole("gridcell")[0]);
  return within(rightPane()).findByRole("region", { name: `${PREFIX[s]}-${id}` });
}

/** A promise a test settles by hand: holds a backend answer back. */
function held<T>() {
  let release!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((r, j) => {
    release = r;
    reject = j;
  });
  return { promise, release, reject };
}

const realCall = (cmd: string, a: unknown) => import("../mock/backend").then((m) => m.mockCall(cmd, a as Record<string, unknown>));

// ---- the grids ----------------------------------------------------------------------------------------------------------
describe("the record sheets", () => {
  it("records_list_with_computed_columns", async () => {
    const hosts = [
      hostOf({
        id: 4, name: "WS01", role: "workstation", entries: 3, first_seen: "2026-01-05T02:14:07.000000Z", last_seen: "2026-01-05T02:20:00.000000Z",
        first_seen_shown: "2026-01-04 21:14:07.000000", last_seen_shown: "2026-01-04 21:20:00.000000",
      }),
      hostOf({ id: 5, name: "SRV01", status: "unreviewed", agent_added: true, created_by: "agent:triage-bot" }),
    ];
    const accounts = [
      accountOf({
        id: 7, name: "jdoe", domain: "ACME", sid: "S-1-5-21-1-2-3-1104", privilege: "domain_admin", entries: 2, hosts_seen: ["SRV01", "WS01"],
        first_seen: "2026-01-05T02:14:07.000000Z", last_seen: "2026-01-05T02:15:00.000000Z", first_seen_shown: "2026-01-04 21:14:07.000000",
        last_seen_shown: "2026-01-04 21:15:00.000000",
      }),
    ];
    const iocs = [
      iocOf({ id: 9, ioc_type: "ip", value: "203.0.113.7", tool: "Cobalt Strike", entries: 1, hosts_seen: ["WS01"], first_seen: "2026-01-05T02:14:07.000000Z", last_seen: "2026-01-05T02:14:07.000000Z", status: "unreviewed", agent_added: true, created_by: "agent:triage-bot" }),
    ];
    overrideCommand("records_list", async (a) => {
      const kind = (a as { query: RecordQuery }).query.kind;
      return { items: kind === "host" ? hosts : kind === "account" ? accounts : iocs, truncated: false };
    });
    const user = await openSheet("Hosts");
    // The engine's order, as given (SRV01 after WS01 here: a sheet that re-sorted by name is caught).
    expect(idsDrawn("Hosts")).toEqual([4, 5]);
    const ws = rowOf("Hosts", 4);
    expect(cellOf("Hosts", ws, "Name").textContent).toBe("WS01");
    expect(cellOf("Hosts", ws, "Role").textContent).toBe("workstation");
    expect(cellOf("Hosts", ws, "Entries").textContent).toBe("3");
    // The engine's text for the time in the case's zone; the header names the zone.
    expect(cellOf("Hosts", ws, "First seen").textContent).toBe("2026-01-04 21:14:07.000000");
    expect(cellOf("Hosts", ws, "Last seen").textContent).toBe("2026-01-04 21:20:00.000000");
    expect(within(grid("Hosts")).getAllByRole("columnheader").map((h) => h.textContent)).toContain("First seen (UTC)");
    expect(cellOf("Hosts", ws, "Status").textContent).toBe("reviewed");
    // An agent's unreviewed record says so in the agent colour.
    const agent = cellOf("Hosts", rowOf("Hosts", 5), "Status");
    expect(agent.textContent).toBe("agent · review");
    expect(agent.querySelector(".status-agent")).not.toBeNull();
    expect(cellOf("Hosts", rowOf("Hosts", 5), "First seen").textContent).toBe("");

    await user.click(screen.getByRole("tab", { name: /^Accounts/ }));
    await waitFor(() => expect(recordRows("Accounts")).toHaveLength(1));
    const jdoe = rowOf("Accounts", 7);
    expect(cellOf("Accounts", jdoe, "Account").textContent).toBe("ACME\\jdoe");
    expect(cellOf("Accounts", jdoe, "SID").textContent).toBe("S-1-5-21-1-2-3-1104");
    expect(cellOf("Accounts", jdoe, "Privilege").textContent).toBe("domain admin");
    expect(cellOf("Accounts", jdoe, "Entries").textContent).toBe("2");
    expect(cellOf("Accounts", jdoe, "Hosts seen").textContent).toBe("SRV01, WS01");
    expect(cellOf("Accounts", jdoe, "Last seen").textContent).toBe("2026-01-04 21:15:00.000000");
    // The editor shows the same, beside the entries they come from.
    overrideCommand("record_get", async () => accounts[0]);
    const account = await select(user, "Accounts", 7);
    await waitFor(() => expect(within(account).getByLabelText("From its entries").textContent).toContain("First seen2026-01-04 21:14:07.000000"));
    expect(within(account).getByLabelText("From its entries").textContent).toContain("Last seen2026-01-04 21:15:00.000000");

    await user.click(screen.getByRole("tab", { name: /^IOCs/ }));
    await waitFor(() => expect(recordRows("IOCs")).toHaveLength(1));
    const ip = rowOf("IOCs", 9);
    expect(cellOf("IOCs", ip, "Value").textContent).toBe("203.0.113.7");
    expect(cellOf("IOCs", ip, "Type").textContent).toBe("IP");
    expect(cellOf("IOCs", ip, "Tool").textContent).toBe("Cobalt Strike");
    expect(cellOf("IOCs", ip, "Hosts seen").textContent).toBe("WS01");
    expect(cellOf("IOCs", ip, "Sightings").textContent).toBe("");
    expect(cellOf("IOCs", ip, "Status").textContent).toBe("agent · review");
    // Nothing D21 leaves out: no verdict, flag, VT or severity column.
    const headers = within(grid("IOCs")).getAllByRole("columnheader").map((h) => h.textContent ?? "");
    expect(headers.some((h) => /verdict|flag|VT|severity/i.test(h))).toBe(false);
  });

  it("sort_filter_row_and_the_tools_view", async () => {
    const iocs = [
      iocOf({ id: 1, value: "b.exe", tool: "Mimikatz", entries: 1 }),
      iocOf({ id: 2, value: "a.exe", tool: null, entries: 5 }),
      iocOf({ id: 3, value: "c.exe", tool: "Cobalt Strike", entries: 3 }),
      iocOf({ id: 4, value: "d.exe", tool: "Mimikatz", entries: 2 }),
    ];
    overrideCommand("records_list", async (a) => ((a as { query: RecordQuery }).query.kind === "ioc" ? { items: iocs, truncated: false } : realCall("records_list", a)));
    const user = await openSheet("IOCs");
    expect(idsDrawn("IOCs")).toEqual([1, 2, 3, 4]);
    // A header asks the engine for its order (ascending, descending, then none) and draws the answer as given: the
    // answer here is not sorted by entries, so a sheet that sorted it again is caught.
    const entries = within(grid("IOCs")).getByRole("button", { name: /^Entries/ });
    const before = sheetQueries("IOCs").length;
    await user.click(entries);
    await waitFor(() => expect(sheetQueries("IOCs").at(-1)).toEqual({ kind: "ioc", ioc_type: null, tool: null, text: null, order: { by: "entries", descending: false } }));
    expect(entries.closest("th")).toHaveAttribute("aria-sort", "ascending");
    expect(sheetQueries("IOCs").length).toBeGreaterThan(before);
    expect(idsDrawn("IOCs")).toEqual([1, 2, 3, 4]);
    await user.click(entries);
    await waitFor(() => expect(sheetQueries("IOCs").at(-1)?.order).toEqual({ by: "entries", descending: true }));
    await user.click(entries);
    await waitFor(() => expect(sheetQueries("IOCs").at(-1)?.order).toBeNull());
    await user.click(within(grid("IOCs")).getByRole("button", { name: /^Value/ }));
    await waitFor(() => expect(sheetQueries("IOCs").at(-1)?.order).toEqual({ by: "value", descending: false }));
    // An order is read from the first page.
    const orderedReads = callsOf("records_list").filter((c) => (c.args as { slot: string }).slot === "sheet:iocs" && (c.args as { query: RecordQuery & { order?: unknown } }).query.order != null);
    expect(orderedReads.every((c) => (c.args as { offset: number }).offset === 0)).toBe(true);
    // A column the engine does not order by has no sort button.
    expect(within(grid("IOCs")).queryByRole("button", { name: /^Tool/ })).toBeNull();

    // The filter row goes to the engine: the value text and the tool.
    await user.type(screen.getByRole("textbox", { name: "Filter Value" }), "exe");
    await waitFor(() => expect(sheetQueries("IOCs").at(-1)).toMatchObject({ kind: "ioc", ioc_type: null, tool: null, text: "exe" }));
    await user.type(screen.getByRole("textbox", { name: "Filter Tool" }), "Mimikatz");
    await waitFor(() => expect(sheetQueries("IOCs").at(-1)).toMatchObject({ kind: "ioc", ioc_type: null, tool: "Mimikatz", text: "exe" }));

    // Grouping by tool is the tools view: the groups in the order their tools first appear in what is shown.
    const reads = sheetQueries("IOCs").length;
    await user.selectOptions(screen.getByRole("combobox", { name: "Group by" }), "tool");
    const groups = () => within(grid("IOCs")).getAllByRole("row").filter((r) => r.getAttribute("data-group") !== null);
    await waitFor(() => expect(groups()).toHaveLength(3));
    expect(groups().map((g) => g.getAttribute("data-group"))).toEqual(["Mimikatz", "no tool", "Cobalt Strike"]);
    expect(groups()[0].textContent).toContain("2");
    expect(idsDrawn("IOCs")).toEqual([1, 4, 2, 3]);
    expect(sheetQueries("IOCs")).toHaveLength(reads);
  });

  it("the_type_strip_counts_and_narrows", async () => {
    const user = await openSheet("IOCs");
    const strip = screen.getByRole("group", { name: "IOC types" });
    // The mock holds an IP, an `other` and a file name: every type the case has, with its count, and All.
    await waitFor(() => expect(within(strip).getAllByRole("button").map((b) => b.textContent)).toEqual(["All 3", "IP 1", "File name 1", "Other 1"]));
    await user.click(within(strip).getByRole("button", { name: "IP 1" }));
    await waitFor(() => expect(sheetQueries("IOCs").at(-1)).toEqual({ kind: "ioc", ioc_type: "ip", tool: null, text: null, order: null }));
    await waitFor(() => expect(idsDrawn("IOCs")).toEqual([1]));
    expect(within(strip).getByRole("button", { name: "IP 1" })).toHaveAttribute("aria-pressed", "true");
  });

  it("the_records_page_past_a_thousand", async () => {
    const all = Array.from({ length: 1001 }, (_, i) => hostOf({ id: i + 1, name: `WS${String(i + 1).padStart(4, "0")}` }));
    overrideCommand("records_list", async (a) => {
      const { limit, offset, query } = a as { limit: number; offset: number; query: RecordQuery };
      if (query.kind !== "host") return realCall("records_list", a);
      return { items: all.slice(offset, offset + limit), truncated: offset + limit < all.length };
    });
    const user = await openSheet("Hosts");
    await waitFor(() => expect(recordRows("Hosts")).toHaveLength(1000));
    expect(screen.getByText("1,000 hosts shown · more in the case")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Show more" }));
    await waitFor(() => expect(recordRows("Hosts")).toHaveLength(1001));
    expect(screen.getByText("1,001 hosts shown")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Show more" })).toBeNull();
  });

  it("hostile_record_values_render_as_text", async () => {
    const user = await openSheet("IOCs");
    expect(grid("IOCs").querySelector("img")).toBeNull();
    expect(cellOf("IOCs", rowOf("IOCs", 2), "Value").textContent).toBe("<img src=x onerror=1>");
    expect(cellOf("IOCs", rowOf("IOCs", 3), "Value").textContent).toBe("invoice\\u{202E}fdp.exe");
    const editor = await select(user, "IOCs", 3);
    // An input holds the raw value (it is text to React); everything drawn is escaped.
    await waitFor(() => expect(within(editor).getByRole("textbox", { name: "Value" })).toHaveValue(HOSTILE_RECORD[2]));
    expect(within(editor).getByRole("textbox", { name: "Note" })).toHaveValue(HOSTILE_RECORD[4]);
    expect(within(editor).getByText(/invoice\\u\{202E\}fdp\.exe/, { selector: ".detail-rel" })).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: /^Accounts/ }));
    await waitFor(() => expect(recordRows("Accounts").length).toBe(2));
    const account = await select(user, "Accounts", 1);
    await waitFor(() => expect(within(account).getByRole("textbox", { name: "Description" })).toHaveValue(HOSTILE_RECORD[0]));
    await user.click(screen.getByRole("tab", { name: /^Hosts/ }));
    const host = await select(user, "Hosts", 2);
    await waitFor(() => expect(within(host).getByRole("textbox", { name: "Note" })).toHaveValue(HOSTILE_RECORD[2]));
    expect(document.querySelector("img")).toBeNull();
  });
});

// ---- the editor ---------------------------------------------------------------------------------------------------------
describe("the record editor", () => {
  it("edits_send_every_field", async () => {
    const user = await openSheet("Hosts");
    const editor = await select(user, "Hosts", 1);
    const role = await within(editor).findByRole("textbox", { name: "Role" });
    await user.type(role, "workstation");
    await user.click(within(editor).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(callsOf("record_upsert")).toHaveLength(1));
    expect(callsOf("record_upsert")[0].args).toEqual({ spec: { kind: "host", id: 1, name: "WS01", role: "workstation", note: "" } });
    await waitFor(() => expect(cellOf("Hosts", rowOf("Hosts", 1), "Role").textContent).toBe("workstation"));

    // An account sends every field, the privilege from its list.
    await user.click(screen.getByRole("tab", { name: /^Accounts/ }));
    const account = await select(user, "Accounts", 2);
    await user.selectOptions(await within(account).findByRole("combobox", { name: "Privilege" }), "local_admin");
    await user.click(within(account).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(callsOf("record_upsert")).toHaveLength(2));
    expect(callsOf("record_upsert")[1].args).toEqual({
      spec: { kind: "account", id: 2, name: "svc_backup", domain: null, sid: null, description: "", privilege: "local_admin", note: HOSTILE_RECORD[4] },
    });
  });

  it("an_engine_refusal_is_shown_in_place_without_its_code", async () => {
    overrideCommand("record_upsert", async () => {
      throw { code: "refused", message: "H-1 is the host of the evidence on E-1: it keeps its name" };
    });
    const user = await openSheet("Hosts");
    const editor = await select(user, "Hosts", 1);
    const name = await within(editor).findByRole("textbox", { name: "Name" });
    await user.type(name, "X");
    await user.click(within(editor).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(within(editor).getByText("H-1 is the host of the evidence on E-1: it keeps its name")).toBeInTheDocument());
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("merge_and_delete_ask_once_and_refresh", async () => {
    const user = await openSheet("Hosts");
    expect(idsDrawn("Hosts")).toEqual([2, 1]);
    // Delete asks once, then deletes once; the sheet re-reads and the record is gone.
    const host = await select(user, "Hosts", 2);
    await user.click(within(host).getByRole("button", { name: "Delete" }));
    const ask = screen.getByRole("dialog", { name: "Delete H-2" });
    expect(ask.textContent).toContain("SRV01");
    expect(callsOf("record_delete")).toHaveLength(0);
    await user.click(within(ask).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(callsOf("record_delete").map((c) => c.args)).toEqual([{ kind: "host", id: 2 }]);
    await waitFor(() => expect(idsDrawn("Hosts")).toEqual([1]));

    // Merge into… asks once (the record it goes into), merges once, and the editor follows the merge.
    await user.click(screen.getByRole("tab", { name: /^Accounts/ }));
    const from = await select(user, "Accounts", 2);
    await user.click(within(from).getByRole("button", { name: "Merge into…" }));
    const dialog = screen.getByRole("dialog", { name: "Merge A-2 into…" });
    const merge = within(dialog).getByRole("button", { name: "Merge" });
    expect(merge).toBeDisabled();
    // The record itself is not offered.
    await waitFor(() => expect(within(dialog).getAllByRole("option").map((o) => o.textContent)).toEqual(["A-1ACME\\jdoe"]));
    await user.click(within(dialog).getByRole("option"));
    expect(within(dialog).getByRole("option")).toHaveAttribute("aria-selected", "true");
    await user.click(merge);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(callsOf("record_merge").map((c) => c.args)).toEqual([{ kind: "account", fromId: 2, intoId: 1 }]);
    await waitFor(() => expect(idsDrawn("Accounts")).toEqual([1]));
    await within(rightPane()).findByRole("region", { name: "Account A-1" });
  });

  it("a_refused_merge_shows_the_engines_reason", async () => {
    overrideCommand("record_merge", async () => {
      throw { code: "refused", message: "H-2 holds the evidence of E-2 from SRV01: it is not merged" };
    });
    const user = await openSheet("Hosts");
    const host = await select(user, "Hosts", 2);
    await user.click(within(host).getByRole("button", { name: "Merge into…" }));
    const dialog = screen.getByRole("dialog", { name: "Merge H-2 into…" });
    await user.click(await within(dialog).findByRole("option"));
    await user.click(within(dialog).getByRole("button", { name: "Merge" }));
    await waitFor(() => expect(within(dialog).getByRole("alert").textContent).toBe("H-2 holds the evidence of E-2 from SRV01: it is not merged"));
    expect(idsDrawn("Hosts")).toEqual([2, 1]);
  });

  it("linked_entries_and_history_are_listed", async () => {
    const user = await openSheet("Accounts");
    const editor = await select(user, "Accounts", 1);
    const linked = await within(editor).findByRole("region", { name: "Linked entries" });
    await waitFor(() => expect(within(linked).getByText("Failed logon burst from 203.0.113.7")).toBeInTheDocument());
    // History is read only when opened, for this record.
    expect(callsOf("history_list").filter((c) => (c.args as { targetKind: string }).targetKind === "account")).toHaveLength(0);
    await user.click(within(editor).getByRole("button", { name: /^History/ }));
    await waitFor(() => expect(callsOf("history_list").some((c) => {
      const a = c.args as { targetKind: string; targetId: string };
      return a.targetKind === "account" && a.targetId === "1";
    })).toBe(true));
    // open → shows the entry in the Timeline.
    await user.click(within(linked).getByRole("button", { name: "Open E-1" }));
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Timeline/ })).toHaveAttribute("aria-selected", "true"));
    await within(rightPane()).findByRole("region", { name: "Entry E-1" });
  });

  it("failed_entry_and_history_reads_are_shown", async () => {
    const user = await openSheet("Accounts");
    // A later page that fails: the first page read, then Show more fails.
    const one = (await realCall("entry_get", { id: 1, slot: "x", queryId: "q" })) as object;
    const first = Array.from({ length: 100 }, (_, i) => ({ ...one, id: i + 1 }));
    overrideCommand("record_entries", async (a) => {
      if ((a as { offset: number }).offset === 0) return { items: first, truncated: true };
      throw { code: "engine", message: "the case could not be read" };
    });
    overrideCommand("history_list", async () => {
      throw { code: "io", message: "history unreadable" };
    });
    const editor = await select(user, "Accounts", 1);
    const linked = await within(editor).findByRole("region", { name: "Linked entries" });
    await user.click(await within(linked).findByRole("button", { name: "Show more" }));
    await waitFor(() => expect(within(linked).getByRole("alert").textContent).toBe("engine: the case could not be read"));
    await user.click(within(editor).getByRole("button", { name: /^History/ }));
    const history = within(editor).getByRole("region", { name: "Record history" });
    await waitFor(() => expect(within(history).getByRole("alert").textContent).toBe("io: history unreadable"));
  });

  it("the_merge_target_goes_when_the_search_changes", async () => {
    const user = await openSheet("Hosts");
    const host = await select(user, "Hosts", 2);
    await user.click(within(host).getByRole("button", { name: "Merge into…" }));
    const dialog = screen.getByRole("dialog", { name: "Merge H-2 into…" });
    await user.click(await within(dialog).findByRole("option"));
    const merge = within(dialog).getByRole("button", { name: "Merge" });
    expect(merge).toBeEnabled();
    // Typing hides WS01: nothing is chosen any more, so nothing can be merged into it unseen.
    await user.type(within(dialog).getByRole("searchbox", { name: "Filter records" }), "zzz");
    await waitFor(() => expect(within(dialog).queryAllByRole("option")).toHaveLength(0));
    expect(merge).toBeDisabled();
    await user.clear(within(dialog).getByRole("searchbox", { name: "Filter records" }));
    await waitFor(() => expect(within(dialog).getAllByRole("option")).toHaveLength(1));
    expect(within(dialog).getByRole("option")).toHaveAttribute("aria-selected", "false");
    expect(merge).toBeDisabled();
    expect(callsOf("record_merge")).toHaveLength(0);
  });

  it("open_from_the_timeline_selects_the_record", async () => {
    const user = await openSheet("Hosts");
    await user.click(screen.getByRole("tab", { name: /^Timeline/ }));
    const row = await waitFor(() => {
      const r = within(screen.getByRole("grid", { name: "Timeline entries" })).getAllByRole("row").find((x) => x.getAttribute("data-entry") === "1");
      expect(r).toBeDefined();
      return r!;
    });
    await user.click(within(row).getAllByRole("gridcell")[3]);
    const entry = await within(rightPane()).findByRole("region", { name: "Entry E-1" });
    await user.click(await within(entry).findByRole("button", { name: "Open Account ACME\\jdoe" }));
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Accounts/ })).toHaveAttribute("aria-selected", "true"));
    await within(rightPane()).findByRole("region", { name: "Account A-1" });
    await waitFor(() => expect(rowOf("Accounts", 1)).toHaveAttribute("aria-selected", "true"));
  });

  it("a_late_record_answer_is_not_drawn", async () => {
    const user = await openSheet("Hosts");
    const old = held<unknown>();
    overrideCommand("record_get", (a) => ((a as { id: number }).id === 2 ? old.promise : realCall("record_get", a)));
    await user.click(within(rowOf("Hosts", 2)).getAllByRole("gridcell")[0]);
    await within(rightPane()).findByRole("region", { name: "Host H-2" });
    const editor = await select(user, "Hosts", 1);
    await waitFor(() => expect(within(editor).getByRole("textbox", { name: "Name" })).toHaveValue("WS01"));
    await act(async () => old.release(hostOf({ id: 2, name: "Stale answer" })));
    expect(within(rightPane()).queryByRole("region", { name: "Host H-2" })).toBeNull();
    expect(within(rightPane()).getByRole("textbox", { name: "Name" })).toHaveValue("WS01");
  });
});

// ---- sightings ----------------------------------------------------------------------------------------------------------
describe("IOC sightings", () => {
  it("sightings_not_found_is_shown", async () => {
    overrideCommand("ioc_sightings", async () => sightingsOf());
    const user = await openSheet("IOCs");
    const editor = await select(user, "IOCs", 1);
    const panel = within(editor).getByRole("region", { name: "Sightings" });
    await user.click(within(panel).getByRole("button", { name: "Find sightings" }));
    await waitFor(() => expect(panel.textContent).toContain("not found in the loaded data"));
    expect(within(panel).queryByRole("button", { name: "Show hits" })).toBeNull();
    expect(cellOf("IOCs", rowOf("IOCs", 1), "Sightings").textContent).toBe("not found");
    expect(callsOf("ioc_sightings").map((c) => (c.args as { slot: string; id: number }))).toMatchObject([{ slot: "sightings", id: 1 }]);
  });

  it("sightings_show_hits_and_open_the_merged_tab", async () => {
    overrideCommand("ioc_sightings", async () =>
      sightingsOf({ hits: 14, relations: [["mock_events", 11], ["mock_hayabusa", 3]], hosts: ["SRV01", "WS01"], first: "2026-01-05 02:14:07", last: "2026-01-05 02:40:00" }),
    );
    const user = await openSheet("IOCs");
    const editor = await select(user, "IOCs", 1);
    const panel = within(editor).getByRole("region", { name: "Sightings" });
    await user.click(within(panel).getByRole("button", { name: "Find sightings" }));
    await waitFor(() => expect(panel.textContent).toContain("14 hits"));
    expect(panel.textContent).toContain("mock_events · 11");
    expect(panel.textContent).toContain("mock_hayabusa · 3");
    expect(panel.textContent).toContain("SRV01, WS01");
    expect(panel.textContent).toContain("first sighting2026-01-05 02:14:07");
    expect(panel.textContent).toContain("last sighting2026-01-05 02:40:00");
    expect(cellOf("IOCs", rowOf("IOCs", 1), "Sightings").textContent).toBe("14");
    await user.click(within(panel).getByRole("button", { name: "Show hits" }));
    const tab = await screen.findByRole("tab", { name: /^Hits: 203\.0\.113\.7/ });
    expect(tab).toHaveAttribute("aria-selected", "true");
    await waitFor(() => expect(callsOf("merged_page").length).toBeGreaterThan(0));
    const source = (callsOf("merged_page").at(-1)!.args as { source: MergedSource }).source;
    expect(source).toEqual({ find: { value: "203.0.113.7", mode: "exact", hosts: [], from: null, to: null } });
  });

  it("a_refused_sighting_shows_the_reason", async () => {
    overrideCommand("ioc_sightings", async () => {
      throw { code: "invalid", message: "a value over 1,000 characters is not searched" };
    });
    const user = await openSheet("IOCs");
    const editor = await select(user, "IOCs", 2);
    const panel = within(editor).getByRole("region", { name: "Sightings" });
    await user.click(within(panel).getByRole("button", { name: "Find sightings" }));
    await waitFor(() => expect(panel.textContent).toContain("a value over 1,000 characters is not searched"));
    expect(panel.textContent).not.toContain("invalid");
  });

  it("sweep_all_runs_one_ioc_at_a_time_and_cancels", async () => {
    const answers: { id: number; queryId: string; answer: ReturnType<typeof held<Sightings>> }[] = [];
    let inFlight = 0;
    let most = 0;
    overrideCommand("ioc_sightings", (a) => {
      const { id, queryId } = a as { id: number; queryId: string };
      const answer = held<Sightings>();
      answers.push({ id, queryId, answer });
      inFlight += 1;
      most = Math.max(most, inFlight);
      return answer.promise.finally(() => (inFlight -= 1));
    });
    const user = await openSheet("IOCs");
    await user.click(screen.getByRole("button", { name: "Sweep all" }));
    await waitFor(() => expect(answers).toHaveLength(1));
    const progress = screen.getByRole("status", { name: "Sweep" });
    expect(progress.textContent).toContain("0 of 3");
    expect(answers[0].id).toBe(1);
    await act(async () => answers[0].answer.release(sightingsOf({ hits: 2 })));
    await waitFor(() => expect(answers).toHaveLength(2));
    expect(answers[1].id).toBe(2);
    expect(most).toBe(1);
    await waitFor(() => expect(progress.textContent).toContain("1 of 3"));
    expect(cellOf("IOCs", rowOf("IOCs", 1), "Sightings").textContent).toBe("2");
    // Every sweep query runs on the one sightings slot.
    expect(callsOf("ioc_sightings").every((c) => (c.args as { slot: string }).slot === "sightings")).toBe(true);

    await user.click(screen.getByRole("button", { name: "Cancel sweep" }));
    await waitFor(() => expect(callsOf("query_cancel").some((c) => (c.args as { queryIds: string[] }).queryIds.includes(answers[1].queryId))).toBe(true));
    await act(async () => answers[1].answer.release(sightingsOf({ hits: 99 })));
    expect(answers).toHaveLength(2);
    expect(cellOf("IOCs", rowOf("IOCs", 2), "Sightings").textContent).toBe("");
    expect(screen.queryByRole("status", { name: "Sweep" })).toBeNull();
    expect(screen.getByRole("button", { name: "Sweep all" })).toBeEnabled();
    // What the sweep found stays until the case changes.
    expect(cellOf("IOCs", rowOf("IOCs", 1), "Sightings").textContent).toBe("2");
  });

  it("sweep_results_go_with_the_next_case_change", async () => {
    overrideCommand("ioc_sightings", async () => sightingsOf({ hits: 4 }));
    const user = await openSheet("IOCs");
    await user.click(screen.getByRole("button", { name: "Sweep all" }));
    await waitFor(() => expect(cellOf("IOCs", rowOf("IOCs", 3), "Sightings").textContent).toBe("4"));
    expect(callsOf("ioc_sightings").map((c) => (c.args as { id: number }).id)).toEqual([1, 2, 3]);
    const editor = await select(user, "IOCs", 1);
    await user.type(await within(editor).findByRole("textbox", { name: "Tool" }), "x");
    await user.click(within(editor).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(cellOf("IOCs", rowOf("IOCs", 3), "Sightings").textContent).toBe(""));
  });
});

// ---- the logon summary --------------------------------------------------------------------------------------------------
describe("the logon summary", () => {
  const success: FilterNode = { and: [{ column: "EventId", op: "eq", value: "4624", case_sensitive: true }, { column: "LogonType", op: "regex", value: "^LogonType 0*10$", case_sensitive: true }] };
  const failure: FilterNode = { and: [{ column: "EventId", op: "eq", value: "4625", case_sensitive: true }] };
  const summary: LogonSummary = {
    rows: [
      { outcome: "success", logon_type: 10, host: "WS01", count: 3, filter: success },
      { outcome: "failure", logon_type: 10, host: "WS01", count: 2, filter: failure },
      { outcome: "success", logon_type: 3, host: null, count: 1, filter: success },
    ],
    unparsed: 4,
    source: "mock_events",
  };

  it("logon_numbers_open_their_rows", async () => {
    overrideCommand("logon_summary", async () => summary);
    const user = await openSheet("Accounts");
    const editor = await select(user, "Accounts", 1);
    const panel = within(editor).getByRole("region", { name: "Logon summary" });
    const table = await within(panel).findByRole("table", { name: "Logons" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows.map((r) => within(r).getAllByRole("cell").map((c) => c.textContent))).toEqual([
      ["WS01", "10", "3", "2"],
      ["no host", "3", "1", "0"],
    ]);
    expect(panel.textContent).toContain("4 logon events did not read as an account and a logon type");
    expect(panel.textContent).toContain("mock_events");
    await user.click(within(panel).getByRole("button", { name: "Open the 3 success logons, type 10, WS01" }));
    const label = "Logons: ACME\\jdoe · type 10 · WS01 · success";
    const tab = await screen.findByRole("tab", { name: new RegExp(`^${label.replace(/[\\.]/g, "\\$&")}`) });
    expect(tab).toHaveAttribute("aria-selected", "true");
    await waitFor(() => expect(callsOf("grid_page").some((c) => (c.args as { state: GridState }).state.chips.length === 1)).toBe(true));
    const state = (callsOf("grid_page").filter((c) => (c.args as { state: GridState }).state.chips.length === 1).at(-1)!.args as { state: GridState }).state;
    expect(state.relation).toBe("mock_events");
    expect(state.chips[0].filter).toEqual(success);
    // The chip reads as the account, not as the regex it holds.
    expect(state.chips[0].label).toBe(label);
    // The failures of the same type and host open their own rows.
    await user.click(screen.getByRole("tab", { name: /^Accounts/ }));
    await user.click(within(rightPane()).getByRole("button", { name: "Open the 2 failure logons, type 10, WS01" }));
    await screen.findByRole("tab", { name: /^Logons: ACME\\jdoe · type 10 · WS01 · failure/ });
    await waitFor(() => expect(callsOf("grid_page").some((c) => (c.args as { state: GridState }).state.chips[0]?.label.endsWith("failure"))).toBe(true));
    const failed = (callsOf("grid_page").filter((c) => (c.args as { state: GridState }).state.chips[0]?.label.endsWith("failure")).at(-1)!.args as { state: GridState }).state;
    expect(failed.relation).toBe("mock_events");
    expect(failed.chips[0].filter).toEqual(failure);
    expect(failed.chips[0].filter).not.toEqual(success);
  });

  it("no_event_logs_says_so", async () => {
    overrideCommand("logon_summary", async () => ({ rows: [], unparsed: 0, source: null }));
    const user = await openSheet("Accounts");
    const editor = await select(user, "Accounts", 1);
    const panel = within(editor).getByRole("region", { name: "Logon summary" });
    await waitFor(() => expect(panel.textContent).toContain("No event-log dataset in this case."));
    expect(within(panel).queryByRole("table")).toBeNull();
  });

  it("an_account_with_no_logons_says_so", async () => {
    overrideCommand("logon_summary", async () => ({ rows: [], unparsed: 0, source: "mock_events" }));
    const user = await openSheet("Accounts");
    const editor = await select(user, "Accounts", 1);
    const panel = within(editor).getByRole("region", { name: "Logon summary" });
    await waitFor(() => expect(panel.textContent).toContain("No logons of this account in mock_events."));
  });
});
