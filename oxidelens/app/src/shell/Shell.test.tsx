import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import * as dialogs from "../dialogs";
import { mockCall } from "../mock/backend";
import { callsOf, emitEvent, overrideCommand } from "../test/backend";
import { caseSummary as caseSummaryFixture } from "../test/fixtures";
import { displaySafe } from "../text";
import Shell from "./Shell";
import { storageKey } from "./tabs";

vi.mock("../dialogs", () => ({
  pickNewCasePath: vi.fn(),
  pickCaseFile: vi.fn(),
  pickRunFolder: vi.fn(),
  pickCsvFile: vi.fn(),
}));

const HOSTILE = '<img src=x onerror=alert(1)>\u202E"; DROP';

// jsdom does not keep a scroll offset; this one does, per element, so a test can scroll and read back.
const scrollTops = new WeakMap<Element, number>();
beforeEach(() => {
  localStorage.clear();
  Object.defineProperty(HTMLElement.prototype, "scrollTop", {
    configurable: true,
    get() {
      return scrollTops.get(this) ?? 0;
    },
    set(v: number) {
      scrollTops.set(this, v);
    },
  });
});
afterEach(() => {
  vi.restoreAllMocks();
});

async function openRecent() {
  const user = userEvent.setup();
  render(<Shell />);
  await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
  await screen.findByRole("tab", { name: "Overview" });
  return user;
}

describe("welcome", () => {
  it("welcome_lists_recent_and_opens", async () => {
    const user = userEvent.setup();
    render(<Shell />);
    const open = await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ });
    expect(screen.getByRole("button", { name: "New case…" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open case…" })).toBeInTheDocument();

    // Removing a recent case asks the backend, which owns the list; no whole-settings write carries it.
    await user.click(screen.getByRole("button", { name: /Remove .*acme\.case\.duckdb/ }));
    await waitFor(() => expect(callsOf("recent_remove")).toHaveLength(1));
    expect(callsOf("recent_remove")[0].args).toEqual({ path: "/cases/acme.case.duckdb" });
    expect(callsOf("settings_update")).toHaveLength(0);
    await waitFor(() => expect(screen.queryByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ })).toBeNull());
    expect(open).not.toBeInTheDocument();
  });

  it("welcome_opens_a_case", async () => {
    vi.mocked(dialogs.pickCaseFile).mockResolvedValue("/cases/picked.duckdb");
    const user = userEvent.setup();
    render(<Shell />);
    await user.click(await screen.findByRole("button", { name: "Open case…" }));
    await screen.findByRole("tab", { name: "Overview" });
    expect(callsOf("case_open")[0].args).toEqual({ path: "/cases/picked.duckdb" });
    expect(screen.queryByRole("button", { name: "Open case…" })).toBeNull(); // the welcome screen is gone
  });

  it("a_settings_write_in_flight_when_a_case_opens_keeps_the_new_recent_case", async () => {
    vi.mocked(dialogs.pickCaseFile).mockResolvedValue("/cases/second.duckdb");
    await openRecent();
    // A pane toggle's write is held until the next case has opened and put itself first in `recent`.
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    overrideCommand("settings_update", async (a) => {
      await gate;
      return mockCall("settings_update", a as Record<string, unknown>);
    });
    fireEvent.keyDown(document.body, { key: "[", ctrlKey: true });
    await waitFor(() => expect(callsOf("settings_update")).toHaveLength(1));
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Close case" }));
    await user.click(await screen.findByRole("button", { name: "Open case…" }));
    await screen.findByRole("tab", { name: "Overview" });
    release();
    await waitFor(async () => {
      const reply = await mockCall<{ settings: { left_pane: string } }>("settings_get");
      expect(reply.settings.left_pane).toBe("rail");
    });
    // The held write carried the older list; the backend kept its own, and the window shows it.
    const reply = await mockCall<{ settings: { recent: string[] } }>("settings_get");
    expect(reply.settings.recent).toEqual(["/cases/second.duckdb", "/cases/acme.case.duckdb"]);
    await user.click(screen.getByRole("button", { name: "Close case" }));
    const list = await screen.findByRole("list");
    expect(within(list).getAllByRole("button", { name: /^\/cases\// }).map((b) => b.textContent)).toEqual([
      "/cases/second.duckdb", "/cases/acme.case.duckdb",
    ]);
  });

  it("opens a recent case, and a new case through the save dialog with a name asked for", async () => {
    const user = await openRecent();
    expect(callsOf("case_open")[0].args).toEqual({ path: "/cases/acme.case.duckdb" });
    await user.click(screen.getByRole("button", { name: "Close case" }));
    await screen.findByRole("button", { name: "New case…" });

    vi.mocked(dialogs.pickNewCasePath).mockResolvedValue("C:\\cases\\triage-01.duckdb");
    await user.click(screen.getByRole("button", { name: "New case…" }));
    const name = await screen.findByRole("textbox", { name: "Case name" });
    expect(name).toHaveValue("triage-01");
    await user.clear(name);
    await user.type(name, "Intrusion 7");
    await user.click(screen.getByRole("button", { name: "Create" }));
    await screen.findByRole("tab", { name: "Overview" });
    expect(callsOf("case_new")[0].args).toEqual({ path: "C:\\cases\\triage-01.duckdb", name: "Intrusion 7" });
  });

  it("shows the damaged-settings note once, dismissibly", async () => {
    overrideCommand("settings_get", () => ({
      settings: {
        version: 1, theme: "dark", font_size: 12, left_pane: "open", right_pane: "open", histogram: true,
        analyst: null, recent: [], layouts: {}, colour_rules: {},
      },
      damaged: { file: "settings.json.bad", reason: "expected value at line 1" },
    }));
    const user = userEvent.setup();
    render(<Shell />);
    const note = await screen.findByRole("status", { name: "Settings notice" });
    expect(note).toHaveTextContent("expected value at line 1");
    await user.click(within(note).getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByRole("status", { name: "Settings notice" })).toBeNull();
  });
});

describe("the open case", () => {
  it("opening_a_case_shows_overview_and_datasets", async () => {
    await openRecent();
    expect(screen.getByRole("tab", { name: "Overview", selected: true })).toBeInTheDocument();
    expect(screen.getByText(/2 hosts · \d+ sources · 5\d{3} rows/)).toBeInTheDocument();
    expect(screen.getByText("analyst01")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Load run…" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open CSV…" })).toBeInTheDocument();

    const overview = screen.getByRole("tabpanel", { name: "Overview" });
    expect(within(overview).getByRole("button", { name: "mock_events" })).toBeInTheDocument();
    expect(within(overview).getByText("SRV01, WS01")).toBeInTheDocument();

    const left = screen.getByRole("complementary", { name: "Case" });
    expect(within(left).getByText("Marked rows")).toBeInTheDocument();
    expect(within(left).getByText("Suspicious")).toBeInTheDocument();
    expect(within(left).getByText("Datasets")).toBeInTheDocument();
    expect(within(left).getByText("Loose files")).toBeInTheDocument();
    expect(within(left).getByText("Saved views")).toBeInTheDocument();
    expect(within(left).getByText("Sessions")).toBeInTheDocument();
    expect(within(left).getByRole("button", { name: /mock_hayabusa/ })).toBeInTheDocument();
    // On the Overview only the pane keys are live (the grids' keys: Marks.test.tsx).
    expect(screen.getByLabelText("Keys").textContent).toBe("Ctrl+[ Ctrl+] panes");
    expect(screen.getByRole("complementary", { name: "Row detail" })).toBeInTheDocument();
  });

  it("rails_keep_mark_counts", async () => {
    await openRecent();
    fireEvent.keyDown(document.body, { key: "[", ctrlKey: true });
    const rail = await screen.findByTestId("left-rail");
    expect(rail).toHaveStyle({ width: "26px" });
    expect(within(rail).getByText("3")).toBeInTheDocument(); // the Suspicious count
    expect(within(rail).getByRole("button", { name: "Expand left pane" })).toBeInTheDocument();
    expect(screen.queryByRole("complementary", { name: "Case" })).toBeNull();
    await waitFor(() => expect(callsOf("settings_update")).toHaveLength(1));
    const saved = (callsOf("settings_update")[0].args as { settings: Record<string, unknown> }).settings;
    expect(saved.left_pane).toBe("rail");
    expect(saved.right_pane).toBe("open");
    expect(saved.recent).toEqual(["/cases/acme.case.duckdb"]);

    fireEvent.keyDown(document.body, { key: "[", ctrlKey: true });
    await screen.findByRole("complementary", { name: "Case" });
    expect(screen.queryByTestId("left-rail")).toBeNull();
    await waitFor(() => expect(callsOf("settings_update")).toHaveLength(2));
    expect((callsOf("settings_update")[1].args as { settings: { left_pane: string } }).settings.left_pane).toBe("open");

    fireEvent.keyDown(document.body, { key: "]", ctrlKey: true });
    const right = await screen.findByTestId("right-rail");
    expect(right).toHaveStyle({ width: "26px" });
    await userEvent.setup().click(within(right).getByRole("button", { name: "Expand right pane" }));
    await screen.findByRole("complementary", { name: "Row detail" });
  });

  it("keys_are_inactive_in_inputs", async () => {
    await openRecent();
    const input = document.createElement("input");
    document.body.appendChild(input);
    input.focus();
    fireEvent.keyDown(input, { key: "[", ctrlKey: true });
    fireEvent.keyDown(input, { key: "]", ctrlKey: true });
    expect(screen.queryByTestId("left-rail")).toBeNull();
    expect(screen.queryByTestId("right-rail")).toBeNull();
    expect(callsOf("settings_update")).toHaveLength(0);
    input.remove();
  });

  it("toggle_persists", async () => {
    const user = await openRecent();
    await user.click(within(screen.getByRole("tabpanel", { name: "Overview" })).getByRole("button", { name: "mock_events" }));
    const toggle = await screen.findByRole("button", { name: "Histogram" });
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    expect(await screen.findByRole("region", { name: "Histogram" })).toBeInTheDocument();
    await user.click(toggle);
    // shown at once, written through the settings queue
    expect(screen.queryByRole("region", { name: "Histogram" })).toBeNull();
    expect(screen.getByRole("button", { name: "Histogram" })).toHaveAttribute("aria-pressed", "false");
    await waitFor(() => expect(callsOf("settings_update")).toHaveLength(1));
    expect((callsOf("settings_update")[0].args as { settings: { histogram: boolean } }).settings.histogram).toBe(false);
    await user.click(screen.getByRole("button", { name: "Histogram" }));
    await waitFor(() => expect(callsOf("settings_update")).toHaveLength(2));
    expect((callsOf("settings_update")[1].args as { settings: { histogram: boolean } }).settings.histogram).toBe(true);
    expect(await screen.findByRole("region", { name: "Histogram" })).toBeInTheDocument();
  });

  it("opens dataset tabs that close and move with the keyboard", async () => {
    const user = await openRecent();
    await user.click(within(screen.getByRole("tabpanel", { name: "Overview" })).getByRole("button", { name: "mock_events" }));
    await screen.findByRole("tab", { name: /mock_events/, selected: true });
    await user.click(within(screen.getByRole("complementary", { name: "Case" })).getByRole("button", { name: /mock_hayabusa/ }));
    const hay = await screen.findByRole("tab", { name: /mock_hayabusa/, selected: true });
    hay.focus();
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: /mock_events/, selected: true })).toHaveFocus();
    await user.keyboard("{Home}");
    expect(screen.getByRole("tab", { name: "Overview", selected: true })).toHaveFocus();
    expect(screen.getByRole("tab", { name: "Overview" })).not.toHaveAccessibleName(/Close/);
    await user.click(screen.getByRole("button", { name: /Close mock_hayabusa/ }));
    expect(screen.queryByRole("tab", { name: /mock_hayabusa/ })).toBeNull();
  });

  it("tabs_persist_per_case_and_survive_throwing_storage", async () => {
    const recent = /^\/cases\/acme\.case\.duckdb$/;
    const caseList = () => within(screen.getByRole("complementary", { name: "Case" }));
    const user = await openRecent();
    await user.click(caseList().getByRole("button", { name: /mock_hayabusa/ }));
    await screen.findByRole("tab", { name: /mock_hayabusa/, selected: true });
    const key = storageKey(MOCK_CASE_ID);
    await waitFor(() => expect(localStorage.getItem(key)).toContain("mock_hayabusa"));

    // A scroll offset reported by the tab's panel is saved with the tabs.
    const panel = screen.getByRole("tabpanel", { name: /mock_hayabusa/ });
    panel.scrollTop = 420;
    fireEvent.scroll(panel);
    await waitFor(() => expect(JSON.parse(localStorage.getItem(key)!).scroll).toEqual({ "dataset:mock_hayabusa": 420 }));

    // A second case id keeps its own tabs and knows nothing of the first one's.
    await user.click(screen.getByRole("button", { name: "Close case" }));
    const base = caseSummaryFixture();
    const other: CaseSummary = {
      ...base,
      case_id: "fedcba9876543210fedcba9876543210",
      relations: [{ ...base.relations[0], name: "mock_events", rows: 5 }, { ...base.relations[0], name: "mock_hayabusa", rows: 3 }],
    };
    let useOther = true;
    overrideCommand("case_open", (a) => (useOther ? other : mockCall("case_open", a as Record<string, unknown>)));
    await user.click(await screen.findByRole("button", { name: recent }));
    await screen.findByRole("tab", { name: "Overview", selected: true });
    expect(screen.queryByRole("tab", { name: /mock_hayabusa/ })).toBeNull();
    await user.click(caseList().getByRole("button", { name: /mock_events/ }));
    await screen.findByRole("tab", { name: /mock_events/, selected: true });
    const otherKey = storageKey(other.case_id);
    await waitFor(() => expect(localStorage.getItem(otherKey)).toContain("mock_events"));
    expect(localStorage.getItem(otherKey)).not.toContain("mock_hayabusa");
    expect(localStorage.getItem(key)).toContain("mock_hayabusa");
    expect(localStorage.getItem(key)).not.toContain("mock_events");

    // The first case comes back with its tab, the tab active, and the scroll offset put back on the panel.
    useOther = false;
    await user.click(screen.getByRole("button", { name: "Close case" }));
    await user.click(await screen.findByRole("button", { name: recent }));
    await screen.findByRole("tab", { name: /mock_hayabusa/, selected: true });
    expect(screen.getByRole("tabpanel", { name: /mock_hayabusa/ }).scrollTop).toBe(420);

    // Storage that throws on every call costs only the memory of the tabs.
    await user.click(screen.getByRole("button", { name: "Close case" }));
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    await user.click(await screen.findByRole("button", { name: recent }));
    await screen.findByRole("tab", { name: "Overview", selected: true });
    expect(screen.queryByRole("tab", { name: /mock_hayabusa/ })).toBeNull();
    await user.click(caseList().getByRole("button", { name: /mock_events/ }));
    await screen.findByRole("tab", { name: /mock_events/, selected: true });
    fireEvent.scroll(screen.getByRole("tabpanel", { name: /mock_events/ }));
    expect(screen.getByRole("tab", { name: /mock_events/, selected: true })).toBeInTheDocument();
  });

  it("serialises settings writes: two quick pane changes, the first write slow, both end up saved", async () => {
    await openRecent();
    let first = true;
    overrideCommand("settings_update", async (a) => {
      if (first) {
        first = false;
        await new Promise((r) => setTimeout(r, 60));
      }
      return mockCall("settings_update", a as Record<string, unknown>);
    });
    fireEvent.keyDown(document.body, { key: "[", ctrlKey: true });
    fireEvent.keyDown(document.body, { key: "]", ctrlKey: true });
    await screen.findByTestId("left-rail");
    await screen.findByTestId("right-rail");
    await waitFor(() => expect(callsOf("settings_update")).toHaveLength(2));
    await new Promise((r) => setTimeout(r, 100));
    const reply = await mockCall<{ settings: { left_pane: string; right_pane: string } }>("settings_get");
    expect(reply.settings.left_pane).toBe("rail");
    expect(reply.settings.right_pane).toBe("rail");
    // The second write was built after the first one's reply, so it carries both changes.
    const second = (callsOf("settings_update")[1].args as { settings: { left_pane: string; right_pane: string } }).settings;
    expect([second.left_pane, second.right_pane]).toEqual(["rail", "rail"]);
    expect(screen.getByTestId("left-rail")).toBeInTheDocument();
    expect(screen.getByTestId("right-rail")).toBeInTheDocument();
  });

  it("a layout save while a pane toggle is in flight: both changes persist through the real queue", async () => {
    const user = await openRecent();
    await user.click(within(screen.getByRole("complementary", { name: "Case" })).getByRole("button", { name: /mock_events/ }));
    await screen.findByRole("tab", { name: /mock_events/, selected: true });
    const grip = await screen.findByRole("separator", { name: "Resize Channel" });
    let first = true;
    overrideCommand("settings_update", async (a) => {
      if (first) {
        first = false;
        await new Promise((r) => setTimeout(r, 500)); // the pane write is still running when the layout is ready to save
      }
      return mockCall("settings_update", a as Record<string, unknown>);
    });
    fireEvent.keyDown(document.body, { key: "]", ctrlKey: true });
    fireEvent.mouseDown(grip, { clientX: 0 });
    fireEvent.mouseMove(document, { clientX: 70 });
    fireEvent.mouseUp(document, { clientX: 70 });
    await waitFor(() => expect(callsOf("settings_update")).toHaveLength(2), { timeout: 3000 });
    await new Promise((r) => setTimeout(r, 100));
    const reply = await mockCall<{ settings: { right_pane: string; layouts: Record<string, { widths: Record<string, number> }> } }>("settings_get");
    expect(reply.settings.right_pane).toBe("rail");
    expect(reply.settings.layouts.mock_events.widths.Channel).toBe(230);
    // The second write was built from the first one's reply plus the queued layout, so it carries both.
    const second = (callsOf("settings_update")[1].args as { settings: { right_pane: string; layouts: Record<string, unknown> } }).settings;
    expect(second.right_pane).toBe("rail");
    expect(Object.keys(second.layouts)).toEqual(["mock_events"]);
  });

  it("ignores a saved tab list that is garbage or names a dataset the case lacks", async () => {
    localStorage.setItem(storageKey(caseSummaryId()), '{"v":1,"tabs":[{"relation":"gone"},7,null],"active":"dataset:gone","scroll":{"a":"x"}}');
    await openRecent();
    expect(screen.getByRole("tab", { name: "Overview", selected: true })).toBeInTheDocument();
    expect(screen.getAllByRole("tab")).toHaveLength(5); // the Overview and the four pinned sheets
  });

  it("case_changed_rereads_without_losing_tabs", async () => {
    const user = await openRecent();
    await user.click(within(screen.getByRole("complementary", { name: "Case" })).getByRole("button", { name: /mock_hayabusa/ }));
    await screen.findByRole("tab", { name: /mock_hayabusa/, selected: true });
    const before = callsOf("case_summary").length;

    const base = caseSummaryFixture();
    const next: CaseSummary = {
      ...base,
      case_id: MOCK_CASE_ID,
      mark_counts: { ...base.mark_counts, suspicious: 9 },
      relations: [
        { ...base.relations[0], name: "mock_events", rows: 123 },
        { ...base.relations[0], name: "mock_hayabusa", rows: 45 },
        { ...base.relations[0], name: "added_later", rows: 6 },
      ],
    };
    overrideCommand("case_summary", () => next);
    act(() => emitEvent("case-changed", { kinds: ["sources"], generation: 50 }));

    await waitFor(() => expect(screen.getByText(/2 hosts · \d+ sources · 174 rows/)).toBeInTheDocument());
    expect(callsOf("case_summary").length).toBeGreaterThan(before);
    expect(callsOf("sessions").length).toBeGreaterThan(1);
    expect(within(screen.getByRole("complementary", { name: "Case" })).getByRole("button", { name: /added_later/ })).toBeInTheDocument();
    expect(within(screen.getByRole("complementary", { name: "Case" })).getByText("9")).toBeInTheDocument();
    // The tab strip is untouched: both tabs, and the same one active.
    expect(screen.getByRole("tab", { name: /mock_hayabusa/, selected: true })).toBeInTheDocument();
    expect(screen.getAllByRole("tab")).toHaveLength(6);
  });
});

describe("pinned sheet tabs", () => {
  const SHEETS = ["Timeline", "Hosts", "Accounts", "IOCs"];

  it("sheet_tabs_are_pinned_and_counted", async () => {
    const user = await openRecent();
    const names = () => screen.getAllByRole("tab").map((t) => t.textContent);
    // After Overview, in order, each with the number its list holds (the mock case seeds records).
    const want = async (q: object) =>
      (await mockCall<{ items: unknown[] }>("records_list", { queryId: "c", slot: "c", query: q, limit: 1000, offset: 0 })).items.length;
    const hosts = await want({ kind: "host" });
    const accounts = await want({ kind: "account" });
    const iocs = await want({ kind: "ioc", ioc_type: null, tool: null, text: null });
    const entries = (await mockCall<{ items: unknown[] }>("entries_list", { queryId: "c", slot: "c", query: { status: [], host: null, tactic: null, text: null, row: null, order: null }, limit: 1000, offset: 0 })).items.length;
    expect(hosts).toBeGreaterThan(0);
    await waitFor(() => expect(names()).toEqual(["Overview", `Timeline ${entries}`, `Hosts ${hosts}`, `Accounts ${accounts}`, `IOCs ${iocs}`]));

    // Never closed: no close button, and Delete on the focused tab does nothing.
    for (const n of SHEETS) expect(screen.queryByRole("button", { name: new RegExp(`^Close ${n}`) })).toBeNull();
    const tl = screen.getByRole("tab", { name: /^Timeline/ });
    await user.click(tl);
    expect(tl).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{Delete}");
    expect(screen.getAllByRole("tab")).toHaveLength(5);
    // Dataset tabs follow the pinned ones.
    await user.click(screen.getByRole("tab", { name: "Overview" }));
    await user.click(within(screen.getByRole("complementary", { name: "Case" })).getByRole("button", { name: /mock_hayabusa/ }));
    await screen.findByRole("tab", { name: /mock_hayabusa/, selected: true });
    expect(names().at(-1)).toMatch(/mock_hayabusa/);
  });

  it("recounts on case-changed, and a late answer for the old read is not drawn", async () => {
    await openRecent();
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Hosts \d/ })).toBeInTheDocument());
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => (release = r));
    overrideCommand("records_list", async (a) => {
      const q = a?.query as { kind: string };
      if (q.kind === "host") await gate;
      return { items: q.kind === "host" ? [{}, {}, {}, {}, {}, {}, {}] : [], truncated: false };
    });
    act(() => emitEvent("case-changed", { kinds: ["records"], generation: 60 }));
    overrideCommand("records_list", (a) => ({ items: (a?.query as { kind: string }).kind === "host" ? [{}, {}] : [], truncated: false }));
    act(() => emitEvent("case-changed", { kinds: ["records"], generation: 61 }));
    await waitFor(() => expect(screen.getByRole("tab", { name: "Hosts 2" })).toBeInTheDocument());
    release();
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.getByRole("tab", { name: "Hosts 2" })).toBeInTheDocument();
  });

  it("a recount that fails leaves no number, and unmounting cancels what is in flight", async () => {
    const user = userEvent.setup();
    const { unmount } = render(<Shell />);
    await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
    await waitFor(() => expect(screen.getByRole("tab", { name: /^Hosts \d/ })).toBeInTheDocument());
    // Hold the next host read, so the recount is pending: the old number is not kept.
    let fail: (e: unknown) => void = () => {};
    overrideCommand("records_list", (a) => {
      if ((a?.query as { kind: string }).kind !== "host") return { items: [], truncated: false };
      return new Promise((_, rej) => (fail = rej));
    });
    act(() => emitEvent("case-changed", { kinds: ["records"], generation: 70 }));
    await waitFor(() => expect(screen.getByRole("tab", { name: "Hosts" })).toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole("tab", { name: "Accounts 0" })).toBeInTheDocument());
    act(() => fail({ code: "io", message: "boom" }));
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.getByRole("tab", { name: "Hosts" })).toBeInTheDocument(); // still no number
    // A recount still in flight when the shell goes away is cancelled.
    act(() => emitEvent("case-changed", { kinds: ["records"], generation: 71 }));
    await waitFor(() => expect(callsOf("records_list").length).toBeGreaterThan(8));
    const before = callsOf("query_cancel").length;
    unmount();
    expect(callsOf("query_cancel").length).toBeGreaterThan(before);
  });
});

describe("loading", () => {
  it("load_errors_show_code_and_message", async () => {
    const user = await openRecent();
    vi.mocked(dialogs.pickRunFolder).mockResolvedValue("D:\\runs\\Processed-1");
    overrideCommand("load_run", () => Promise.reject({ code: "refused", message: "already in this case" }));
    await user.click(screen.getByRole("button", { name: "Load run…" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("refused: already in this case");
    expect(callsOf("load_run")[0].args).toEqual({ path: "D:\\runs\\Processed-1" });
  });

  it("shows the job's progress line while a load runs and clears it after", async () => {
    const user = await openRecent();
    vi.mocked(dialogs.pickCsvFile).mockResolvedValue("D:\\x\\loose.csv");
    await user.click(screen.getByRole("button", { name: "Open CSV…" }));
    await waitFor(() => expect(callsOf("load_file")).toHaveLength(1));
    expect(callsOf("load_file")[0].args).toEqual({ path: "D:\\x\\loose.csv", options: null });
    await waitFor(() => expect(screen.queryByTestId("progress")).toBeEmptyDOMElement());
    act(() => emitEvent("job-started", { kind: "load_file", label: "loose.csv" }));
    expect(screen.getByTestId("progress")).toHaveTextContent("Loading loose.csv…");
    act(() => emitEvent("job-finished", { kind: "load_file", ok: true, message: null }));
    expect(screen.getByTestId("progress")).toBeEmptyDOMElement();
  });
});

describe("untrusted names", () => {
  it("hostile_relation_and_host_names_render_as_text", async () => {
    const base = caseSummaryFixture();
    const hostile: CaseSummary = {
      ...base,
      name: HOSTILE,
      hosts: [HOSTILE],
      relations: [{ ...base.relations[0], name: HOSTILE, hosts: [HOSTILE, "WS01"] }, { ...base.relations[0], name: "plain", kind: "file" }],
    };
    overrideCommand("case_open", () => hostile);
    overrideCommand("case_summary", () => hostile);
    const user = userEvent.setup();
    render(<Shell />);
    await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
    await screen.findByRole("tab", { name: "Overview" });
    await user.click(within(screen.getByRole("complementary", { name: "Case" })).getByRole("button", { name: new RegExp("img src=x") }));
    await screen.findByRole("tab", { name: /img src=x/, selected: true });

    expect(document.querySelector("img")).toBeNull();
    expect(document.body.innerHTML).not.toContain("<img");
    expect(screen.getAllByText(displaySafe(HOSTILE), { exact: false }).length).toBeGreaterThan(3);
    expect(document.body.textContent).toContain("\\u{202E}");
    expect(document.body.textContent).not.toContain("\u202E");
  });
});

const MOCK_CASE_ID = "0123456789abcdef0123456789abcdef";
function caseSummaryId(): string {
  return MOCK_CASE_ID;
}
