// Row selection by row index in the current state's order: click selects one row, shift-click the range from the
// anchor, ctrl-click toggles one row. The selection is a sorted list of inclusive index ranges, so a range across
// blocks that are not loaded is still the whole range, and its size is exact without any rows in memory. It is
// cleared when the grid's state changes (the indexes mean something else then). Copy fetches what it needs.
//
// A write to the case re-reads the rows, and a load or a supersede can put other rows at the same indexes, so across a
// re-read the selection is kept by row id (ruling S10): `ids` holds the selected rows' ids once something resolved them
// (the engine's `grid_row_ids` for an action, or the rows in memory), and the grid rebuilds the ranges from where the
// engine says those ids are now (`grid_row_positions`, then `fromPositions`). Any new pick of rows drops the ids. The
// anchor is kept the same way: `anchorId` is resolved with the ids, and a re-read puts the anchor where that row is now,
// or clears it when the row left (no other row stands in for it, or the next shift-click would take a range the analyst
// never pointed at).

export type Range = readonly [from: number, to: number];

export interface Selection {
  ranges: readonly Range[];
  /** The row index shift-click extends from. */
  anchor: number | null;
  /** The selected rows' ids in index order, once resolved; null until then, and after every new pick. */
  ids: number[] | null;
  /** The anchor's row id, resolved with `ids`; null until then, after every new pick, and when there is no anchor. */
  anchorId: number | null;
}

export const emptySelection: Selection = { ranges: [], anchor: null, ids: null, anchorId: null };

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

export const selectOnly = (index: number): Selection => ({ ranges: [[index, index]], anchor: index, ids: null, anchorId: null });

export function contains(sel: Selection, index: number): boolean {
  return sel.ranges.some(([a, b]) => index >= a && index <= b);
}

export function size(sel: Selection): number {
  return sel.ranges.reduce((n, [a, b]) => n + (b - a + 1), 0);
}

export function toggle(sel: Selection, index: number): Selection {
  if (!contains(sel, index)) return { ranges: normalise([...sel.ranges, [index, index]]), anchor: index, ids: null, anchorId: null };
  const ranges: Range[] = [];
  for (const [a, b] of sel.ranges) {
    if (index < a || index > b) ranges.push([a, b]);
    else {
      if (a < index) ranges.push([a, index - 1]);
      if (index < b) ranges.push([index + 1, b]);
    }
  }
  return { ranges, anchor: index, ids: null, anchorId: null };
}

/** Every row from the anchor to `index`, loaded or not. */
export function selectRange(sel: Selection, index: number): Selection {
  const from = sel.anchor ?? index;
  return { ranges: [from <= index ? [from, index] : [index, from]], anchor: from, ids: null, anchorId: null };
}

/** The selected row indexes in order. */
export function* indices(sel: Selection): Generator<number> {
  for (const [a, b] of sel.ranges) for (let i = a; i <= b; i++) yield i;
}

/**
 * The anchor's row id when the anchor is one of the selected rows: the id at its place among `ids` (the selected rows'
 * ids in index order). Null when there is no anchor or it is not selected (a ctrl-click that unselected its row); the
 * grid then names it from the rows in memory, where the row it just clicked is.
 */
export function anchorIdOf(sel: Selection, ids: readonly number[]): number | null {
  const at = sel.anchor;
  if (at === null) return null;
  let before = 0;
  for (const [a, b] of sel.ranges) {
    if (at >= a && at <= b) return ids[before + (at - a)] ?? null;
    before += b - a + 1;
  }
  return null;
}

/**
 * The selection rebuilt from where the engine says its rows are now (`grid_row_positions`: each id with its index, or
 * null when the answer no longer holds it). `ids` are the rows that were selected; `positions` may also hold the
 * anchor's row when it was not one of them. Rows that left drop out; the anchor goes to `anchorId`'s row, or is
 * cleared when that row left.
 */
export function fromPositions(
  positions: ReadonlyArray<readonly [number, number | null]>,
  ids: readonly number[],
  anchorId: number | null,
): Selection {
  const selected = new Set(ids);
  const kept = positions
    .filter((p): p is readonly [number, number] => p[1] !== null && selected.has(p[0]))
    .sort((a, b) => a[1] - b[1]);
  if (kept.length === 0) return emptySelection;
  const anchor = anchorId === null ? null : (positions.find(([id, at]) => id === anchorId && at !== null)?.[1] ?? null);
  return {
    ranges: normalise(kept.map(([, at]) => [at, at])),
    anchor,
    ids: kept.map(([id]) => id),
    anchorId: anchor === null ? null : anchorId,
  };
}
