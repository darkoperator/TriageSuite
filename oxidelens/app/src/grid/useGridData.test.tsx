import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { caseOpen } from "../api";
import type { GridState } from "../contract/generated/GridState";
import { mockCall } from "../mock/backend";
import { callsOf, overrideCommand } from "../test/backend";
import { initialGridState } from "./gridState";
import { stableKey, useGridData } from "./useGridData";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

beforeEach(async () => {
  await caseOpen("/cases/x.duckdb");
});

describe("useGridData", () => {
  it("stale_answers_are_not_drawn", async () => {
    const asc = initialGridState("mock_events");
    const desc: GridState = { ...asc, sort: [{ column: "RecordId", descending: true }] };
    let n = 0;
    overrideCommand("grid_page", async (args) => {
      n += 1;
      if (n === 1) await sleep(300); // the first answer arrives last
      return mockCall("grid_page", args);
    });
    const errors: unknown[] = [];
    const { result, rerender } = renderHook(
      ({ state }) => useGridData({ tabId: "t1", state, zone: "UTC", enabled: true, onError: (e) => errors.push(e) }),
      { initialProps: { state: asc } },
    );
    act(() => result.current.want(0, 30));
    await waitFor(() => expect(callsOf("grid_page")).toHaveLength(1));
    const firstId = (callsOf("grid_page")[0].args as { queryId: string }).queryId;

    await act(() => sleep(100));
    rerender({ state: desc });

    await waitFor(() => expect(result.current.rowAt(0)).not.toBeNull());
    expect(result.current.rowAt(0)!.id).toBe(5000); // RecordId descending: the last row first
    await act(() => sleep(400)); // the delayed first answer has arrived by now
    expect(result.current.rowAt(0)!.id).toBe(5000);
    const cancels = callsOf("query_cancel").flatMap((c) => (c.args as { queryIds: string[] }).queryIds);
    expect(cancels).toContain(firstId);
    expect(errors).toEqual([]);
  });

  it("no_old_row_is_read_under_a_new_state_even_before_the_effects_run", async () => {
    const asc = initialGridState("mock_events");
    const desc: GridState = { ...asc, sort: [{ column: "RecordId", descending: true }] };
    const seen: Array<number | null> = [];
    const onError = () => {};
    const { result, rerender } = renderHook(
      ({ state }) => {
        const d = useGridData({ tabId: "f1", state, zone: "UTC", enabled: true, onError });
        seen.push(d.rowAt(0)?.id ?? null); // what a render would paint
        return d;
      },
      { initialProps: { state: asc } },
    );
    await waitFor(() => expect(result.current.rowAt(0)?.id).toBe(1));
    seen.length = 0;
    rerender({ state: desc });
    expect(seen[0]).toBeNull(); // the very first render under the new state shows none of the old rows
    await waitFor(() => expect(result.current.rowAt(0)?.id).toBe(5000));
  });

  it("an_equal_but_new_state_object_is_not_a_new_query", async () => {
    const onError = () => {};
    const { result, rerender } = renderHook(
      ({ state }) => useGridData({ tabId: "f5", state, zone: "UTC", enabled: true, onError }),
      { initialProps: { state: initialGridState("mock_events") } },
    );
    await waitFor(() => expect(result.current.rowAt(0)).not.toBeNull());
    expect(callsOf("grid_page")).toHaveLength(1);
    rerender({ state: initialGridState("mock_events") });
    await act(() => sleep(80));
    expect(result.current.rowAt(0)).not.toBeNull();
    expect(callsOf("grid_page")).toHaveLength(1);
    expect(callsOf("grid_counts")).toHaveLength(1);
    expect(callsOf("query_cancel")).toHaveLength(0);
  });

  it("states_equal_with_keys_in_another_order_are_not_a_new_query", async () => {
    const onError = () => {};
    const a = initialGridState("mock_events");
    const b = JSON.parse(JSON.stringify(a)) as GridState;
    const reordered = {
      columns: b.columns, sort: b.sort, hosts: b.hosts, search: { mode: b.search.mode, case_sensitive: false, regex: false, text: "" },
      chips: b.chips, relation: b.relation,
    } as GridState;
    expect(Object.keys(reordered)).not.toEqual(Object.keys(a));
    const { result, rerender } = renderHook(
      ({ state }) => useGridData({ tabId: "f5b", state, zone: "UTC", enabled: true, onError }),
      { initialProps: { state: a } },
    );
    await waitFor(() => expect(result.current.rowAt(0)).not.toBeNull());
    rerender({ state: reordered });
    await act(() => sleep(80));
    expect(callsOf("grid_page")).toHaveLength(1);
    expect(callsOf("query_cancel")).toHaveLength(0);
    expect(stableKey({ b: 1, a: [{ y: 1, x: 2 }] })).toBe(stableKey({ a: [{ x: 2, y: 1 }], b: 1 }));
    expect(stableKey([1, 2])).not.toBe(stableKey([2, 1]));
  });

  it("a_state_change_cancels_a_copys_fetches", async () => {
    const asc = initialGridState("mock_events");
    const desc: GridState = { ...asc, sort: [{ column: "RecordId", descending: true }] };
    overrideCommand("grid_page", async (args) => {
      const slot = (args as { slot: string }).slot;
      if (slot.includes(":copy:")) await sleep(300);
      return mockCall("grid_page", args);
    });
    const onError = () => {};
    const { result, rerender } = renderHook(
      ({ state }) => useGridData({ tabId: "n1", state, zone: "UTC", enabled: true, onError }),
      { initialProps: { state: asc } },
    );
    await waitFor(() => expect(result.current.rowAt(0)).not.toBeNull());
    const session = result.current.beginCopy();
    const pending = session.page(3);
    await waitFor(() => expect(callsOf("grid_page").some((c) => (c.args as { slot: string }).slot.includes(":copy:600"))).toBe(true));
    const copyId = (callsOf("grid_page").find((c) => (c.args as { slot: string }).slot.includes(":copy:600"))!.args as { queryId: string }).queryId;
    rerender({ state: desc });
    expect(await pending).toBeNull();
    expect(session.alive()).toBe(false);
    const cancels = callsOf("query_cancel").flatMap((c) => (c.args as { queryIds: string[] }).queryIds);
    expect(cancels).toContain(copyId);
  });

  it("starting_a_new_copy_cancels_the_old_ones_fetches", async () => {
    overrideCommand("grid_page", async (args) => {
      if ((args as { slot: string }).slot.includes(":copy:")) await sleep(200);
      return mockCall("grid_page", args);
    });
    const state = initialGridState("mock_events");
    const onError = () => {};
    const { result } = renderHook(() => useGridData({ tabId: "n1b", state, zone: "UTC", enabled: true, onError }));
    await waitFor(() => expect(result.current.rowAt(0)).not.toBeNull());
    const first = result.current.beginCopy();
    const pending = first.page(3);
    await waitFor(() => expect(callsOf("grid_page").some((c) => (c.args as { slot: string }).slot.includes(":copy:600"))).toBe(true));
    const second = result.current.beginCopy();
    expect(await pending).toBeNull();
    expect(first.alive()).toBe(false);
    expect(first.replaced()).toBe(true);
    expect(second.alive()).toBe(true);
    expect(callsOf("query_cancel").length).toBeGreaterThan(0);
  });

  it("a_cancelled_page_that_later_rejects_with_another_error_is_not_reported", async () => {
    let n = 0;
    overrideCommand("grid_page", async (args) => {
      n += 1;
      if (n === 1) {
        await sleep(150);
        throw { code: "engine", message: "obsolete" };
      }
      return mockCall("grid_page", args);
    });
    const errors: unknown[] = [];
    const state = initialGridState("mock_events");
    const onError = (e: unknown) => errors.push(e);
    const { result } = renderHook(() => useGridData({ tabId: "f4", state, zone: "UTC", enabled: true, onError }));
    await waitFor(() => expect(callsOf("grid_page")).toHaveLength(1));
    act(() => result.current.want(1000, 1010)); // block 0 is no longer wanted: cancelled
    act(() => result.current.want(0, 10)); // wanted again: a second query for the same block
    await waitFor(() => expect(result.current.rowAt(0)).not.toBeNull());
    await act(() => sleep(250)); // the cancelled one rejects now
    expect(errors).toEqual([]);
    expect(result.current.rowAt(0)).not.toBeNull();
    expect(callsOf("grid_page").filter((c) => (c.args as { offset: number }).offset === 0)).toHaveLength(2);
  });

  it("an_interrupted_or_superseded_answer_is_not_an_error", async () => {
    overrideCommand("grid_page", () => Promise.reject({ code: "interrupted", message: "x" }));
    const errors: unknown[] = [];
    const state = initialGridState("mock_events"); // the state keeps its identity between renders: a new object is a new query
    const onError = (e: unknown) => errors.push(e);
    const { result } = renderHook(() => useGridData({ tabId: "t2", state, zone: "UTC", enabled: true, onError }));
    act(() => result.current.want(0, 10));
    await act(() => sleep(100));
    expect(errors).toEqual([]);
    expect(result.current.rowAt(0)).toBeNull();
  });

  it("counts_arrive_on_their_own_slot_and_do_not_gate_rows", async () => {
    overrideCommand("grid_counts", async () => {
      await sleep(200);
      return { rows: 5000, total: 5000, failed_values: 0 };
    });
    const state = initialGridState("mock_events");
    const onError = () => {};
    const { result } = renderHook(() => useGridData({ tabId: "t3", state, zone: "UTC", enabled: true, onError }));
    await waitFor(() => expect(result.current.rowAt(0)).not.toBeNull());
    expect(result.current.counts).toBeNull();
    await waitFor(() => expect(result.current.counts?.rows).toBe(5000));
    expect((callsOf("grid_counts")[0].args as { slot: string }).slot).toBe("tab:t3:counts");
  });
});
