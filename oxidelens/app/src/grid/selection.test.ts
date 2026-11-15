import { describe, expect, it } from "vitest";
import { MAX_COPY_ROWS, anchorIdOf, contains, emptySelection, fromPositions, indices, selectOnly, selectRange, size, toggle } from "./selection";

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
    const sel = fromPositions([[12, 2], [13, null], [14, 3], [40, 90]], [12, 13, 14, 40], 14);
    expect(sel.ranges).toEqual([[2, 3], [90, 90]]);
    expect(sel.ids).toEqual([12, 14, 40]);
    expect(sel.anchor).toBe(3);
    expect(sel.anchorId).toBe(14);
    // An anchor whose row left is cleared: no other row stands in for it. Nothing left is the empty selection.
    const gone = fromPositions([[40, 90], [12, 2], [13, null]], [40, 12], 13);
    expect(gone.anchor).toBeNull();
    expect(gone.anchorId).toBeNull();
    expect(gone.ids).toEqual([12, 40]);
    expect(fromPositions([[13, null]], [13], 13)).toEqual(emptySelection);
  });

  it("from_positions_places_an_anchor_outside_the_selection", () => {
    // A ctrl-click that unselected the anchor's row: the anchor is asked for beside the ids, but is not selected.
    const sel = fromPositions([[12, 2], [99, 7]], [12], 99);
    expect(sel.ranges).toEqual([[2, 2]]);
    expect(sel.ids).toEqual([12]);
    expect(sel.anchor).toBe(7);
    expect(sel.anchorId).toBe(99);
  });

  it("anchor_id_follows_the_anchor_index", () => {
    // The anchor's id is the id at its place among the selected rows, or the row's own id when it is not selected.
    expect(anchorIdOf(selectRange(selectOnly(9), 4), [24, 25, 26, 27, 28, 29])).toBe(29);
    expect(anchorIdOf(selectRange(selectOnly(2), 4), [12, 13, 14])).toBe(12);
    const split = toggle(selectRange(selectOnly(2), 6), 4);
    expect(anchorIdOf(split, [12, 13, 15, 16])).toBeNull();
    expect(anchorIdOf(toggle(split, 5), [12, 13, 16])).toBeNull();
    expect(anchorIdOf({ ...split, anchor: 6 }, [12, 13, 15, 16])).toBe(16);
    expect(anchorIdOf(emptySelection, [])).toBeNull();
  });
});
