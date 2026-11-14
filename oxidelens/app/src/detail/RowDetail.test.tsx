import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { caseOpen, caseSummary, settingsGet } from "../api";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { Chip } from "../contract/generated/Chip";
import type { RowView } from "../contract/generated/RowView";
import type { WindowTotal } from "../contract/generated/WindowTotal";
import { MergedTab } from "../grid/MergedTab";
import { HOSTILE } from "../mock/generated";
import { mockCall } from "../mock/backend";
import { ShellCtx, type ShellContext } from "../shell/context";
import type { Tab } from "../shell/tabs";
import { callsOf, overrideCommand } from "../test/backend";
import Grid from "../grid/Grid";
import * as copy from "../grid/copy";
import RowDetail from "./RowDetail";

vi.mock("../grid/copy", async (orig) => ({ ...(await orig<typeof import("../grid/copy")>()), writeClipboard: vi.fn(async () => {}) }));

const NY = "2026-01-04 21:13:58.2040000 -05:00";
/** What the mock holds for row 1: the stored (UTC) text of a column. */
async function stored(column: string): Promise<string | null> {
  const v = await mockCall<RowView>("row_view", { queryId: "t", slot: "t", rowId: 1, zone: "UTC" });
  return v.fields.find((f) => f.meta.name === column)!.text;
}

type Args = Record<string, unknown> | undefined;

/** The mock's answer for `rowId` with `edit` applied, as a `row_view` override. */
function viewOverride(edit: (v: RowView) => void) {
  overrideCommand("row_view", async (args: Args) => {
    const v = await mockCall<RowView>("row_view", args);
    edit(v);
    return v;
  });
}
const setField = (v: RowView, column: string, text: string | null) => {
  const f = v.fields.find((x) => x.meta.name === column)!;
  f.text = text;
  v.detail.fields.find(([n]) => n === column)![1].text = text;
};

interface Mounted {
  ctx: ShellContext;
  openScoped: ReturnType<typeof vi.fn>;
  openRow: ReturnType<typeof vi.fn>;
  onFilter: ReturnType<typeof vi.fn>;
  onShowRow: ReturnType<typeof vi.fn>;
  container: HTMLElement;
  rerender(summary: CaseSummary, rowId?: number, generation?: number): void;
  user: ReturnType<typeof userEvent.setup>;
}

async function mount(opts: { rowId?: number; zone?: string } = {}): Promise<Mounted> {
  await caseOpen("/cases/x.duckdb");
  const base = await caseSummary("t");
  const summary: CaseSummary = { ...base, display_zone: opts.zone ?? base.display_zone };
  const settings = (await settingsGet()).settings;
  const openScoped = vi.fn();
  const openRow = vi.fn();
  const onFilter = vi.fn();
  const onShowRow = vi.fn();
  const ctxOf = (s: CaseSummary, generation = 0): ShellContext => ({
    summary: s, settings, change: { generation, kinds: [] }, openDataset: () => {}, openScoped, openRow, setGridStatus: () => {},
    setRightPane: () => {}, rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError: vi.fn(), saveLayout: vi.fn(), setHistogram: vi.fn(),
    saveColourRules: vi.fn(),
  });
  const tree = (s: CaseSummary, rowId: number, generation = 0) => (
    <ShellCtx.Provider value={ctxOf(s, generation)}>
      <RowDetail rowId={rowId} onFilter={onFilter} onShowRow={onShowRow} />
    </ShellCtx.Provider>
  );
  const utils = render(tree(summary, opts.rowId ?? 1));
  return {
    container: utils.container, ctx: ctxOf(summary), openScoped, openRow, onFilter, onShowRow, user: userEvent.setup(),
    rerender: (s, rowId = opts.rowId ?? 1, generation = 0) => utils.rerender(tree(s, rowId, generation)),
  };
}

const field = (name: string) => screen.getByTestId(`field-${name}`);
const settle = (ms = 100) => act(() => new Promise((r) => setTimeout(r, ms)));

beforeEach(() => vi.mocked(copy.writeClipboard).mockClear());

describe("row detail", () => {
  it("detail_shows_full_precision_in_zone", async () => {
    viewOverride((v) => {
      v.zone = "America/New_York";
      v.fields.find((f) => f.meta.name === "TimeCreated")!.display = NY;
    });
    const m = await mount({ zone: "America/New_York" });
    await waitFor(() => expect(field("TimeCreated")).toHaveTextContent(NY));
    // The stored UTC text is not what is drawn as the value, and no digit of the fraction is lost.
    expect(field("TimeCreated")).not.toHaveTextContent((await stored("TimeCreated"))!);
    expect(callsOf("row_view").at(-1)!.args).toMatchObject({ rowId: 1, zone: "America/New_York", slot: "detail" });
    // The zone changes: the row is read again in the new zone, and no old answer is drawn meanwhile.
    viewOverride((v) => {
      v.zone = "UTC";
      v.fields.find((f) => f.meta.name === "TimeCreated")!.display = "2026-01-05 02:14:03.0036349 Z";
    });
    m.rerender({ ...(await caseSummary("t")), display_zone: "UTC" });
    await waitFor(() => expect(field("TimeCreated")).toHaveTextContent("2026-01-05 02:14:03.0036349 Z"));
    expect(callsOf("row_view").at(-1)!.args).toMatchObject({ zone: "UTC" });
  });

  it("hide_empty_hides_empty_fields", async () => {
    viewOverride((v) => {
      setField(v, "_triage_identity", null);
      setField(v, "UserName", "");
    });
    const { user } = await mount();
    await waitFor(() => expect(field("EventId")).toBeInTheDocument());
    expect(field("_triage_identity")).toBeInTheDocument();
    expect(field("UserName")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Hide empty" }));
    expect(screen.queryByTestId("field-_triage_identity")).toBeNull();
    expect(screen.queryByTestId("field-UserName")).toBeNull();
    expect(field("EventId")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Show all" }));
    expect(field("_triage_identity")).toBeInTheDocument();
  });

  it("pivot_counts_and_open_merged_tab", async () => {
    overrideCommand("around_totals", (a: Args) => (a!.windows as number[]).map((seconds, i): WindowTotal => ({ seconds, rows: [7, 31, 412][i] })));
    const m = await mount();
    await waitFor(() => expect(screen.getByRole("button", { name: "±1 min · 7" })).toBeInTheDocument());
    expect(callsOf("around_totals").at(-1)!.args).toMatchObject({ rowId: 1, windows: [60, 300, 3600], slot: "detail:around" });
    expect(screen.getByRole("button", { name: "±5 min · 31" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "±1 h · 412" })).toBeInTheDocument();
    await m.user.click(screen.getByRole("button", { name: "±5 min · 31" }));
    expect(m.openScoped).toHaveBeenCalledTimes(1);
    const tab = m.openScoped.mock.calls[0][0] as Tab;
    expect(tab.kind).toBe("merged");
    const anchor = (await mockCall<RowView>("row_view", { queryId: "t", slot: "t", rowId: 1, zone: "UTC" })).anchor!;
    expect(tab.merged!.source).toEqual({ around: { host: anchor.host, at: anchor.at, window_seconds: 300 } });

    // The merged tab lists the rows around, with the written value of an untimed-in-zone row labelled ?tz.
    const open = vi.fn();
    const tabCtx: ShellContext = { ...m.ctx, openRow: open };
    render(
      <ShellCtx.Provider value={tabCtx}>
        <MergedTab tab={tab} active />
      </ShellCtx.Provider>,
    );
    await waitFor(() => expect(screen.getAllByRole("row").length).toBeGreaterThan(2));
    expect(screen.getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["", "Time", "Host", "Dataset", "Summary", ""]);
    expect(callsOf("merged_page").at(-1)!.args).toMatchObject({ source: tab.merged!.source, zone: "UTC" });
    const first = screen.getAllByRole("row")[1];
    expect(within(first).getByText(/^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d/)).toBeInTheDocument();
    await userEvent.click(within(first).getByRole("button", { name: "open" }));
    expect(open).toHaveBeenCalledWith(expect.stringMatching(/^mock_/), expect.any(Number));
  });

  it("merged_tab_labels_a_time_it_could_not_convert", async () => {
    overrideCommand("merged_page", () => ({
      items: [
        {
          row: { row_id: 9, relation: "mock_events", time: "2026-01-05 02:13:58", time_semantics: "unknown", host: "WS01", summary: "x", mark: null },
          display_time: null,
        },
      ],
      truncated: false,
    }));
    const m = await mount();
    const tab: Tab = { id: "m", kind: "merged", relation: null, title: "t", merged: { source: { around: { host: "WS01", at: "2026-01-05T02:13:58.2040000Z", window_seconds: 60 } } } };
    render(
      <ShellCtx.Provider value={m.ctx}>
        <MergedTab tab={tab} active />
      </ShellCtx.Provider>,
    );
    await waitFor(() => expect(screen.getAllByRole("row").length).toBeGreaterThan(1));
    const row = screen.getAllByRole("row")[1];
    expect(row).toHaveTextContent("2026-01-05 02:13:58");
    expect(within(row).getByText("?tz")).toBeInTheDocument();
  });

  it("untimed_row_has_no_pivot", async () => {
    overrideCommand("around_totals", () => Promise.reject({ code: "invalid", message: "row 1 has no usable time" }));
    viewOverride((v) => {
      v.anchor = null;
    });
    await mount();
    await waitFor(() => expect(screen.getByText("no time on this row")).toBeInTheDocument());
    expect(callsOf("around_totals").length).toBeGreaterThan(0); // the engine's `invalid` is what says it
    expect(screen.queryByRole("button", { name: /±/ })).toBeNull();
  });

  it("the_engines_anchor_opens_the_merged_tab_unchanged", async () => {
    // The stored text of the time is in another shape than the anchor's; the pivot uses the anchor as the engine gave it.
    viewOverride((v) => {
      setField(v, "TimeCreated", "2026-01-05 02:13:58");
      v.anchor = { host: "SRV01", at: "2026-01-05T02:13:58.204000Z" };
    });
    const m = await mount();
    const button = await screen.findByRole("button", { name: /^±1 h · \d+$/ });
    await waitFor(() => expect(button).toBeEnabled());
    await m.user.click(button);
    const tab = m.openScoped.mock.calls[0][0] as Tab;
    expect(tab.merged!.source).toEqual({ around: { host: "SRV01", at: "2026-01-05T02:13:58.204000Z", window_seconds: 3600 } });
  });

  it("a_row_without_an_anchor_cannot_open_a_pivot", async () => {
    viewOverride((v) => {
      v.anchor = null;
    });
    await mount();
    const button = await screen.findByRole("button", { name: /^±1 min · \d+$/ });
    await waitFor(() => expect(button).toBeDisabled());
  });

  it("the_title_is_the_summary_line_or_the_relation_and_row", async () => {
    viewOverride((v) => {
      v.summary = "Security · 4625 · An account failed to log on";
    });
    const m = await mount();
    expect(await screen.findByTestId("detail-title")).toHaveTextContent("Security · 4625 · An account failed to log on");
    expect(screen.getByTestId("detail-row")).toHaveTextContent("row 1");
    viewOverride((v) => {
      v.summary = null;
    });
    m.rerender(await caseSummary("t"), 2);
    await waitFor(() => expect(screen.getByTestId("detail-row")).toHaveTextContent("row 2"));
    expect(screen.getByTestId("detail-title")).toHaveTextContent("mock_events");
    // A hostile summary is text.
    viewOverride((v) => {
      v.summary = HOSTILE.html + HOSTILE.bidi;
    });
    m.rerender(await caseSummary("t"), 3);
    await waitFor(() => expect(screen.getByTestId("detail-row")).toHaveTextContent("row 3"));
    expect(screen.getByTestId("detail-title")).toHaveTextContent(HOSTILE.html + "report\\u{202E}gpj.exe");
    expect(m.container.querySelector("img")).toBeNull();
  });

  it("provenance_shows_locator_and_manifest_check", async () => {
    viewOverride((v) => {
      v.provenance = {
        host: "WS01", source_id: 3, run_id: "20260104000000-aaaa", evidence_path: "C:\\Windows\\System32\\winevt\\Logs\\Security.evtx",
        locator: ["EventRecordId", "88213"], output_file: "Processed-WS01/EventLogs/evtx.csv", output_file_listed: true,
      };
    });
    await mount();
    const box = await screen.findByRole("region", { name: "Provenance" });
    expect(box).toHaveTextContent("WS01");
    expect(box).toHaveTextContent("20260104000000-aaaa");
    expect(box).toHaveTextContent("Security.evtx");
    expect(box).toHaveTextContent("record 88213");
    expect(box).toHaveTextContent("Processed-WS01/EventLogs/evtx.csv");
    expect(box).toHaveTextContent("✓ listed in the run manifest");
    expect(box.textContent).not.toMatch(/hash/i);
  });

  it("provenance_says_not_listed_and_says_nothing_when_unknown", async () => {
    viewOverride((v) => {
      v.provenance.output_file_listed = false;
    });
    const { rerender } = await mount();
    expect(await screen.findByText("not listed")).toBeInTheDocument();
    viewOverride((v) => {
      v.provenance.output_file_listed = null;
    });
    rerender(await caseSummary("t"), 2);
    await waitFor(() => expect(callsOf("row_view").length).toBeGreaterThan(1));
    await waitFor(() => expect(screen.queryByText("not listed")).toBeNull());
    expect(screen.queryByText(/listed in the run manifest/)).toBeNull();
  });

  it("copy_as_json_uses_stored_text", async () => {
    viewOverride((v) => {
      v.fields.find((f) => f.meta.name === "TimeCreated")!.display = NY;
      setField(v, "UserName", null);
    });
    const { user } = await mount();
    await waitFor(() => expect(field("TimeCreated")).toBeInTheDocument());
    await user.click(screen.getByRole("button", { name: "Copy as JSON" }));
    await waitFor(() => expect(copy.writeClipboard).toHaveBeenCalledTimes(1));
    const json = vi.mocked(copy.writeClipboard).mock.calls[0][0];
    const obj = JSON.parse(json) as Record<string, string | null>;
    const time = (await stored("TimeCreated"))!;
    expect(obj.TimeCreated).toBe(time);
    expect(json).not.toContain("-05:00");
    expect(obj.UserName).toBeNull();
    expect(obj.EventId).toBe(await stored("EventId"));
    // A single value copies as its stored text as well.
    await user.click(screen.getByRole("button", { name: "Copy TimeCreated" }));
    expect(copy.writeClipboard).toHaveBeenLastCalledWith(time);
  });

  it("filter_in_and_out_make_cell_chips", async () => {
    viewOverride((v) => {
      setField(v, "UserName", null);
      setField(v, "Computer", "");
    });
    const { user, onFilter } = await mount();
    await waitFor(() => expect(field("EventId")).toBeInTheDocument());
    const id = (await stored("EventId"))!;
    await user.click(screen.getByRole("button", { name: "Filter in EventId" }));
    await user.click(screen.getByRole("button", { name: "Filter out EventId" }));
    await user.click(screen.getByRole("button", { name: "Filter in UserName" }));
    await user.click(screen.getByRole("button", { name: "Filter out Computer" }));
    const leaves = (onFilter.mock.calls as [Chip][]).map(([c]) => c.filter);
    expect(leaves).toEqual([
      { column: "EventId", op: "eq", value: id, case_sensitive: true },
      { column: "EventId", op: "ne", value: id, case_sensitive: true },
      { column: "UserName", op: "is_empty", value: null, case_sensitive: false },
      { column: "Computer", op: "ne", value: "", case_sensitive: true },
    ]);
  });

  it("superseded_row_links_its_current_row", async () => {
    viewOverride((v) => {
      v.detail.standing = "superseded";
      v.detail.current_row = 77;
      v.detail.active = false;
    });
    const { user, onShowRow } = await mount();
    expect(await screen.findByText(/superseded/i)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Show the current row (77)" }));
    expect(onShowRow).toHaveBeenCalledWith(77);
  });

  it("removed_row_says_so", async () => {
    viewOverride((v) => {
      v.detail.standing = "removed";
      v.detail.fields = [];
      v.fields = [];
    });
    await mount();
    expect(await screen.findByText(/removed/i)).toBeInTheDocument();
  });

  it("tags_and_notes_are_read_only", async () => {
    viewOverride((v) => {
      v.detail.tags = ["lateral-movement"];
      v.detail.notes = [{ id: 1, row_id: 1, text: "seen on SRV01\nsecond line", written_by: "analyst:analyst01", written_at: "2026-01-06T10:00:00Z" }];
      v.detail.mark = "suspicious";
    });
    await mount();
    expect(await screen.findByText("lateral-movement")).toBeInTheDocument();
    expect(screen.getByText(/seen on SRV01/)).toBeInTheDocument();
    expect(screen.getByText(/Suspicious/)).toBeInTheDocument();
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("hostile_values_in_detail_render_inert", async () => {
    viewOverride((v) => {
      setField(v, "UserName", HOSTILE.html);
      setField(v, "ProcessName", HOSTILE.bidi);
      setField(v, "Message", HOSTILE.long);
      setField(v, "CommandLine", HOSTILE.formula);
      setField(v, "Channel", HOSTILE.bell + " " + HOSTILE.joiner);
      const f = v.fields.find((x) => x.meta.name === "Computer")!;
      f.meta = { ...f.meta, name: HOSTILE.html }; // a copy: the mock shares its column objects
      v.detail.fields.find(([n]) => n === "Computer")![0] = HOSTILE.html;
      v.detail.tags = [HOSTILE.html];
      v.detail.notes = [{ id: 1, row_id: 1, text: HOSTILE.bidi, written_by: HOSTILE.html, written_at: "2026-01-06T10:00:00Z" }];
      v.provenance.evidence_path = HOSTILE.bidi;
      v.provenance.host = HOSTILE.html;
    });
    const { container, user } = await mount();
    await waitFor(() => expect(field("UserName")).toBeInTheDocument());
    expect(container.querySelector("img")).toBeNull();
    expect(field("UserName")).toHaveTextContent(HOSTILE.html);
    expect(field("ProcessName")).toHaveTextContent("report\\u{202E}gpj.exe");
    expect(field("Channel")).toHaveTextContent("ring\\u{0007}ring a\\u{200D}b"); // bell and joiner escaped
    expect(field("CommandLine")).toHaveTextContent(HOSTILE.formula);
    expect(field("Message").textContent).toContain(HOSTILE.long.slice(0, 200)); // shown whole, not clipped
    expect(field("Message").textContent).toContain(HOSTILE.long.slice(-200));
    expect(container.innerHTML).not.toContain("\u202E");
    // Copy as JSON carries the bytes as stored.
    await user.click(screen.getByRole("button", { name: "Copy as JSON" }));
    await waitFor(() => expect(copy.writeClipboard).toHaveBeenCalled());
    const obj = JSON.parse(vi.mocked(copy.writeClipboard).mock.calls[0][0]) as Record<string, string>;
    expect(obj.ProcessName).toBe(HOSTILE.bidi);
    expect(obj.Message).toBe(HOSTILE.long);
    expect(obj.CommandLine).toBe(HOSTILE.formula);
    expect(obj[HOSTILE.html]).toBe(await stored("Computer"));
  });

  it("a_slow_old_row_is_not_drawn_after_another_is_selected", async () => {
    overrideCommand("row_view", async (args: Args) => {
      const v = await mockCall<RowView>("row_view", args);
      if (args!.rowId === 1) await new Promise((r) => setTimeout(r, 150));
      return v;
    });
    const m = await mount({ rowId: 1 });
    m.rerender(await caseSummary("t"), 2);
    await settle(300);
    expect(screen.getByTestId("detail-row")).toHaveTextContent("2");
    expect(screen.getByTestId("detail-row")).not.toHaveTextContent(/\b1\b/);
    // The first read was told to stop.
    expect(callsOf("query_cancel").length).toBeGreaterThan(0);
  });

  it("detail_keeps_its_rows_while_it_re_reads", async () => {
    const m = await mount({ rowId: 1 });
    await screen.findByTestId("detail-row");
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const oldText = field("Computer").textContent;
    overrideCommand("row_view", async (args: Args) => {
      await gate;
      const v = await mockCall<RowView>("row_view", args);
      setField(v, "Computer", "WS99-RE-READ");
      return v;
    });
    m.rerender(await caseSummary("t"), 1, 1); // case-changed: a new generation, the same row
    await settle(50);
    expect(screen.getByTestId("detail-row")).toHaveTextContent("1"); // the old detail stays up
    expect(field("Computer").textContent).toBe(oldText);
    expect(oldText).not.toContain("WS99-RE-READ");
    expect(screen.queryByText(/Loading row/)).toBeNull();
    release();
    await waitFor(() => expect(field("Computer").textContent).toContain("WS99-RE-READ")); // the new answer replaces it
    expect(callsOf("row_view")).toHaveLength(2);
  });

  it("a_different_row_does_not_show_the_old_one_while_it_loads", async () => {
    const m = await mount({ rowId: 1 });
    await screen.findByTestId("detail-row");
    overrideCommand("row_view", () => new Promise(() => {}));
    m.rerender(await caseSummary("t"), 2);
    await settle(50);
    expect(screen.queryByTestId("detail-row")).toBeNull();
    expect(screen.getByText(/Loading row 2/)).toBeInTheDocument();
  });
});

// ---- the grid hands the selected row to the right pane --------------------------------------------------------------
describe("the grid and the right pane", () => {
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
      },
    });
  });

  it("a_single_selection_and_enter_load_the_row", async () => {
    await caseOpen("/cases/x.duckdb");
    const summary = await caseSummary("t");
    const settings = (await settingsGet()).settings;
    let pane: ReactNode = null;
    const ctx: ShellContext = {
      summary, settings, change: { generation: 0, kinds: [] }, openDataset: () => {}, openScoped: () => {}, setGridStatus: () => {},
      setRightPane: (n) => {
        pane = n;
      },
      rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError: vi.fn(), saveLayout: vi.fn(), setHistogram: vi.fn(), saveColourRules: vi.fn(),
    };
    const tab: Tab = { id: "dataset:mock_events", kind: "dataset", relation: "mock_events" };
    render(
      <ShellCtx.Provider value={ctx}>
        <Grid tab={tab} active scrollOffset={0} onScroll={() => {}} />
      </ShellCtx.Provider>,
    );
    await waitFor(() => expect(screen.queryAllByRole("row").filter((r) => r.getAttribute("data-row-index") !== null).length).toBeGreaterThan(0));
    expect(callsOf("row_view")).toHaveLength(0);
    const row = screen.queryAllByRole("row").find((r) => r.getAttribute("data-row-index") === "2")!;
    fireEvent.click(within(row).getAllByRole("gridcell")[1]);
    await waitFor(() => expect(pane).not.toBeNull());
    const shown = render(<ShellCtx.Provider value={ctx}>{pane}</ShellCtx.Provider>);
    await waitFor(() => expect(callsOf("row_view").at(-1)!.args).toMatchObject({ rowId: 3 }));
    shown.unmount();
    // Enter on the focused row shows it too (here the row is already the selection).
    fireEvent.keyDown(screen.getByRole("grid", { name: "Rows" }), { key: "ArrowDown" });
    fireEvent.keyDown(screen.getByRole("grid", { name: "Rows" }), { key: "Enter" });
    await waitFor(() => expect(callsOf("row_view").length).toBeGreaterThan(0));
    const shown2 = render(<ShellCtx.Provider value={ctx}>{pane}</ShellCtx.Provider>);
    await waitFor(() => expect(callsOf("row_view").at(-1)!.args).toMatchObject({ rowId: 4 }));
    shown2.unmount();
  });
});
