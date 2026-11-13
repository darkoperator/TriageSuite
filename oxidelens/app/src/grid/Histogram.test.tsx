// The histogram strip above the grid: bars of the filter, the column picker, the legend, the brush that sets a date range
// and the toggle. Everything runs against the mock backend; the strip draws what the engine answers and computes nothing
// about time.
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { caseOpen, caseSummary, settingsGet } from "../api";
import type { Chip } from "../contract/generated/Chip";
import type { GridState } from "../contract/generated/GridState";
import type { Histogram } from "../contract/generated/Histogram";
import { HOSTILE } from "../mock/generated";
import { ShellCtx, type ShellContext } from "../shell/context";
import type { Tab } from "../shell/tabs";
import { callsOf, overrideCommand } from "../test/backend";
import Grid from "./Grid";
import { initialGridState } from "./gridState";

vi.mock("./measure", () => ({ measureText: vi.fn(() => 50) }));

beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 440 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const events: Tab = { id: "dataset:mock_events", kind: "dataset", relation: "mock_events" };

function Host({ ctx, tab = events }: { ctx: ShellContext; tab?: Tab }) {
  const [status, setStatus] = useState<ReactNode>(null);
  return (
    <ShellCtx.Provider value={{ ...ctx, setGridStatus: setStatus }}>
      <div data-testid="status">{status}</div>
      <Grid tab={tab} active scrollOffset={0} onScroll={() => {}} />
    </ShellCtx.Provider>
  );
}

async function mount(
  o: { zone?: string; histogram?: boolean; waitBars?: boolean; chips?: Chip[]; primaryTime?: null } = {},
) {
  await caseOpen("/cases/x.duckdb");
  const base = await caseSummary("t");
  const summary = {
    ...base, display_zone: o.zone ?? "UTC",
    relations: o.primaryTime === null ? base.relations.map((r) => ({ ...r, primary_time: null })) : base.relations,
  };
  const tab: Tab = o.chips
    ? { ...events, seed: { state: { ...initialGridState("mock_events"), chips: o.chips }, by: [] } }
    : events;
  const settings = { ...(await settingsGet()).settings, histogram: o.histogram ?? true };
  const reportError = vi.fn();
  const setHistogram = vi.fn();
  const ctx: ShellContext = {
    summary, settings, change: { generation: 0, kinds: [] }, openDataset: () => {}, openScoped: () => {}, setGridStatus: () => {},
    setRightPane: () => {}, rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError, saveLayout: vi.fn(), setHistogram,
  };
  const utils = render(<Host ctx={ctx} tab={tab} />);
  await waitFor(() => expect(dataRows().length).toBeGreaterThan(0));
  if (o.waitBars !== false && o.histogram !== false) await waitFor(() => expect(bars().length).toBeGreaterThan(0));
  const again = (zone: string) =>
    utils.rerender(<Host ctx={{ ...ctx, summary: { ...summary, display_zone: zone } }} tab={tab} />);
  return { ...utils, again, reportError, setHistogram, user: userEvent.setup() };
}

const dataRows = () => screen.queryAllByRole("row").filter((r) => r.getAttribute("data-row-index") !== null);
const strip = () => screen.getByRole("region", { name: "Histogram" });
const bars = () => [...document.querySelectorAll<SVGRectElement>("[data-bar]")];
const total = () => bars().reduce((n, b) => n + Number(b.getAttribute("data-count")), 0);
const stateOf = (c: { args?: Record<string, unknown> }) => (c.args as { state: GridState }).state;
const lastPageState = (): GridState =>
  stateOf(callsOf("grid_page").filter((c) => !/:(validate|search)$/.test(String(c.args!.slot))).at(-1)!);
const chips = (): Chip[] => lastPageState().chips;

/** An engine-shaped answer for the brush tests: four non-empty hours on a New York clock. */
const hist = (over: Partial<Histogram> = {}): Histogram => ({
  column: "TimeCreated", time_semantics: "utc", zone: "America/New_York", bucket_seconds: 3600, untimed: 0, slots: 4,
  buckets: [0, 1, 2, 3].map((h) => ({
    index: h,
    start: `2026-01-05T0${h + 5}:00:00.000000Z`, label: `2026-01-05 0${h}:00:00`, end_label: `2026-01-05 0${h}:59:59`,
    count: 10 + h, suspicious: 0,
  })),
  ...over,
});

/** Drags over columns `from` to `to` of a strip drawn `colWidth` px per column (24 px, the widest a column is drawn). */
const drag = (from: number, to: number, colWidth = 24) => {
  const layer = document.querySelector<SVGRectElement>("[data-hit]")!;
  fireEvent.mouseDown(layer, { clientX: from * colWidth + 1 });
  fireEvent.mouseMove(layer, { clientX: to * colWidth + 1 });
  fireEvent.mouseUp(layer, { clientX: to * colWidth + 1 });
};

describe("histogram strip", () => {
  it("bars_follow_the_filter", async () => {
    const { user } = await mount();
    const all = total();
    expect(all).toBeGreaterThan(0);
    // one request for the strip, asked for one bucket per 6 px of its 1200 px, on its own slot
    const first = callsOf("grid_histogram")[0].args as { slot: string; maxBuckets: number; zone: string; column: string | null };
    expect(first).toMatchObject({ slot: "tab:dataset:mock_events:hist", maxBuckets: 200, zone: "UTC", column: "TimeCreated" });

    await user.type(screen.getByRole("textbox", { name: "Match Channel" }), "Security");
    await waitFor(() => expect(total()).toBeLessThan(all), { timeout: 2000 });
    const asked = stateOf(callsOf("grid_histogram").at(-1)!);
    expect(asked.chips).toHaveLength(1);
    expect(asked.chips[0].filter).toMatchObject({ column: "Channel", value: "Security" });
    // the strip counts the filtered rows the grid counts
    await waitFor(() => {
      const rows = Number(/^([\d,]+) of/.exec(screen.getByTestId("status").textContent ?? "")?.[1]?.replace(/,/g, ""));
      const untimed = Number(/([\d,]+) untimed/.exec(strip().textContent ?? "")?.[1]?.replace(/,/g, "") ?? 0);
      expect(total() + untimed).toBe(rows);
    });
  });

  it("brush_makes_a_date_range_chip", async () => {
    overrideCommand("grid_histogram", () => hist());
    const { user, reportError } = await mount({ zone: "America/New_York" });
    expect(bars()).toHaveLength(4);
    drag(1, 2);
    await waitFor(() => expect(chips()).toHaveLength(1));
    expect(callsOf("date_range_chip").at(-1)!.args).toMatchObject({
      relation: "mock_events", column: "TimeCreated", from: "2026-01-05 01:00:00", to: "2026-01-05 02:59:59", zone: "America/New_York",
    });
    expect(chips()[0]).toMatchObject({ origin: "histogram", enabled: true });
    expect(chips()[0].id).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
    // a drag the other way round selects the same buckets
    drag(3, 2);
    await waitFor(() => expect(callsOf("date_range_chip")).toHaveLength(2));
    expect(callsOf("date_range_chip").at(-1)!.args).toMatchObject({ from: "2026-01-05 02:00:00", to: "2026-01-05 03:59:59" });
    // a new brush replaces the earlier one for the column; the other filters stay
    await waitFor(() => expect(chips()[0].label).toContain("2026-01-05 02:00:00"));
    expect(chips()).toHaveLength(1);
    // a date range chip made elsewhere is not the strip's to replace
    expect(reportError).not.toHaveBeenCalled();
    // double-click clears it
    fireEvent.doubleClick(strip().querySelector("svg")!);
    await waitFor(() => expect(chips()).toEqual([]));
    void user;
  });

  it("brush_leaves_other_chips_and_date_tab_ranges_alone", async () => {
    overrideCommand("grid_histogram", () => hist());
    const tabRange: Chip = {
      id: "dr-tab", label: "TimeCreated: from the Date tab", enabled: true, origin: "date_range",
      filter: { column: "TimeCreated", op: "gte", value: "2026-01-01 00:00:00", case_sensitive: false },
    };
    const { user } = await mount({ zone: "America/New_York", chips: [tabRange] });
    await user.type(screen.getByRole("textbox", { name: "Match Channel" }), "Sec");
    await waitFor(() => expect(chips()).toHaveLength(2), { timeout: 2000 });
    drag(0, 0);
    await waitFor(() => expect(chips()).toHaveLength(3));
    drag(1, 1);
    await waitFor(() => expect(callsOf("date_range_chip")).toHaveLength(2));
    await waitFor(() => expect(chips().map((c) => c.origin).sort()).toEqual(["date_range", "filter_row", "histogram"]));
    expect(chips()).toHaveLength(3);
    expect(chips().find((c) => c.id === "dr-tab")).toEqual(tabRange);
  });

  it("bars_sit_at_their_engine_index_and_a_brush_spans_the_gap", async () => {
    // hours 0, 1 and 5 of six positions: the positions between are empty
    overrideCommand("grid_histogram", () => {
      const h = hist({ slots: 6 });
      h.buckets = [0, 1, 5].map((i) => ({
        ...h.buckets[Math.min(i, 3)], index: i, start: `s${i}`, label: `2026-01-05 0${i}:00:00`, end_label: `2026-01-05 0${i}:59:59`,
      }));
      return h;
    });
    await mount({ zone: "America/New_York" });
    const xs = bars().map((b) => Number(b.getAttribute("x")));
    expect(bars()).toHaveLength(3);
    const slot = xs[1] - xs[0];
    expect(xs[2] - xs[1]).toBeCloseTo(4 * slot); // three empty positions between them, not adjacent
    expect(document.querySelectorAll("[data-hit]")).toHaveLength(1); // one shared hit layer, not one per position
    // across the gap: from the first to the last non-empty bucket inside the range
    drag(1, 5);
    await waitFor(() => expect(callsOf("date_range_chip")).toHaveLength(1));
    expect(callsOf("date_range_chip")[0].args).toMatchObject({ from: "2026-01-05 01:00:00", to: "2026-01-05 05:59:59" });
    // a range of empty positions does nothing
    drag(2, 4);
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(callsOf("date_range_chip")).toHaveLength(1);
  });

  it("a_span_with_more_slots_than_columns_is_drawn_in_at_most_the_columns_asked_for", async () => {
    // no width fit: 1,500 positions for a strip of 200 columns (1,200 px, 6 px each); two buckets at the two ends
    overrideCommand("grid_histogram", () => {
      const h = hist({ slots: 1500, bucket_seconds: 2_592_000 });
      h.buckets = [
        { ...h.buckets[0], index: 0, start: "a", label: "1900-01-01 00:00:00", end_label: "1900-01-31 23:59:59" },
        { ...h.buckets[1], index: 1499, start: "b", label: "2026-01-01 00:00:00", end_label: "2026-01-31 23:59:59" },
      ];
      return h;
    });
    await mount({ zone: "America/New_York" });
    expect(callsOf("grid_histogram")[0].args).toMatchObject({ maxBuckets: 200 });
    expect(strip().querySelectorAll("svg *").length).toBeLessThanOrEqual(200);
    expect(bars()).toHaveLength(2);
    const xs = bars().map((b) => Number(b.getAttribute("x")));
    expect(xs[1] - xs[0]).toBeCloseTo(199 * 6); // the last bucket sits in the last of the 200 columns
    drag(0, 199, 6);
    await waitFor(() => expect(callsOf("date_range_chip")).toHaveLength(1));
    expect(callsOf("date_range_chip")[0].args).toMatchObject({ from: "1900-01-01 00:00:00", to: "2026-01-31 23:59:59" });
    // only the first column
    drag(0, 0, 6);
    await waitFor(() => expect(callsOf("date_range_chip")).toHaveLength(2));
    expect(callsOf("date_range_chip")[1].args).toMatchObject({ from: "1900-01-01 00:00:00", to: "1900-01-31 23:59:59" });
  });

  it("a_press_outside_the_drawn_columns_does_nothing", async () => {
    // one bucket: one 24 px column in a 1,200 px strip
    overrideCommand("grid_histogram", () => {
      const h = hist({ slots: 1 });
      h.buckets = [h.buckets[0]];
      return h;
    });
    await mount({ zone: "America/New_York" });
    const layer = document.querySelector<SVGRectElement>("[data-hit]")!;
    expect(Number(layer.getAttribute("width"))).toBe(24); // the hit layer is as wide as the columns drawn
    fireEvent.mouseDown(layer, { clientX: 1000 });
    fireEvent.mouseUp(layer, { clientX: 1000 });
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(callsOf("date_range_chip")).toHaveLength(0);
    // a drag that starts inside and ends beyond the right edge stops at the last column
    fireEvent.mouseDown(layer, { clientX: 5 });
    fireEvent.mouseMove(layer, { clientX: 1000 });
    fireEvent.mouseUp(layer, { clientX: 1000 });
    await waitFor(() => expect(callsOf("date_range_chip")).toHaveLength(1));
    expect(callsOf("date_range_chip")[0].args).toMatchObject({ from: "2026-01-05 00:00:00", to: "2026-01-05 00:59:59" });
  });

  it("buckets_sharing_a_column_are_summed", async () => {
    overrideCommand("grid_histogram", () => {
      const h = hist({ slots: 400 });
      h.buckets = [0, 1, 2].map((i) => ({ ...h.buckets[i], index: i, start: `s${i}`, suspicious: i }));
      return h;
    });
    await mount();
    // 400 positions in 200 columns: positions 0 and 1 share the first column, position 2 has the second
    expect(bars().map((b) => b.getAttribute("data-count"))).toEqual(["21", "12"]);
    expect(strip().textContent).toContain("3 marked");
  });

  it("an_old_brush_answer_is_dropped_when_the_zone_the_column_or_the_state_changes", async () => {
    overrideCommand("grid_histogram", () => hist());
    let release: (() => void) | undefined;
    overrideCommand("date_range_chip", (a) => {
      const args = a as { column: string };
      return new Promise((resolve) => {
        release = () =>
          resolve({ id: "dr-late", label: "late", enabled: true, origin: "date_range", filter: { column: args.column, op: "gte", value: "x", case_sensitive: false } });
      });
    });
    const { user, again } = await mount({ zone: "America/New_York" });
    const land = async () => {
      await act(async () => release!());
      await act(() => new Promise((r) => setTimeout(r, 50)));
    };
    const pending = async () => {
      release = undefined;
      drag(1, 1);
      await waitFor(() => expect(release).toBeDefined());
    };
    // a new zone
    await pending();
    again("UTC");
    await land();
    expect(chips().filter((c) => c.origin === "histogram")).toEqual([]);
    // a new column
    await waitFor(() => expect(bars().length).toBeGreaterThan(0));
    await pending();
    await user.selectOptions(within(strip()).getByRole("combobox", { name: "Histogram column" }), "LocalTime");
    await land();
    expect(chips().filter((c) => c.origin === "histogram")).toEqual([]);
    // a new state
    await waitFor(() => expect(bars().length).toBeGreaterThan(0));
    await pending();
    await user.type(screen.getByRole("textbox", { name: "Match Channel" }), "Sec");
    await waitFor(() => expect(chips()).toHaveLength(1), { timeout: 2000 });
    await land();
    expect(chips().map((c) => c.origin)).toEqual(["filter_row"]);
  });

  it("brush_shows_the_engines_refusal", async () => {
    const message = "2026-11-01 01:00:00 happened twice in America/New_York, at 2026-11-01T05:00:00Z and at 2026-11-01T06:00:00Z UTC";
    overrideCommand("grid_histogram", () => hist());
    overrideCommand("date_range_chip", () => Promise.reject({ code: "invalid", message }));
    const { reportError } = await mount({ zone: "America/New_York" });
    drag(1, 1);
    const alert = await within(strip()).findByRole("alert");
    expect(alert.textContent).toBe(message);
    expect(chips()).toEqual([]);
    expect(reportError).not.toHaveBeenCalled();
  });

  it("a_double_click_is_not_undone_by_a_brush_still_in_flight", async () => {
    overrideCommand("grid_histogram", () => hist());
    let release: (() => void) | undefined;
    overrideCommand("date_range_chip", (a) => {
      const args = a as { column: string };
      return new Promise((resolve) => {
        release = () =>
          resolve({ id: "dr-late", label: "late", enabled: true, origin: "date_range", filter: { column: args.column, op: "gte", value: "x", case_sensitive: false } });
      });
    });
    await mount({ zone: "America/New_York" });
    drag(1, 1);
    await waitFor(() => expect(release).toBeDefined());
    fireEvent.doubleClick(strip().querySelector("svg")!);
    await act(async () => release!());
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(chips()).toEqual([]);
  });

  it("the_column_shown_is_the_column_asked_for", async () => {
    // no primary time: the picker shows the first TIMESTAMP column and the request names it, not null
    await mount({ primaryTime: null });
    expect(within(strip()).getByRole("combobox", { name: "Histogram column" })).toHaveValue("TimeCreated");
    expect(callsOf("grid_histogram")[0].args).toMatchObject({ column: "TimeCreated" });
  });

  it("column_picker_lists_timestamp_columns", async () => {
    const { user } = await mount();
    const picker = within(strip()).getByRole("combobox", { name: "Histogram column" });
    const options = within(picker).getAllByRole("option").map((o) => o.textContent);
    expect(options).toEqual(["TimeCreated", "LocalTime"]); // the relation's TIMESTAMP columns, nothing else
    expect(picker).toHaveValue("TimeCreated"); // the primary time is the default
    await user.selectOptions(picker, "LocalTime");
    await waitFor(() => expect(callsOf("grid_histogram").at(-1)!.args).toMatchObject({ column: "LocalTime" }));
    await waitFor(() => expect(strip().textContent).toContain("as written") ); // an unzoned column says so
  });

  it("shows_marked_and_untimed_counts", async () => {
    overrideCommand("grid_histogram", () => {
      const h = hist({ untimed: 1234 });
      h.buckets[0].suspicious = 2;
      h.buckets[2].suspicious = 3;
      return h;
    });
    await mount({ zone: "America/New_York" });
    const legend = within(strip()).getByRole("list", { name: "Legend" });
    expect(legend.textContent).toContain("1 hour");
    expect(legend.textContent).toContain("America/New_York");
    expect(within(legend).getByText("5 marked")).toBeInTheDocument();
    expect(within(legend).getByText("1,234 untimed")).toBeInTheDocument();
    expect(legend.querySelector(".hist-amber")).not.toBeNull();
  });

  it("hostile_text_is_drawn_as_text", async () => {
    overrideCommand("grid_histogram", () => {
      const h = hist({ zone: HOSTILE.html });
      h.buckets[0].label = HOSTILE.html;
      return h;
    });
    await mount();
    expect(strip().querySelector("img")).toBeNull();
    expect(strip().textContent).toContain("<img src=x onerror=alert(1)>");
  });

  it("paging_is_not_blocked_by_a_slow_histogram", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    overrideCommand("grid_histogram", () => new Promise((r) => setTimeout(() => r(hist()), 2000)));
    await mount({ waitBars: false });
    // the rows are on screen while the strip is still waiting
    expect(dataRows().length).toBeGreaterThan(0);
    expect(bars()).toHaveLength(0);
    expect(callsOf("grid_histogram")).toHaveLength(1);
    await act(() => vi.advanceTimersByTimeAsync(2100));
    await waitFor(() => expect(bars()).toHaveLength(4));
  });

  it("an_older_answer_is_never_drawn", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let n = 0;
    overrideCommand("grid_histogram", () => {
      const mine = ++n;
      return new Promise((r) => setTimeout(() => r(mine === 1 ? hist({ untimed: 111 }) : hist({ untimed: 222 })), mine === 1 ? 2000 : 100));
    });
    const { user } = await mount({ waitBars: false });
    await user.type(screen.getByRole("textbox", { name: "Match Channel" }), "Security");
    await act(() => vi.advanceTimersByTimeAsync(600));
    await act(() => vi.advanceTimersByTimeAsync(3000));
    await waitFor(() => expect(strip().textContent).toContain("222 untimed"));
    expect(strip().textContent).not.toContain("111 untimed");
  });

  it("is_absent_when_switched_off", async () => {
    const { user, setHistogram } = await mount({ histogram: false });
    expect(screen.queryByRole("region", { name: "Histogram" })).toBeNull();
    expect(callsOf("grid_histogram")).toHaveLength(0);
    await user.click(screen.getByRole("button", { name: "Histogram" }));
    expect(setHistogram).toHaveBeenCalledWith(true);
  });
});
