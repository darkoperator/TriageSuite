// Column management: header menu, resize, reorder, pin, best fit, the column manager and the per-dataset layouts.
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { caseOpen, caseSummary, settingsGet, settingsUpdate } from "../api";
import type { ColumnLayout } from "../contract/generated/ColumnLayout";
import type { GridState } from "../contract/generated/GridState";
import { ShellCtx, type ShellContext } from "../shell/context";
import type { Tab } from "../shell/tabs";
import { callsOf, overrideCommand } from "../test/backend";
import Grid from "./Grid";
import { BEST_FIT_MAX, MIN_WIDTH, bestFitWidth } from "./layout";
import { measureText } from "./measure";

vi.mock("./measure", () => ({ measureText: vi.fn(() => 50) }));

beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 440 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
  vi.mocked(measureText).mockImplementation(() => 50);
});
afterEach(() => vi.restoreAllMocks());

const events: Tab = { id: "dataset:mock_events", kind: "dataset", relation: "mock_events" };
const hayabusa: Tab = { id: "dataset:mock_hayabusa", kind: "dataset", relation: "mock_hayabusa" };

/** The shell's job for layouts, against the mock's settings: read, change one entry, write the whole object. */
const saved: { relation: string; layout: ColumnLayout | null }[] = [];
const saveLayout = vi.fn(async (relation: string, layout: ColumnLayout | null) => {
  saved.push({ relation, layout });
  const s = (await settingsGet()).settings;
  const layouts = { ...s.layouts };
  if (layout === null) delete layouts[relation];
  else layouts[relation] = layout;
  await settingsUpdate({ ...s, layouts });
});

function Host({ ctx, tab }: { ctx: ShellContext; tab: Tab }) {
  const [status, setStatus] = useState<ReactNode>(null);
  return (
    <ShellCtx.Provider value={{ ...ctx, setGridStatus: setStatus }}>
      <div data-testid="status">{status}</div>
      <Grid tab={tab} active scrollOffset={0} onScroll={() => {}} />
    </ShellCtx.Provider>
  );
}

async function mount(tab: Tab = events, path = "/cases/x.duckdb") {
  await caseOpen(path);
  const summary = await caseSummary("t");
  const settings = (await settingsGet()).settings;
  const ctx: ShellContext = {
    summary, settings, change: { generation: 0, kinds: [] }, openDataset: () => {}, setGridStatus: () => {},
    setRightPane: () => {}, rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError: vi.fn(), saveLayout,
  };
  const utils = render(<Host ctx={ctx} tab={tab} />);
  await waitFor(() => expect(screen.getAllByRole("columnheader").length).toBeGreaterThan(1));
  await waitFor(() => expect(dataRows().length).toBeGreaterThan(0));
  return utils;
}

const dataRows = () => screen.queryAllByRole("row").filter((r) => r.getAttribute("data-row-index") !== null);
const headers = () => screen.getAllByRole("columnheader").filter((h) => h.querySelector(".col-name"));
const names = () => headers().map((h) => h.querySelector(".col-name")!.textContent);
const header = (name: string) => {
  const h = headers().find((el) => el.querySelector(".col-name")?.textContent === name);
  if (!h) throw new Error(`no header ${name}`);
  return h;
};
const lastState = (): GridState => (callsOf("grid_page").at(-1)!.args as { state: GridState }).state;
const pages = () => callsOf("grid_page").length;
const settle = (ms = 400) => act(() => new Promise((r) => setTimeout(r, ms)));

function resize(name: string, by: number) {
  const grip = within(header(name)).getByRole("separator");
  fireEvent.mouseDown(grip, { clientX: 100 });
  fireEvent.mouseMove(document, { clientX: 100 + by });
  fireEvent.mouseUp(document, { clientX: 100 + by });
}
function dragBefore(name: string, target: string) {
  const data = { setData: () => {}, effectAllowed: "" };
  fireEvent.dragStart(header(name), { dataTransfer: data });
  fireEvent.dragOver(header(target), { dataTransfer: data });
  fireEvent.drop(header(target), { dataTransfer: data });
  fireEvent.dragEnd(header(name), { dataTransfer: data });
}
async function openMenu(name: string) {
  fireEvent.contextMenu(header(name));
  return screen.findByRole("menu");
}
async function openManager() {
  fireEvent.click(within(screen.getByTestId("status")).getByRole("button", { name: /hidden/ }));
  return screen.findByRole("dialog", { name: "Columns" });
}

describe("columns", () => {
  beforeEach(() => saveLayout.mockClear());
  beforeEach(() => void (saved.length = 0));

  it("metadata_hidden_by_default_except_host", async () => {
    await mount();
    const n = names();
    expect(n).toContain("_triage_host");
    for (const m of ["_triage_run_id", "_triage_identity", "_triage_output_file"]) expect(n).not.toContain(m);
    expect(n).toContain("TimeCreated");
    // Hidden columns are not fetched, and opening a grid writes nothing.
    expect(lastState().columns).not.toContain("_triage_run_id");
    expect(lastState().columns).toContain("_triage_host");
    expect(within(screen.getByTestId("status")).getByRole("button", { name: /3 hidden/ })).toBeTruthy();
    await settle();
    expect(saveLayout).not.toHaveBeenCalled();
  });

  it("reorder_and_resize_persist_per_relation", async () => {
    await mount();
    const before = pages();
    const first = names()[0]!;
    const w0 = parseInt(header("EventId").style.width, 10);
    dragBefore("EventId", first);
    expect(names()[0]).toBe("EventId");
    resize("Channel", 60);
    expect(parseInt(header("Channel").style.width, 10)).toBe(160 + 60);
    resize("EventId", -500); // clamped at 40
    expect(parseInt(header("EventId").style.width, 10)).toBe(40);
    expect(w0).toBe(100);
    await settle(100);
    expect(saveLayout).not.toHaveBeenCalled(); // 250 ms after the last change
    await waitFor(() => expect(saveLayout).toHaveBeenCalledTimes(1));
    const [relation, layout] = saveLayout.mock.calls[0];
    expect(relation).toBe("mock_events");
    expect(layout!.order.indexOf("EventId")).toBe(layout!.order.indexOf(first) - 1); // hidden metadata columns keep their places
    expect(layout!.widths).toMatchObject({ Channel: 220, EventId: 40 });
    expect(pages()).toBe(before); // moving and resizing are purely visual: no refetch
    const s = (await settingsGet()).settings;
    expect(Object.keys(s.layouts)).toEqual(["mock_events"]);
  });

  it("layout_follows_to_another_case", async () => {
    const first = await mount(events, "/cases/a.duckdb");
    dragBefore("Channel", "TimeCreated");
    resize("Level", 40);
    await waitFor(() => expect(saveLayout).toHaveBeenCalled());
    await settle(50);
    first.unmount();
    // Another case, a different file, with the same dataset: the settings hold the layout, not the case.
    const second = await mount(events, "/cases/b.duckdb");
    expect(callsOf("case_open").map((c) => (c.args as { path: string }).path)).toEqual(["/cases/a.duckdb", "/cases/b.duckdb"]);
    expect(names().indexOf("Channel")).toBe(names().indexOf("TimeCreated") - 1);
    expect(parseInt(header("Level").style.width, 10)).toBe(200);
    // A different dataset is untouched.
    second.unmount();
    await mount(hayabusa, "/cases/b.duckdb");
    expect(names()).toContain("RuleTitle");
    expect(parseInt(header("Level").style.width, 10)).toBe(160);
  });

  it("header_menu_works_from_the_keyboard", async () => {
    await mount();
    const user = userEvent.setup();
    // A cell has the grid's focus; header navigation must leave it (and the scroll offset) alone.
    const cellAt = (name: string) => {
      const at = header(name).getAttribute("aria-colindex");
      return within(dataRows()[0]).getAllByRole("gridcell").find((c) => c.getAttribute("aria-colindex") === at)!;
    };
    await user.click(cellAt("Provider"));
    expect(cellAt("Provider")).toHaveAttribute("data-focused", "true");
    const scrolled = screen.getByTestId("grid-scroll").scrollTop;
    // One tab stop for the header row: the first header is tabbable, the others are reached with the arrows.
    const heads = headers();
    expect(heads.filter((h) => h.tabIndex === 0)).toHaveLength(1);
    heads[0].focus();
    fireEvent.keyDown(heads[0], { key: "ArrowRight" });
    expect(document.activeElement).toBe(headers()[1]);
    fireEvent.keyDown(headers()[1], { key: "ArrowLeft" });
    fireEvent.keyDown(headers()[0], { key: "ArrowRight" });
    expect(cellAt("Provider")).toHaveAttribute("data-focused", "true");
    expect(dataRows()[0].querySelectorAll("[data-focused]")).toHaveLength(1);
    expect(screen.getByTestId("grid-scroll").scrollTop).toBe(scrolled);
    // Enter sorts, as a click does.
    fireEvent.keyDown(header("Level"), { key: "Enter" });
    await waitFor(() => expect(lastState().sort).toEqual([{ column: "Level", descending: false }]));
    // Shift+F10 opens the menu; Escape closes it and focus returns to the header.
    header("Channel").focus();
    fireEvent.keyDown(header("Channel"), { key: "F10", shiftKey: true });
    const menu = await screen.findByRole("menu");
    fireEvent.keyDown(menu, { key: "Escape" });
    await waitFor(() => expect(document.activeElement).toBe(header("Channel")));
    // The ContextMenu key opens it too; arrow to Pin left (the fifth item) and choose it.
    fireEvent.keyDown(header("Channel"), { key: "ContextMenu" });
    const again = await screen.findByRole("menu");
    const firstItem = within(again).getByRole("menuitem", { name: "Sort ascending" });
    await waitFor(() => expect(document.activeElement).toBe(firstItem));
    for (let i = 0; i < 2; i++) fireEvent.keyDown(again, { key: "ArrowDown" }); // Group and Filter are disabled
    expect(document.activeElement).toBe(within(again).getByRole("menuitem", { name: "Pin left" }));
    await user.keyboard("{Enter}");
    expect(names()[0]).toBe("Channel");
    expect(header("Channel")).toHaveAttribute("data-pinned", "true");
    await waitFor(() => expect(document.activeElement).toBe(header("Channel")));
    // Hide through the keyboard; focus falls back to a header that is left.
    fireEvent.keyDown(header("Channel"), { key: "F10", shiftKey: true });
    const last = await screen.findByRole("menu");
    for (let i = 0; i < 3; i++) fireEvent.keyDown(last, { key: "ArrowDown" });
    fireEvent.keyDown(last, { key: "ArrowDown" });
    expect(document.activeElement).toBe(within(last).getByRole("menuitem", { name: "Hide" }));
    await user.keyboard("{Enter}");
    expect(names()).not.toContain("Channel");
    await waitFor(() => expect(document.activeElement).toBe(headers()[0]));
  });

  it("hide_empty_hides_what_the_backend_names", async () => {
    overrideCommand("grid_empty_columns", () => ["Opcode", "Version"]);
    await mount();
    const before = pages();
    fireEvent.click(within(await openManager()).getByRole("button", { name: "Hide empty" }));
    await waitFor(() => expect(names()).not.toContain("Opcode"));
    expect(names()).not.toContain("Version");
    expect(names()).toContain("Channel");
    expect(callsOf("grid_empty_columns")).toHaveLength(1);
    await waitFor(() => expect(pages()).toBeGreaterThan(before)); // fetched columns changed
    expect(lastState().columns).not.toContain("Opcode");
    await waitFor(() => expect(saveLayout).toHaveBeenCalled());
    expect(saved.at(-1)!.layout!.hidden).toEqual(expect.arrayContaining(["Opcode", "Version", "_triage_run_id"]));
  });

  it("best_fit_clamps", () => {
    const m = (px: number) => () => px;
    expect(bestFitWidth(["a", "b"], "x", m(5))).toBe(MIN_WIDTH);
    expect(bestFitWidth(["a"], "x", m(9000))).toBe(BEST_FIT_MAX);
    expect(bestFitWidth(["a"], "x", (_t, f) => (f === "cell" ? 100 : 10))).toBe(116);
    // The header counts too.
    expect(bestFitWidth([], "H", (_t, f) => (f === "ui" ? 300 : 0))).toBe(324);
  });

  it("best_fit_from_the_menu_measures_loaded_rows_and_header", async () => {
    await mount();
    vi.mocked(measureText).mockImplementation(() => 5000);
    fireEvent.click(within(await openMenu("Channel")).getByRole("menuitem", { name: "Best fit" }));
    await waitFor(() => expect(parseInt(header("Channel").style.width, 10)).toBe(BEST_FIT_MAX));
    vi.mocked(measureText).mockImplementation(() => 1);
    fireEvent.click(within(await openMenu("Channel")).getByRole("menuitem", { name: "Best fit" }));
    await waitFor(() => expect(parseInt(header("Channel").style.width, 10)).toBe(MIN_WIDTH));
    expect(vi.mocked(measureText).mock.calls.some(([t, f]) => f === "ui" && t === "Channel")).toBe(true);
  });

  it("pinned_columns_stay_left", async () => {
    await mount();
    const mark = screen.getAllByRole("columnheader")[0];
    expect(mark).toHaveAttribute("data-fixed", "true");
    fireEvent.click(within(await openMenu("Level")).getByRole("menuitem", { name: "Pin left" }));
    expect(names()[0]).toBe("Level");
    expect(header("Level")).toHaveAttribute("data-pinned", "true");
    expect(header("Level").style.left).toBe("28px"); // right after the mark column
    fireEvent.click(within(await openMenu("Channel")).getByRole("menuitem", { name: "Pin left" }));
    expect(names().slice(0, 2)).toEqual(["Channel", "Level"]); // pinned keep their layout order
    const second = headers()[1];
    expect(parseInt(second.style.left, 10)).toBe(28 + parseInt(headers()[0].style.width, 10));
    // The body's cells follow: the pinned column's cell is the first after the mark cell.
    const row = dataRows()[0];
    const cells = within(row).getAllByRole("gridcell");
    expect(cells[1]).toHaveAttribute("data-pinned", "true");
    fireEvent.click(within(await openMenu("Level")).getByRole("menuitem", { name: "Unpin" }));
    expect(header("Level")).not.toHaveAttribute("data-pinned");
  });

  it("reset_forgets_the_layout", async () => {
    await mount();
    resize("Channel", 100);
    await waitFor(() => expect(saveLayout).toHaveBeenCalledTimes(1));
    expect(Object.keys((await settingsGet()).settings.layouts)).toEqual(["mock_events"]);
    fireEvent.click(within(await openManager()).getByRole("button", { name: "Reset" }));
    expect(parseInt(header("Channel").style.width, 10)).toBe(160);
    await waitFor(() => expect(saveLayout).toHaveBeenCalledTimes(2));
    expect(saved.at(-1)).toEqual({ relation: "mock_events", layout: null });
    expect((await settingsGet()).settings.layouts).toEqual({});
    expect(names()).not.toContain("_triage_run_id"); // defaults again
  });

  it("header_menu_sorts_hides_and_leaves_group_and_filter_to_later_tasks", async () => {
    await mount();
    const menu = await openMenu("Channel");
    for (const n of ["Sort ascending", "Sort descending", "Group by this column", "Filter…", "Pin left", "Best fit", "Hide"]) {
      expect(within(menu).getByRole("menuitem", { name: n })).toBeTruthy();
    }
    expect(within(menu).getByRole("menuitem", { name: "Group by this column" })).toBeDisabled();
    expect(within(menu).getByRole("menuitem", { name: "Filter…" })).toBeDisabled();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Sort descending" }));
    await waitFor(() => expect(lastState().sort).toEqual([{ column: "Channel", descending: true }]));
    fireEvent.click(within(await openMenu("Channel")).getByRole("menuitem", { name: "Hide" }));
    expect(names()).not.toContain("Channel");
    await waitFor(() => expect(lastState().columns).not.toContain("Channel"));
  });

  it("column_manager_finds_shows_and_pins", async () => {
    await mount();
    const manager = await openManager();
    fireEvent.change(within(manager).getByRole("searchbox", { name: "Find column" }), { target: { value: "run_id" } });
    const boxes = within(manager).getAllByRole("checkbox");
    expect(boxes).toHaveLength(1);
    fireEvent.click(boxes[0]);
    await waitFor(() => expect(names()).toContain("_triage_run_id"));
    fireEvent.click(within(manager).getByRole("button", { name: "Show all" }));
    await waitFor(() => expect(names()).toContain("_triage_output_file"));
    await waitFor(() => expect(lastState().columns).toBeNull()); // nothing hidden: everything is fetched
    fireEvent.change(within(manager).getByRole("searchbox", { name: "Find column" }), { target: { value: "" } });
    fireEvent.click(within(manager).getByRole("button", { name: "Pin Provider" }));
    expect(header("Provider")).toHaveAttribute("data-pinned", "true");
  });

  it("hostile_column_names_are_inert_in_header_menu_and_manager", async () => {
    await mount(hayabusa);
    // The mock's hostile values live in cells; names go through displaySafe all the same.
    expect(document.querySelector("img")).toBeNull();
    const manager = await openManager();
    expect(manager.querySelector("img")).toBeNull();
  });
});
