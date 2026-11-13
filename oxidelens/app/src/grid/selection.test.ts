import { describe, expect, it } from "vitest";
import { MAX_COPY_ROWS, contains, emptySelection, indices, selectOnly, selectRange, size, toggle } from "./selection";

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
});
