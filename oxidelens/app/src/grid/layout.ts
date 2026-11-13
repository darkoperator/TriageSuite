// Column layout: the pure rules for which columns show, in what order, how wide, and which are pinned. A layout is the
// `ColumnLayout` the settings keep per dataset; with none saved the defaults apply (metadata columns hidden, except
// the host column). The edits here take the *materialised* current layout, so the first edit writes down what the
// analyst was looking at, defaults included.
import type { ColumnLayout } from "../contract/generated/ColumnLayout";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";

export const MIN_WIDTH = 40;
export const MAX_WIDTH = 2000;
export const BEST_FIT_MAX = 600;
/** Space a best-fit width adds to the widest text: the cell's padding either side. */
export const BEST_FIT_PAD = 16;
/** Extra room the header needs beside its name (sort arrow, resize grip). */
export const HEADER_EXTRA = 24;

export const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));
export const clampWidth = (n: number) => clamp(Math.round(n), MIN_WIDTH, MAX_WIDTH);

/** A default width by what the column holds. */
export function defaultWidth(c: ColumnMeta): number {
  if (c.sql_type === "TIMESTAMP") return 200;
  if (c.sql_type === "BIGINT" || c.sql_type === "UBIGINT") return 100;
  return 160;
}

/** Metadata columns (role other than data) are hidden by default, except the host column. */
export const defaultHidden = (metas: readonly ColumnMeta[]): string[] =>
  metas.filter((c) => c.role !== "data" && c.role !== "host").map((c) => c.name);

export interface Resolved {
  /** Every column's name in the layout's own order (pinning not applied). */
  order: string[];
  /** Every column of the dataset in layout order, pinned ones first. */
  all: ColumnMeta[];
  /** The columns shown, in display order (pinned first). */
  shown: ColumnMeta[];
  hidden: ReadonlySet<string>;
  pinned: ReadonlySet<string>;
  width(name: string): number;
}

/** What the grid draws for `layout` (null: the defaults) over the dataset's columns. Names the dataset lacks are ignored. */
export function resolve(layout: ColumnLayout | null, metas: readonly ColumnMeta[]): Resolved {
  const byName = new Map(metas.map((c) => [c.name, c]));
  const order: string[] = [];
  const seen = new Set<string>();
  for (const n of layout?.order ?? []) {
    if (byName.has(n) && !seen.has(n)) {
      seen.add(n);
      order.push(n);
    }
  }
  for (const c of metas) if (!seen.has(c.name)) order.push(c.name);
  const pinned = new Set((layout?.pinned ?? []).filter((n) => byName.has(n)));
  const hidden = new Set((layout ? layout.hidden : defaultHidden(metas)).filter((n) => byName.has(n)));
  const all = [...order.filter((n) => pinned.has(n)), ...order.filter((n) => !pinned.has(n))].map((n) => byName.get(n)!);
  const widths = layout?.widths ?? {};
  return {
    order,
    all,
    shown: all.filter((c) => !hidden.has(c.name)),
    hidden,
    pinned,
    width: (name) => {
      const w = widths[name];
      return w === undefined ? defaultWidth(byName.get(name)!) : clampWidth(w);
    },
  };
}

/** The layout as the analyst sees it now, written out in full; names the dataset lacks that the layout holds are kept. */
export function materialise(layout: ColumnLayout | null, metas: readonly ColumnMeta[]): ColumnLayout {
  const r = resolve(layout, metas);
  const known = new Set(metas.map((c) => c.name));
  const extra = (list: readonly string[] | undefined) => (list ?? []).filter((n) => !known.has(n));
  return {
    order: [...r.order, ...extra(layout?.order)],
    widths: { ...(layout?.widths ?? {}) },
    hidden: [...r.hidden, ...extra(layout?.hidden)],
    pinned: [...r.pinned, ...extra(layout?.pinned)],
  };
}

export const setWidth = (l: ColumnLayout, name: string, width: number): ColumnLayout => ({
  ...l, widths: { ...l.widths, [name]: clampWidth(width) },
});

export const setWidths = (l: ColumnLayout, widths: Record<string, number>): ColumnLayout => ({
  ...l, widths: { ...l.widths, ...Object.fromEntries(Object.entries(widths).map(([k, v]) => [k, clampWidth(v)])) },
});

export const hide = (l: ColumnLayout, names: readonly string[]): ColumnLayout => ({
  ...l, hidden: [...new Set([...l.hidden, ...names])],
});

export const show = (l: ColumnLayout, names: readonly string[]): ColumnLayout => ({
  ...l, hidden: l.hidden.filter((n) => !names.includes(n)),
});

export const showAll = (l: ColumnLayout): ColumnLayout => ({ ...l, hidden: [] });

export const togglePin = (l: ColumnLayout, name: string): ColumnLayout => ({
  ...l, pinned: l.pinned.includes(name) ? l.pinned.filter((n) => n !== name) : [...l.pinned, name],
});

/**
 * Moves `name` to where `target` is (before it when coming from the right, after it when coming from the left), in the
 * layout's order. Pinning is a separate property: a pinned column stays among the pinned ones.
 */
export function moveColumn(l: ColumnLayout, name: string, target: string): ColumnLayout {
  if (name === target) return l;
  const from = l.order.indexOf(name);
  const to = l.order.indexOf(target);
  if (from < 0 || to < 0) return l;
  const order = l.order.filter((n) => n !== name);
  order.splice(order.indexOf(target) + (from < to ? 1 : 0), 0, name);
  return { ...l, order };
}

export type Measure = (text: string, font: "cell" | "ui") => number;

/** Width for a column from the widest of its texts and its header, as `measure` sizes them; 40 to 600. */
export function bestFitWidth(texts: readonly string[], header: string, measure: Measure): number {
  let widest = measure(header, "ui") + HEADER_EXTRA - BEST_FIT_PAD;
  for (const t of texts) widest = Math.max(widest, measure(t, "cell"));
  return clamp(Math.ceil(widest + BEST_FIT_PAD), MIN_WIDTH, BEST_FIT_MAX);
}

/** The names the engine fetches: `null` (everything) when nothing is hidden, else the shown ones in catalog order. */
export function fetchedColumns(r: Resolved, metas: readonly ColumnMeta[]): string[] | null {
  if (r.hidden.size === 0) return null;
  return metas.filter((c) => !r.hidden.has(c.name)).map((c) => c.name);
}
