// Nested grouping: the group bar, group header rows, expanding to fetch rows, paging groups, expand/collapse all, and the
// double-click tab. Hostile cell text and stale answers are pinned here too.
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { caseOpen, caseSummary, settingsGet } from "../api";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { GridState } from "../contract/generated/GridState";
import type { GroupView } from "../contract/generated/GroupView";
import { HOSTILE } from "../mock/generated";
import { mockCall } from "../mock/backend";
import { ShellCtx, type ShellContext } from "../shell/context";
import type { Tab } from "../shell/tabs";
import { callsOf, overrideCommand } from "../test/backend";
import Grid from "./Grid";

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
      queueMicrotask(() => (this as HTMLElement).dispatchEvent(new Event("scroll")));
    },
  });
});
const scrollToEnd = async () => {
  const el = screen.getByTestId("grid-scroll");
  await act(async () => {
    el.scrollTop = 1_000_000;
  });
};
afterEach(() => vi.restoreAllMocks());

const events: Tab = { id: "dataset:mock_events", kind: "dataset", relation: "mock_events" };

interface Mounted {
  utils: ReturnType<typeof render>;
  ctx: ShellContext;
  summary: CaseSummary;
  rerenderWith(patch: Partial<ShellContext>): void;
  openScoped: ReturnType<typeof vi.fn>;
}

async function mount(tab: Tab = events): Promise<Mounted> {
  await caseOpen("/cases/x.duckdb");
  const summary = await caseSummary("t");
  const settings = (await settingsGet()).settings;
  const openScoped = vi.fn();
  const ctx: ShellContext = {
    summary, settings, change: { generation: 0, kinds: [] }, openDataset: () => {}, openScoped, setGridStatus: () => {},
    setRightPane: () => {}, rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError: vi.fn(), saveLayout: vi.fn(), setHistogram: vi.fn(), saveColourRules: vi.fn(),
  };
  const tree = (c: ShellContext) => (
    <ShellCtx.Provider value={c}>
      <Grid tab={tab} active scrollOffset={0} onScroll={() => {}} />
    </ShellCtx.Provider>
  );
  const utils = render(tree(ctx));
  await waitFor(() => expect(screen.getAllByRole("columnheader").length).toBeGreaterThan(1));
  await waitFor(() => expect(flatRows().length).toBeGreaterThan(0));
  return { utils, ctx, summary, openScoped, rerenderWith: (patch) => utils.rerender(tree({ ...ctx, ...patch })) };
}

const flatRows = () => screen.queryAllByRole("row").filter((r) => r.getAttribute("data-row-index") !== null);
const groupRows = () => screen.queryAllByRole("row").filter((r) => r.getAttribute("data-kind") === "group");
const memberRows = () => screen.queryAllByRole("row").filter((r) => r.getAttribute("data-kind") === "row");
const valueOf = (r: HTMLElement) => r.querySelector(".group-value")?.textContent ?? "";
const groupRow = (key: string, depth?: number) => {
  const r = groupRows().find((x) => valueOf(x) === key && (depth === undefined || x.getAttribute("data-depth") === String(depth)));
  if (!r) throw new Error(`no group row ${key}`);
  return r;
};
const header = (name: string) => {
  const h = screen.getAllByRole("columnheader").find((el) => el.querySelector(".col-name")?.textContent === name);
  if (!h) throw new Error(`no header ${name}`);
  return h;
};
async function groupBy(...columns: string[]) {
  for (const c of columns) {
    fireEvent.contextMenu(header(c));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Group by this column" }));
  }
  await waitFor(() => expect(groupRows().length).toBeGreaterThan(0));
}
const toggle = (row: HTMLElement) => fireEvent.click(within(row).getByRole("button", { name: /^(Expand|Collapse) / }));
const settle = (ms = 50) => act(() => new Promise((r) => setTimeout(r, ms)));
const groupCalls = () => callsOf("grid_groups").map((c) => c.args as { by: string[]; parent: (string | null)[]; limit: number; offset: number; zone: string; slot: string; queryId: string; state: GridState });
const pageCalls = () => callsOf("grid_page").map((c) => c.args as { state: GridState; limit: number; offset: number; slot: string });

const view = (key: string | null, count: number, over: Partial<GroupView> = {}): GroupView => ({
  key, key_clipped: false, full_key: null, count, first: null, last: null, suspicious: 0, ...over,
});

describe("grouping", () => {
  it("group_headers_show_counts_and_zoned_spans", async () => {
    const m = await mount();
    overrideCommand("grid_groups", () => ({
      items: [
        view("Security", 1234, { first: "2026-01-04 21:13:58.2040000", last: "2026-01-05 05:00:01.0000000", suspicious: 3 }),
        view(null, 5),
        view("System", 1, { suspicious: 0 }),
      ],
      truncated: false,
    }));
    await groupBy("Channel");
    const call = groupCalls().at(-1)!;
    expect(call.by).toEqual(["Channel"]);
    expect(call.parent).toEqual([]);
    expect([call.limit, call.offset, call.zone]).toEqual([200, 0, "UTC"]);
    expect(call.slot).toMatch(/^tab:dataset:mock_events:groups:/);

    const sec = groupRow("Security");
    expect(sec.textContent).toContain("1,234");
    expect(sec.textContent).toContain("2026-01-04 21:13:58.2040000");
    expect(sec.textContent).toContain("2026-01-05 05:00:01.0000000");
    expect(sec.textContent).toContain("3 suspicious");
    expect(groupRow("(empty)").textContent).toContain("5");
    expect(groupRow("(empty)").textContent).not.toContain("suspicious");

    // A zone change re-reads the groups in the new zone.
    const before = groupCalls().length;
    m.rerenderWith({ summary: { ...m.summary, display_zone: "America/New_York" } });
    await waitFor(() => expect(groupCalls().length).toBeGreaterThan(before));
    expect(groupCalls().at(-1)!.zone).toBe("America/New_York");
  });

  it("only_expanded_groups_fetch", async () => {
    await mount();
    const pages0 = pageCalls().length;
    await groupBy("Channel", "Level");
    await settle();
    const base = groupCalls().length; // adding the second column read the first level again
    expect(groupCalls().at(-1)!.by).toEqual(["Channel", "Level"]);
    expect(pageCalls().length).toBe(pages0); // grouping reads groups, not rows
    expect(memberRows()).toHaveLength(0);

    const first = groupRows()[0];
    const key = valueOf(first);
    toggle(first);
    await waitFor(() => expect(groupCalls().slice(base).map((c) => c.parent)).toEqual([[key]]));
    expect(pageCalls().length).toBe(pages0); // a middle level fetches groups only
    await waitFor(() => expect(groupRows().some((r) => r.getAttribute("data-depth") === "1")).toBe(true));

    const leaf = groupRows().find((r) => r.getAttribute("data-depth") === "1")!;
    toggle(leaf);
    await waitFor(() => expect(pageCalls().length).toBe(pages0 + 1));
    await waitFor(() => expect(memberRows().length).toBeGreaterThan(0));
    // Siblings stayed closed: no other group was read.
    expect(groupCalls()).toHaveLength(base + 1);
    const chips = pageCalls().at(-1)!.state.chips;
    expect(chips.map((c) => c.origin)).toEqual(["group", "group"]);
    expect(pageCalls().at(-1)!.limit).toBe(200);
  });

  it("nested_groups_fetch_with_parent_keys", async () => {
    await mount();
    await groupBy("Channel", "Level", "Computer");
    await settle();
    const base = groupCalls().length;
    expect(groupCalls().at(-1)!.by).toEqual(["Channel", "Level", "Computer"]);
    const a = groupRows()[0];
    const ka = valueOf(a);
    toggle(a);
    await waitFor(() => expect(groupRows().some((r) => r.getAttribute("data-depth") === "1")).toBe(true));
    const b = groupRows().find((r) => r.getAttribute("data-depth") === "1")!;
    const kb = valueOf(b);
    toggle(b);
    await waitFor(() => expect(groupRows().some((r) => r.getAttribute("data-depth") === "2")).toBe(true));
    expect(groupCalls().slice(base).map((c) => c.parent)).toEqual([[ka], [ka, kb]]);
    // Each level is indented one more than its parent.
    expect(groupRows().map((r) => r.getAttribute("data-depth"))).toContain("2");
  });

  it("clipped_keys_drill_with_full_key", async () => {
    await mount();
    const full = "x".repeat(2500);
    overrideCommand("grid_groups", (a) => {
      const parent = (a as { parent: unknown[] }).parent;
      if (parent.length === 0) return { items: [view("x".repeat(2000), 4, { key_clipped: true, full_key: full })], truncated: false };
      return { items: [view("Information", 4)], truncated: false };
    });
    await groupBy("Message", "Level");
    await settle();
    const base = groupCalls().length;
    expect(groupRows()[0].textContent).toContain("…");
    toggle(groupRows()[0]);
    await waitFor(() => expect(groupCalls().length).toBe(base + 1));
    expect(groupCalls()[base].parent).toEqual([full]); // the whole key, not the cut one
    await waitFor(() => expect(groupRow("Information")).toBeTruthy());
    toggle(groupRow("Information"));
    await waitFor(() => expect(pageCalls().length).toBeGreaterThan(0));
    const chips = pageCalls().at(-1)!.state.chips;
    expect(chips[0]).toMatchObject({ origin: "group", filter: { column: "Message", op: "eq", value: full, case_sensitive: true } });
    expect(chips[1]).toMatchObject({ origin: "group", filter: { column: "Level", op: "eq", value: "Information", case_sensitive: true } });
    for (const c of chips) {
      expect(c.id).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
      expect([...c.label].length).toBeLessThanOrEqual(200);
    }
  });

  it("double_click_opens_a_tab_with_group_chips", async () => {
    const m = await mount();
    overrideCommand("grid_groups", () => ({
      items: [view("Security", 10), view(null, 2), view(HOSTILE.html, 1), view(HOSTILE.bidi, 1)],
      truncated: false,
    }));
    await groupBy("Channel", "Level");
    fireEvent.doubleClick(groupRow("Security"));
    expect(m.openScoped).toHaveBeenCalledTimes(1);
    const tab = m.openScoped.mock.calls[0][0] as Tab;
    expect(tab.relation).toBe("mock_events");
    expect(tab.kind).toBe("dataset");
    expect(tab.seed!.by).toEqual(["Level"]); // the levels below the group stay grouped
    const [chip] = tab.seed!.state.chips;
    expect(chip).toMatchObject({ origin: "group", enabled: true, filter: { column: "Channel", op: "eq", value: "Security", case_sensitive: true } });

    // An empty key is `is_empty`; hostile text never reaches an id and the label is made safe.
    fireEvent.doubleClick(groupRow("(empty)"));
    expect((m.openScoped.mock.calls[1][0] as Tab).seed!.state.chips[0].filter).toMatchObject({ column: "Channel", op: "is_empty" });
    fireEvent.doubleClick(groupRows().find((r) => valueOf(r).includes("\\u{202E}"))!);
    const hostile = (m.openScoped.mock.calls[2][0] as Tab).seed!.state.chips[0];
    expect(hostile.id).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
    expect(hostile.label).not.toContain("‮");
    expect(hostile.filter).toMatchObject({ value: HOSTILE.bidi });
    // The same group again names the same tab, so the shell switches to it.
    fireEvent.doubleClick(groupRow("Security"));
    expect((m.openScoped.mock.calls[3][0] as Tab).id).toBe(tab.id);
  });

  it("expand_all_is_capped", async () => {
    await mount();
    const all = Array.from({ length: 300 }, (_, i) => view(`k${String(i).padStart(3, "0")}`, 300 - i));
    overrideCommand("grid_groups", (a) => {
      const { offset, limit, parent } = a as { offset: number; limit: number; parent: unknown[] };
      if (parent.length > 0) return { items: [], truncated: false };
      return { items: all.slice(offset, offset + limit), truncated: offset + limit < all.length };
    });
    overrideCommand("grid_page", (a) => mockCall("grid_page", { ...(a as object), limit: 1 }));
    await groupBy("Channel");
    expect(groupRows()).not.toHaveLength(0);
    await scrollToEnd();
    expect(screen.getByRole("button", { name: /more groups/ })).toBeTruthy();
    const pages0 = pageCalls().length;
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    await waitFor(() => expect(pageCalls().length - pages0).toBe(200), { timeout: 5000 });
    await settle(100);
    expect(pageCalls().length - pages0).toBe(200); // the first level's first 200 groups, no more
    // Paging in the rest and expanding again opens nothing beyond the 200.
    await scrollToEnd();
    fireEvent.click(await screen.findByRole("button", { name: /more groups/ }));
    await waitFor(() => expect(groupCalls().some((c) => c.offset === 200)).toBe(true));
    fireEvent.click(screen.getByRole("button", { name: "Expand all" }));
    await settle(100);
    expect(pageCalls().length - pages0).toBe(200);
    expect(groupCalls().find((c) => c.offset === 200)!.limit).toBe(200);
  });

  it("collapse_all_drops_fetched_rows", async () => {
    await mount();
    await groupBy("Channel");
    const first = groupRows()[0];
    toggle(first);
    await waitFor(() => expect(memberRows().length).toBeGreaterThan(0));
    const pages = pageCalls().length;
    fireEvent.click(screen.getByRole("button", { name: "Collapse all" }));
    expect(memberRows()).toHaveLength(0);
    expect(groupRows().every((r) => within(r).getByRole("button", { name: /^Expand / }))).toBe(true);
    toggle(groupRows()[0]);
    await waitFor(() => expect(pageCalls().length).toBe(pages + 1)); // the rows were dropped, so opening reads them again
    await waitFor(() => expect(memberRows().length).toBeGreaterThan(0));
  });
});

describe("grouping: the group bar", () => {
  it("adds_removes_and_reorders_up_to_four_columns", async () => {
    const reportError = vi.fn();
    const m = await mount();
    m.rerenderWith({ reportError });
    await groupBy("Channel", "Level");
    const bar = screen.getByRole("group", { name: "Group by" });
    expect(within(bar).getAllByRole("listitem").map((x) => x.querySelector(".group-pill-name")!.textContent)).toEqual(["Channel", "Level"]);
    fireEvent.click(within(bar).getByRole("button", { name: "Move Level earlier" }));
    await waitFor(() =>
      expect(within(bar).getAllByRole("listitem").map((x) => x.querySelector(".group-pill-name")!.textContent)).toEqual(["Level", "Channel"]),
    );
    await waitFor(() => expect(groupCalls().at(-1)!.by).toEqual(["Level", "Channel"]));
    await groupBy("Computer", "Provider");
    expect(within(bar).getAllByRole("listitem")).toHaveLength(4);
    fireEvent.contextMenu(header("UserName"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Group by this column" }));
    expect(reportError).toHaveBeenCalled();
    expect(within(bar).getAllByRole("listitem")).toHaveLength(4);
    fireEvent.click(within(bar).getByRole("button", { name: "Remove grouping Computer" }));
    expect(within(bar).getAllByRole("listitem")).toHaveLength(3);
    // Removing the last ones returns to the plain grid.
    for (const c of ["Provider", "Channel", "Level"]) fireEvent.click(within(bar).getByRole("button", { name: `Remove grouping ${c}` }));
    await waitFor(() => expect(flatRows().length).toBeGreaterThan(0));
    expect(groupRows()).toHaveLength(0);
  });

  it("a_dropped_header_joins_the_group_bar", async () => {
    await mount();
    const data = { setData: vi.fn(), getData: vi.fn(() => "Channel"), types: ["application/x-oxl-column"], effectAllowed: "" };
    fireEvent.dragStart(header("Channel"), { dataTransfer: data });
    const bar = screen.getByRole("group", { name: "Group by" });
    fireEvent.dragOver(bar, { dataTransfer: data });
    fireEvent.drop(bar, { dataTransfer: data });
    await waitFor(() => expect(groupRows().length).toBeGreaterThan(0));
    expect(groupCalls()[0].by).toEqual(["Channel"]);
  });

  it("group_by_mark_uses_the_mark_chip_shape", async () => {
    await mount();
    overrideCommand("grid_groups", () => ({ items: [view("suspicious", 2, { suspicious: 2 }), view(null, 9)], truncated: false }));
    fireEvent.click(screen.getByRole("button", { name: "Group by Mark" }));
    await waitFor(() => expect(groupRows()).toHaveLength(2));
    expect(groupCalls()[0].by).toEqual(["_mark"]);
    fireEvent.doubleClick(groupRows()[1]);
    // not reached through openScoped here: the ctx of this mount is checked in the double-click test; the rows fetch is:
    toggle(groupRows()[1]);
    await waitFor(() => expect(pageCalls().length).toBeGreaterThan(0));
    expect(pageCalls().at(-1)!.state.chips[0].filter).toMatchObject({ column: "_mark", op: "eq", value: "none" });
    toggle(groupRows()[0]);
    await waitFor(() => expect(pageCalls().length).toBeGreaterThan(1));
    expect(pageCalls().at(-1)!.state.chips[0].filter).toMatchObject({ column: "_mark", op: "eq", value: "suspicious" });
  });
});

describe("grouping: stale answers and hostile text", () => {
  it("a_state_change_drops_an_older_groups_answer_and_cancels_it", async () => {
    const m = await mount();
    const answers: ((v: unknown) => void)[] = [];
    overrideCommand("grid_groups", () => new Promise((r) => answers.push(r)));
    fireEvent.contextMenu(header("Channel"));
    fireEvent.click(await screen.findByRole("menuitem", { name: "Group by this column" }));
    await waitFor(() => expect(answers).toHaveLength(1));
    const firstId = groupCalls()[0].queryId;
    // The analyst narrows the grid while the first answer is still out.
    const user = userEvent.setup();
    await user.type(screen.getByRole("textbox", { name: /Match Channel/ }), "Sys");
    await waitFor(() => expect(answers).toHaveLength(2), { timeout: 3000 });
    expect(callsOf("query_cancel").some((c) => ((c.args as { queryIds?: string[]; ids?: string[] }).queryIds ?? (c.args as { ids?: string[] }).ids ?? []).includes(firstId))).toBe(true);
    // The new answer arrives first, then the old one: only the new is drawn.
    await act(async () => answers[1]({ items: [view("New", 7)], truncated: false }));
    await act(async () => answers[0]({ items: [view("Old", 99)], truncated: false }));
    expect(groupRows().map(valueOf)).toEqual(["New"]);
    expect(m.ctx.reportError).not.toHaveBeenCalled();
  });

  it("a_collapsed_and_reopened_group_ignores_the_first_rows_answer", async () => {
    await mount();
    await groupBy("Channel");
    const answers: ((v: unknown) => void)[] = [];
    overrideCommand("grid_page", () => new Promise((r) => answers.push(r)));
    const key = valueOf(groupRows()[0]);
    toggle(groupRows()[0]);
    await waitFor(() => expect(answers).toHaveLength(1));
    toggle(groupRow(key)); // closed: the read is cancelled
    toggle(groupRow(key)); // opened again: a second read
    await waitFor(() => expect(answers).toHaveLength(2));
    const rows = (n: number) => mockCall("grid_page", { ...pageCalls()[0], state: pageCalls()[1].state, limit: n });
    await act(async () => answers[1](await rows(3)));
    await act(async () => answers[0](await rows(1)));
    expect(memberRows()).toHaveLength(3);
  });

  it("an_expanded_group_survives_a_filter_change_and_reads_again", async () => {
    await mount();
    await groupBy("Channel");
    const key = valueOf(groupRows()[0]);
    toggle(groupRows()[0]);
    await waitFor(() => expect(memberRows().length).toBeGreaterThan(0));
    const before = callsOf("grid_groups").length;
    const user = userEvent.setup();
    await user.type(screen.getByRole("textbox", { name: /Match Level/ }), "Inf");
    await waitFor(() => expect(callsOf("grid_groups").length).toBeGreaterThan(before));
    await waitFor(() => expect(groupRows().some((r) => valueOf(r) === key)).toBe(true));
    await waitFor(() => expect(memberRows().length).toBeGreaterThan(0));
    // Everything shown was fetched under the new filter.
    expect(pageCalls().at(-1)!.state.chips.map((c) => c.origin)).toContain("filter_row");
  });

  it("hostile_keys_render_inert_in_group_rows", async () => {
    await mount();
    overrideCommand("grid_groups", () => ({
      items: [view(HOSTILE.html, 3), view(HOSTILE.bidi, 2), view(HOSTILE.bell, 1)],
      truncated: false,
    }));
    await groupBy("UserName");
    const rows = groupRows();
    expect(rows).toHaveLength(3);
    for (const r of rows) {
      expect(r.querySelector("img")).toBeNull();
      expect(r.textContent).not.toContain("‮");
      expect(r.textContent).not.toContain("\u0007");
    }
    expect(rows[0].textContent).toContain("<img src=x onerror=alert(1)>");
    expect(rows[1].textContent).toContain("\\u{202E}");
  });

  it("space_and_enter_on_a_group_toggle_toggle_it", async () => {
    await mount();
    await groupBy("Channel");
    const user = userEvent.setup();
    const key = valueOf(groupRows()[0]);
    within(groupRow(key)).getByRole("button").focus();
    await user.keyboard(" ");
    await waitFor(() => expect(groupRow(key).getAttribute("aria-expanded")).toBe("true"));
    await user.keyboard("{Enter}");
    await waitFor(() => expect(groupRow(key).getAttribute("aria-expanded")).toBe("false"));
    // The flat grid's keys do nothing over group rows.
    await user.keyboard("{ArrowDown}{F3}");
    expect(memberRows()).toHaveLength(0);
  });

  it("more_groups_pages_in_200s", async () => {
    await mount();
    const all = Array.from({ length: 250 }, (_, i) => view(`g${String(i).padStart(3, "0")}`, 250 - i));
    overrideCommand("grid_groups", (a) => {
      const { offset, limit } = a as { offset: number; limit: number };
      return { items: all.slice(offset, offset + limit), truncated: offset + limit < all.length };
    });
    await groupBy("Channel");
    await scrollToEnd();
    fireEvent.click(await screen.findByRole("button", { name: /more groups/ }));
    await waitFor(() => expect(groupCalls().map((c) => c.offset)).toEqual([0, 200]));
    await scrollToEnd();
    await waitFor(() => expect(screen.queryByRole("button", { name: /more groups/ })).toBeNull());
    expect(screen.getByText("g249", { exact: false })).toBeTruthy();
  });
});
