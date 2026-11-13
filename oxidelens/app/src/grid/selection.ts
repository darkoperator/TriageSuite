// Row selection by row index in the current state's order: click selects one row, shift-click the range from the
// anchor, ctrl-click toggles one row. The selection is a sorted list of inclusive index ranges, so a range across
// blocks that are not loaded is still the whole range, and its size is exact without any rows in memory. It is
// cleared when the grid's state changes (the indexes mean something else then). Copy fetches what it needs.

export type Range = readonly [from: number, to: number];

export interface Selection {
  ranges: readonly Range[];
  /** The row index shift-click extends from. */
  anchor: number | null;
}

export const emptySelection: Selection = { ranges: [], anchor: null };

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

export const selectOnly = (index: number): Selection => ({ ranges: [[index, index]], anchor: index });

export function contains(sel: Selection, index: number): boolean {
  return sel.ranges.some(([a, b]) => index >= a && index <= b);
}

export function size(sel: Selection): number {
  return sel.ranges.reduce((n, [a, b]) => n + (b - a + 1), 0);
}

export function toggle(sel: Selection, index: number): Selection {
  if (!contains(sel, index)) return { ranges: normalise([...sel.ranges, [index, index]]), anchor: index };
  const ranges: Range[] = [];
  for (const [a, b] of sel.ranges) {
    if (index < a || index > b) ranges.push([a, b]);
    else {
      if (a < index) ranges.push([a, index - 1]);
      if (index < b) ranges.push([index + 1, b]);
    }
  }
  return { ranges, anchor: index };
}

/** Every row from the anchor to `index`, loaded or not. */
export function selectRange(sel: Selection, index: number): Selection {
  const from = sel.anchor ?? index;
  return { ranges: [from <= index ? [from, index] : [index, from]], anchor: from };
}

/** The selected row indexes in order. */
export function* indices(sel: Selection): Generator<number> {
  for (const [a, b] of sel.ranges) for (let i = a; i <= b; i++) yield i;
}
