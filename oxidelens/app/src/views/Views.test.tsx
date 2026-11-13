import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CmdError } from "../api";
import type { GridState } from "../contract/generated/GridState";
import type { MatchingView } from "../contract/generated/MatchingView";
import type { Session } from "../contract/generated/Session";
import type { ViewDoc } from "../contract/generated/ViewDoc";
import * as dialogs from "../dialogs";
import { mockCall } from "../mock/backend";
import { callsOf, overrideCommand } from "../test/backend";
import { displaySafe } from "../text";
import Shell from "../shell/Shell";
import ExportDialog from "./ExportDialog";
import Grid from "../grid/Grid";
import { ShellCtx, type ShellContext } from "../shell/context";
import type { Tab } from "../shell/tabs";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { Settings } from "../contract/generated/Settings";

vi.mock("../dialogs", () => ({
  pickNewCasePath: vi.fn(),
  pickCaseFile: vi.fn(),
  pickRunFolder: vi.fn(),
  pickCsvFile: vi.fn(),
  pickCsvSavePath: vi.fn(),
  pickViewSavePath: vi.fn(),
  pickViewFile: vi.fn(),
}));

const HOSTILE = '<img src=x onerror=alert(1)>‮"; DROP';

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

const pane = () => within(screen.getByRole("complementary", { name: "Case" }));

async function openCase() {
  const user = userEvent.setup();
  render(<Shell />);
  await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
  await screen.findByRole("tab", { name: "Overview" });
  return user;
}

async function openDataset(user: ReturnType<typeof userEvent.setup>, relation: string) {
  await user.click(await pane().findByRole("button", { name: new RegExp(`^${relation}`) }));
  await waitFor(() => expect(screen.getAllByRole("columnheader").length).toBeGreaterThan(1));
}

const viewButton = (name: string | RegExp) => pane().findByRole("button", { name });
const savedView = async (name: string, pattern: string, doc: Partial<ViewDoc> = {}) =>
  mockCall("view_save", {
    name, kind: "view", pattern,
    doc: { chips: [], search: { text: "", regex: false, case_sensitive: false, mode: "filter" }, hosts: [], group_by: [], sort: [], layout: null, colour_rules: [], ...doc },
  });

const matched = (id: number, name: string, author: string, relations = ["mock_events"]): MatchingView => ({
  view: {
    id, name, kind: "view", pattern: "mock_*", created_by: author, updated_by: author, created_at: "x", updated_at: "x",
    doc: { chips: [], search: { text: "", regex: false, case_sensitive: false, mode: "filter" }, hosts: [], group_by: [], sort: [], layout: null, colour_rules: [] },
  },
  relations,
});

describe("saved views", () => {
  it("save_and_apply_round_trip", async () => {
    // A value of its own for every part, so a part dropped or swapped on the way shows.
    const doc: ViewDoc = {
      chips: [
        { id: "c1", label: "EventId = 4624", enabled: true, origin: "cell", filter: { column: "EventId", op: "eq", value: "4624", case_sensitive: false } },
        { id: "c2", label: "Level = 4", enabled: false, origin: "values", filter: { column: "Level", op: "eq", value: "4", case_sensitive: true } },
      ],
      search: { text: "powershell", regex: true, case_sensitive: true, mode: "filter" },
      hosts: ["WS01"],
      group_by: ["Channel", "Provider"],
      sort: [{ column: "Provider", descending: true }, { column: "RecordId", descending: false }],
      layout: { order: ["Provider", "Channel"], widths: { Provider: 210 }, hidden: ["Computer"], pinned: ["Provider"] },
      colour_rules: [
        { id: "r1", name: "Sysmon red", enabled: true, scope: "row", colour: "red", filter: { column: "Channel", op: "eq", value: "Sysmon", case_sensitive: false } },
        { id: "r2", name: "Blue cell", enabled: false, scope: "cell", colour: "blue", filter: { column: "Level", op: "eq", value: "2", case_sensitive: false } },
      ] as ViewDoc["colour_rules"],
    };
    const user = await openCase();
    await savedView("Distinct", "mock_*", doc);
    await openDataset(user, "mock_events");

    // Applying a view replaces the tab's state and group-by with the engine's answer.
    await user.click(await viewButton(/^Distinct/));
    await waitFor(() => expect(callsOf("view_apply")).toHaveLength(1));
    expect(callsOf("view_apply")[0].args).toMatchObject({ relation: "mock_events" });
    expect(await screen.findByText("EventId = 4624")).toBeInTheDocument();
    await waitFor(() => expect(callsOf("grid_groups").length).toBeGreaterThan(0));
    const shown = (callsOf("grid_groups").at(-1)!.args as { state: GridState; by: string[] });
    expect(shown.state.search).toEqual(doc.search);
    expect(shown.state.hosts).toEqual(["WS01"]);
    expect(shown.state.chips).toEqual(doc.chips);
    expect(shown.state.sort).toEqual(doc.sort);
    expect(shown.by).toEqual(["Channel", "Provider"]);

    // Save what the tab shows, under a new name, with the pattern defaulting to the relation's name.
    await user.click(screen.getByRole("button", { name: "Save view…" }));
    const dialog = await screen.findByRole("dialog", { name: "Save view" });
    expect(within(dialog).getByLabelText("Dataset pattern")).toHaveValue("mock_events");
    await user.type(within(dialog).getByLabelText("View name"), "Copy");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(callsOf("view_save").filter((c) => (c.args as { name: string }).name === "Copy")).toHaveLength(1));
    const sent = callsOf("view_save").at(-1)!.args as { name: string; kind: string; pattern: string; doc: ViewDoc };
    expect(sent).toMatchObject({ name: "Copy", kind: "view", pattern: "mock_events" });
    expect(sent.doc).toEqual(doc);
    expect(screen.queryByRole("dialog", { name: "Save view" })).toBeNull();

    // Clear the filters, apply the saved copy: they are back, switched as they were.
    await user.click(screen.getByRole("button", { name: "Clear all" }));
    await waitFor(() => expect(screen.queryByText("EventId = 4624")).toBeNull());
    await user.click(await viewButton(/^Copy/));
    expect(await screen.findByText("EventId = 4624")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Use filter Level = 4" })).toHaveAttribute("aria-checked", "false");
    expect(callsOf("view_apply")).toHaveLength(2);
  });

  it("apply_shows_warnings_for_missing_columns", async () => {
    const rule = (id: string, column: string) => ({ id, name: `rule ${id}`, enabled: true, scope: "row", colour: "red", filter: { column, op: "eq", value: "x", case_sensitive: false } });
    const user = await openCase();
    await savedView("Ghosts", "mock_*", {
      chips: [{ id: "c1", label: "Ghost chip", enabled: true, origin: "cell", filter: { column: "NoSuchChip", op: "eq", value: "1", case_sensitive: false } }],
      sort: [{ column: "NoSuchSort", descending: false }],
      group_by: ["NoSuchGroup"],
      colour_rules: [rule("r1", "NoSuchRule")] as ViewDoc["colour_rules"],
    });
    await openDataset(user, "mock_hayabusa");
    await user.click(await viewButton(/^Ghosts/));
    const bar = await screen.findByRole("region", { name: "View notices" });
    // Each inactive part is named, with its column; the chip that names a missing column is switched off, not dropped.
    const warned = within(bar).getAllByRole("listitem").map((li) => ({
      part: li.querySelector(".view-warning-part")?.textContent,
      name: li.querySelector(".view-warning-name")?.textContent,
    }));
    expect(warned).toEqual([
      { part: "chip", name: "NoSuchChip" },
      { part: "sort", name: "NoSuchSort" },
      { part: "group", name: "NoSuchGroup" },
      { part: "colour_rule", name: "NoSuchRule" },
    ]);
    expect(await screen.findByText("Ghost chip")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Use filter Ghost chip" })).toHaveAttribute("aria-checked", "false");
    await user.click(within(bar).getByRole("button", { name: "Dismiss view notices" }));
    expect(screen.queryByRole("region", { name: "View notices" })).toBeNull();
  });

  it("a_colour_rule_naming_a_missing_column_is_kept_switched_off_and_stays_inert", async () => {
    const ghost = { id: "r1", name: "Ghost rule", enabled: true, scope: "row", colour: "red", filter: { column: "NoSuchRule", op: "eq", value: "x", case_sensitive: false } };
    const user = await openCase();
    await savedView("Ghost rules", "mock_*", { colour_rules: [ghost] as ViewDoc["colour_rules"] });
    await openDataset(user, "mock_hayabusa");
    await user.click(await viewButton(/^Ghost rules/));
    // The rule is kept, switched off, in what the settings hold for the relation and in the colour-rule editor.
    await waitFor(() => expect(callsOf("settings_update").length).toBeGreaterThan(0));
    const sent = callsOf("settings_update").at(-1)!.args as { settings: Settings };
    expect(sent.settings.colour_rules.mock_hayabusa).toEqual([{ ...ghost, enabled: false }]);
    expect(screen.getByRole("button", { name: /^Colours/ })).toHaveTextContent("Colours 0");
    await user.click(screen.getByRole("button", { name: /^Colours/ }));
    const dialog = await screen.findByRole("dialog", { name: "Colour rules" });
    const tick = within(dialog).getByRole("checkbox", { name: "Ghost rule enabled" });
    expect(tick).not.toBeChecked();
    // Switched back on while the column is still missing, it tints nothing: the engine skips it and says why.
    await user.click(tick);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(/skipped: .*NoSuchRule/);
    const pages = callsOf("grid_page").map((c) => c.args as { options: { rules: { id: string; enabled: boolean }[] } });
    expect(pages.at(-1)!.options.rules).toEqual([expect.objectContaining({ id: "r1", enabled: true })]);
  });

  it("apply_writes_layout_and_colour_rules_through_the_settings_queue", async () => {
    const layout = { order: ["Channel"], widths: {}, hidden: [], pinned: [] };
    const rules = [{ id: "r1", name: "Red", enabled: true, scope: "row", colour: "red", filter: { column: "Channel", op: "eq", value: "Sysmon", case_sensitive: false } }];
    const user = await openCase();
    await savedView("Styled", "mock_*", { layout, colour_rules: rules as never });
    await openDataset(user, "mock_events");
    await user.click(await viewButton(/^Styled/));
    await waitFor(() => expect(callsOf("settings_update").length).toBeGreaterThan(0));
    const last = callsOf("settings_update").at(-1)!.args as { settings: { layouts: Record<string, unknown>; colour_rules: Record<string, unknown> } };
    expect(last.settings.layouts.mock_events).toEqual(layout);
    expect(last.settings.colour_rules.mock_events).toEqual(rules);
    expect(await screen.findByRole("region", { name: "View notices" })).toHaveTextContent(/saved for mock_events on this machine/);
  });

  it("a_layout_applied_in_one_tab_reaches_another_tab_of_the_relation_and_its_next_edit_builds_on_it", async () => {
    // Two tabs of mock_events: the dataset tab, and a session's tab scoped to a host.
    overrideCommand("sessions", () => [{ source_id: 1, name: "Sess", unmatched: 0, relations: [{ relation: "mock_events", host: "WS01" }] }]);
    const layout = { order: ["Channel", "EventId"], widths: { Channel: 300 }, hidden: [], pinned: [] };
    const user = await openCase();
    await savedView("Styled", "mock_*", { layout });
    await openDataset(user, "mock_events");
    const heads = () => screen.getAllByRole("columnheader").map((h) => h.querySelector(".col-name")?.textContent).filter(Boolean);
    expect(heads()[0]).not.toBe("Channel");
    await user.click(pane().getByRole("button", { name: /^Sess/ }));
    await screen.findByRole("tab", { name: "Sess · mock_events", selected: true });
    await waitFor(() => expect(heads().length).toBeGreaterThan(1));

    // The view is applied in the session's tab and its layout written for the relation.
    await user.click(await viewButton(/^Styled/));
    await waitFor(() => expect(heads()[0]).toBe("Channel"));
    await waitFor(() => expect((callsOf("settings_update").at(-1)!.args as { settings: Settings }).settings.layouts.mock_events).toEqual(layout));

    // The dataset tab shows it too.
    await user.click(screen.getByRole("tab", { name: "mock_events" }));
    await waitFor(() => expect(heads().slice(0, 2)).toEqual(["Channel", "EventId"]));
    const channel = screen.getAllByRole("columnheader").find((h) => h.querySelector(".col-name")?.textContent === "Channel")!;
    expect(parseInt(channel.style.width, 10)).toBe(300);

    // Its next edit is made on top of that layout, not the one it opened with.
    const writes = callsOf("settings_update").length;
    const grip = within(screen.getAllByRole("columnheader").find((h) => h.querySelector(".col-name")?.textContent === "EventId")!).getByRole("separator");
    fireEvent.mouseDown(grip, { clientX: 100 });
    fireEvent.mouseMove(document, { clientX: 150 });
    fireEvent.mouseUp(document, { clientX: 150 });
    await waitFor(() => expect(callsOf("settings_update").length).toBeGreaterThan(writes));
    const saved = (callsOf("settings_update").at(-1)!.args as { settings: Settings }).settings.layouts.mock_events;
    expect(saved.order.slice(0, 2)).toEqual(["Channel", "EventId"]);
    expect(saved.widths).toMatchObject({ Channel: 300, EventId: 150 });
  });

  it("pattern_views_list_for_matching_relations", async () => {
    const user = await openCase();
    await savedView(HOSTILE, "mock_*");
    await savedView("Elsewhere", "nomatch_*");
    await savedView("Hayabusa only", "mock_hayabusa");
    const list = await pane().findByRole("list", { name: "Saved views" });
    await waitFor(() => expect(within(list).getAllByRole("button").length).toBeGreaterThanOrEqual(3));
    // One read answers for every relation (the engine matches the patterns); a view matching none is not offered.
    expect(callsOf("views_list")).toHaveLength(0);
    const last = callsOf("views_matching").at(-1)!.args as { limit: number; offset: number };
    expect(last).toMatchObject({ limit: 200, offset: 0 });
    expect(within(list).queryByText("Elsewhere")).toBeNull();
    // Hostile names are text; presets are marked.
    expect(within(list).getByText(displaySafe(HOSTILE))).toBeInTheDocument();
    expect(within(list).getByText("preset")).toBeInTheDocument();

    // A view that matches only another relation opens that relation's tab, whatever tab is showing.
    await user.click(within(list).getByRole("button", { name: /Hayabusa only/ }));
    await screen.findByRole("tab", { name: "mock_hayabusa" });
    expect(callsOf("view_apply").at(-1)!.args).toMatchObject({ relation: "mock_hayabusa" });
  });

  it("export_and_import_view_files", async () => {
    vi.mocked(dialogs.pickViewSavePath).mockResolvedValue("/out/view.json");
    vi.mocked(dialogs.pickViewFile).mockResolvedValue("/in/view.json");
    const user = await openCase();
    const item = await viewButton(/^Logons/);
    fireEvent.contextMenu(item);
    await user.click(await screen.findByRole("menuitem", { name: "Export…" }));
    await waitFor(() => expect(callsOf("view_export")).toHaveLength(1));
    expect(callsOf("view_export")[0].args).toMatchObject({ id: 1, path: "/out/view.json" });

    await user.click(pane().getByRole("button", { name: "Import view…" }));
    await waitFor(() => expect(callsOf("view_import")).toHaveLength(1));
    expect(callsOf("view_import")[0].args).toEqual({ path: "/in/view.json" });
    expect(await viewButton(/^Imported view/)).toBeInTheDocument();

    // A refusal is shown with its message.
    overrideCommand("view_import", () => {
      throw { code: "refused", message: "not a view file: unexpected end" } satisfies CmdError;
    });
    await user.click(pane().getByRole("button", { name: "Import view…" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("not a view file: unexpected end");
  });

  it("deleting_a_view_asks_and_names_an_agent_author", async () => {
    overrideCommand("views_matching", () => ({ items: [matched(9, "By agent", "agent:claude")], truncated: false }));
    const user = await openCase();
    const item = await viewButton(/By agent/);
    expect(item).toHaveTextContent("agent:claude");
    fireEvent.contextMenu(item);
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    expect(callsOf("view_delete")).toHaveLength(0);
    await user.click(await screen.findByRole("button", { name: "Delete view" }));
    await waitFor(() => expect(callsOf("view_delete")).toHaveLength(1));
    expect(callsOf("view_delete")[0].args).toEqual({ id: 9 });
  });

  it("the_list_pages_with_more_beyond_the_first_page", async () => {
    overrideCommand("views_matching", (args) => {
      const { offset } = args as { offset: number };
      return offset === 0
        ? { items: [matched(1, "First page", "analyst:a")], truncated: true }
        : { items: [matched(2, "Second page", "analyst:a")], truncated: false };
    });
    const user = await openCase();
    await viewButton(/First page/);
    await user.click(pane().getByRole("button", { name: "More…" }));
    await viewButton(/Second page/);
    await viewButton(/First page/);
    expect(callsOf("views_matching").at(-1)!.args).toMatchObject({ limit: 200, offset: 1 });
    expect(pane().queryByRole("button", { name: "More…" })).toBeNull();
  });

  it("click_applies_to_the_current_relation_when_among_the_matches_else_the_first", async () => {
    overrideCommand("views_matching", () => ({
      items: [{ ...matched(5, "Two relations", "analyst:a"), relations: ["mock_events", "mock_hayabusa"] }],
      truncated: false,
    }));
    const user = await openCase();
    await openDataset(user, "mock_hayabusa");
    await user.click(await viewButton(/Two relations/));
    await waitFor(() => expect(callsOf("view_apply")).toHaveLength(1));
    expect(callsOf("view_apply")[0].args).toMatchObject({ id: 5, relation: "mock_hayabusa" });
    await user.click(screen.getByRole("tab", { name: "Overview" }));
    await user.click(await viewButton(/Two relations/));
    await waitFor(() => expect(callsOf("view_apply")).toHaveLength(2));
    expect(callsOf("view_apply")[1].args).toMatchObject({ id: 5, relation: "mock_events" });
  });

  /** Answers `views_matching` by hand: each call waits in `calls` until the test resolves it. */
  function pendingReads() {
    const calls: { offset: number; resolve(v: { items: MatchingView[]; truncated: boolean }): void }[] = [];
    overrideCommand("views_matching", (args) => new Promise((resolve) => {
      calls.push({ offset: (args as { offset: number }).offset, resolve: resolve as never });
    }));
    return calls;
  }
  const namesIn = (list: HTMLElement) => within(list).getAllByRole("button").map((b) => b.querySelector(".pane-item-name")?.textContent);

  it("a_reload_disables_more_until_page_zero_arrives", async () => {
    const calls = pendingReads();
    await openCase();
    await waitFor(() => expect(calls).toHaveLength(1));
    calls[0].resolve({ items: [matched(1, "Alpha", "analyst:a")], truncated: true });
    await viewButton(/Alpha/);
    // A save reloads the list; until page 0 is back the old list stays but More is off.
    await savedView("Trigger", "mock_*");
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1].offset).toBe(0);
    expect(pane().getByRole("button", { name: "More…" })).toBeDisabled();
    calls[1].resolve({ items: [matched(1, "Alpha", "analyst:a"), matched(2, "Newer", "analyst:a")], truncated: true });
    await viewButton(/Newer/);
    expect(namesIn(pane().getByRole("list", { name: "Saved views" }))).toEqual(["Alpha", "Newer"]);
    await waitFor(() => expect(pane().getByRole("button", { name: "More…" })).toBeEnabled());
  });

  it("a_more_page_that_a_reload_overtook_is_dropped", async () => {
    const user = userEvent.setup();
    const calls = pendingReads();
    await openCase();
    await waitFor(() => expect(calls).toHaveLength(1));
    calls[0].resolve({ items: [matched(1, "Alpha", "analyst:a")], truncated: true });
    await viewButton(/Alpha/);
    await user.click(pane().getByRole("button", { name: "More…" }));
    await waitFor(() => expect(calls).toHaveLength(2));
    expect(calls[1].offset).toBe(1);
    await savedView("Trigger", "mock_*"); // a reload starts after More
    await waitFor(() => expect(calls).toHaveLength(3));
    calls[1].resolve({ items: [matched(7, "Stale", "analyst:a")], truncated: false });
    calls[2].resolve({ items: [matched(1, "Alpha", "analyst:a"), matched(2, "Newest", "analyst:a")], truncated: false });
    await viewButton(/Newest/);
    expect(namesIn(pane().getByRole("list", { name: "Saved views" }))).toEqual(["Alpha", "Newest"]);
    expect(pane().queryByRole("button", { name: "More…" })).toBeNull();
  });

  it("a_confirmed_delete_moves_focus_to_a_neighbour_then_the_heading", async () => {
    const user = await openCase();
    await savedView("Aaa", "mock_*");
    await savedView("Bbb", "mock_*");
    await savedView("Ccc", "mock_*");
    await viewButton(/^Ccc/);
    const remove = async (name: RegExp) => {
      fireEvent.contextMenu(await viewButton(name));
      await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
      await user.click(await screen.findByRole("button", { name: "Delete view" }));
    };
    await remove(/^Bbb/);
    await waitFor(() => expect(pane().queryByRole("button", { name: /^Bbb/ })).toBeNull());
    await waitFor(() => expect(pane().getByRole("button", { name: /^Ccc/ })).toHaveFocus());
    // The last one in the list falls back to the one before it.
    await remove(/^Sysmon only/);
    await waitFor(() => expect(pane().queryByRole("button", { name: /^Sysmon only/ })).toBeNull());
    await waitFor(() => expect(pane().getByRole("button", { name: /^Logons/ })).toHaveFocus());
    // With the list empty it falls back to the heading.
    for (const name of [/^Aaa/, /^Ccc/, /^Logons/]) {
      await remove(name);
      await waitFor(() => expect(pane().queryByRole("button", { name })).toBeNull());
    }
    await waitFor(() => expect(pane().getByRole("heading", { name: "Saved views" })).toHaveFocus());
  });

  it("the_view_menu_returns_focus_to_its_view", async () => {
    const user = await openCase();
    const item = await viewButton(/^Logons/);
    item.focus();
    fireEvent.contextMenu(item);
    await screen.findByRole("menuitem", { name: "Export…" });
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(item).toHaveFocus();
    // Also after the delete question is answered No.
    fireEvent.contextMenu(item);
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    await user.click(await screen.findByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(item).toHaveFocus();
  });
});

describe("sessions", () => {
  it("session_opens_tabs_scoped_to_hosts", async () => {
    const session: Session = {
      source_id: 1, name: "Execution_Analysis", unmatched: 2,
      relations: [
        { relation: "mock_events", host: "SRV01" },
        { relation: "mock_events", host: "WS01" },
        { relation: "mock_hayabusa", host: "WS01" },
      ],
    };
    overrideCommand("sessions", () => [session]);
    const user = await openCase();
    expect(await pane().findByText("2 session files matched no loaded dataset.")).toBeInTheDocument();
    await user.click(pane().getByRole("button", { name: /^Execution_Analysis/ }));
    // One tab per relation, the first of them showing, each scoped to the hosts the session names for it.
    await screen.findByRole("tab", { name: "Execution_Analysis · mock_events" });
    await screen.findByRole("tab", { name: "Execution_Analysis · mock_hayabusa" });
    expect(screen.getAllByRole("tab")).toHaveLength(3);
    expect(screen.getByRole("tab", { name: "Execution_Analysis · mock_events" })).toHaveAttribute("aria-selected", "true");
    await waitFor(() => expect(callsOf("grid_page").length).toBeGreaterThan(0));
    const hostsOf = (rel: string) =>
      callsOf("grid_page").map((c) => (c.args as { state: { relation: string; hosts: string[] } }).state).filter((s) => s.relation === rel).map((s) => s.hosts);
    expect(hostsOf("mock_events").at(-1)).toEqual(["SRV01", "WS01"]);
    await user.click(screen.getByRole("tab", { name: "Execution_Analysis · mock_hayabusa" }));
    await waitFor(() => expect(hostsOf("mock_hayabusa").length).toBeGreaterThan(0));
    expect(hostsOf("mock_hayabusa").at(-1)).toEqual(["WS01"]);
  });
});

describe("export", () => {
  async function startExport(user: ReturnType<typeof userEvent.setup>, zone = false) {
    vi.mocked(dialogs.pickCsvSavePath).mockResolvedValue("/out/grid.csv");
    await openDataset(user, "mock_events");
    await user.click(screen.getByRole("button", { name: "Export…" }));
    const dialog = await screen.findByRole("dialog", { name: "Export" });
    const tick = within(dialog).getByRole("checkbox", { name: "Times in the display zone" });
    expect(tick).not.toBeChecked();
    if (zone) await user.click(tick);
    await user.click(within(dialog).getByRole("button", { name: "Choose file and export…" }));
  }

  it("export_runs_as_a_cancellable_job", async () => {
    let finish: (e: unknown) => void = () => {};
    let asked: Record<string, unknown> = {};
    overrideCommand("grid_export", (args) => {
      asked = args ?? {};
      return new Promise((_, reject) => {
        finish = reject;
      });
    });
    const user = await openCase();
    await startExport(user, true);
    await waitFor(() => expect(callsOf("grid_export")).toHaveLength(1));
    expect(asked).toMatchObject({ path: "/out/grid.csv", zoneTimes: "UTC" });
    expect((asked.state as { relation: string }).relation).toBe("mock_events");
    const cancel = await screen.findByRole("button", { name: "Cancel export" });
    expect(screen.getByTestId("progress")).toBeInTheDocument();
    await user.click(cancel);
    await waitFor(() => expect(callsOf("query_cancel")).toHaveLength(1));
    expect(callsOf("query_cancel")[0].args).toEqual({ queryIds: [asked.queryId] });
    finish({ code: "interrupted", message: "the query was cancelled" } satisfies CmdError);
    expect(await screen.findByText("Export cancelled.")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("button", { name: "Cancel export" })).toBeNull());
  });

  it("only_one_export_runs_at_a_time_across_tabs", async () => {
    let finish: (v: unknown) => void = () => {};
    overrideCommand("grid_export", () => new Promise((resolve) => {
      finish = resolve;
    }));
    const user = await openCase();
    await startExport(user, true);
    await waitFor(() => expect(callsOf("grid_export")).toHaveLength(1));
    // The Export button of this tab and of another is off, with a note, while it runs; Cancel stays.
    expect(screen.getByRole("button", { name: "Export…" })).toBeDisabled();
    expect(screen.getByText("An export is running.")).toBeInTheDocument();
    await openDataset(user, "mock_hayabusa");
    expect(screen.getByRole("button", { name: "Export…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Cancel export" })).toBeInTheDocument();
    // While it runs in the display zone the status bar says so, not that exports stay UTC.
    expect(screen.getByText(/this export's times are in UTC/)).toBeInTheDocument();
    expect(screen.queryByText(/export stay UTC/)).toBeNull();
    finish({ rows: 3, path: "/out/grid.csv" });
    await waitFor(() => expect(screen.queryByRole("button", { name: "Cancel export" })).toBeNull());
    expect(screen.getByRole("button", { name: "Export…" })).toBeEnabled();
    expect(screen.getByText(/storage and export stay UTC/)).toBeInTheDocument();
    expect(callsOf("grid_export")).toHaveLength(1);
  });

  it("the_export_dialog_shows_the_zone_as_safe_text", () => {
    render(<ExportDialog anchor={{ current: document.body }} zone={"Zone‮<b>x</b>"} onStart={() => {}} onClose={() => {}} />);
    const dialog = screen.getByRole("dialog", { name: "Export" });
    expect(dialog.textContent).not.toContain("‮");
    expect(dialog.querySelector("b")).toBeNull();
    expect(dialog.textContent).toContain(displaySafe("Zone‮<b>x</b>"));
  });

  it("an_open_export_dialog_closes_when_its_tab_is_hidden", async () => {
    const user = await openCase();
    await openDataset(user, "mock_events");
    await user.click(screen.getByRole("button", { name: "Export…" }));
    await screen.findByRole("dialog", { name: "Export" });
    // Switched by a key or a click that does not press the mouse: the popover's own outside-press close does not run.
    fireEvent.click(screen.getByRole("tab", { name: "Overview" }));
    fireEvent.click(screen.getByRole("tab", { name: "mock_events" }));
    expect(screen.queryByRole("dialog", { name: "Export" })).toBeNull();
  });

  it("a_second_export_is_refused_even_from_a_dialog_left_open", async () => {
    vi.mocked(dialogs.pickCsvSavePath).mockResolvedValue("/out/two.csv");
    await mockCall("case_open", { path: "/cases/x.duckdb", name: "x" });
    const summary = await mockCall<CaseSummary>("case_summary", { queryId: "q", slot: "s" });
    const settings = (await mockCall<{ settings: Settings }>("settings_get")).settings;
    const base: ShellContext = {
      summary, settings, change: { generation: 0, kinds: [] }, openDataset: () => {}, openScoped: () => {}, setGridStatus: () => {},
      setRightPane: () => {}, rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError: vi.fn(), saveLayout: vi.fn(),
      setHistogram: vi.fn(), saveColourRules: vi.fn(),
    };
    const tab: Tab = { id: "dataset:mock_events", kind: "dataset", relation: "mock_events" };
    const ui = (ctx: ShellContext) => (
      <ShellCtx.Provider value={ctx}>
        <Grid tab={tab} active scrollOffset={0} onScroll={() => {}} />
      </ShellCtx.Provider>
    );
    const user = userEvent.setup();
    const { rerender } = render(ui(base));
    await waitFor(() => expect(screen.getAllByRole("columnheader").length).toBeGreaterThan(1));
    await user.click(screen.getByRole("button", { name: "Export…" }));
    const dialog = await screen.findByRole("dialog", { name: "Export" });
    // Another tab starts an export in the meantime: the shell now owns a running job.
    const other = { queryId: "other-job", zoneTimes: false };
    const begin = vi.fn(() => false);
    const end = vi.fn();
    rerender(ui({ ...base, exportJob: other, exportRunning: () => true, beginExport: begin, endExport: end }));
    await user.click(within(dialog).getByRole("button", { name: "Choose file and export…" }));
    expect((await screen.findAllByText("An export is running.")).length).toBeGreaterThan(0);
    expect(callsOf("grid_export")).toHaveLength(0);
    expect(end).not.toHaveBeenCalledWith("other-job"); // the other job's entry is untouched
  });

  it("export_writes_rows_and_reports_them", async () => {
    const user = await openCase();
    await startExport(user);
    expect((callsOf("grid_export")[0].args as { zoneTimes: string | null }).zoneTimes).toBeNull();
    expect(await screen.findByText(/Exported \d[\d,]* rows to \/out\/grid\.csv/)).toBeInTheDocument();
  });

  it("export_refusal_is_shown", async () => {
    overrideCommand("grid_export", () => {
      throw { code: "refused", message: `the file would be inside loaded run output: ${HOSTILE}` } satisfies CmdError;
    });
    const user = await openCase();
    await startExport(user);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(displaySafe(`the file would be inside loaded run output: ${HOSTILE}`));
    expect(alert.querySelector("img")).toBeNull();
  });
});
