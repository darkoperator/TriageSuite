// The search box (apply, errors, find mode and stepping), the host scope and the display zone picker, against the mock.
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { caseOpen, caseSummary, settingsGet } from "../api";
import type { GridState } from "../contract/generated/GridState";
import { mockCall } from "../mock/backend";
import { HOSTILE } from "../mock/generated";
import { ShellCtx, type ShellContext } from "../shell/context";
import type { Tab } from "../shell/tabs";
import { callsOf, overrideCommand } from "../test/backend";
import Grid from "./Grid";

vi.mock("./measure", () => ({ measureText: vi.fn(() => 50) }));

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
  // 5,000 rows of 22 px: the virtualiser clamps a scroll to scrollHeight - clientHeight
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", { configurable: true, get: () => 110_000 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 440 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const events: Tab = { id: "dataset:mock_events", kind: "dataset", relation: "mock_events" };

function Host({ ctx }: { ctx: ShellContext }) {
  const [status, setStatus] = useState<ReactNode>(null);
  return (
    <ShellCtx.Provider value={{ ...ctx, setGridStatus: setStatus }}>
      <div data-testid="status">{status}</div>
      <Grid tab={events} active scrollOffset={0} onScroll={() => {}} />
    </ShellCtx.Provider>
  );
}

async function mount() {
  await caseOpen("/cases/x.duckdb");
  const summary = await caseSummary("t");
  const settings = (await settingsGet()).settings;
  const reportError = vi.fn();
  const ctx: ShellContext = {
    summary, settings, change: { generation: 0, kinds: [] }, openDataset: () => {}, setGridStatus: () => {},
    setRightPane: () => {}, rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError, saveLayout: vi.fn(),
  };
  const utils = render(<Host ctx={ctx} />);
  await waitFor(() => expect(screen.getAllByRole("columnheader").length).toBeGreaterThan(1));
  await waitFor(() => expect(dataRows().length).toBeGreaterThan(0));
  return { ...utils, reportError, user: userEvent.setup() };
}

const dataRows = () => screen.queryAllByRole("row").filter((r) => r.getAttribute("data-row-index") !== null);
const stateOf = (c: { args?: Record<string, unknown> }) => (c.args as { state: GridState }).state;
// The grid's own pages; the search box checks a candidate on a slot of its own.
const lastState = (): GridState =>
  stateOf(callsOf("grid_page").filter((c) => !/:(validate|search)$/.test(String(c.args!.slot))).at(-1)!);
const box = () => screen.getByRole("searchbox", { name: "Search" });
const focusedRow = () =>
  document.querySelector("[data-focused='true']")?.closest("[data-row-index]")?.getAttribute("data-row-index") ?? null;

describe("search", () => {
  it("search_applies_and_shows_engine_errors", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { user, reportError } = await mount();
    const before = dataRows().length;
    expect(screen.queryByText(/^search:/)).toBeNull();

    // 400 ms of quiet applies; nothing before that
    await user.type(box(), "Security");
    expect(lastState().search.text).toBe("");
    await act(() => vi.advanceTimersByTimeAsync(300));
    expect(lastState().search.text).toBe("");
    await act(() => vi.advanceTimersByTimeAsync(200));
    await waitFor(() => expect(lastState().search).toEqual({ text: "Security", regex: false, case_sensitive: false, mode: "filter" }));
    expect(screen.getByText("search: Security")).toBeInTheDocument();
    const rowsShown = () => /^([\d,]+) of 5,000 rows/.exec(screen.getByTestId("status").textContent ?? "")?.[1] ?? "";
    await waitFor(() => expect(rowsShown()).not.toBe("5,000"));
    const narrowed = rowsShown();
    expect(before).toBeGreaterThan(0);

    // Rx with a text the engine refuses: its message under the box, the earlier results stay, the state is unchanged
    await user.click(screen.getByRole("button", { name: "Regular expression" }));
    await user.clear(box());
    await user.type(box(), "(");
    await user.keyboard("{Enter}");
    const msg = await screen.findByRole("alert");
    expect(msg.textContent).toContain("not a regular expression: (");
    expect(rowsShown()).toBe(narrowed); // the earlier results are still what the grid counts and draws
    expect(dataRows().length).toBeGreaterThan(0);
    expect(lastState().search.text).toBe("Security");
    expect(screen.getByText("search: Security")).toBeInTheDocument();
    expect(reportError).not.toHaveBeenCalled();

    // a good text clears the message; the pill goes away with its ×
    await user.clear(box());
    await user.type(box(), "Sec");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(lastState().search).toMatchObject({ text: "Sec", regex: true }));
    expect(screen.queryByRole("alert")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Clear search" }));
    await waitFor(() => expect(lastState().search.text).toBe(""));
    expect((box() as HTMLInputElement).value).toBe("");
    expect(screen.queryByText(/^search:/)).toBeNull();
  });

  it("an_engine_message_with_hostile_text_is_inert", async () => {
    const { user } = await mount();
    overrideCommand("grid_page", (a) => {
      if (String((a as { slot: string }).slot).endsWith(":search")) return Promise.reject({ code: "invalid", message: HOSTILE.html });
      return mockCall("grid_page", a);
    });
    await user.type(box(), "x{Enter}");
    const msg = await screen.findByRole("alert");
    expect(msg.textContent).toContain("<img src=x onerror=alert(1)>");
    expect(msg.querySelector("img")).toBeNull();
  });

  it("find_mode_steps_through_matches", async () => {
    const { user } = await mount();
    await user.type(box(), "Security{Enter}");
    await waitFor(() => expect(lastState().search.text).toBe("Security"));
    await user.click(screen.getByRole("radio", { name: "find" }));
    await waitFor(() => expect(lastState().search.mode).toBe("find"));
    const label = await screen.findByTestId("match-position");
    await waitFor(() => expect(label.textContent).toMatch(/^0 of \d+$/));
    const count = Number(label.textContent!.split(" of ")[1]);
    expect(count).toBeGreaterThan(2);
    // every row stays and the matching ones are marked
    await waitFor(() => expect(screen.getByTestId("status").textContent).toMatch(/5,000 of 5,000 rows/));
    await waitFor(() => expect(document.querySelectorAll(".row-hit").length).toBeGreaterThan(0));

    const first = await mockCall<{ items: { offset: number }[] }>("grid_matches", {
      queryId: "q", slot: "x", state: lastState(), from: null, backward: false, limit: 10,
    });
    const at = first.items.map((m) => m.offset);

    fireEvent.keyDown(box(), { key: "F3" });
    await waitFor(() => expect(label.textContent).toBe(`1 of ${count}`));
    expect(focusedRow()).toBe(String(at[0]));
    await user.click(screen.getByRole("button", { name: "Next match" }));
    await waitFor(() => expect(label.textContent).toBe(`2 of ${count}`));
    expect(focusedRow()).toBe(String(at[1]));
    fireEvent.keyDown(box(), { key: "F3", shiftKey: true });
    await waitFor(() => expect(label.textContent).toBe(`1 of ${count}`));
    // before the first wraps to the last, and after the last wraps to the first
    await user.click(screen.getByRole("button", { name: "Previous match" }));
    await waitFor(() => expect(label.textContent).toBe(`${count} of ${count}`));
    // the match far down was scrolled to: the grid moved there and drew its row
    await waitFor(() => expect(screen.getByTestId("grid-scroll").scrollTop).toBeGreaterThan(1000));
    await waitFor(() => expect(Number(focusedRow())).toBeGreaterThan(at[1]));
    const last = Number(focusedRow());
    expect(callsOf("grid_page").some((c) => (c.args as { offset: number }).offset === Math.floor(last / 200) * 200)).toBe(true);
    await user.click(screen.getByRole("button", { name: "Next match" }));
    await waitFor(() => expect(label.textContent).toBe(`1 of ${count}`));
    expect(focusedRow()).toBe(String(at[0]));
    // filter mode shows no stepping
    await user.click(screen.getByRole("radio", { name: "filter" }));
    await waitFor(() => expect(lastState().search.mode).toBe("filter"));
    expect(screen.queryByTestId("match-position")).toBeNull();
  });

  async function inFind() {
    const m = await mount();
    await m.user.type(box(), "Security{Enter}");
    await waitFor(() => expect(lastState().search.text).toBe("Security"));
    await m.user.click(screen.getByRole("radio", { name: "find" }));
    await waitFor(() => expect(lastState().search.mode).toBe("find"));
    await waitFor(() => expect(screen.getByTestId("match-position").textContent).toMatch(/^0 of \d+$/));
    return m;
  }
  const stepCalls = () => callsOf("grid_matches").map((c) => c.args as { from: number | null; backward: boolean; limit: number });

  it("a_step_is_one_query_in_the_engines_direction_and_wraps_with_none", async () => {
    const { user } = await inFind();
    const label = screen.getByTestId("match-position");
    const count = Number(label.textContent!.split(" of ")[1]);
    await user.click(screen.getByRole("button", { name: "Next match" }));
    await waitFor(() => expect(label.textContent).toBe(`1 of ${count}`));
    expect(stepCalls()).toEqual([{ ...stepCalls()[0], from: null, backward: false, limit: 1 }]);
    const firstOffset = Number(focusedRow());
    await user.click(screen.getByRole("button", { name: "Next match" }));
    await waitFor(() => expect(label.textContent).toBe(`2 of ${count}`));
    expect(stepCalls().at(-1)).toMatchObject({ from: firstOffset, backward: false, limit: 1 });
    // back twice: the second finds nothing before the first match and asks again from nothing, backward
    await user.click(screen.getByRole("button", { name: "Previous match" }));
    await waitFor(() => expect(label.textContent).toBe(`1 of ${count}`));
    const before = stepCalls().length;
    await user.click(screen.getByRole("button", { name: "Previous match" }));
    await waitFor(() => expect(label.textContent).toBe(`${count} of ${count}`));
    expect(stepCalls().slice(before)).toMatchObject([
      { from: firstOffset, backward: true, limit: 1 },
      { from: null, backward: true, limit: 1 },
    ]);
    expect(stepCalls().every((c) => c.limit === 1)).toBe(true); // never a list
    // forward from the last wraps to the first with none
    await user.click(screen.getByRole("button", { name: "Next match" }));
    await waitFor(() => expect(label.textContent).toBe(`1 of ${count}`));
    expect(focusedRow()).toBe(String(firstOffset));
  });

  it("stepping_from_a_manually_focused_row_still_says_k_of_n", async () => {
    const { user } = await inFind();
    const label = screen.getByTestId("match-position");
    const count = Number(label.textContent!.split(" of ")[1]);
    const clicked = dataRows()[10];
    const from = Number(clicked.getAttribute("data-row-index"));
    await user.click(clicked.querySelectorAll("[role='gridcell']")[2]);
    const next = await mockCall<{ items: { offset: number; ordinal: number }[] }>("grid_matches", {
      queryId: "q", slot: "x", state: lastState(), from, backward: false, limit: 1,
    });
    await user.keyboard("{F3}");
    await waitFor(() => expect(label.textContent).toBe(`${next.items[0].ordinal} of ${count}`));
    expect(next.items[0].ordinal).toBeGreaterThan(1); // not the first match: the rank is the engine's, not counted from a wrap
    expect(focusedRow()).toBe(String(next.items[0].offset));
    // and backward from a focused row
    await user.click(dataRows()[20].querySelectorAll("[role='gridcell']")[2]);
    const row = Number(dataRows()[20].getAttribute("data-row-index"));
    const back = await mockCall<{ items: { ordinal: number }[] }>("grid_matches", {
      queryId: "q", slot: "x", state: lastState(), from: row, backward: true, limit: 1,
    });
    await user.keyboard("{Shift>}{F3}{/Shift}");
    await waitFor(() => expect(label.textContent).toBe(`${back.items[0].ordinal} of ${count}`));
  });

  it("a_state_change_cancels_match_queries_and_a_step_that_was_running_moves_nothing", async () => {
    const { user } = await inFind();
    let release: () => void = () => {};
    overrideCommand("grid_matches", (a) => new Promise((res) => (release = () => res(mockCall("grid_matches", a)))));
    fireEvent.keyDown(box(), { key: "F3" });
    await waitFor(() => expect(callsOf("grid_matches")).toHaveLength(1));
    const stepId = String(callsOf("grid_matches")[0].args!.queryId);
    const top = screen.getByTestId("grid-scroll").scrollTop;
    await user.click(screen.getByRole("button", { name: "Match case" })); // a new state
    await waitFor(() => expect(lastState().search.case_sensitive).toBe(true));
    const cancelled = () => callsOf("query_cancel").flatMap((c) => (c.args as { queryIds: string[] }).queryIds);
    expect(cancelled()).toContain(stepId);
    release();
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(focusedRow()).toBeNull();
    expect(screen.getByTestId("grid-scroll").scrollTop).toBe(top);
    expect(screen.getByTestId("match-position").textContent).toMatch(/^0 of /);
  });

  it("a_state_change_cancels_the_match_count_in_flight", async () => {
    const { user } = await mount();
    overrideCommand("grid_match_count", () => new Promise(() => {}));
    await user.type(box(), "Security{Enter}");
    await waitFor(() => expect(lastState().search.text).toBe("Security"));
    await user.click(screen.getByRole("radio", { name: "find" }));
    await waitFor(() => expect(callsOf("grid_match_count")).toHaveLength(1));
    const countId = String(callsOf("grid_match_count")[0].args!.queryId);
    await user.click(screen.getByRole("button", { name: "Match case" }));
    await waitFor(() => expect(callsOf("query_cancel").flatMap((c) => (c.args as { queryIds: string[] }).queryIds)).toContain(countId));
  });

  it("an_external_search_change_drops_a_pending_idle_apply", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { user } = await mount();
    await user.type(box(), "Sec{Enter}");
    await waitFor(() => expect(lastState().search.text).toBe("Sec"));
    await user.type(box(), "xyz"); // 400 ms not yet up
    await user.click(screen.getByRole("button", { name: "Clear search" }));
    await waitFor(() => expect(lastState().search.text).toBe(""));
    await act(() => vi.advanceTimersByTimeAsync(800));
    expect(lastState().search.text).toBe("");
    expect((box() as HTMLInputElement).value).toBe("");
    expect(screen.queryByText(/^search:/)).toBeNull();
  });
});

describe("host scope", () => {
  it("host_scope_sets_state_hosts", async () => {
    const { user } = await mount();
    const button = screen.getByRole("button", { name: /^Hosts: all 2/ });
    await user.click(button);
    const list = await screen.findByRole("dialog", { name: "Hosts" });
    expect(within(list).getByRole("checkbox", { name: "WS01" })).toBeChecked();
    expect(within(list).getByRole("checkbox", { name: "SRV01" })).toBeChecked();
    await user.click(within(list).getByRole("checkbox", { name: "SRV01" }));
    await waitFor(() => expect(lastState().hosts).toEqual(["WS01"]));
    expect(screen.getByRole("button", { name: /^Hosts: WS01/ })).toBeInTheDocument();
    // the last host cannot be unticked
    expect(within(list).getByRole("checkbox", { name: "WS01" })).toBeDisabled();
    await user.click(within(list).getByRole("checkbox", { name: "SRV01" }));
    await waitFor(() => expect(lastState().hosts).toEqual([])); // every host ticked is "all"
    expect(screen.getByRole("button", { name: /^Hosts: all 2/ })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Hosts" })).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: /^Hosts: all 2/ }));
  });
});

describe("display zone picker", () => {
  it("zone_picker_lists_host_zones_first_and_sets_the_case_zone", async () => {
    const { user } = await mount();
    await user.click(screen.getByRole("button", { name: /^TZ: UTC/ }));
    const dlg = await screen.findByRole("dialog", { name: "Display time zone" });
    const options = await within(dlg).findAllByRole("option");
    const labels = options.map((o) => o.textContent);
    expect(labels.slice(0, 2)).toEqual(["SRV01 · Etc/UTC", "WS01 · America/New_York"]);
    expect(labels).toContain("Asia/Tokyo");
    expect(labels.indexOf("Asia/Tokyo")).toBeGreaterThan(1);

    const find = within(dlg).getByRole("searchbox", { name: "Find a zone" });
    await user.type(find, "tok");
    expect(within(dlg).getAllByRole("option").map((o) => o.textContent)).toEqual(["Asia/Tokyo"]);
    await user.clear(find);
    await user.click(within(dlg).getByRole("option", { name: "WS01 · America/New_York" }));
    await waitFor(() => expect(callsOf("display_zone_set")).toHaveLength(1));
    expect(callsOf("display_zone_set")[0].args).toEqual({ zone: "America/New_York" });
    expect(screen.queryByRole("dialog", { name: "Display time zone" })).toBeNull();
  });
});

describe("keys while typing", () => {
  it("keys_do_not_fire_while_typing_in_search", async () => {
    const { user } = await mount();
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    await user.click(dataRows()[2].querySelectorAll("[role='gridcell']")[2]);
    const row = focusedRow();
    expect(row).not.toBeNull();
    await user.click(box());
    await user.keyboard(" ab{ArrowDown}{ArrowDown}{PageDown}{End}");
    await user.keyboard("{Control>}c{/Control}{Control>}{Shift>}c{/Shift}{/Control}");
    expect((box() as HTMLInputElement).value).toBe(" ab");
    expect(focusedRow()).toBe(row);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(callsOf("cell_value")).toHaveLength(0);
    expect(writeText).not.toHaveBeenCalled();
  });
});
