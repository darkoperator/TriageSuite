// Filter chips: how the grid's filter surfaces make, read and edit the `Chip`s in `GridState.chips`. The front end only
// composes leaf filters `{column, op, value, case_sensitive}` and combinators; the engine compiles and checks them.
//
// Chip ids (1 to 64 of [A-Za-z0-9_-], unique): one chip per column and surface where the surface replaces its own chip,
// so those ids are a prefix and a hash of the column name (a name may hold anything, an id may not):
//   fr-<h>  filter row         vl-<h>  Values tab      tx-<h>  Text tab      mk  the Mark column
// A cell filter and an Advanced chip are new every time: ce-<token> and adv-<token>. A date range chip's id is the
// engine's (`dr-` plus hex).
//
// Labels are display text only (the engine never parses them) and are cut at the engine's cap.
import type { Chip } from "../contract/generated/Chip";
import type { FilterNode } from "../contract/generated/FilterNode";
import type { Op } from "../contract/generated/Op";
import type { SqlType } from "../contract/generated/SqlType";
import { displaySafe } from "../text";

export const MAX_CHIPS = 50;
export const MAX_CHIP_LABEL = 200;
/** A chip's own filter may be this deep: the grid wraps the enabled chips in one AND and the engine allows 8. */
export const MAX_CHIP_DEPTH = 7;
/** The pseudo-column that filters on a row's mark: `eq`, `ne` or `in` with `suspicious` or `none`. */
export const MARK_COLUMN = "_mark";
export const MARK_VALUES = ["suspicious", "none"] as const;
export type MarkValue = (typeof MARK_VALUES)[number];
export const MARK_LABEL: Record<MarkValue, string> = { suspicious: "Suspicious", none: "None" };

/** Operators the Text tab offers, in the order shown. `takesValue` false means the filter carries no value. */
export const TEXT_OPS: readonly { op: Op; label: string; takesValue: boolean }[] = [
  { op: "eq", label: "equals", takesValue: true },
  { op: "ne", label: "not equals", takesValue: true },
  { op: "contains", label: "contains", takesValue: true },
  { op: "not_contains", label: "not contains", takesValue: true },
  { op: "starts_with", label: "starts with", takesValue: true },
  { op: "ends_with", label: "ends with", takesValue: true },
  { op: "regex", label: "regex", takesValue: true },
  { op: "is_empty", label: "is empty", takesValue: false },
  { op: "not_empty", label: "is not empty", takesValue: false },
];
export const CONVERSION_FAILED = { op: "conversion_failed" as Op, label: "did not convert", takesValue: false };

/** Every operator the advanced editor offers. */
export const ALL_OPS: readonly { op: Op; label: string }[] = [
  ...TEXT_OPS.map(({ op, label }) => ({ op, label })),
  { op: "in", label: "is one of" },
  { op: "not_in", label: "is not one of" },
  { op: "between", label: "is between" },
  { op: "gt", label: ">" },
  { op: "gte", label: ">=" },
  { op: "lt", label: "<" },
  { op: "lte", label: "<=" },
  { op: CONVERSION_FAILED.op, label: CONVERSION_FAILED.label },
];
export const opLabel = (op: Op): string => ALL_OPS.find((o) => o.op === op)?.label ?? op;
export const opTakesValue = (op: Op) => op !== "is_empty" && op !== "not_empty" && op !== "conversion_failed";

/** `text` cut to the engine's label cap, counting characters (code points), with an ellipsis when cut. */
export function clipLabel(text: string): string {
  const chars = [...text];
  return chars.length <= MAX_CHIP_LABEL ? text : chars.slice(0, MAX_CHIP_LABEL - 1).join("") + "…";
}

/** A short clip of one value for use inside a label, so a long value does not crowd out the rest of the label. */
const quote = (v: string) => `"${[...v].length > 60 ? [...v].slice(0, 59).join("") + "…" : v}"`;

/** 16 hex digits from two FNV-1a passes over the column name: a name of any content makes a valid id. */
export function hash(s: string): string {
  let a = 0x811c9dc5;
  let b = 0x01000193 ^ 0x9e3779b9;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    a = Math.imul(a ^ c, 0x01000193) >>> 0;
    b = Math.imul(b + c + i, 0x85ebca6b) >>> 0;
    b ^= b >>> 13;
  }
  return a.toString(16).padStart(8, "0") + b.toString(16).padStart(8, "0");
}
export const filterRowId = (column: string) => `fr-${hash(column)}`;
export const valuesId = (column: string) => `vl-${hash(column)}`;
export const textId = (column: string) => `tx-${hash(column)}`;
export const MARK_CHIP_ID = "mk";

let counter = 0;
/** A token no other chip of this window has: a counter and some randomness (never a clock). */
const token = () => `${(counter++).toString(36)}${Math.random().toString(36).slice(2, 8)}`;
export const cellChipId = () => `ce-${token()}`;
export const advancedChipId = () => `adv-${token()}`;

// ---- leaves ------------------------------------------------------------------------------------------------------
export const leaf = (column: string, op: Op, value: unknown = null, caseSensitive = false): FilterNode => ({
  column, op, value: value as never, case_sensitive: caseSensitive,
});

const chip = (id: string, label: string, origin: Chip["origin"], filter: FilterNode): Chip => ({
  id, label: clipLabel(label), enabled: true, origin, filter,
});

/** The filter row's box: a case-insensitive `contains`. */
export function filterRowChip(column: string, text: string): Chip {
  return chip(filterRowId(column), `${column} contains ${quote(text)}`, "filter_row", leaf(column, "contains", text));
}

/** What the filter row shows for a column: the text of its filter-row chip, or empty. */
export function filterRowText(chips: readonly Chip[], column: string): string {
  const f = chips.find((c) => c.id === filterRowId(column))?.filter;
  return f && "op" in f && typeof f.value === "string" ? f.value : "";
}

/** The Text tab's chip. Value-less operators carry `null`. */
export function textChip(column: string, op: Op, text: string, caseSensitive: boolean): Chip {
  const takes = opTakesValue(op);
  const label = takes ? `${column} ${opLabel(op)} ${quote(text)}` : `${column} ${opLabel(op)}`;
  return chip(textId(column), label, "text", leaf(column, op, takes ? text : null, takes && caseSensitive));
}

/**
 * Filter in (`eq`) and Filter out (`ne`) on a cell: case-sensitive. The engine's `is_empty` matches NULL and `''`
 * together, so it stands in only for a NULL cell (`text: null`); a stored empty string stays `eq ""` / `ne ""` and
 * selects exactly the rows whose text is empty.
 */
export function cellChip(column: string, op: "eq" | "ne", text: string | null): Chip {
  if (text === null) {
    const empty: Op = op === "eq" ? "is_empty" : "not_empty";
    return chip(cellChipId(), `${column} ${opLabel(empty)}`, "cell", leaf(column, empty));
  }
  return chip(cellChipId(), `${column} ${opLabel(op)} ${quote(text)}`, "cell", leaf(column, op, text, true));
}

/**
 * The value an ordered operator (`gt`, `gte`, `lt`, `lte`, `between`) sends, by the column's type, as the engine takes it
 * (filter/compile.rs `typed_value`): a BOOLEAN column needs a JSON boolean, BIGINT and UBIGINT a JSON number (or a numeric
 * string), TIMESTAMP a UTC text, and an untyped column compares the text as written. Text that is not of the column's
 * type is sent as typed so the engine refuses it by name; an integer too large for an exact JS number stays a string.
 */
export function orderedValue(sqlType: SqlType | null, text: string): string | number | boolean {
  if (sqlType === "BOOLEAN") return text === "true" ? true : text === "false" ? false : text;
  if (sqlType === "BIGINT" || sqlType === "UBIGINT") {
    const whole = sqlType === "BIGINT" ? /^-?\d+$/ : /^\d+$/;
    if (whole.test(text) && Number.isSafeInteger(Number(text))) return Number(text);
  }
  return text;
}

// ---- the Values tab -----------------------------------------------------------------------------------------------
export type ValuesMode = "include" | "exclude";

/**
 * The filter of a Values chip. Empty cells are not a string, so they ride beside the list as `is_empty` (include) or
 * `not_empty` (exclude); only the non-empty values go into the `in` / `not_in` list.
 */
export function valuesFilter(column: string, values: readonly (string | null)[], mode: ValuesMode): FilterNode {
  const strings = values.filter((v): v is string => v !== null);
  const empty = values.some((v) => v === null);
  const include = mode === "include";
  const parts: FilterNode[] = [];
  if (strings.length > 0) parts.push(leaf(column, include ? "in" : "not_in", strings, true));
  if (empty) parts.push(leaf(column, include ? "is_empty" : "not_empty"));
  return parts.length === 1 ? parts[0] : include ? { or: parts } : { and: parts };
}

export function valuesChip(column: string, values: readonly (string | null)[], mode: ValuesMode): Chip {
  const shown = values.slice(0, 3).map((v) => (v === null ? "(empty)" : quote(v))).join(", ");
  const more = values.length > 3 ? ` +${values.length - 3} more` : "";
  return chip(valuesId(column), `${column} ${mode === "include" ? "is" : "is not"} ${shown}${more}`, "values", valuesFilter(column, values, mode));
}

/** A Values chip's ticks and mode, read back from its filter; null when the chip is not the shape `valuesFilter` makes. */
export function readValuesChip(c: Chip): { mode: ValuesMode; values: (string | null)[] } | null {
  const leaves: FilterNode[] = "and" in c.filter ? c.filter.and : "or" in c.filter ? c.filter.or : [c.filter];
  const values: (string | null)[] = [];
  let mode: ValuesMode | null = null;
  for (const l of leaves) {
    if (!("op" in l)) return null;
    const m: ValuesMode | null = l.op === "in" || l.op === "is_empty" ? "include" : l.op === "not_in" || l.op === "not_empty" ? "exclude" : null;
    if (m === null || (mode !== null && mode !== m)) return null;
    mode = m;
    if (l.op === "is_empty" || l.op === "not_empty") values.push(null);
    else if (Array.isArray(l.value)) values.push(...(l.value as string[]));
    else return null;
  }
  return mode === null ? null : { mode, values };
}

// ---- the Mark column ----------------------------------------------------------------------------------------------
/** Ticking one of Suspicious / None filters to it; ticking both or neither is no filter, so no chip. */
export function markChip(selected: readonly MarkValue[]): Chip | null {
  if (selected.length !== 1) return null;
  const v = selected[0];
  return { id: MARK_CHIP_ID, label: `Mark: ${MARK_LABEL[v]}`, enabled: true, origin: "values", filter: leaf(MARK_COLUMN, "in", [v]) };
}

export function markSelection(chips: readonly Chip[]): MarkValue[] {
  const f = chips.find((c) => c.id === MARK_CHIP_ID)?.filter;
  if (!f || !("op" in f) || !Array.isArray(f.value)) return [];
  return (f.value as string[]).filter((v): v is MarkValue => (MARK_VALUES as readonly string[]).includes(v));
}

// ---- editing the list ---------------------------------------------------------------------------------------------
/** Replaces the chip with the same id where it stands, else appends. */
export function upsertChip(chips: readonly Chip[], next: Chip): Chip[] {
  return chips.some((c) => c.id === next.id) ? chips.map((c) => (c.id === next.id ? next : c)) : [...chips, next];
}
export const removeChip = (chips: readonly Chip[], id: string): Chip[] => chips.filter((c) => c.id !== id);
export const toggleChip = (chips: readonly Chip[], id: string): Chip[] =>
  chips.map((c) => (c.id === id ? { ...c, enabled: !c.enabled } : c));

/** The columns a filter names (the first is what a date range chip is about). */
export function filterColumns(node: FilterNode, out: string[] = []): string[] {
  if ("and" in node) node.and.forEach((n) => filterColumns(n, out));
  else if ("or" in node) node.or.forEach((n) => filterColumns(n, out));
  else if ("not" in node) filterColumns(node.not, out);
  else out.push(node.column);
  return out;
}

/** A readable one-line form of a filter, for an Advanced chip's label. */
export function describe(node: FilterNode): string {
  if ("and" in node) return `(${node.and.map(describe).join(" AND ")})`;
  if ("or" in node) return `(${node.or.map(describe).join(" OR ")})`;
  if ("not" in node) return `NOT ${describe(node.not)}`;
  const v = node.value;
  const text = Array.isArray(v) ? v.map((x) => quote(String(x))).join(", ") : v === null ? "" : quote(String(v));
  return `${node.column} ${opLabel(node.op)} ${text}`.trim();
}

// ---- groups -------------------------------------------------------------------------------------------------------
/**
 * The chip that selects exactly one group's rows, as the engine's `group_filter` does: a key is a case-sensitive `eq` on
 * the whole key (pass `full_key ?? key`), a missing key is `is_empty`; the `_mark` pseudo-column takes `eq` with the mark
 * (`suspicious`) or `none`. The id is `gr-` and a hash of the column, with `-n` when that id is taken: a key (any text)
 * never reaches an id. The label goes through `displaySafe` and is cut at the cap.
 */
export function groupChip(column: string, key: string | null, taken: ReadonlySet<string>): Chip {
  const empty = key === null || key === "";
  let id = `gr-${hash(column)}`;
  for (let n = 1; taken.has(id); n++) id = `gr-${hash(column)}-${n}`;
  if (column === MARK_COLUMN) {
    const mark = empty ? "none" : key;
    return { id, label: clipLabel(displaySafe(`Mark: ${mark === "none" ? "None" : mark === "suspicious" ? "Suspicious" : mark}`)), enabled: true, origin: "group", filter: leaf(column, "eq", mark, true) };
  }
  const label = empty ? `${column} is empty` : `${column} = ${quote(key)}`;
  return {
    id, label: clipLabel(displaySafe(label)), enabled: true, origin: "group",
    filter: empty ? leaf(column, "is_empty") : leaf(column, "eq", key, true),
  };
}

/** One group chip per level of `by` for the keys of a path, ids unique among `existing` and each other. */
export function groupChips(by: readonly string[], keys: readonly (string | null)[], existing: readonly Chip[]): Chip[] {
  const taken = new Set(existing.map((c) => c.id));
  const out: Chip[] = [];
  keys.forEach((key, i) => {
    const c = groupChip(by[i], key, taken);
    taken.add(c.id);
    out.push(c);
  });
  return out;
}
