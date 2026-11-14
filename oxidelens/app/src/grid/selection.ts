// Row selection by row index in the current state's order: click selects one row, shift-click the range from the
// anchor, ctrl-click toggles one row. The selection is a sorted list of inclusive index ranges, so a range across
// blocks that are not loaded is still the whole range, and its size is exact without any rows in memory. It is
// cleared when the grid's state changes (the indexes mean something else then). Copy fetches what it needs.
//
// A write to the case re-reads the rows, and a load or a supersede can put other rows at the same indexes, so across a
// re-read the selection is kept by row id (ruling S10): `ids` holds the selected rows' ids once something resolved them
// (the engine's `grid_row_ids` for an action, or the rows in memory), and the grid rebuilds the ranges from where the
// engine says those ids are now (`grid_row_positions`, then `fromPositions`). Any new pick of rows drops the ids.

export type Range = readonly [from: number, to: number];

export interface Selection {
  ranges: readonly Range[];
  /** The row index shift-click extends from. */
  anchor: number | null;
  /** The selected rows' ids in index order, once resolved; null until then, and after every new pick. */
  ids: number[] | null;
}

export const emptySelection: Selection = { ranges: [], anchor: null, ids: null };

/**
 * The most rows the grid resolves to ids (the engine's `MAX_ACTION_ROWS`, ruling S10: `grid_row_ids` refuses more).
 * A larger selection is not asked for, so it has no ids and does not survive a re-read.
 */
export const MAX_ACTION_ROWS = 10_000;

/** The most rows Copy rows will fetch and write. */
export const MAX_COPY_ROWS = 50_000;

function normalise(ranges: Range[]): Range[] {
  const sorted = [...ranges].sort((a, b) => a[0] - b[0]);
  const out: Range[] = [];
  for (const r of sorted) {
    const last = out[out.length - 1];
    if (last && r[0] <= last[1] + 1) out[out.length - 1] = [last[0], Math.max(last[1], r[1])];
    else out.push(r);
  }
  return out;
}

export const selectOnly = (index: number): Selection => ({ ranges: [[index, index]], anchor: index, ids: null });

export function contains(sel: Selection, index: number): boolean {
  return sel.ranges.some(([a, b]) => index >= a && index <= b);
}

export function size(sel: Selection): number {
  return sel.ranges.reduce((n, [a, b]) => n + (b - a + 1), 0);
}

export function toggle(sel: Selection, index: number): Selection {
  if (!contains(sel, index)) return { ranges: normalise([...sel.ranges, [index, index]]), anchor: index, ids: null };
  const ranges: Range[] = [];
  for (const [a, b] of sel.ranges) {
    if (index < a || index > b) ranges.push([a, b]);
    else {
      if (a < index) ranges.push([a, index - 1]);
      if (index < b) ranges.push([index + 1, b]);
    }
  }
  return { ranges, anchor: index, ids: null };
}

/** Every row from the anchor to `index`, loaded or not. */
export function selectRange(sel: Selection, index: number): Selection {
  const from = sel.anchor ?? index;
  return { ranges: [from <= index ? [from, index] : [index, from]], anchor: from, ids: null };
}

/** The selected row indexes in order. */
export function* indices(sel: Selection): Generator<number> {
  for (const [a, b] of sel.ranges) for (let i = a; i <= b; i++) yield i;
}

/**
 * The selection rebuilt from where the engine says its rows are now (`grid_row_positions`: each id with its index, or
 * null when the answer no longer holds it). Rows that left drop out; the anchor follows `anchorId`'s row while it is
 * still selected, else the first selected row.
 */
export function fromPositions(positions: ReadonlyArray<readonly [number, number | null]>, anchorId: number | null): Selection {
  const kept = positions.filter((p): p is readonly [number, number] => p[1] !== null).sort((a, b) => a[1] - b[1]);
  if (kept.length === 0) return emptySelection;
  const anchor = kept.find(([id]) => id === anchorId)?.[1] ?? kept[0][1];
  return { ranges: normalise(kept.map(([, at]) => [at, at])), anchor, ids: kept.map(([id]) => id) };
}
