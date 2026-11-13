// Filtering in the grid: the filter row, the column filter panel, the chip bar, cell filters, the advanced editor and
// presets. Everything runs against the mock backend; a filter is a `Chip` in `GridState.chips`, never SQL.
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { caseOpen, caseSummary, settingsGet } from "../api";
import type { Chip } from "../contract/generated/Chip";
import type { GridState } from "../contract/generated/GridState";
import { HOSTILE } from "../mock/generated";
import { mockCall } from "../mock/backend";
import { ShellCtx, type ShellContext } from "../shell/context";
import type { Tab } from "../shell/tabs";
import { callsOf, overrideCommand } from "../test/backend";
import { displaySafe } from "../text";
import AdvancedEditor from "./AdvancedEditor";
import Grid from "./Grid";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";

vi.mock("./measure", () => ({ measureText: vi.fn(() => 50) }));

beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 440 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
});
afterEach(() => vi.restoreAllMocks());

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

type User = ReturnType<typeof userEvent.setup>;

async function mount() {
  await caseOpen("/cases/x.duckdb");
  const summary = await caseSummary("t");
  const settings = (await settingsGet()).settings;
  const reportError = vi.fn();
  const ctx: ShellContext = {
    summary, settings, change: { generation: 0, kinds: [] }, openDataset: () => {}, openScoped: () => {}, setGridStatus: () => {},
    setRightPane: () => {}, rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError, saveLayout: vi.fn(), setHistogram: vi.fn(), saveColourRules: vi.fn(),
  };
  const utils = render(<Host ctx={ctx} />);
  await waitFor(() => expect(screen.getAllByRole("columnheader").length).toBeGreaterThan(1));
  await waitFor(() => expect(dataRows().length).toBeGreaterThan(0));
  return { ...utils, reportError, user: userEvent.setup() };
}

const dataRows = () => screen.queryAllByRole("row").filter((r) => r.getAttribute("data-row-index") !== null);
const names = () => screen.getAllByRole("columnheader").filter((h) => h.querySelector(".col-name")).map((h) => h.querySelector(".col-name")!.textContent!);
const stateOf = (c: { args?: Record<string, unknown> }) => (c.args as { state: GridState }).state;
// The grid's own pages; the editor and tabs also ask the engine to check a candidate on a slot of their own.
const lastState = (): GridState => stateOf(callsOf("grid_page").filter((c) => !String(c.args!.slot).endsWith(":validate")).at(-1)!);
const chips = (): Chip[] => lastState().chips;
const settle = (ms = 400) => act(() => new Promise((r) => setTimeout(r, ms)));
const chipBar = () => screen.getByRole("region", { name: "Filters" });
const cellText = (row: number, column: string) => {
  const r = dataRows().find((el) => el.getAttribute("data-row-index") === String(row))!;
  return r.querySelectorAll("[role='gridcell']")[names().indexOf(column) + 1] as HTMLElement;
};
const status = () => screen.getByTestId("status").textContent ?? "";
const valuesArgs = () => callsOf("grid_values").map((c) => c.args as { exceptChips: string[]; contains: string | null; order: string });

async function openPanel(user: User, column: string) {
  await user.click(screen.getByRole("button", { name: `Filter ${column}` }));
  return screen.findByRole("dialog", { name: `Filter ${column}` });
}

describe("filter row", () => {
  it("filter_row_debounces_into_one_chip", async () => {
    const { user } = await mount();
    const input = screen.getByRole("textbox", { name: "Match UserName" });
    await user.type(input, "adm");
    expect(chips()).toEqual([]); // nothing yet: 300 ms of quiet first
    await waitFor(() => expect(chips()).toHaveLength(1), { timeout: 2000 });
    expect(chips()[0]).toMatchObject({
      origin: "filter_row", enabled: true, filter: { column: "UserName", op: "contains", value: "adm", case_sensitive: false },
    });
    expect(chips()[0].id).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
    await settle(200);
    // three keystrokes, one state with a filter: one page fetch for it
    expect(callsOf("grid_page").filter((c) => stateOf(c).chips.length > 0)).toHaveLength(1);
    await user.type(input, "i"); // replaces, never adds a second chip for the column
    await waitFor(() => expect(chips()[0].filter).toMatchObject({ value: "admi" }), { timeout: 2000 });
    expect(chips()).toHaveLength(1);
    await user.clear(input);
    await waitFor(() => expect(chips()).toEqual([]), { timeout: 2000 });
  });
});

describe("filter row and clearing", () => {
  it("clear_all_cancels_a_pending_filter_row_edit", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const { user } = await mount();
      await user.type(screen.getByRole("textbox", { name: "Match Channel" }), "Sec");
      await act(() => vi.advanceTimersByTimeAsync(400));
      await waitFor(() => expect(chips()).toHaveLength(1));
      // an edit of another box, then Clear all inside the 300 ms
      fireEvent.change(screen.getByRole("textbox", { name: "Match Level" }), { target: { value: "Err" } });
      await user.click(within(chipBar()).getByRole("button", { name: "Clear all" }));
      await act(() => vi.advanceTimersByTimeAsync(600));
      expect(chips()).toEqual([]);
      expect((screen.getByRole("textbox", { name: "Match Level" }) as HTMLInputElement).value).toBe("");
      // removing one chip cancels a pending edit of that same box
      fireEvent.change(screen.getByRole("textbox", { name: "Match Channel" }), { target: { value: "Sys" } });
      await act(() => vi.advanceTimersByTimeAsync(400));
      await waitFor(() => expect(chips()).toHaveLength(1));
      fireEvent.change(screen.getByRole("textbox", { name: "Match Channel" }), { target: { value: "Sysx" } });
      await user.click(within(chipBar()).getByRole("button", { name: /^Remove filter/ }));
      await act(() => vi.advanceTimersByTimeAsync(600));
      expect(chips()).toEqual([]);
      expect((screen.getByRole("textbox", { name: "Match Channel" }) as HTMLInputElement).value).toBe("");
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("values tab", () => {
  it("values_tab_counts_exclude_its_own_chip", async () => {
    const { user } = await mount();
    const dialog = await openPanel(user, "Channel");
    await user.click(await within(dialog).findByRole("checkbox", { name: "Security" }));
    await waitFor(() => expect(chips()).toHaveLength(1));
    const own = chips()[0];
    expect(own).toMatchObject({ origin: "values", filter: { column: "Channel", op: "in", value: ["Security"] } });
    await waitFor(() => expect(valuesArgs().at(-1)!.exceptChips).toEqual([own.id]));
    expect(within(dialog).getByText(/counts respect the other filters/i)).toBeTruthy();
    // the first list was fetched before any chip existed: nothing to leave out
    expect(valuesArgs()[0].exceptChips).toEqual([]);
    // the list still holds the values the chip leaves out
    expect(within(dialog).getByRole("checkbox", { name: "System" })).toBeTruthy();
    expect((within(dialog).getByRole("checkbox", { name: "Security" }) as HTMLInputElement).checked).toBe(true);
    // a search box narrows with a literal substring after 200 ms
    await user.type(within(dialog).getByRole("searchbox", { name: "Search values" }), "Sys");
    await waitFor(() => expect(valuesArgs().at(-1)!.contains).toBe("Sys"));
    expect(valuesArgs().filter((a) => a.contains !== null)).toHaveLength(1);
  });

  it("rarest_first_order", async () => {
    const { user } = await mount();
    const dialog = await openPanel(user, "Channel");
    await within(dialog).findByRole("checkbox", { name: "Security" });
    await user.selectOptions(within(dialog).getByLabelText("Order"), "rarest");
    await waitFor(() => expect(valuesArgs().at(-1)!.order).toBe("rarest"));
    await waitFor(() => {
      const counts = within(dialog).getAllByTestId("value-count").map((e) => Number((e.textContent ?? "").replace(/,/g, "")));
      expect(counts.length).toBeGreaterThan(2);
      expect(counts).toEqual([...counts].sort((a, b) => a - b));
    });
    await user.selectOptions(within(dialog).getByLabelText("Order"), "value");
    await waitFor(() => expect(valuesArgs().at(-1)!.order).toBe("value"));
  });

  it("include_exclude_switch_makes_in_and_not_in", async () => {
    const { user } = await mount();
    const dialog = await openPanel(user, "Channel");
    await user.click(await within(dialog).findByRole("checkbox", { name: "Security" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "System" }));
    await waitFor(() => expect(chips()[0].filter).toMatchObject({ op: "in", value: ["Security", "System"] }));
    const id = chips()[0].id;
    await user.click(within(dialog).getByRole("radio", { name: "Exclude" }));
    await waitFor(() => expect(chips()[0].filter).toMatchObject({ column: "Channel", op: "not_in", value: ["Security", "System"] }));
    expect(chips()).toHaveLength(1);
    expect(chips()[0].id).toBe(id);
    await user.click(within(dialog).getByRole("checkbox", { name: "Security" })); // untick
    await waitFor(() => expect(chips()[0].filter).toMatchObject({ op: "not_in", value: ["System"] }));
    await user.click(within(dialog).getByRole("checkbox", { name: "System" }));
    await waitFor(() => expect(chips()).toEqual([])); // nothing ticked, no chip
  });

  it("hostile_values_in_values_tab_and_chips_render_inert", async () => {
    const { user } = await mount();
    const dialog = await openPanel(user, "UserName");
    const html = await within(dialog).findByRole("checkbox", { name: displaySafe(HOSTILE.html) });
    expect(within(dialog).getByText(displaySafe(HOSTILE.bidi))).toBeTruthy(); // U+202E shown as text
    expect(dialog.querySelector("img")).toBeNull();
    await user.click(html);
    await user.click(within(dialog).getByRole("checkbox", { name: displaySafe(HOSTILE.bidi) }));
    await waitFor(() => expect(chips()).toHaveLength(1));
    // the stored value is the original, byte for byte
    expect(chips()[0].filter).toMatchObject({ op: "in", value: [HOSTILE.html, HOSTILE.bidi] });
    expect(chipBar().querySelector("img")).toBeNull();
    expect(chipBar().textContent).toContain("\\u{202E}");
    expect(chipBar().textContent).not.toContain("‮");
    // the filter row text becomes a label too
    await user.type(screen.getByRole("textbox", { name: "Match Message" }), HOSTILE.bell);
    await waitFor(() => expect(chips()).toHaveLength(2), { timeout: 2000 });
    expect(chipBar().textContent).toContain("\\u{0007}");
    expect(document.querySelector("img")).toBeNull();
  });
});

describe("text and date tabs", () => {
  it("text_tab_applies_an_operator_and_case", async () => {
    const { user } = await mount();
    const dialog = await openPanel(user, "Channel");
    await user.click(within(dialog).getByRole("tab", { name: "Text" }));
    expect(within(dialog).queryByRole("option", { name: "did not convert" })).toBeNull(); // untyped column
    await user.selectOptions(within(dialog).getByLabelText("Operator"), "starts_with");
    await user.type(within(dialog).getByLabelText("Text"), "Sec");
    await user.click(within(dialog).getByRole("button", { name: "Match case" }));
    await user.click(within(dialog).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(chips()).toHaveLength(1));
    expect(chips()[0]).toMatchObject({ origin: "text", enabled: true, filter: { column: "Channel", op: "starts_with", value: "Sec", case_sensitive: true } });
    // is empty takes no text, and replaces this column's Text chip
    const again = await openPanel(user, "Channel");
    await user.click(within(again).getByRole("tab", { name: "Text" }));
    await user.selectOptions(within(again).getByLabelText("Operator"), "is_empty");
    expect(within(again).queryByLabelText("Text")).toBeNull();
    await user.click(within(again).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(chips()[0].filter).toMatchObject({ op: "is_empty", value: null }));
    expect(chips()).toHaveLength(1);
  });

  it("typed_columns_offer_values_that_did_not_convert", async () => {
    const { user } = await mount();
    const dialog = await openPanel(user, "EventId");
    await user.click(within(dialog).getByRole("tab", { name: "Text" }));
    await user.selectOptions(within(dialog).getByLabelText("Operator"), "conversion_failed");
    await user.click(within(dialog).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(chips()[0].filter).toMatchObject({ column: "EventId", op: "conversion_failed", value: null }));
  });

  it("date_tab_only_on_time_columns_and_makes_the_engines_chip", async () => {
    const { user } = await mount();
    const channel = await openPanel(user, "Channel");
    expect(within(channel).queryByRole("tab", { name: "Date range" })).toBeNull();
    await user.keyboard("{Escape}");
    const dialog = await openPanel(user, "TimeCreated");
    await user.click(within(dialog).getByRole("tab", { name: "Date range" }));
    await user.type(within(dialog).getByLabelText("From"), "2026-01-05 03:00:00");
    await user.type(within(dialog).getByLabelText("To"), "2026-01-05 04:00:00");
    await user.click(within(dialog).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(chips()).toHaveLength(1));
    expect(chips()[0].origin).toBe("date_range");
    expect(callsOf("date_range_chip").at(-1)!.args).toMatchObject({
      relation: "mock_events", column: "TimeCreated", from: "2026-01-05 03:00:00", to: "2026-01-05 04:00:00", zone: "UTC",
    });
  });

  it("date_tab_shows_the_engines_refusal", async () => {
    const message = "2026-11-01 01:30:00 is ambiguous in America/New_York: 2026-11-01T05:30:00Z or 2026-11-01T06:30:00Z";
    overrideCommand("date_range_chip", () => Promise.reject({ code: "invalid", message }));
    const { user } = await mount();
    const dialog = await openPanel(user, "TimeCreated");
    await user.click(within(dialog).getByRole("tab", { name: "Date range" }));
    await user.type(within(dialog).getByLabelText("From"), "2026-11-01 01:30:00");
    await user.click(within(dialog).getByRole("button", { name: "Apply" }));
    const alert = await within(dialog).findByRole("alert");
    expect(alert.textContent).toBe(message);
    expect(chips()).toEqual([]);
    expect(dialog.isConnected).toBe(true); // stays open for a correction
  });
});

describe("chip bar", () => {
  it("disabled_chip_stays_but_does_not_apply", async () => {
    const { user } = await mount();
    await user.type(screen.getByRole("textbox", { name: "Match Channel" }), "Security");
    await waitFor(() => expect(chips()).toHaveLength(1), { timeout: 2000 });
    await waitFor(() => expect(status()).toMatch(/^[\d,]+ of 5,000 rows/));
    expect(status()).not.toMatch(/^5,000 of/);
    const toggle = within(chipBar()).getByRole("switch");
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    await user.click(toggle);
    await waitFor(() => expect(chips()[0].enabled).toBe(false)); // the page request carries it, off
    expect(chips()).toHaveLength(1);
    expect(within(chipBar()).getByRole("switch").getAttribute("aria-checked")).toBe("false");
    expect(within(chipBar()).getByRole("listitem").getAttribute("data-enabled")).toBe("false");
    await waitFor(() => expect(status()).toMatch(/^5,000 of 5,000 rows/)); // and no longer applied
    await user.click(within(chipBar()).getByRole("switch"));
    await waitFor(() => expect(chips()[0].enabled).toBe(true));
  });

  it("remove_and_clear_all", async () => {
    const { user } = await mount();
    await user.type(screen.getByRole("textbox", { name: "Match Channel" }), "Sec");
    await user.type(screen.getByRole("textbox", { name: "Match Level" }), "Err");
    await waitFor(() => expect(chips()).toHaveLength(2), { timeout: 2000 });
    await user.click(within(chipBar()).getAllByRole("button", { name: /^Remove filter/ })[0]);
    await waitFor(() => expect(chips()).toHaveLength(1));
    // removing the chip empties its box in the filter row
    expect((screen.getByRole("textbox", { name: "Match Channel" }) as HTMLInputElement).value).toBe("");
    await user.click(within(chipBar()).getByRole("button", { name: "Clear all" }));
    await waitFor(() => expect(chips()).toEqual([]));
    expect((screen.getByRole("textbox", { name: "Match Level" }) as HTMLInputElement).value).toBe("");
  });

  it("header_menu_filter_opens_the_panel", async () => {
    const { user } = await mount();
    const h = screen.getAllByRole("columnheader").find((el) => el.querySelector(".col-name")?.textContent === "Provider")!;
    fireEvent.contextMenu(h);
    await user.click(await screen.findByRole("menuitem", { name: "Filter…" }));
    expect(await screen.findByRole("dialog", { name: "Filter Provider" })).toBeTruthy();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Filter Provider" })).toBeNull();
  });

  it("plus_filter_opens_the_panel_for_a_chosen_column", async () => {
    const { user } = await mount();
    await user.click(within(chipBar()).getByRole("button", { name: "+ filter" }));
    await user.click(await screen.findByRole("menuitem", { name: "Provider" }));
    expect(await screen.findByRole("dialog", { name: "Filter Provider" })).toBeTruthy();
  });

  it("at_most_fifty_filters", async () => {
    const { user, reportError } = await mount();
    // fifty cell filters fill the bar; the 51st is refused with a message
    for (let i = 0; i < 51; i++) {
      await waitFor(() => expect(cellText(0, "Channel").textContent).not.toBe(""));
      fireEvent.contextMenu(cellText(0, "Channel"));
      await user.click(await screen.findByRole("menuitem", { name: "Filter in" }));
    }
    expect(chips()).toHaveLength(50);
    expect(reportError).toHaveBeenCalled();
  }, 60_000);
});

describe("cell filters", () => {
  it("cell_filter_in_and_out", async () => {
    const { user } = await mount();
    const channel = cellText(0, "Channel").textContent!;
    fireEvent.contextMenu(cellText(0, "Channel"));
    await user.click(await screen.findByRole("menuitem", { name: "Filter in" }));
    await waitFor(() => expect(chips()).toHaveLength(1));
    expect(chips()[0]).toMatchObject({ origin: "cell", enabled: true, filter: { column: "Channel", op: "eq", value: channel, case_sensitive: true } });
    await waitFor(() => expect(cellText(0, "Provider").textContent).not.toBe(""));
    const provider = cellText(0, "Provider").textContent!;
    fireEvent.contextMenu(cellText(0, "Provider"));
    await user.click(await screen.findByRole("menuitem", { name: "Filter out" }));
    await waitFor(() => expect(chips()).toHaveLength(2));
    expect(chips()[1]).toMatchObject({ origin: "cell", filter: { column: "Provider", op: "ne", value: provider, case_sensitive: true } });
  });

  it("a_clipped_cell_filters_by_its_whole_value", async () => {
    const { user } = await mount();
    // row 6 holds the 5,000 character message
    await waitFor(() => expect(cellText(6, "Message").textContent).toContain("LONG-"));
    fireEvent.contextMenu(cellText(6, "Message"));
    await user.click(await screen.findByRole("menuitem", { name: "Filter in" }));
    await waitFor(() => expect(chips()).toHaveLength(1));
    expect((chips()[0].filter as { value: string }).value).toBe(HOSTILE.long);
  });
});

describe("mark pseudo-column", () => {
  it("mark_pseudo_column_filters", async () => {
    const { user } = await mount();
    const dialog = await openPanel(user, "Mark");
    await user.click(within(dialog).getByRole("checkbox", { name: "Suspicious" }));
    await waitFor(() => expect(chips()).toHaveLength(1));
    expect(chips()[0]).toMatchObject({ filter: { column: "_mark", op: "in", value: ["suspicious"], case_sensitive: false } });
    expect(within(chipBar()).getByText("Mark: Suspicious")).toBeTruthy();
    await waitFor(() => expect(status()).toMatch(/^3 of 5,000 rows/));
    await user.click(within(dialog).getByRole("checkbox", { name: "Suspicious" }));
    await user.click(within(dialog).getByRole("checkbox", { name: "None" }));
    await waitFor(() => expect(chips()[0].filter).toMatchObject({ column: "_mark", op: "in", value: ["none"] }));
    await user.click(within(dialog).getByRole("checkbox", { name: "Suspicious" })); // both: no filter
    await waitFor(() => expect(chips()).toEqual([]));
  });
});

describe("advanced editor", () => {
  const pick = (label: string, i: number) => screen.getAllByLabelText(label)[i];

  it("advanced_editor_builds_and_or_groups", async () => {
    const { user } = await mount();
    await user.click(within(chipBar()).getByRole("button", { name: "Advanced…" }));
    const dialog = await screen.findByRole("dialog", { name: "Advanced filter" });
    await user.selectOptions(pick("Column", 0), "Channel");
    await user.selectOptions(pick("Operator", 0), "eq");
    await user.type(pick("Value", 0), "Security");
    await user.click(within(dialog).getByRole("button", { name: "Add condition" }));
    await user.selectOptions(pick("Column", 1), "EventId");
    await user.selectOptions(pick("Operator", 1), "eq");
    await user.type(pick("Value", 1), "4624");
    await user.selectOptions(pick("Match", 0), "or");
    await user.click(within(dialog).getByRole("button", { name: "Add group" }));
    await user.selectOptions(pick("Column", 2), "Level");
    await user.selectOptions(pick("Operator", 2), "eq");
    await user.type(pick("Value", 2), "Error");
    await user.click(within(dialog).getByRole("button", { name: "Add filter" }));
    await waitFor(() => expect(chips()).toHaveLength(1));
    expect(chips()[0]).toMatchObject({ origin: "advanced", enabled: true });
    expect(chips()[0].id).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
    const leaf = (column: string, value: string) => ({ column, op: "eq", value, case_sensitive: false });
    expect(chips()[0].filter).toEqual({ or: [leaf("Channel", "Security"), leaf("EventId", "4624"), { and: [leaf("Level", "Error")] }] });
    expect(screen.queryByRole("dialog", { name: "Advanced filter" })).toBeNull();
  });

  it("advanced_editor_stops_nesting_at_seven", async () => {
    const { user } = await mount();
    await user.click(within(chipBar()).getByRole("button", { name: "Advanced…" }));
    const dialog = await screen.findByRole("dialog", { name: "Advanced filter" });
    // the root group is level 1 and its leaves level 2; a chip may be 7 deep (the grid wraps it in one more AND)
    for (let i = 0; i < 5; i++) await user.click(within(dialog).getAllByRole("button", { name: "Add group" })[0]); // the deepest is first
    const adds = within(dialog).getAllByRole("button", { name: "Add group" });
    expect(adds).toHaveLength(6);
    expect((adds[0] as HTMLButtonElement).disabled).toBe(true);
    expect((adds[1] as HTMLButtonElement).disabled).toBe(false);
  });

  it("an_engine_refusal_shows_inline_and_adds_nothing", async () => {
    overrideCommand("grid_page", (args) => {
      if ((args as { state: GridState }).state.chips.some((c) => c.origin === "advanced")) {
        return Promise.reject({ code: "invalid", message: 'filter on "Channel": regex is not a valid pattern' });
      }
      return mockCall("grid_page", args);
    });
    const { user } = await mount();
    await user.click(within(chipBar()).getByRole("button", { name: "Advanced…" }));
    const dialog = await screen.findByRole("dialog", { name: "Advanced filter" });
    await user.selectOptions(pick("Column", 0), "Channel");
    await user.selectOptions(pick("Operator", 0), "regex");
    await user.type(pick("Value", 0), "(");
    await user.click(within(dialog).getByRole("button", { name: "Add filter" }));
    const alert = await within(dialog).findByRole("alert");
    expect(alert.textContent).toBe('filter on "Channel": regex is not a valid pattern');
    expect(dialog.isConnected).toBe(true);
    await settle(100);
    expect(chips()).toEqual([]);
  });

  it("a_group_without_conditions_is_refused_before_the_engine", async () => {
    const { user } = await mount();
    await user.click(within(chipBar()).getByRole("button", { name: "Advanced…" }));
    const dialog = await screen.findByRole("dialog", { name: "Advanced filter" });
    await user.click(within(dialog).getByRole("button", { name: "Add group" }));
    await user.click(within(dialog).getAllByRole("button", { name: /^Remove condition/ })[1]); // empties the nested group
    await user.click(within(dialog).getByRole("button", { name: "Add filter" }));
    expect((await within(dialog).findByRole("alert")).textContent).toMatch(/no conditions/);
  });
});

describe("presets", () => {
  it("save_preset_saves_only_chips", async () => {
    const { user } = await mount();
    await user.click(screen.getAllByRole("columnheader").find((h) => h.querySelector(".col-name")?.textContent === "Channel")!); // a sort
    await user.type(screen.getByRole("textbox", { name: "Match Channel" }), "Sec");
    await waitFor(() => expect(chips()).toHaveLength(1), { timeout: 2000 });
    expect(lastState().sort).not.toEqual([]);
    await user.click(within(chipBar()).getByRole("button", { name: "Save preset…" }));
    await user.type(screen.getByRole("textbox", { name: "Preset name" }), "Sec only");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(callsOf("view_save")).toHaveLength(1));
    expect(callsOf("view_save")[0].args).toEqual({
      name: "Sec only", kind: "preset", pattern: "mock_events",
      doc: {
        chips: [chips()[0]],
        search: { text: "", regex: false, case_sensitive: false, mode: "filter" },
        hosts: [], group_by: [], sort: [], layout: null, colour_rules: [],
      },
    });
  });

  it("a_refused_preset_name_shows_inline", async () => {
    overrideCommand("view_save", () => Promise.reject({ code: "invalid", message: "a preset name must not be empty" }));
    const { user } = await mount();
    await user.type(screen.getByRole("textbox", { name: "Match Channel" }), "Sec");
    await waitFor(() => expect(chips()).toHaveLength(1), { timeout: 2000 });
    await user.click(within(chipBar()).getByRole("button", { name: "Save preset…" }));
    await user.type(screen.getByRole("textbox", { name: "Preset name" }), "x");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect((await screen.findByRole("alert")).textContent).toBe("a preset name must not be empty");
  });
});

describe("advanced editor by column type", () => {
  const col = (name: string, sql_type: ColumnMeta["sql_type"]): ColumnMeta =>
    ({ name, role: "data", sql_type, time_semantics: null, origin: null }) as ColumnMeta;
  const columns = [col("Flag", "BOOLEAN"), col("Count", "BIGINT"), col("Name", null)];

  it("boolean_and_bigint_columns_send_typed_ordered_values", async () => {
    const user = userEvent.setup();
    const onAdd = vi.fn();
    render(<AdvancedEditor columns={columns} validate={async () => null} onAdd={onAdd} onClose={() => {}} />);
    await user.selectOptions(screen.getAllByLabelText("Column")[0], "Flag");
    await user.selectOptions(screen.getAllByLabelText("Operator")[0], "gte");
    await user.selectOptions(screen.getAllByLabelText("Value")[0], "true");
    await user.click(screen.getByRole("button", { name: "Add condition" }));
    await user.selectOptions(screen.getAllByLabelText("Column")[1], "Count");
    await user.selectOptions(screen.getAllByLabelText("Operator")[1], "between");
    await user.type(screen.getAllByLabelText("Value")[1], "10");
    await user.type(screen.getByLabelText("Upper value"), "20");
    await user.click(screen.getByRole("button", { name: "Add condition" }));
    await user.selectOptions(screen.getAllByLabelText("Column")[2], "Name");
    await user.selectOptions(screen.getAllByLabelText("Operator")[2], "gt");
    await user.type(screen.getAllByLabelText("Value")[2], "10");
    await user.click(screen.getByRole("button", { name: "Add filter" }));
    expect(onAdd).toHaveBeenCalledTimes(1);
    expect(onAdd.mock.calls[0][0].filter).toEqual({
      and: [
        { column: "Flag", op: "gte", value: true, case_sensitive: false },
        { column: "Count", op: "between", value: [10, 20], case_sensitive: false },
        { column: "Name", op: "gt", value: "10", case_sensitive: false },
      ],
    });
  });

  it("did_not_convert_is_offered_only_on_typed_columns", async () => {
    const user = userEvent.setup();
    render(<AdvancedEditor columns={columns} validate={async () => null} onAdd={() => {}} onClose={() => {}} />);
    const ops = () => within(screen.getAllByLabelText("Operator")[0]).queryAllByRole("option").map((o) => o.textContent);
    await user.selectOptions(screen.getAllByLabelText("Column")[0], "Name");
    expect(ops()).not.toContain("did not convert");
    await user.selectOptions(screen.getAllByLabelText("Column")[0], "Count");
    expect(ops()).toContain("did not convert");
  });
});

describe("dialog focus", () => {
  it("the_filter_panel_takes_focus_and_gives_it_back", async () => {
    const { user } = await mount();
    const button = screen.getByRole("button", { name: "Filter Channel" });
    const dialog = await openPanel(user, "Channel");
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(document.activeElement).toBe(button));
  });

  it("the_advanced_editor_traps_tab_and_restores_focus", async () => {
    const { user } = await mount();
    const opener = within(chipBar()).getByRole("button", { name: "Advanced…" });
    await user.click(opener);
    const dialog = await screen.findByRole("dialog", { name: "Advanced filter" });
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    for (let i = 0; i < 25; i++) {
      await user.tab();
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
    for (let i = 0; i < 25; i++) {
      await user.tab({ shift: true });
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Advanced filter" })).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(opener));
  });

  it("plus_filter_by_keyboard_returns_focus_to_the_button", async () => {
    const { user } = await mount();
    const plus = within(chipBar()).getByRole("button", { name: "+ filter" });
    plus.focus();
    await user.keyboard("{Enter}");
    await screen.findByRole("menu", { name: "Filter column" });
    await user.keyboard("{ArrowDown}{Enter}");
    const dialog = await screen.findByRole("dialog", { name: /^Filter / });
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(document.activeElement).toBe(plus));
  });
});

describe("filter row pending edits", () => {
  it("removing_an_unrelated_sole_chip_keeps_a_pending_filter_row_edit", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const { user } = await mount();
      await user.type(screen.getByRole("textbox", { name: "Match Channel" }), "Sec");
      await act(() => vi.advanceTimersByTimeAsync(400));
      await waitFor(() => expect(chips()).toHaveLength(1));
      fireEvent.change(screen.getByRole("textbox", { name: "Match Level" }), { target: { value: "Err" } });
      await user.click(within(chipBar()).getByRole("button", { name: /^Remove filter/ })); // the sole chip, another column's
      await act(() => vi.advanceTimersByTimeAsync(600));
      await waitFor(() => expect(chips()).toHaveLength(1));
      expect(chips()[0].filter).toMatchObject({ column: "Level", op: "contains", value: "Err" });
    } finally {
      vi.useRealTimers();
    }
  });
});
