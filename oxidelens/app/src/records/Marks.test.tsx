// The mark keys on the grids (S, U, M, T, N, A, I), the three glyphs, the row detail's marks, tags, notes and linked
// entries, and the left pane's marked rows: per-state counts that open the worklist, and dataset names in the colour
// of their most severe mark. Against the mock backend, through the real `api.ts`; most tests mount the whole shell so a
// write's `case-changed` re-reads the grid, the pane and the summary as the app does.
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { createRef, useEffect, useImperativeHandle, useLayoutEffect, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { caseOpen, caseSummary, settingsGet } from "../api";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { ColumnLayout } from "../contract/generated/ColumnLayout";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";
import type { GridPage } from "../contract/generated/GridPage";
import type { GridState } from "../contract/generated/GridState";
import type { MergedSource } from "../contract/generated/MergedSource";
import Grid from "../grid/Grid";
import { initialGridState } from "../grid/gridState";
import { materialise, togglePin } from "../grid/layout";
import type { Entry } from "../contract/generated/Entry";
import { HAYABUSA_FIRST_ID, hayabusaRows } from "../mock/generated";
import { mockCall, setMockMarks } from "../mock/backend";
import { ShellCtx, type ShellContext } from "../shell/context";
import LeftPane from "../shell/LeftPane";
import Shell from "../shell/Shell";
import type { Tab } from "../shell/tabs";
import { callsOf, overrideCommand } from "../test/backend";

vi.mock("../grid/measure", () => ({ measureText: vi.fn(() => 50) }));
vi.mock("../dialogs", () => ({ pickNewCasePath: vi.fn(), pickCaseFile: vi.fn(), pickRunFolder: vi.fn(), pickCsvFile: vi.fn() }));

// jsdom has no layout: every element is 1200 x 440, and scrollTop is kept per element so a test can scroll.
const scrollTops = new WeakMap<Element, number>();
beforeEach(() => {
  localStorage.clear();
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 440 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "scrollTop", {
    configurable: true,
    get() {
      return scrollTops.get(this) ?? 0;
    },
    set(v: number) {
      scrollTops.set(this, v);
      queueMicrotask(() => (this as HTMLElement).dispatchEvent(new Event("scroll")));
    },
  });
});
afterEach(() => vi.restoreAllMocks());

const gridCss = readFileSync("src/grid/grid.css", "utf8");

// ---- helpers ------------------------------------------------------------------------------------------------------------
const dataRows = () => screen.queryAllByRole("row").filter((r) => r.getAttribute("data-row-index") !== null);
const rowOf = (i: number) => dataRows().find((r) => r.getAttribute("data-row-index") === String(i))!;
const header = (name: string) => {
  const h = screen.getAllByRole("columnheader").find((el) => el.querySelector(".col-name")?.textContent === name);
  if (!h) throw new Error(`no header ${name}`);
  return h;
};
const cellOf = (row: HTMLElement, name: string) => {
  const at = Number(header(name).getAttribute("aria-colindex"));
  const c = within(row).getAllByRole("gridcell").find((el) => Number(el.getAttribute("aria-colindex")) === at);
  if (!c) throw new Error(`no cell ${name}`);
  return c;
};
const markOf = (row: HTMLElement) => within(row).getAllByRole("gridcell")[0];
const lastState = (): GridState => (callsOf("grid_page").at(-1)!.args as { state: GridState }).state;
const rowsOf = (cmd: string) => callsOf(cmd).map((c) => (c.args as { rows: number[] }).rows);
/** The row ids the engine gives indexes `from..=to` of the grid's present order. */
const idsAt = (from: number, to: number) =>
  mockCall<number[]>("grid_row_ids", { queryId: "t", slot: "t", state: lastState(), ranges: [[from, to]] });
const indexOf = async (rowId: number) =>
  (await mockCall<number | null>("grid_row_offset", { queryId: "t", slot: "t", state: lastState(), rowId }))!;
const settle = () => act(() => new Promise((r) => setTimeout(r, 30)));

/** The shell with the recent case open and `relation`'s tab showing rows. */
async function openShell(relation = "mock_events") {
  const user = userEvent.setup();
  render(<Shell />);
  await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
  await screen.findByRole("tab", { name: "Overview" });
  const left = screen.getByRole("complementary", { name: "Case" });
  await user.click(within(left).getByRole("button", { name: new RegExp(`^${relation}`) }));
  await screen.findByRole("tab", { name: new RegExp(relation), selected: true });
  await waitFor(() => expect(dataRows().length).toBeGreaterThan(10));
  await waitFor(() => expect(cellOf(rowOf(0), "Channel").textContent).not.toBe(""));
  return user;
}

const grid = () => screen.getByRole("grid", { name: "Rows" });
const rightPane = () => screen.getByRole("complementary", { name: "Row detail" });

/** A grid alone, with a shell context the test holds. */
async function mountGrid(tab: Tab, prepare: () => void = () => {}) {
  await caseOpen("/cases/x.duckdb");
  prepare();
  const summary: CaseSummary = await caseSummary("t");
  const settings = (await settingsGet()).settings;
  const ctx: ShellContext = {
    summary, settings, change: { generation: 0, kinds: [] }, openDataset: () => {}, openScoped: vi.fn(), setGridStatus: () => {},
    setRightPane: () => {}, rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError: vi.fn(), saveLayout: vi.fn(),
    setHistogram: vi.fn(), saveColourRules: vi.fn(),
  };
  render(
    <ShellCtx.Provider value={ctx}>
      <Grid tab={tab} active scrollOffset={0} onScroll={() => {}} />
    </ShellCtx.Provider>,
  );
  // A grouped list has group rows, not the flat list's indexed rows.
  if (!tab.seed?.by.length) await waitFor(() => expect(dataRows().length).toBeGreaterThan(0));
  return { ctx, user: userEvent.setup() };
}
const eventsTab: Tab = { id: "dataset:mock_events", kind: "dataset", relation: "mock_events" };
const hayabusaTab: Tab = { id: "dataset:mock_hayabusa", kind: "dataset", relation: "mock_hayabusa" };

// ---- the keys -----------------------------------------------------------------------------------------------------------
describe("mark keys", () => {
  it("s_marks_the_selection_without_a_dialog", async () => {
    const user = await openShell();
    // Rows 0 to 2 are unmarked in the mock (its marks are on rows 5, 7, 12, 41 and 902).
    await user.click(cellOf(rowOf(0), "Channel"));
    await user.keyboard("{Shift>}");
    await user.click(cellOf(rowOf(2), "Channel"));
    await user.keyboard("{/Shift}");
    const ids = await idsAt(0, 2);
    await user.keyboard("s");
    await waitFor(() => expect(rowsOf("mark_set")).toEqual([ids]));
    expect(screen.queryByRole("dialog")).toBeNull();
    // The write's `case-changed` re-reads the rows: each shows the amber dot, and the selection is still those rows.
    for (const i of [0, 1, 2]) {
      await waitFor(() => expect(within(markOf(rowOf(i))).getByRole("img", { name: "suspicious" }).textContent).toBe("●"));
      expect(rowOf(i)).toHaveAttribute("aria-selected", "true");
    }
    expect(screen.queryByRole("dialog")).toBeNull();
    // The left pane's count follows the summary.
    await waitFor(() => expect(within(screen.getByRole("complementary", { name: "Case" })).getByRole("button", { name: /^Suspicious 6$/ })).toBeInTheDocument());
  });

  it("u_on_a_malicious_row_shows_the_reason", async () => {
    const user = await openShell();
    // Row 7 is evidence on the analyst's reviewed entry E-1: Malicious, so U is refused with the engine's reason.
    const at = await indexOf(7);
    expect(within(markOf(rowOf(at))).getByRole("img", { name: "malicious" }).textContent).toBe("■");
    await user.click(cellOf(rowOf(at), "Channel"));
    await user.keyboard("u");
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("evidence on entry E-1; detach it there");
    expect(alert.textContent).not.toMatch(/^(invalid|refused):/);
    expect(rowsOf("mark_clear")).toEqual([[7]]);
    expect(screen.queryByRole("dialog")).toBeNull();
    // S is refused the same way, with no dialog either.
    await user.click(screen.getByRole("button", { name: "Dismiss error" }));
    await user.click(cellOf(rowOf(at), "Channel"));
    await user.keyboard("s");
    expect((await screen.findByRole("alert")).textContent).toContain("evidence on entry E-1; detach it there");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("m_selects_the_new_entry", async () => {
    const user = await openShell();
    const [id] = await idsAt(3, 3);
    await user.click(cellOf(rowOf(3), "Channel"));
    await user.keyboard("m");
    await waitFor(() => expect(rowsOf("rows_promote")).toEqual([[id]]));
    expect(screen.queryByRole("dialog")).toBeNull();
    // The mock's four seeded entries are E-1 to E-4: the new one is E-5, and the right pane shows it.
    const entry = await within(rightPane()).findByRole("region", { name: "Entry E-5" });
    expect(within(entry).getByText("E-5")).toBeInTheDocument();
    expect(within(entry).getByText("draft")).toBeInTheDocument();
    // The row is now Malicious: evidence on the analyst's entry.
    await waitFor(() => expect(within(markOf(rowOf(3))).getByRole("img", { name: "malicious" }).textContent).toBe("■"));
  });

  it("m_on_a_row_the_engine_refuses_says_why_in_the_status_bar", async () => {
    const user = await openShell();
    overrideCommand("rows_promote", async (args) => {
      const rows = (args as { rows: number[] }).rows;
      return rows.map((row_id) => ({ row_id, entry_id: null, created: false, reason: `row ${row_id} is not in its relation's view` }));
    });
    const [id] = await idsAt(3, 3);
    await user.click(cellOf(rowOf(3), "Channel"));
    await user.keyboard("m");
    expect((await screen.findByRole("alert")).textContent).toContain(`row ${id} is not in its relation's view`);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(within(rightPane()).queryByRole("region", { name: /^Entry/ })).toBeNull();
  });

  it("i_preselects_the_ioc_type", async () => {
    const user = await openShell();
    const cell = cellOf(rowOf(0), "SourceIp");
    const value = cell.textContent!;
    expect(value).toMatch(/^\d+\.\d+\.\d+\.\d+$/);
    await user.click(cell);
    await user.keyboard("i");
    const dialog = await screen.findByRole("dialog", { name: "Add IOC" });
    // The engine guesses the type; the screen preselects it and the analyst confirms.
    await waitFor(() => expect(within(dialog).getByRole("combobox", { name: "IOC type" })).toHaveValue("ip"));
    expect(callsOf("ioc_guess").map((c) => c.args)).toEqual([{ value }]);
    expect(within(dialog).getByRole("textbox", { name: "IOC value" })).toHaveValue(value);
    expect(callsOf("record_upsert")).toHaveLength(0); // nothing is written before the analyst confirms
    await user.click(within(dialog).getByRole("button", { name: "Add IOC" }));
    await waitFor(() => expect(callsOf("record_upsert")).toHaveLength(1));
    expect((callsOf("record_upsert")[0].args as { spec: unknown }).spec).toEqual({
      kind: "ioc", id: null, ioc_type: "ip", value, tool: null, attack_ids: [], note: "",
    });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add IOC" })).toBeNull());
    expect(cellOf(rowOf(0), "SourceIp").closest("[role='grid']")).toBe(grid()); // focus goes back to the grid
    expect(grid()).toHaveFocus();
  });

  it("a_late_ioc_guess_does_not_replace_the_analysts_choice", async () => {
    const user = await openShell();
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    overrideCommand("ioc_guess", async (args) => {
      await gate;
      return mockCall("ioc_guess", args);
    });
    await user.click(cellOf(rowOf(0), "SourceIp"));
    await user.keyboard("i");
    const dialog = await screen.findByRole("dialog", { name: "Add IOC" });
    await user.selectOptions(within(dialog).getByRole("combobox", { name: "IOC type" }), "hostname");
    await act(async () => {
      release();
      await new Promise((r) => setTimeout(r, 30));
    });
    expect(within(dialog).getByRole("combobox", { name: "IOC type" })).toHaveValue("hostname");
  });

  it("an_ioc_the_engine_refuses_keeps_the_form_open_with_its_reason", async () => {
    const user = await openShell();
    overrideCommand("record_upsert", async () => {
      throw { code: "invalid", message: "not an IPv4 or IPv6 address: x" };
    });
    await user.click(cellOf(rowOf(0), "SourceIp"));
    await user.keyboard("i");
    const dialog = await screen.findByRole("dialog", { name: "Add IOC" });
    await waitFor(() => expect(within(dialog).getByRole("combobox", { name: "IOC type" })).toHaveValue("ip"));
    await user.click(within(dialog).getByRole("button", { name: "Add IOC" }));
    expect((await within(dialog).findByRole("alert")).textContent).toBe("not an IPv4 or IPv6 address: x");
    expect(screen.getByRole("dialog", { name: "Add IOC" })).toBeInTheDocument();
  });

  it("keys_are_ignored_while_typing", async () => {
    const user = await openShell();
    await user.click(cellOf(rowOf(0), "Channel"));
    // The filter row's input sits inside the grid; the search box is above it. Typing in either is text, not a key.
    const filter = screen.getAllByRole("textbox").find((el) => el.closest("[role='grid']") !== null)!;
    await user.click(filter);
    await user.keyboard("sumtnai");
    const search = screen.getByRole("searchbox");
    await user.click(search);
    await user.keyboard("sumtnai");
    await settle();
    // (The row detail's own reads run for the clicked row; no key reached a write, a guess or a dialog.)
    for (const cmd of ["mark_set", "mark_clear", "rows_promote", "tag_add", "tags_list", "note_add", "rows_attach", "ioc_guess"]) {
      expect(callsOf(cmd)).toHaveLength(0);
    }
    expect(screen.queryByRole("dialog")).toBeNull();
    // The same keys on the grid itself act (once the typed text is cleared from the boxes again).
    await user.clear(search);
    await user.clear(filter);
    await waitFor(() => expect(cellOf(rowOf(0), "Channel").textContent).not.toBe(""));
    await user.click(cellOf(rowOf(0), "Channel"));
    await user.keyboard("s");
    await waitFor(() => expect(callsOf("mark_set")).toHaveLength(1));
  });

  it("modified_keys_are_not_mark_keys", async () => {
    const user = await openShell();
    await user.click(cellOf(rowOf(0), "Channel"));
    fireEvent.keyDown(grid(), { key: "s", ctrlKey: true });
    fireEvent.keyDown(grid(), { key: "m", altKey: true });
    fireEvent.keyDown(grid(), { key: "u", metaKey: true });
    await settle();
    expect(callsOf("mark_set")).toHaveLength(0);
    expect(callsOf("rows_promote")).toHaveLength(0);
    expect(callsOf("mark_clear")).toHaveLength(0);
  });

  it("ten_thousand_and_one_rows_are_refused_with_the_count", async () => {
    // The relation is made 12,000 rows long: the mock's 5,000 again and again, under ids of their own.
    const LONG = 12_000;
    overrideCommand("grid_counts", async (args) => ({ ...((await mockCall("grid_counts", args)) as object), rows: LONG, total: LONG }));
    overrideCommand("grid_page", async (args) => {
      const a = args as { offset: number };
      const lap = Math.floor(a.offset / 5000);
      const page = (await mockCall("grid_page", { ...a, offset: a.offset % 5000 })) as GridPage;
      return { ...page, row_ids: page.row_ids.map((id) => id + lap * 100_000) };
    });
    const { ctx, user } = await mountGrid(eventsTab);
    await waitFor(() => expect(cellOf(rowOf(0), "Channel").textContent).not.toBe(""));
    await user.click(cellOf(rowOf(0), "Channel"));
    scrollTops.set(screen.getByTestId("grid-scroll"), 10_000 * 22);
    await act(async () => {
      fireEvent.scroll(screen.getByTestId("grid-scroll"));
    });
    await waitFor(() => expect(rowOf(10_000)).toBeTruthy());
    await waitFor(() => expect(cellOf(rowOf(10_000), "Channel").textContent).not.toBe(""), { timeout: 5000 });
    await user.keyboard("{Shift>}");
    await user.click(cellOf(rowOf(10_000), "Channel"));
    await user.keyboard("{/Shift}");
    await waitFor(() => expect(rowOf(10_000)).toHaveAttribute("aria-selected", "true"));
    await user.keyboard("s");
    await waitFor(() => expect(ctx.reportError).toHaveBeenCalled());
    const message = (vi.mocked(ctx.reportError).mock.calls.at(-1)![0] as Error).message;
    expect(message).toBe("select at most 10,000 rows (you have 10,001)");
    expect(callsOf("mark_set")).toHaveLength(0);
    expect(callsOf("grid_row_ids")).toHaveLength(0); // refused before any round trip
    expect(screen.queryByRole("dialog")).toBeNull();
  }, 30_000);

  it("ten_thousand_rows_are_marked_by_the_ids_resolved_before_the_write", async () => {
    // Review Focus 4: 10,000 rows across blocks never read; S acts on the ids the engine named for the selection.
    const LONG = 12_000;
    overrideCommand("grid_counts", async (args) => ({ ...((await mockCall("grid_counts", args)) as object), rows: LONG, total: LONG }));
    overrideCommand("grid_page", async (args) => {
      const a = args as { offset: number };
      const lap = Math.floor(a.offset / 5000);
      const page = (await mockCall("grid_page", { ...a, offset: a.offset % 5000 })) as GridPage;
      return { ...page, row_ids: page.row_ids.map((id) => id + lap * 100_000) };
    });
    overrideCommand("grid_row_ids", async (args) => {
      const ranges = (args as { ranges: Array<[number, number]> }).ranges;
      return ranges.flatMap(([a, b]) => Array.from({ length: b - a + 1 }, (_, k) => 1_000_000 + a + k));
    });
    overrideCommand("mark_set", async (args) => (args as { rows: number[] }).rows.map((row_id) => ({ row_id, done: true, reason: null })));
    const { ctx, user } = await mountGrid(eventsTab);
    await waitFor(() => expect(cellOf(rowOf(0), "Channel").textContent).not.toBe(""));
    await user.click(cellOf(rowOf(0), "Channel"));
    scrollTops.set(screen.getByTestId("grid-scroll"), 9_999 * 22);
    await act(async () => {
      fireEvent.scroll(screen.getByTestId("grid-scroll"));
    });
    await waitFor(() => expect(rowOf(9_999)).toBeTruthy());
    await waitFor(() => expect(cellOf(rowOf(9_999), "Channel").textContent).not.toBe(""), { timeout: 5000 });
    await user.keyboard("{Shift>}");
    await user.click(cellOf(rowOf(9_999), "Channel"));
    await user.keyboard("{/Shift}");
    await waitFor(() => expect(callsOf("grid_row_ids")).toHaveLength(1));
    await settle();
    await user.keyboard("s");
    await waitFor(() => expect(callsOf("mark_set")).toHaveLength(1));
    const rows = rowsOf("mark_set")[0];
    expect(rows).toHaveLength(10_000);
    expect(rows[0]).toBe(1_000_000);
    expect(rows[9_999]).toBe(1_009_999);
    expect(ctx.reportError).not.toHaveBeenCalled();
  }, 30_000);
});

describe("dialog keys", () => {
  it("t_tags_the_selection_and_gives_focus_back", async () => {
    const user = await openShell();
    await user.click(cellOf(rowOf(0), "Channel"));
    await user.keyboard("{Shift>}");
    await user.click(cellOf(rowOf(1), "Channel"));
    await user.keyboard("{/Shift}");
    const ids = await idsAt(0, 1);
    await user.keyboard("t");
    const dialog = await screen.findByRole("dialog", { name: "Tag rows" });
    const input = within(dialog).getByRole("textbox", { name: "Tag" });
    expect(input).toHaveFocus();
    await user.type(input, "lateral");
    await user.click(within(dialog).getByRole("button", { name: "Add tag" }));
    await waitFor(() => expect(callsOf("tag_add").map((c) => c.args)).toEqual([{ rows: ids, tag: "lateral" }]));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Tag rows" })).toBeNull());
    expect(grid()).toHaveFocus();
  });

  it("escape_closes_a_dialog_with_nothing_written", async () => {
    const user = await openShell();
    await user.click(cellOf(rowOf(0), "Channel"));
    await user.keyboard("n");
    const dialog = await screen.findByRole("dialog", { name: "Add note" });
    await user.type(within(dialog).getByRole("textbox", { name: "Note" }), "draft text");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(callsOf("note_add")).toHaveLength(0);
    expect(grid()).toHaveFocus();
  });

  it("n_notes_the_focused_row", async () => {
    const user = await openShell();
    const [id] = await idsAt(2, 2);
    await user.click(cellOf(rowOf(2), "Channel"));
    await user.keyboard("n");
    const dialog = await screen.findByRole("dialog", { name: "Add note" });
    // Keys typed in the dialog are text: "s" here marks nothing.
    await user.type(within(dialog).getByRole("textbox", { name: "Note" }), "seen on the jump host");
    await user.click(within(dialog).getByRole("button", { name: "Add note" }));
    await waitFor(() => expect(callsOf("note_add").map((c) => c.args)).toEqual([{ rowId: id, text: "seen on the jump host" }]));
    expect(callsOf("mark_set")).toHaveLength(0);
  });

  it("a_attaches_the_selection_to_a_picked_entry", async () => {
    const user = await openShell();
    await user.click(cellOf(rowOf(0), "Channel"));
    await user.keyboard("{Shift>}");
    await user.click(cellOf(rowOf(1), "Channel"));
    await user.keyboard("{/Shift}");
    const ids = await idsAt(0, 1);
    await user.keyboard("a");
    const dialog = await screen.findByRole("dialog", { name: "Attach to an entry" });
    await waitFor(() => expect(within(dialog).getAllByRole("option").length).toBe(4));
    // Typing filters through the engine (the entry list's text filter), not in the screen.
    await user.type(within(dialog).getByRole("searchbox", { name: "Filter entries" }), "wipe");
    await waitFor(() => expect(within(dialog).getAllByRole("option").length).toBe(1));
    expect((callsOf("entries_list").at(-1)!.args as { query: { text: string | null } }).query.text).toBe("wipe");
    await user.click(within(dialog).getByRole("option", { name: /E-4/ }));
    await waitFor(() => expect(callsOf("rows_attach").map((c) => c.args)).toEqual([{ entryId: 4, rows: ids }]));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("a_late_entry_list_is_not_drawn", async () => {
    const user = await openShell();
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    let first = true;
    overrideCommand("entries_list", async (args) => {
      if (first) {
        first = false;
        await gate;
      }
      return mockCall("entries_list", args);
    });
    await user.click(cellOf(rowOf(0), "Channel"));
    await user.keyboard("a");
    const dialog = await screen.findByRole("dialog", { name: "Attach to an entry" });
    await user.type(within(dialog).getByRole("searchbox", { name: "Filter entries" }), "wipe");
    await waitFor(() => expect(within(dialog).getAllByRole("option").length).toBe(1));
    await act(async () => {
      release();
      await new Promise((r) => setTimeout(r, 30));
    });
    expect(within(dialog).getAllByRole("option").length).toBe(1);
  });
});

// ---- the glyphs -----------------------------------------------------------------------------------------------------------
describe("glyphs", () => {
  it("glyphs_hold_under_a_colour_rule", async () => {
    // Hayabusa's default rules tint `crit` rows red. A crit row marked Malicious and another Agent: malicious keep their
    // glyphs, in their own colour token: the wash is on the row and never on the mark cell or the glyph.
    const crit = hayabusaRows().flatMap((r, i) => (r[5] === "crit" ? [HAYABUSA_FIRST_ID + i] : []));
    const [malicious, agent] = crit;
    await mountGrid(hayabusaTab, () => setMockMarks([[malicious, "malicious"], [agent, "agent_malicious"]]));
    await waitFor(() => expect(document.querySelector(".mark-malicious")).not.toBeNull());
    const square = screen.getByRole("img", { name: "malicious" });
    expect(square.textContent).toBe("■");
    expect(square).toHaveClass("mark-malicious");
    const diamond = screen.getByRole("img", { name: "agent: malicious" });
    expect(diamond.textContent).toBe("◆");
    expect(diamond).toHaveClass("mark-agent");
    for (const glyph of [square, diamond]) {
      const row = glyph.closest<HTMLElement>("[role='row']")!;
      expect(row.className).toContain("rule-row-red"); // the row is tinted...
      expect(glyph.closest(".cell-mark")!.className).not.toMatch(/rule-/); // ...the mark cell is not
    }
    // The tokens: each glyph's class is its reserved colour and no rule colour reaches either.
    expect(gridCss).toMatch(/\.mark-malicious\s*\{\s*color:\s*var\(--malicious\);\s*\}/);
    expect(gridCss).toMatch(/\.mark-agent\s*\{\s*color:\s*var\(--agent\);\s*\}/);
    expect(gridCss).toMatch(/\.mark-suspicious\s*\{\s*color:\s*var\(--suspicious\);\s*\}/);
    expect(gridCss).not.toMatch(/\.mark-(malicious|agent|suspicious)[^{]*\{[^}]*--rule-/);
    expect(gridCss).not.toMatch(/\.rule-[a-z-]+[^{]*\.mark-/);
  });
});

// ---- the row detail -------------------------------------------------------------------------------------------------------
describe("row detail", () => {
  it("row_detail_marks_tags_notes_and_lists_entries", async () => {
    const user = await openShell();
    // Row 5 is Suspicious, tagged "seen" and has a note in the mock.
    const at = await indexOf(5);
    await user.click(cellOf(rowOf(at), "Channel"));
    const pane = rightPane();
    await within(pane).findByRole("region", { name: "Marks" });
    const marks = within(pane).getByRole("region", { name: "Marks" });
    expect(within(marks).getByRole("img", { name: "suspicious" })).toBeInTheDocument();
    await user.click(within(marks).getByRole("button", { name: "Clear mark (U)" }));
    await waitFor(() => expect(rowsOf("mark_clear")).toEqual([[5]]));
    await waitFor(() => expect(within(within(rightPane()).getByRole("region", { name: "Marks" })).queryByRole("img")).toBeNull());

    const tags = within(rightPane()).getByRole("region", { name: "Tags" });
    expect(within(tags).getByText("seen")).toBeInTheDocument();
    await user.type(within(tags).getByRole("textbox", { name: "New tag" }), "exfil");
    await user.click(within(tags).getByRole("button", { name: "Add tag" }));
    await waitFor(() => expect(callsOf("tag_add").map((c) => c.args)).toEqual([{ rows: [5], tag: "exfil" }]));
    await waitFor(() => expect(within(within(rightPane()).getByRole("region", { name: "Tags" })).getByText("exfil")).toBeInTheDocument());
    await user.click(within(within(rightPane()).getByRole("region", { name: "Tags" })).getByRole("button", { name: "Remove tag seen" }));
    await waitFor(() => expect(callsOf("tag_remove").map((c) => c.args)).toEqual([{ rows: [5], tag: "seen" }]));

    const notes = within(rightPane()).getByRole("region", { name: "Notes" });
    await user.type(within(notes).getByRole("textbox", { name: "New note" }), "checked with the owner");
    await user.click(within(notes).getByRole("button", { name: "Add note" }));
    await waitFor(() => expect(callsOf("note_add").map((c) => c.args)).toEqual([{ rowId: 5, text: "checked with the owner" }]));
    await waitFor(() => expect(within(within(rightPane()).getByRole("region", { name: "Notes" })).getByText("checked with the owner")).toBeInTheDocument());
    const deletes = within(within(rightPane()).getByRole("region", { name: "Notes" })).getAllByRole("button", { name: "Delete note" });
    await user.click(deletes[0]);
    await waitFor(() => expect(callsOf("note_delete")).toHaveLength(1));

    // Linked entries: row 7 is evidence on E-1.
    const seven = await indexOf(7);
    await user.click(cellOf(rowOf(seven), "Channel"));
    const linked = await within(rightPane()).findByRole("region", { name: "Linked entries" });
    await waitFor(() => expect(within(linked).getAllByRole("button").map((b) => b.textContent)).toEqual([expect.stringContaining("E-1")]));
    expect((callsOf("entries_list").at(-1)!.args as { query: { row: number | null } }).query.row).toBe(7);
    await user.click(within(linked).getByRole("button", { name: /E-1/ }));
    await within(rightPane()).findByRole("region", { name: "Entry E-1" });
  });

  it("a_late_linked_entries_answer_is_not_drawn", async () => {
    const user = await openShell();
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    overrideCommand("entries_list", async (args) => {
      const q = (args as { query: { row: number | null } }).query;
      if (q.row === 7) await gate;
      return mockCall("entries_list", args);
    });
    await user.click(cellOf(rowOf(await indexOf(7)), "Channel"));
    await within(rightPane()).findByRole("region", { name: "Linked entries" });
    const [first] = await idsAt(0, 0);
    // Another row is shown before row 7's answer comes: its (empty) list stays.
    await user.click(cellOf(rowOf(0), "Channel"));
    await waitFor(() => expect(within(rightPane()).getByTestId("detail-row").textContent).toBe(`row ${first}`));
    await act(async () => {
      release();
      await new Promise((r) => setTimeout(r, 30));
    });
    const linked = within(rightPane()).getByRole("region", { name: "Linked entries" });
    expect(within(linked).queryAllByRole("button")).toHaveLength(0);
  });
});

// ---- the left pane ------------------------------------------------------------------------------------------------------
describe("left pane", () => {
  const paneWith = async (by: CaseSummary["mark_counts"]["by_relation"]) => {
    await caseOpen("/cases/x.duckdb");
    const base = await caseSummary("t");
    const total = (k: "suspicious" | "malicious" | "agent_malicious") => by.reduce((n, r) => n + r[k], 0);
    const summary: CaseSummary = {
      ...base,
      mark_counts: { suspicious: total("suspicious"), malicious: total("malicious"), agent_malicious: total("agent_malicious"), by_relation: by },
    };
    const onOpenWorklist = vi.fn();
    const settings = (await settingsGet()).settings;
    const ctx: ShellContext = {
      summary, settings, change: { generation: 0, kinds: [] }, openDataset: () => {}, openScoped: vi.fn(), setGridStatus: () => {},
      setRightPane: () => {}, rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError: vi.fn(), saveLayout: vi.fn(),
      setHistogram: vi.fn(), saveColourRules: vi.fn(),
    };
    const utils = render(
      <ShellCtx.Provider value={ctx}>
        <LeftPane summary={summary} sessions={[]} onOpenDataset={() => {}} onOpenSession={() => {}} onOpenWorklist={onOpenWorklist} onCollapse={() => {}} />
      </ShellCtx.Provider>,
    );
    return { ...utils, onOpenWorklist };
  };
  const datasetButton = (name: string) => screen.getByRole("button", { name: new RegExp(`^${name}`) });

  it("marked_datasets_are_coloured_with_their_count", async () => {
    // mock_events: malicious, agent and suspicious rows, so malicious wins; mock_hayabusa: agent beats suspicious.
    const { unmount } = await paneWith([
      { relation: "mock_events", suspicious: 3, malicious: 1, agent_malicious: 1 },
      { relation: "mock_hayabusa", suspicious: 4, malicious: 0, agent_malicious: 2 },
    ]);
    const events = datasetButton("mock_events");
    expect(events.querySelector(".pane-item-name")).toHaveClass("mark-malicious");
    expect(within(events).getByLabelText("5 marked rows")).toHaveClass("mark-malicious");
    const hayabusa = datasetButton("mock_hayabusa");
    expect(hayabusa.querySelector(".pane-item-name")).toHaveClass("mark-agent");
    expect(within(hayabusa).getByLabelText("6 marked rows")).toHaveClass("mark-agent");
    unmount();

    // Suspicious alone is amber; a dataset with no marked rows has no mark colour and no marked count.
    await paneWith([{ relation: "mock_hayabusa", suspicious: 2, malicious: 0, agent_malicious: 0 }]);
    expect(datasetButton("mock_hayabusa").querySelector(".pane-item-name")).toHaveClass("mark-suspicious");
    expect(within(datasetButton("mock_hayabusa")).getByLabelText("2 marked rows")).toHaveClass("mark-suspicious");
    const plain = datasetButton("mock_events");
    expect(plain.querySelector(".pane-item-name")!.className).not.toMatch(/mark-/);
    expect(within(plain).queryByLabelText(/marked rows/)).toBeNull();
  });

  it("each_mark_count_opens_its_worklist", async () => {
    const user = userEvent.setup();
    const { onOpenWorklist } = await paneWith([{ relation: "mock_events", suspicious: 3, malicious: 1, agent_malicious: 2 }]);
    const marked = screen.getByRole("region", { name: "Marked rows" });
    await user.click(within(marked).getByRole("button", { name: "Suspicious 3" }));
    await user.click(within(marked).getByRole("button", { name: "Malicious 1" }));
    await user.click(within(marked).getByRole("button", { name: "Agent: malicious 2" }));
    expect(onOpenWorklist.mock.calls.map((c) => c[0])).toEqual(["suspicious", "malicious", "agent_malicious"]);
  });

  it("the_worklist_lists_every_marked_row", async () => {
    const user = userEvent.setup();
    render(<Shell />);
    await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
    await screen.findByRole("tab", { name: "Overview" });
    // 250 Suspicious rows across both datasets: more than one page of the worklist.
    const marked = [...Array.from({ length: 200 }, (_, i) => 100 + i), ...Array.from({ length: 50 }, (_, i) => HAYABUSA_FIRST_ID + i)];
    setMockMarks(marked.map((id) => [id, "suspicious"] as const));
    await act(async () => {
      // A write elsewhere re-reads the summary.
      await mockCall("display_zone_set", { zone: "UTC" });
    });
    const left = screen.getByRole("complementary", { name: "Case" });
    await user.click(await within(left).findByRole("button", { name: "Suspicious 250" }));
    const tab = await screen.findByRole("tab", { name: /Suspicious rows/, selected: true });
    expect(tab).toBeInTheDocument();
    const panel = screen.getByRole("tabpanel", { name: /Suspicious rows/ });
    await waitFor(() => expect(within(panel).getAllByRole("row").length).toBe(1 + 200));
    const source = (callsOf("merged_page")[0].args as { source: MergedSource }).source;
    expect(source).toEqual({ marked: { marks: ["suspicious"] } });
    await user.click(within(panel).getByRole("button", { name: "Load more" }));
    await waitFor(() => expect(within(panel).getAllByRole("row").length).toBe(1 + 250));
    const listed = within(panel).getAllByRole("checkbox").map((c) => Number(c.getAttribute("data-row-id")));
    expect([...listed].sort((a, b) => a - b)).toEqual([...marked].sort((a, b) => a - b));
    expect(within(panel).getAllByRole("img", { name: "suspicious" })).toHaveLength(250);
  });

  it("s_in_the_worklist_marks_the_checked_rows", async () => {
    const user = userEvent.setup();
    render(<Shell />);
    await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
    await screen.findByRole("tab", { name: "Overview" });
    const left = screen.getByRole("complementary", { name: "Case" });
    await user.click(within(left).getByRole("button", { name: "Agent: malicious 1" }));
    const panel = await screen.findByRole("tabpanel", { name: /Agent: malicious rows/ });
    const box = await within(panel).findByRole("checkbox", { name: "Select row 12" });
    await user.click(box);
    await user.keyboard("s");
    await waitFor(() => expect(rowsOf("mark_set")).toEqual([[12]]));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("the_rails_keep_every_count", async () => {
    render(<Shell />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
    await screen.findByRole("tab", { name: "Overview" });
    fireEvent.keyDown(document.body, { key: "[", ctrlKey: true });
    const rail = await screen.findByTestId("left-rail");
    expect(within(rail).getByLabelText("3 suspicious")).toBeInTheDocument();
    expect(within(rail).getByLabelText("1 malicious")).toBeInTheDocument();
    expect(within(rail).getByLabelText("1 agent: malicious")).toBeInTheDocument();
  });
});

// ---- fix round 1 ----------------------------------------------------------------------------------------------------------
/** The query id of the last call of `cmd` that `pick` takes. */
const queryIdOf = (cmd: string, pick: (args: Record<string, unknown>) => boolean = () => true) =>
  (callsOf(cmd).filter((c) => pick(c.args as Record<string, unknown>)).at(-1)!.args as { queryId: string }).queryId;
const cancelledIds = () => callsOf("query_cancel").flatMap((c) => (c.args as { queryIds: string[] }).queryIds);

describe("fix round 1", () => {
  it("grouped_rows_take_the_row_keys", async () => {
    const grouped: Tab = {
      id: "scoped:grouped", kind: "dataset", relation: "mock_events", title: "grouped",
      seed: { state: initialGridState("mock_events"), by: ["Channel"] },
    };
    const { user } = await mountGrid(grouped);
    const toggle = await screen.findAllByRole("button", { name: /^Expand Channel/ });
    await user.click(toggle[0]);
    await waitFor(() => expect(document.querySelectorAll("[data-kind='row']").length).toBeGreaterThan(2));
    const rows = () => [...document.querySelectorAll<HTMLElement>("[data-kind='row']")];
    // The cells of a group row are drawn in the header's order after the mark cell; Channel is plain text.
    const names = screen.getAllByRole("columnheader").flatMap((h) => (h.querySelector(".col-name") ? [h.querySelector(".col-name")!.textContent] : []));
    const channel = names.indexOf("Channel") + 1;
    const first = rows()[0];
    const firstId = Number(first.getAttribute("data-row-id"));
    expect(firstId).toBeGreaterThan(0);
    await user.click(within(first).getAllByRole("gridcell")[channel]);
    expect(rows()[0]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("s");
    await waitFor(() => expect(rowsOf("mark_set")).toEqual([[firstId]]));
    // The arrows move to the next data row; U acts on it.
    await user.keyboard("{ArrowDown}");
    const secondId = Number(rows()[1].getAttribute("data-row-id"));
    expect(rows()[1]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("u");
    await waitFor(() => expect(rowsOf("mark_clear")).toEqual([[secondId]]));
    // I takes the focused cell's value.
    await user.keyboard("i");
    const dialog = await screen.findByRole("dialog", { name: "Add IOC" });
    expect((within(dialog).getByRole("textbox", { name: "IOC value" }) as HTMLInputElement).value).toBe(
      within(rows()[1]).getAllByRole("gridcell")[channel].textContent,
    );
  });

  it("the_key_strip_follows_the_active_tab", async () => {
    const user = userEvent.setup();
    render(<Shell />);
    await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
    await screen.findByRole("tab", { name: "Overview" });
    const strip = () => screen.getByLabelText("Keys").textContent;
    expect(strip()).toBe("Ctrl+[ Ctrl+] panes");
    const left = screen.getByRole("complementary", { name: "Case" });
    await user.click(within(left).getByRole("button", { name: /^mock_events/ }));
    await screen.findByRole("tab", { name: /mock_events/, selected: true });
    expect(strip()).toBe(
      "S suspicious · U clear mark · M malicious → timeline · T tag · N note · A attach · I add as IOC · " +
        "Space full value · Enter row detail · Ctrl+C copy · Ctrl+Shift+C copy rows · Ctrl+[ Ctrl+] panes",
    );
    await user.click(within(left).getByRole("button", { name: "Suspicious 3" }));
    await screen.findByRole("tab", { name: /Suspicious rows/, selected: true });
    expect(strip()).toBe("S suspicious · U clear mark · M malicious → timeline · T tag · N note · A attach · Ctrl+[ Ctrl+] panes");
  });

  it("a_lists_the_newest_entries_first", async () => {
    const user = await openShell();
    await user.click(cellOf(rowOf(0), "Channel"));
    await user.keyboard("a");
    const dialog = await screen.findByRole("dialog", { name: "Attach to an entry" });
    await waitFor(() => expect(within(dialog).getAllByRole("option").length).toBe(4));
    expect(within(dialog).getAllByRole("option").map((o) => o.querySelector(".record-id")!.textContent)).toEqual(["E-4", "E-3", "E-2", "E-1"]);
    expect((callsOf("entries_list").at(-1)!.args as { query: { order: string | null } }).query.order).toBe("recent");
  });

  it("an_agents_entry_says_added_by_agent_before_and_after_review", async () => {
    const user = await openShell();
    await user.click(cellOf(rowOf(await indexOf(12)), "Channel"));
    const linked = await within(rightPane()).findByRole("region", { name: "Linked entries" });
    await user.click(await within(linked).findByRole("button", { name: /E-2/ }));
    const card = await within(rightPane()).findByRole("region", { name: "Entry E-2" });
    await waitFor(() => expect(card.textContent).toContain("added by agent: triage-bot"));
    expect(card.textContent).toContain("unreviewed");
    await act(async () => {
      await mockCall("entry_status", { id: 2, status: "reviewed" });
    });
    await waitFor(() => expect(within(rightPane()).getByRole("region", { name: "Entry E-2" }).textContent).toContain("reviewed by"));
    expect(within(rightPane()).getByRole("region", { name: "Entry E-2" }).textContent).toContain("added by agent: triage-bot");
    // An analyst's entry names its author as stored.
    await user.click(cellOf(rowOf(await indexOf(7)), "Channel"));
    await user.click(await within(await within(rightPane()).findByRole("region", { name: "Linked entries" })).findByRole("button", { name: /E-1/ }));
    const own = await within(rightPane()).findByRole("region", { name: "Entry E-1" });
    await waitFor(() => expect(own.textContent).toContain("analyst:analyst01"));
    expect(own.textContent).not.toContain("added by agent");
  });

  it("linked_entries_page_past_fifty", async () => {
    await caseOpen("/cases/x.duckdb");
    const template = await mockCall<Entry>("entry_get", { queryId: "t", slot: "t", id: 1 });
    const all: Entry[] = Array.from({ length: 51 }, (_, i) => ({ ...template, id: 100 + i, title: `Linked ${i}` }));
    overrideCommand("entries_list", async (args) => {
      const a = args as { query: { row: number | null }; limit: number; offset: number };
      if (a.query.row === null) return mockCall("entries_list", args);
      return { items: all.slice(a.offset, a.offset + a.limit), truncated: a.offset + a.limit < all.length };
    });
    const user = await openShell();
    await user.click(cellOf(rowOf(0), "Channel"));
    const linked = await within(rightPane()).findByRole("region", { name: "Linked entries" });
    await waitFor(() => expect(within(linked).getAllByRole("button", { name: /^E-/ })).toHaveLength(50));
    await user.click(within(linked).getByRole("button", { name: "Show more" }));
    await waitFor(() => expect(within(linked).getAllByRole("button", { name: /^E-/ })).toHaveLength(51));
    expect(within(linked).getByRole("button", { name: /E-150/ })).toBeInTheDocument();
    expect(within(linked).queryByRole("button", { name: "Show more" })).toBeNull();
  });

  it("a_worklist_selection_past_page_one_survives_a_refresh", async () => {
    const user = userEvent.setup();
    render(<Shell />);
    await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
    await screen.findByRole("tab", { name: "Overview" });
    setMockMarks(Array.from({ length: 250 }, (_, i) => [100 + i, "suspicious"] as const));
    await act(async () => {
      await mockCall("display_zone_set", { zone: "UTC" });
    });
    const left = screen.getByRole("complementary", { name: "Case" });
    await user.click(await within(left).findByRole("button", { name: "Suspicious 250" }));
    const panel = await screen.findByRole("tabpanel", { name: /Suspicious rows/ });
    await waitFor(() => expect(within(panel).getAllByRole("checkbox")).toHaveLength(200));
    await user.click(within(panel).getByRole("button", { name: "Load more" }));
    await waitFor(() => expect(within(panel).getAllByRole("checkbox")).toHaveLength(250));
    await user.click(within(panel).getByRole("checkbox", { name: "Select row 320" }));
    const reads = callsOf("merged_page").length;
    await act(async () => {
      await mockCall("display_zone_set", { zone: "UTC" });
    });
    await waitFor(() => expect(callsOf("merged_page").length).toBeGreaterThanOrEqual(reads + 2));
    await waitFor(() => expect(within(panel).getAllByRole("checkbox")).toHaveLength(250));
    const again = within(panel).getByRole("checkbox", { name: "Select row 320" });
    expect(again).toBeChecked();
    again.focus();
    await user.keyboard("t");
    const dialog = await screen.findByRole("dialog", { name: "Tag rows" });
    await user.type(within(dialog).getByRole("textbox", { name: "Tag" }), "late");
    await user.click(within(dialog).getByRole("button", { name: "Add tag" }));
    await waitFor(() => expect(callsOf("tag_add").map((c) => c.args)).toEqual([{ rows: [320], tag: "late" }]));
  });

  it("a_deleted_entry_shows_as_gone", async () => {
    const user = await openShell();
    await user.click(cellOf(rowOf(3), "Channel"));
    await user.keyboard("m");
    const card = await within(rightPane()).findByRole("region", { name: "Entry E-5" });
    await waitFor(() => expect(card.textContent).toContain("draft"));
    await act(async () => {
      await mockCall("entry_delete", { id: 5 });
    });
    await waitFor(() => expect(within(rightPane()).getByRole("region", { name: "Entry E-5" }).textContent).toContain("This entry was deleted."));
    expect(within(rightPane()).getByRole("region", { name: "Entry E-5" }).textContent).not.toContain("draft");
  });

  it("closing_the_tag_dialog_cancels_its_read", async () => {
    const user = await openShell();
    overrideCommand("tags_list", () => new Promise(() => {}));
    await user.click(cellOf(rowOf(0), "Channel"));
    await user.keyboard("t");
    await screen.findByRole("dialog", { name: "Tag rows" });
    await waitFor(() => expect(callsOf("tags_list")).toHaveLength(1));
    const id = queryIdOf("tags_list");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(cancelledIds()).toContain(id));
  });

  it("a_state_change_cancels_the_actions_row_ids", async () => {
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    overrideCommand("grid_row_ids", async (args) => {
      await gate;
      return mockCall("grid_row_ids", args);
    });
    const { user } = await mountGrid(eventsTab);
    await waitFor(() => expect(cellOf(rowOf(0), "Channel").textContent).not.toBe(""));
    await user.click(cellOf(rowOf(10), "Channel"));
    scrollTops.set(screen.getByTestId("grid-scroll"), 1500 * 22);
    await act(async () => {
      fireEvent.scroll(screen.getByTestId("grid-scroll"));
    });
    await waitFor(() => expect(rowOf(1500)).toBeTruthy());
    await waitFor(() => expect(cellOf(rowOf(1500), "Channel").textContent).not.toBe(""), { timeout: 5000 });
    await user.keyboard("{Shift>}");
    await user.click(cellOf(rowOf(1500), "Channel"));
    await user.keyboard("{/Shift}");
    await user.keyboard("s");
    const isAction = (a: Record<string, unknown>) => String(a.slot).endsWith(":action");
    await waitFor(() => expect(callsOf("grid_row_ids").filter((c) => isAction(c.args as Record<string, unknown>))).toHaveLength(1));
    const id = queryIdOf("grid_row_ids", isAction);
    // Hiding a column changes the state the indexes belong to: the action's read is cancelled and nothing is written.
    fireEvent.contextMenu(header("Channel"));
    fireEvent.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Hide" }));
    await waitFor(() => expect(cancelledIds()).toContain(id));
    await act(async () => {
      release();
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(callsOf("mark_set")).toHaveLength(0);
  }, 30_000);

  it("a_long_tag_reaches_the_engine_and_its_refusal_is_shown", async () => {
    overrideCommand("tag_add", async (args) => {
      const tag = (args as { tag: string }).tag;
      throw { code: "invalid", message: `a tag is at most 64 characters, not ${tag.length}` };
    });
    const user = await openShell();
    await user.click(cellOf(rowOf(0), "Channel"));
    const tags = await within(rightPane()).findByRole("region", { name: "Tags" });
    const long = "x".repeat(210);
    const input = within(tags).getByRole("textbox", { name: "New tag" });
    expect(input).not.toHaveAttribute("maxlength");
    await user.click(input);
    await user.paste(long);
    await user.click(within(tags).getByRole("button", { name: "Add tag" }));
    await waitFor(() => expect(callsOf("tag_add").map((c) => (c.args as { tag: string }).tag)).toEqual([long]));
    expect((await screen.findByRole("alert")).textContent).toContain("a tag is at most 64 characters, not 210");
    // The tag dialog has no cap of its own either.
    await user.click(screen.getByRole("button", { name: "Dismiss error" }));
    await user.click(cellOf(rowOf(0), "Channel"));
    await user.keyboard("t");
    const dialog = await screen.findByRole("dialog", { name: "Tag rows" });
    expect(within(dialog).getByRole("textbox", { name: "Tag" })).not.toHaveAttribute("maxlength");
  });
});

describe("fix round 2", () => {
  it("the_grouped_strip_lists_only_its_live_keys", async () => {
    const user = await openShell();
    const strip = () => screen.getByLabelText("Keys").textContent;
    fireEvent.contextMenu(header("Channel"));
    await user.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Group by this column" }));
    await waitFor(() => expect(strip()).toBe("S suspicious · U clear mark · M malicious → timeline · T tag · N note · A attach · I add as IOC · Ctrl+[ Ctrl+] panes"));
    // Ungrouped again, the flat grid's keys come back.
    await user.click(screen.getByRole("button", { name: "Remove grouping Channel" }));
    await waitFor(() => expect(strip()).toContain("Space full value · Enter row detail · Ctrl+C copy · Ctrl+Shift+C copy rows"));
  });

  it("a_grouped_i_read_for_another_column_is_dropped_and_cancelled", async () => {
    // Every cell is clipped, so I reads the whole value; the read is held while the focus moves to another column.
    overrideCommand("grid_page", async (args) => {
      const page = (await mockCall("grid_page", args)) as GridPage;
      return { ...page, cells: page.cells.map((row) => row.map((c) => (c === null ? c : { ...c, clipped: true }))) };
    });
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    overrideCommand("cell_value", async (args) => {
      await gate;
      return { text: `whole ${(args as { column: string }).column}`, parts: [] };
    });
    const grouped: Tab = {
      id: "scoped:grouped", kind: "dataset", relation: "mock_events", title: "grouped",
      seed: { state: initialGridState("mock_events"), by: ["Channel"] },
    };
    const { user } = await mountGrid(grouped);
    await user.click((await screen.findAllByRole("button", { name: /^Expand Channel/ }))[0]);
    await waitFor(() => expect(document.querySelectorAll("[data-kind='row']").length).toBeGreaterThan(2));
    const first = document.querySelector<HTMLElement>("[data-kind='row']")!;
    await user.click(within(first).getAllByRole("gridcell")[2]);
    await user.keyboard("i");
    await waitFor(() => expect(callsOf("cell_value")).toHaveLength(1));
    const id = queryIdOf("cell_value");
    await user.keyboard("{ArrowRight}");
    await waitFor(() => expect(cancelledIds()).toContain(id));
    await act(async () => {
      release();
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(screen.queryByRole("dialog", { name: "Add IOC" })).toBeNull();
  });
});

describe("fix round 3", () => {
  it("a_grouped_i_read_is_dropped_when_another_column_takes_its_place", async () => {
    // Every cell is clipped, so I reads the whole value; the read is held while a pin moves the columns.
    overrideCommand("grid_page", async (args) => {
      const page = (await mockCall("grid_page", args)) as GridPage;
      return { ...page, cells: page.cells.map((row) => row.map((c) => (c === null ? c : { ...c, clipped: true }))) };
    });
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    overrideCommand("cell_value", async (args) => {
      await gate;
      return { text: `whole ${(args as { column: string }).column}`, parts: [] };
    });
    const grouped: Tab = {
      id: "scoped:grouped", kind: "dataset", relation: "mock_events", title: "grouped",
      seed: { state: initialGridState("mock_events"), by: ["Channel"] },
    };
    const { user } = await mountGrid(grouped);
    await user.click((await screen.findAllByRole("button", { name: /^Expand Channel/ }))[0]);
    await waitFor(() => expect(document.querySelectorAll("[data-kind='row']").length).toBeGreaterThan(2));
    const names = () => screen.getAllByRole("columnheader").flatMap((h) => (h.querySelector(".col-name") ? [h.querySelector(".col-name")!.textContent] : []));
    const at = names().indexOf("Channel");
    const first = document.querySelector<HTMLElement>("[data-kind='row']")!;
    await user.click(within(first).getAllByRole("gridcell")[at + 1]);
    await user.keyboard("i");
    await waitFor(() => expect(callsOf("cell_value")).toHaveLength(1));
    expect((callsOf("cell_value")[0].args as { column: string }).column).toBe("Channel");
    const id = queryIdOf("cell_value");
    // Pinning a later column puts it first: the focused position now holds the column before Channel's old place.
    fireEvent.contextMenu(header("Provider"));
    fireEvent.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Pin left" }));
    await waitFor(() => expect(names()[at]).not.toBe("Channel"));
    await waitFor(() => expect(cancelledIds()).toContain(id));
    await act(async () => {
      release();
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(screen.queryByRole("dialog", { name: "Add IOC" })).toBeNull();
  });
});

describe("fix round 4", () => {
  it("a_grouped_i_answer_between_a_commit_and_its_passive_effects_opens_nothing", async () => {
    // Every cell is clipped, so I reads the whole value; the read is held.
    overrideCommand("grid_page", async (args) => {
      const page = (await mockCall("grid_page", args)) as GridPage;
      return { ...page, cells: page.cells.map((row) => row.map((c) => (c === null ? c : { ...c, clipped: true }))) };
    });
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    overrideCommand("cell_value", async (args) => {
      await gate;
      return { text: `whole ${(args as { column: string }).column}`, parts: [] };
    });
    await caseOpen("/cases/x.duckdb");
    const summary: CaseSummary = await caseSummary("t");
    const first = (await settingsGet()).settings;
    const ctx: ShellContext = {
      summary, settings: first, change: { generation: 0, kinds: [] }, openDataset: () => {}, openScoped: vi.fn(), setGridStatus: () => {},
      setRightPane: () => {}, rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError: vi.fn(), saveLayout: vi.fn(),
      setHistogram: vi.fn(), saveColourRules: vi.fn(),
    };
    const grouped: Tab = {
      id: "scoped:grouped", kind: "dataset", relation: "mock_events", title: "grouped",
      seed: { state: initialGridState("mock_events"), by: ["Channel"] },
    };
    // The shell's settings, held here. `adopt` is a pin saved from another tab of the relation arriving: it is not an
    // input event, so React renders it at default priority and leaves the passive effects of its commits to a later
    // scheduler task. (After a click React runs them inside the commit, which leaves no gap for an answer.)
    const host = createRef<{ adopt(layout: ColumnLayout): void }>();
    // Host's commits, and how many of them have had their passive effects run.
    const ran = { commits: 0, effects: 0 };
    function Host() {
      const [settings, setSettings] = useState(first);
      const [, setSeen] = useState(first);
      useImperativeHandle(host, () => ({
        adopt: (layout) => setSettings((s) => ({ ...s, layouts: { ...s.layouts, mock_events: layout } })),
      }));
      // The grid adopts the new layout in a passive effect. This one sets state in the same flush, so Host renders
      // again in the commit that moves the columns, and the two counters below then speak for that commit.
      useEffect(() => setSeen(settings), [settings]);
      useLayoutEffect(() => {
        ran.commits += 1;
      });
      useEffect(() => {
        ran.effects += 1;
      });
      return (
        <ShellCtx.Provider value={{ ...ctx, settings }}>
          <Grid tab={grouped} active scrollOffset={0} onScroll={() => {}} />
        </ShellCtx.Provider>
      );
    }
    render(<Host />);
    const user = userEvent.setup();
    await user.click((await screen.findAllByRole("button", { name: /^Expand Channel/ }))[0]);
    await waitFor(() => expect(document.querySelectorAll("[data-kind='row']").length).toBeGreaterThan(2));
    const names = () => [...document.querySelectorAll("[role='columnheader'] .col-name")].map((n) => n.textContent);
    const at = names().indexOf("Channel");
    await user.click(within(document.querySelector<HTMLElement>("[data-kind='row']")!).getAllByRole("gridcell")[at + 1]);
    await user.keyboard("i");
    await waitFor(() => expect(callsOf("cell_value")).toHaveLength(1));
    expect((callsOf("cell_value")[0].args as { column: string }).column).toBe("Channel");
    const metas = await mockCall<ColumnMeta[]>("columns", { queryId: "t", slot: "t", relation: "mock_events" });
    // Pinning a later column puts it first: the focused position then holds the column before Channel's old place.
    const pinned = togglePin(materialise(null, metas), "Provider");

    // The answer is released as soon as the pin is in the document. A mutation observer's callback runs when the task
    // that committed ends, and every step of the answer is a promise step, so the answer is handled right there:
    // after the commit and its layout effects, before the scheduler's next task.
    let behind: number | null = null;
    const committed = new MutationObserver(() => {
      if (behind !== null || names()[at] === "Channel") return;
      behind = ran.commits - ran.effects;
      release();
    });
    committed.observe(document.body, { childList: true, subtree: true, attributes: true });
    const env = globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean };
    env.IS_REACT_ACT_ENVIRONMENT = false;
    try {
      host.current!.adopt(pinned);
      await waitFor(() => expect(behind).not.toBeNull(), { timeout: 5000 });
    } finally {
      committed.disconnect();
      env.IS_REACT_ACT_ENVIRONMENT = true;
    }
    await settle();
    // The ordering this test is for: the commit that moved the columns had not run its passive effects.
    expect(behind).toBe(1);
    expect(names()[at]).not.toBe("Channel");
    expect(screen.queryByRole("dialog", { name: "Add IOC" })).toBeNull();
  });
});
