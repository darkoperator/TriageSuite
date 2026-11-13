import { describe, expect, it } from "vitest";
import { BLOCK_ROWS, BlockCache, MAX_BLOCKS, blockOf, blocksCovering } from "./blocks";

describe("blocks", () => {
  it("blocks_are_200_rows_and_covering_is_inclusive", () => {
    expect(BLOCK_ROWS).toBe(200);
    expect(MAX_BLOCKS).toBe(12);
    expect(blockOf(0)).toBe(0);
    expect(blockOf(199)).toBe(0);
    expect(blockOf(200)).toBe(1);
    expect(blocksCovering(190, 410)).toEqual([0, 1, 2]);
    expect(blocksCovering(5, 5)).toEqual([0]);
    expect(blocksCovering(-3, 4)).toEqual([0]);
  });

  it("drops_the_least_recently_drawn_beyond_the_cap_but_never_a_protected_block", () => {
    const cache = new BlockCache<number>(3);
    for (let b = 0; b < 3; b++) cache.set(b, b, new Set());
    cache.touch(0); // 1 is now the oldest
    cache.set(3, 3, new Set());
    expect([...cache.keys()].sort()).toEqual([0, 2, 3]);
    cache.set(4, 4, new Set([0, 2, 3, 4])); // everything protected: the cap yields rather than dropping a drawn block
    expect(cache.size).toBe(4);
    cache.set(5, 5, new Set([5]));
    expect(cache.size).toBe(3);
    expect(cache.has(5)).toBe(true);
  });
});
