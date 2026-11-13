// The merged tab's *open* link: the dataset tab shows the row, scrolls its grid to it and focuses it; a row the tab's
// filters hide says so and can be opened in an unfiltered tab, which scrolls there.
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { caseOpen, caseSummary, settingsGet } from "../api";
import type { GridState } from "../contract/generated/GridState";
import Grid from "../grid/Grid";
import { cellChip } from "../grid/chips";
import { initialGridState } from "../grid/gridState";
import { mockCall } from "../mock/backend";
import { ShellCtx, type ShellContext } from "../shell/context";
import type { Tab } from "../shell/tabs";
import { callsOf, overrideCommand } from "../test/backend";

vi.mock("../grid/measure", () => ({ measureText: vi.fn(() => 50) }));

// jsdom has no layout: every element is 1200 x 440, and scrolling (scrollTo, scrollTop) is kept per element and announced.
const scrollTops = new WeakMap<Element, number>();
beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "scrollTop", {
    configurable: true,
    get() {
      return scrollTops.get(this) ?? 0;
    },
    set(v: number) {
      scrollTops.set(this, v);
    },
  });
  Object.defineProperty(HTMLElement.prototype, "scrollTo", {
    configurable: true,
    value(this: HTMLElement, o: { top?: number }) {
      scrollTops.set(this, o.top ?? 0);
      this.dispatchEvent(new Event("scroll"));
    },
  });
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", { configurable: true, get: () => 110_000 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 440 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
});
afterEach(() => vi.restoreAllMocks());

const events: Tab = { id: "dataset:mock_events", kind: "dataset", relation: "mock_events" };
const ROW = 3000;

async function context(extra: Partial<ShellContext> = {}) {
  await caseOpen("/cases/x.duckdb");
  const summary = await caseSummary("t");
  const settings = (await settingsGet()).settings;
  const panes: ReactNode[] = [];
  const ctx: ShellContext = {
    summary, settings, change: { generation: 0, kinds: [] }, openDataset: () => {}, openScoped: vi.fn(), setGridStatus: () => {},
    setRightPane: (n) => panes.push(n), rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError: vi.fn(), saveLayout: vi.fn(),
    setHistogram: vi.fn(), saveColourRules: vi.fn(), ...extra,
  };
  return { ctx, panes };
}

const focusedRow = () => document.querySelector("[data-focused]")?.closest("[data-row-index]")?.getAttribute("data-row-index") ?? null;
const scroller = () => screen.getByTestId("grid-scroll");

function show(ctx: ShellContext, tab: Tab) {
  return render(
    <ShellCtx.Provider value={ctx}>
      <Grid tab={tab} active scrollOffset={0} onScroll={() => {}} />
    </ShellCtx.Provider>,
  );
}

describe("open a row from the merged tab", () => {
  it("open_scrolls_the_grid_to_the_row_and_focuses_it", async () => {
    const clear = vi.fn();
    const { ctx, panes } = await context({ pendingRow: { tabId: events.id, rowId: ROW }, clearPendingRow: clear });
    const offset = (await mockCall<number | null>("grid_row_offset", { queryId: "q", slot: "x", state: initialGridState("mock_events"), rowId: ROW }))!;
    expect(offset).toBeGreaterThan(1000);
    show(ctx, events);
    await waitFor(() => expect(callsOf("grid_row_offset")).toHaveLength(1));
    const asked = callsOf("grid_row_offset")[0].args as { state: GridState; rowId: number; slot: string };
    expect(asked.rowId).toBe(ROW);
    expect(asked.state.relation).toBe("mock_events");
    expect(asked.slot).toBe(`tab:${events.id}:reveal`);
    expect(clear).toHaveBeenCalled();
    // The grid moved to the row, drew it and focused it; it is the one selected.
    await waitFor(() => expect(scroller().scrollTop).toBeGreaterThan(1000));
    await waitFor(() => expect(focusedRow()).toBe(String(offset)));
    expect(document.querySelector(`[data-row-index='${offset}']`)).toHaveAttribute("aria-selected", "true");
    // The right pane was handed the detail of that row.
    await waitFor(() => expect(panes.at(-1)).not.toBeNull());
    render(<ShellCtx.Provider value={ctx}>{panes.at(-1)}</ShellCtx.Provider>);
    await waitFor(() => expect(callsOf("row_view").at(-1)!.args).toMatchObject({ rowId: ROW }));
  });

  it("a_row_hidden_by_the_filters_says_so_and_opens_unfiltered", async () => {
    const { ctx } = await context({ pendingRow: { tabId: events.id, rowId: ROW }, clearPendingRow: vi.fn() });
    overrideCommand("grid_row_offset", () => null);
    const user = userEvent.setup();
    show(ctx, events);
    expect(await screen.findByText("This row is hidden by the current filters.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Open mock_events unfiltered" }));
    expect(ctx.openScoped).toHaveBeenCalledTimes(1);
    const tab = vi.mocked(ctx.openScoped).mock.calls[0][0];
    expect(tab).toMatchObject({ kind: "dataset", relation: "mock_events", reveal: ROW });
    expect(tab.seed!.state.chips).toEqual([]);
    expect(screen.queryByText("This row is hidden by the current filters.")).toBeNull();

    // That tab, when it mounts, asks where the row is in its own (unfiltered) order and scrolls there.
    overrideCommand("grid_row_offset", (a) => mockCall("grid_row_offset", a as Record<string, unknown>));
    const offset = (await mockCall<number | null>("grid_row_offset", { queryId: "q", slot: "x", state: tab.seed!.state, rowId: ROW }))!;
    render(
      <ShellCtx.Provider value={ctx}>
        <Grid tab={tab} active scrollOffset={0} onScroll={() => {}} />
      </ShellCtx.Provider>,
    );
    await waitFor(() => expect(focusedRow()).toBe(String(offset)));
    expect(scrollTops.get(screen.getAllByTestId("grid-scroll")[1])).toBeGreaterThan(1000);
  });
});

const settle = (ms: number) => act(() => new Promise((r) => setTimeout(r, ms)));
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const realOffset = async (rowId: number, state: GridState = initialGridState("mock_events")) =>
  (await mockCall<number | null>("grid_row_offset", { queryId: "q", slot: "x", state, rowId }))!;

describe("a reveal that has gone stale is dropped", () => {
  it("a_state_change_before_the_row_count_allows_the_scroll_drops_the_target", async () => {
    // The counts are slow, so the scroll waits; the analyst re-sorts meanwhile: the offset was for the old order.
    overrideCommand("grid_counts", async (a) => {
      await sleep(400);
      return mockCall("grid_counts", a as Record<string, unknown>);
    });
    const { ctx } = await context({ pendingRow: { tabId: events.id, rowId: ROW }, clearPendingRow: vi.fn() });
    const user = userEvent.setup();
    show(ctx, events);
    await waitFor(() => expect(callsOf("grid_row_offset")).toHaveLength(1));
    await settle(100); // the offset has been answered and is waiting for the row count
    const header = screen.getAllByRole("columnheader").find((h) => h.querySelector(".col-name")?.textContent === "EventId")!;
    await user.click(header);
    await waitFor(() => expect(callsOf("grid_page").some((c) => (c.args as { state: GridState }).state.sort.length > 0)).toBe(true));
    await settle(1200); // the counts for both states have arrived: 5,000 rows now cover the old offset
    expect(focusedRow()).toBeNull();
    expect(scroller().scrollTop).toBe(0);
  });

  it("an_answer_that_arrives_while_the_tab_is_hidden_applies_on_activation_if_its_state_is_unchanged", async () => {
    const { ctx } = await context({ pendingRow: { tabId: events.id, rowId: ROW }, clearPendingRow: vi.fn() });
    const offset = await realOffset(ROW);
    const tree = (active: boolean) => (
      <ShellCtx.Provider value={ctx}>
        <Grid tab={events} active={active} scrollOffset={0} onScroll={() => {}} />
      </ShellCtx.Provider>
    );
    const view = render(tree(false));
    await waitFor(() => expect(callsOf("grid_row_offset")).toHaveLength(1));
    await settle(200);
    expect(focusedRow()).toBeNull();
    expect(scroller().scrollTop).toBe(0);
    view.rerender(tree(true));
    await waitFor(() => expect(focusedRow()).toBe(String(offset)));
  });

  it("a_newer_open_request_supersedes_an_older_answer", async () => {
    let first = true;
    overrideCommand("grid_row_offset", async (a) => {
      const slow = first;
      first = false;
      if (slow) await sleep(300);
      return mockCall("grid_row_offset", a as Record<string, unknown>);
    });
    const { ctx } = await context({ pendingRow: { tabId: events.id, rowId: ROW }, clearPendingRow: vi.fn() });
    const tree = (rowId: number) => (
      <ShellCtx.Provider value={{ ...ctx, pendingRow: { tabId: events.id, rowId } }}>
        <Grid tab={events} active scrollOffset={0} onScroll={() => {}} />
      </ShellCtx.Provider>
    );
    const view = render(tree(ROW));
    await waitFor(() => expect(callsOf("grid_row_offset")).toHaveLength(1));
    view.rerender(tree(400));
    const wanted = await realOffset(400);
    await waitFor(() => expect(focusedRow()).toBe(String(wanted)));
    await settle(600); // the slow answer for the first request arrives late and is not drawn
    expect(focusedRow()).toBe(String(wanted));
  });
});

describe("open unfiltered", () => {
  it("every_request_opens_a_tab_that_reveals_its_row_even_after_a_filter_was_added", async () => {
    const ROW2 = ROW; // the same row again: under a fixed id per row the second request would only re-activate tab A
    // A tab with a filter hides the row; an unfiltered tab shows it.
    overrideCommand("grid_row_offset", (a) => {
      const { state } = a as { state: GridState };
      return state.chips.length > 0 ? null : mockCall("grid_row_offset", a as Record<string, unknown>);
    });
    const filtered: Tab = {
      ...events, seed: { state: { ...initialGridState("mock_events"), chips: [cellChip("EventId", "eq", "4624")] }, by: [] },
    };
    const { ctx } = await context({ pendingRow: { tabId: events.id, rowId: ROW }, clearPendingRow: vi.fn() });
    const user = userEvent.setup();
    const first = show(ctx, filtered);
    await user.click(await screen.findByRole("button", { name: "Open mock_events unfiltered" }));
    const tabA = vi.mocked(ctx.openScoped).mock.calls[0][0];
    first.unmount();

    // Tab A opens and shows its row; then the analyst adds a filter in it.
    const offsetA = await realOffset(ROW);
    const offsetB = await realOffset(ROW2);
    const a = show(ctx, tabA);
    await waitFor(() => expect(focusedRow()).toBe(String(offsetA)));
    await user.type(screen.getByLabelText("Match EventId"), "4624");
    await waitFor(() => expect(callsOf("grid_page").at(-1)!.args!.state).toMatchObject({ chips: [expect.anything()] }));
    // Open unfiltered again, for another row, from that filtered tab: a new request, a tab of its own.
    a.rerender(
      <ShellCtx.Provider value={{ ...ctx, pendingRow: { tabId: tabA.id, rowId: ROW2 } }}>
        <Grid tab={tabA} active scrollOffset={0} onScroll={() => {}} />
      </ShellCtx.Provider>,
    );
    await user.click(await screen.findByRole("button", { name: "Open mock_events unfiltered" }));
    expect(ctx.openScoped).toHaveBeenCalledTimes(2);
    const tabB = vi.mocked(ctx.openScoped).mock.calls[1][0];
    expect(tabB.id).not.toBe(tabA.id);
    expect(tabB.seed!.state.chips).toEqual([]);
    a.unmount();
    show(ctx, tabB);
    await waitFor(() => expect(focusedRow()).toBe(String(offsetB)));
  });
});
