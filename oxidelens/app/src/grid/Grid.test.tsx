import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { caseOpen, caseSummary, settingsGet } from "../api";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { GridPage } from "../contract/generated/GridPage";
import type { GridState } from "../contract/generated/GridState";
import { HOSTILE } from "../mock/generated";
import { mockCall } from "../mock/backend";
import { callsOf, overrideCommand } from "../test/backend";
import { ShellCtx, type ShellContext } from "../shell/context";
import type { Tab } from "../shell/tabs";
import { displaySafe } from "../text";
import * as copy from "./copy";
import Grid from "./Grid";

vi.mock("./copy", async (orig) => ({ ...(await orig<typeof import("./copy")>()), writeClipboard: vi.fn(async () => {}) }));

// jsdom has no layout: every element is 1200 x 440, and scrollTop is kept per element so a test can scroll.
const scrollTops = new WeakMap<Element, number>();
beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 440 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "scrollTop", {
    configurable: true,
    get() {
      return scrollTops.get(this) ?? 0;
    },
    set(v: number) {
      scrollTops.set(this, v);
      queueMicrotask(() => (this as HTMLElement).dispatchEvent(new Event("scroll"))); // as a browser does
    },
  });
  vi.mocked(copy.writeClipboard).mockClear();
});
afterEach(() => vi.restoreAllMocks());

const events: Tab = { id: "dataset:mock_events", kind: "dataset", relation: "mock_events" };
const hayabusa: Tab = { id: "dataset:mock_hayabusa", kind: "dataset", relation: "mock_hayabusa" };

async function mount(tab: Tab = events, scrollOffset = 0) {
  await caseOpen("/cases/x.duckdb");
  const summary: CaseSummary = await caseSummary("t");
  const settings = (await settingsGet()).settings;
  const status = vi.fn();
  const ctx: ShellContext = {
    summary, settings, change: { generation: 0, kinds: [] }, openDataset: () => {}, openScoped: () => {}, setGridStatus: status,
    setRightPane: () => {}, rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError: vi.fn(), saveLayout: vi.fn(), setHistogram: vi.fn(), saveColourRules: vi.fn(),
  };
  const onScroll = vi.fn();
  const utils = render(
    <ShellCtx.Provider value={ctx}>
      <Grid tab={tab} active scrollOffset={scrollOffset} onScroll={onScroll} />
    </ShellCtx.Provider>,
  );
  await waitFor(() => expect(screen.getAllByRole("columnheader").length).toBeGreaterThan(1));
  await waitFor(() => expect(dataRows().length).toBeGreaterThan(0));
  return { ...utils, status, onScroll, ctx, user: userEvent.setup() };
}

/** The status line as text: the grid hands the shell a node (its "N hidden" is a button). */
const statusText = (status: ReturnType<typeof vi.fn>): string => {
  const node = status.mock.calls.at(-1)?.[0] as ReactNode;
  if (node === null || node === undefined) return "";
  const { container, unmount } = render(<>{node}</>);
  const text = container.textContent ?? "";
  unmount();
  return text;
};

const scroller = () => screen.getByTestId("grid-scroll");
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
const lastState = (): GridState => (callsOf("grid_page").at(-1)!.args as { state: GridState }).state;

async function scrollToRow(row: number) {
  scrollTops.set(scroller(), row * 22);
  await act(async () => {
    fireEvent.scroll(scroller());
  });
}

describe("grid", () => {
  it("renders_a_window_not_every_row", async () => {
    await mount();
    const rows = dataRows();
    expect(rows.length).toBeGreaterThan(10);
    expect(rows.length).toBeLessThan(80);
    expect(scroller().querySelectorAll("[data-row-index]").length).toBeLessThan(80);
  });

  it("scrolling_fetches_blocks_and_drops_far_ones", async () => {
    await mount();
    const offsets = () => callsOf("grid_page").map((c) => (c.args as { offset: number }).offset);
    expect(offsets()).toEqual([0]);
    for (const a of callsOf("grid_page")) {
      const args = a.args as { limit: number; slot: string; offset: number };
      expect(args.limit).toBe(200);
      expect(args.slot).toBe(`tab:dataset:mock_events:page:${args.offset}`);
    }
    // Walk down the whole table, one block at a time: 25 blocks, at most 12 kept.
    for (let b = 1; b < 25; b++) {
      await scrollToRow(b * 200 + 5);
      await waitFor(() => expect(offsets()).toContain(b * 200));
    }
    expect(offsets().every((o) => o % 200 === 0)).toBe(true);
    await waitFor(() => expect(dataRows().length).toBeGreaterThan(0));
    const before = offsets().filter((o) => o === 0).length;
    await scrollToRow(0); // block 0 was dropped long ago: it comes again
    await waitFor(() => expect(offsets().filter((o) => o === 0).length).toBe(before + 1));
    // A block still near stays cached: scrolling a little does not refetch it.
    await waitFor(() => expect(dataRows().length).toBeGreaterThan(0));
    const n = callsOf("grid_page").length;
    await scrollToRow(30);
    await act(() => new Promise((r) => setTimeout(r, 60)));
    expect(callsOf("grid_page").length).toBe(n);
  });

  it("shift_click_adds_secondary_sort", async () => {
    await mount();
    fireEvent.click(header("Channel"));
    await waitFor(() => expect(lastState().sort).toEqual([{ column: "Channel", descending: false }]));
    fireEvent.click(header("Channel"));
    await waitFor(() => expect(lastState().sort).toEqual([{ column: "Channel", descending: true }]));
    fireEvent.click(header("Channel"));
    await waitFor(() => expect(lastState().sort).toEqual([]));
    fireEvent.click(header("Channel"));
    await waitFor(() => expect(lastState().sort).toHaveLength(1));
    fireEvent.click(header("Level"), { shiftKey: true });
    await waitFor(() =>
      expect(lastState().sort).toEqual([
        { column: "Channel", descending: false },
        { column: "Level", descending: false },
      ]),
    );
    await waitFor(() => expect(header("Level")).toHaveAttribute("aria-sort", "ascending"));
    expect(header("Channel")).toHaveAttribute("aria-sort", "ascending");
  });

  it("mark_column_is_first_and_fixed", async () => {
    await mount();
    const heads = screen.getAllByRole("columnheader");
    expect(heads).toHaveLength(38); // the mark column and the 37 shown columns (three metadata columns are hidden by default)
    expect(heads[0]).toHaveAttribute("data-fixed", "true");
    expect(heads[0]).toHaveAccessibleName("Mark");
    expect(within(rowOf(4)).getByLabelText("suspicious")).toBeInTheDocument(); // row id 5 is marked
    expect(within(rowOf(0)).queryByLabelText("suspicious")).toBeNull();
  });

  it("failed_and_clipped_cells_are_marked", async () => {
    overrideCommand("grid_page", async (args) => {
      const page = (await mockCall("grid_page", args)) as GridPage;
      const i = page.columns.findIndex((c) => c.name === "EventId");
      page.cells[0][i] = { text: "12x", display: null, failed: true, clipped: false };
      return page;
    });
    await mount();
    const failed = within(rowOf(0)).getByText("12x");
    expect(failed).toHaveAttribute("title", "did not convert");
    expect(failed.className).toContain("cell-failed");
    // Row index 6 holds the 5,000 character value, which the engine clipped to 2,000.
    const clipped = cellOf(rowOf(6), "Message").querySelector("[data-clipped]")!;
    expect(clipped.textContent!.startsWith("LONG-")).toBe(true);
    expect(clipped.textContent!.endsWith("…")).toBe(true);
    // The unknown-zone time column carries the ?tz badge.
    expect(header("LocalTime").textContent).toContain("?tz");
    expect(header("TimeCreated").textContent).not.toContain("?tz");
  });

  it("copy_cell_and_rows_use_stored_text", async () => {
    overrideCommand("grid_page", async (args) => {
      const page = (await mockCall("grid_page", args)) as GridPage;
      const i = page.columns.findIndex((c) => c.name === "TimeCreated");
      page.cells[0][i] = { text: "2026-01-05T02:13:58.2040000Z", display: "2026-01-05 02:13:58.2040000", failed: false, clipped: false };
      return page;
    });
    const { user } = await mount();
    const cell = cellOf(rowOf(0), "TimeCreated");
    expect(cell).toHaveTextContent("2026-01-05 02:13:58.2040000");
    await user.click(cell);
    await user.keyboard("{Control>}c{/Control}");
    await waitFor(() => expect(copy.writeClipboard).toHaveBeenCalledWith("2026-01-05T02:13:58.2040000Z"));

    await user.keyboard("{Control>}{Shift>}c{/Shift}{/Control}");
    await waitFor(() => expect(vi.mocked(copy.writeClipboard)).toHaveBeenCalledTimes(2));
    const text = vi.mocked(copy.writeClipboard).mock.calls[1][0];
    const lines = text.split("\n");
    expect(lines).toHaveLength(2);
    const names = lines[0].split("\t");
    expect(names[0]).toBe("_triage_host");
    expect(names).toHaveLength(37);
    const values = lines[1].split("\t");
    expect(values[names.indexOf("TimeCreated")]).toBe("2026-01-05T02:13:58.2040000Z");
    expect(text).not.toContain("2026-01-05 02:13:58");
  });

  it("copy_cell_of_a_clipped_value_fetches_the_whole_text", async () => {
    const { user } = await mount();
    await user.click(cellOf(rowOf(6), "Message"));
    await user.keyboard("{Control>}c{/Control}");
    await waitFor(() => expect(copy.writeClipboard).toHaveBeenCalledWith(HOSTILE.long));
  });

  it("a_state_change_during_a_copys_cell_value_fetch_copies_nothing", async () => {
    overrideCommand("cell_value", async (args) => {
      await new Promise((r) => setTimeout(r, 300));
      return mockCall("cell_value", args);
    });
    const { user, ctx } = await mount();
    await user.click(cellOf(rowOf(6), "Message")); // the clipped 5,000 character value
    await user.keyboard("{Control>}{Shift>}c{/Shift}{/Control}");
    await waitFor(() => expect(callsOf("cell_value").length).toBeGreaterThan(0));
    const valueId = (callsOf("cell_value")[0].args as { queryId: string }).queryId;
    fireEvent.click(header("Channel")); // the view changes while the value is still on its way
    await act(() => new Promise((r) => setTimeout(r, 500)));
    expect(copy.writeClipboard).not.toHaveBeenCalled();
    expect(ctx.reportError).toHaveBeenCalledWith(expect.objectContaining({ message: "the view changed; copy again" }));
    const cancels = callsOf("query_cancel").flatMap((c) => (c.args as { queryIds: string[] }).queryIds);
    expect(cancels).toContain(valueId);
  });

  it("space_opens_split_value", async () => {
    overrideCommand("cell_value", () => ({
      text: "User: Administrator ¦ SrcIP: 203.0.113.7",
      parts: [{ key: "User", value: "Administrator" }, { key: "SrcIP", value: "203.0.113.7" }],
    }));
    const { user } = await mount(hayabusa);
    await user.click(cellOf(rowOf(0), "Details"));
    await user.keyboard(" ");
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText("User: Administrator")).toBeInTheDocument();
    expect(within(dialog).getByText("SrcIP: 203.0.113.7")).toBeInTheDocument();
    const args = callsOf("cell_value")[0].args as { rowId: number; column: string };
    expect(args.column).toBe("Details");
    expect(args.rowId).toBe(5001);
    await user.click(within(dialog).getByRole("button", { name: "Copy" }));
    expect(copy.writeClipboard).toHaveBeenCalledWith("User: Administrator ¦ SrcIP: 203.0.113.7");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("the_viewer_traps_focus", async () => {
    const { user } = await mount();
    await user.click(cellOf(rowOf(0), "Message"));
    await user.keyboard(" ");
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    for (let i = 0; i < 5; i++) {
      await user.tab();
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
    await user.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("hostile_cells_render_inert", async () => {
    const { user } = await mount();
    const grid = screen.getByRole("grid");
    expect(grid.querySelector("img")).toBeNull();
    for (const v of [HOSTILE.html, HOSTILE.joiner, HOSTILE.bell, HOSTILE.formula]) {
      expect(within(grid).getByText(displaySafe(v))).toBeInTheDocument();
    }
    expect(within(grid).getByText("report\\u{202E}gpj.exe")).toBeInTheDocument();
    expect(grid.textContent).not.toContain("\u202E");
    expect(grid.textContent).not.toContain("\u0007");
    // The 5,000 character value is drawn clipped, ending with an ellipsis, and as text.
    const clipped = cellOf(rowOf(6), "Message").querySelector("[data-clipped]")!;
    expect(clipped.textContent!.length).toBeLessThan(HOSTILE.long.length);
    expect(clipped.textContent!.endsWith("…")).toBe(true);

    // It opens whole in the viewer; hostile text in the viewer is inert too.
    await user.click(cellOf(rowOf(6), "Message"));
    await user.keyboard(" ");
    let dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(dialog.querySelector(".viewer-text")?.textContent).toBe(HOSTILE.long));
    await user.keyboard("{Escape}");
    for (const [row, column, shown] of [
      [1, "UserName", displaySafe(HOSTILE.html)],
      [2, "UserName", "report\\u{202E}gpj.exe"],
      [3, "ProcessName", displaySafe(HOSTILE.joiner)],
      [4, "Message", displaySafe(HOSTILE.bell)],
      [5, "CommandLine", displaySafe(HOSTILE.formula)],
    ] as const) {
      await user.click(cellOf(rowOf(row), column));
      await user.keyboard(" ");
      dialog = await screen.findByRole("dialog");
      await waitFor(() => expect(dialog.querySelector(".viewer-text")?.textContent).toBe(shown));
      expect(dialog.querySelector("img")).toBeNull();
      expect(dialog.textContent).not.toContain("\u202E");
      expect(dialog.textContent).not.toContain("\u0007");
      await user.keyboard("{Escape}");
      await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    }
  });

  it("no_old_row_text_is_in_the_dom_after_a_state_change_before_the_new_page_arrives", async () => {
    let n = 0;
    overrideCommand("grid_page", async (args) => {
      n += 1;
      if (n > 1) await new Promise((r) => setTimeout(r, 300));
      return mockCall("grid_page", args);
    });
    await mount();
    const old = cellOf(rowOf(0), "RecordId").textContent!;
    expect(old).toBe("100000");
    fireEvent.click(header("RecordId")); // ascending: still the same first row, so use descending next
    fireEvent.click(header("RecordId"));
    await waitFor(() => expect(lastState().sort).toEqual([{ column: "RecordId", descending: true }]));
    expect(within(screen.getByRole("grid")).queryByText("100000")).toBeNull();
    expect(within(screen.getByRole("grid")).queryByText("100001")).toBeNull();
    await waitFor(() => expect(within(screen.getByRole("grid")).queryByText("104999")).not.toBeNull());
  });

  it("shift_click_across_unloaded_blocks_selects_the_whole_range_and_copy_fetches_it", async () => {
    const { user, status } = await mount();
    await user.click(cellOf(rowOf(10), "Channel"));
    await scrollToRow(1500);
    await waitFor(() => expect(callsOf("grid_page").some((c) => (c.args as { offset: number }).offset === 1400)).toBe(true));
    await waitFor(() => expect(rowOf(1500)).toBeTruthy());
    await waitFor(() => expect(cellOf(rowOf(1500), "Channel").textContent).not.toBe(""), { timeout: 5000 });
    await user.keyboard("{Shift>}");
    await user.click(cellOf(rowOf(1500), "Channel"));
    await user.keyboard("{/Shift}");
    await waitFor(() => expect(statusText(status)).toContain(" · 1,491 selected · "));
    const before = callsOf("grid_page").length;
    await user.keyboard("{Control>}{Shift>}c{/Shift}{/Control}");
    await waitFor(() => expect(copy.writeClipboard).toHaveBeenCalledTimes(1), { timeout: 15_000 });
    const text = vi.mocked(copy.writeClipboard).mock.calls[0][0];
    // The mock holds no newline, so one line per record: the header and 1,491 rows.
    expect(text.split("\n")).toHaveLength(1492);
    const copySlots = callsOf("grid_page").slice(before).map((c) => (c.args as { slot: string }).slot);
    expect(copySlots.length).toBeGreaterThan(0);
    expect(copySlots.every((sl) => sl.startsWith("tab:dataset:mock_events:copy:"))).toBe(true);
  }, 30_000);

  it("selection_click_shift_ctrl_and_the_status_line", async () => {
    const { user, status } = await mount();
    await user.click(cellOf(rowOf(2), "Channel"));
    expect(rowOf(2)).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{Shift>}");
    await user.click(cellOf(rowOf(5), "Channel"));
    await user.keyboard("{/Shift}");
    for (const i of [2, 3, 4, 5]) expect(rowOf(i)).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{Control>}");
    await user.click(cellOf(rowOf(3), "Channel"));
    await user.keyboard("{/Control}");
    expect(rowOf(3)).toHaveAttribute("aria-selected", "false");
    expect(rowOf(4)).toHaveAttribute("aria-selected", "true");
    // The status is a node (its "N hidden" is a button that opens the column manager); the metadata columns but the host's are hidden.
    await waitFor(() => {
      const node = status.mock.calls.at(-1)?.[0] as ReactNode;
      expect(node).not.toBeNull();
      const { container } = render(<>{node}</>);
      expect(container.textContent).toBe("5,000 of 5,000 rows · 3 selected · 37 of 40 columns · 3 hidden ▾ · 0 values failed to convert");
    });
  });

  it("arrows_move_the_cell_focus_and_the_cell_menu_copies", async () => {
    const { user } = await mount();
    await user.click(cellOf(rowOf(0), "Channel"));
    await user.keyboard("{ArrowDown}{ArrowRight}");
    expect(cellOf(rowOf(1), "Provider")).toHaveAttribute("data-focused", "true");
    fireEvent.contextMenu(cellOf(rowOf(1), "Provider"));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual([
      "Copy cell", "Copy rows (with headers)", "Show full value (Space)", "Filter in", "Filter out",
    ]);
    await user.click(within(menu).getByRole("menuitem", { name: "Copy cell" }));
    await waitFor(() => expect(copy.writeClipboard).toHaveBeenCalled());
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("restores_and_reports_the_scroll_offset", async () => {
    const { onScroll } = await mount(events, 22 * 300);
    await waitFor(() => expect(callsOf("grid_page").some((c) => (c.args as { offset: number }).offset === 200)).toBe(true));
    await scrollToRow(10);
    expect(onScroll).toHaveBeenLastCalledWith(220);
  });
});
