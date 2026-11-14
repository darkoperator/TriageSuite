import { describe, expect, it } from "vitest";
import { MAX_COPY_ROWS, contains, emptySelection, fromPositions, indices, selectOnly, selectRange, size, toggle } from "./selection";

describe("selection", () => {
  it("a_range_is_exact_whatever_is_loaded", () => {
    const sel = selectRange(selectOnly(10), 1500);
    expect(size(sel)).toBe(1491);
    expect(contains(sel, 10)).toBe(true);
    expect(contains(sel, 1500)).toBe(true);
    expect(contains(sel, 9)).toBe(false);
    expect(selectRange(selectOnly(20), 5).ranges).toEqual([[5, 20]]);
  });

  it("ctrl_toggles_split_and_merge_ranges", () => {
    let sel = selectRange(selectOnly(2), 6);
    sel = toggle(sel, 4);
    expect(sel.ranges).toEqual([[2, 3], [5, 6]]);
    expect([...indices(sel)]).toEqual([2, 3, 5, 6]);
    sel = toggle(sel, 4);
    expect(sel.ranges).toEqual([[2, 6]]);
    sel = toggle(sel, 20);
    expect(size(sel)).toBe(6);
    expect(size(emptySelection)).toBe(0);
  });

  it("the_copy_cap_is_50000", () => {
    expect(MAX_COPY_ROWS).toBe(50_000);
    expect(size(selectRange(selectOnly(0), MAX_COPY_ROWS))).toBeGreaterThan(MAX_COPY_ROWS);
  });

  it("a_new_pick_drops_the_ids", () => {
    const resolved = { ...selectRange(selectOnly(2), 4), ids: [12, 13, 14] };
    expect(toggle(resolved, 9).ids).toBeNull();
    expect(selectRange(resolved, 6).ids).toBeNull();
    expect(selectOnly(1).ids).toBeNull();
    expect(emptySelection.ids).toBeNull();
  });

  it("from_positions_keeps_the_rows_still_there_in_index_order", () => {
    // 13 left the answer; 14 moved up to where 13 was; 40 sits far below.
    const sel = fromPositions([[12, 2], [13, null], [14, 3], [40, 90]], 14);
    expect(sel.ranges).toEqual([[2, 3], [90, 90]]);
    expect(sel.ids).toEqual([12, 14, 40]);
    expect(sel.anchor).toBe(3);
    // An anchor whose row left falls back to the first selected row; nothing left is the empty selection.
    expect(fromPositions([[40, 90], [12, 2]], 13).anchor).toBe(2);
    expect(fromPositions([[13, null]], 13)).toEqual(emptySelection);
  });
});
