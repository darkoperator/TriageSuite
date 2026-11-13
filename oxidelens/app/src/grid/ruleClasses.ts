// Colour rules, the pure parts: the fixed colour set mapped to fixed class names, the rules a new rule is built from, and
// the list edits. Nothing here reads data: a rule's colour is one of seven tokens the engine already validated, and a
// value outside them (a settings file edited by hand) maps to no class at all.
import type { Chip } from "../contract/generated/Chip";
import type { ColourRule } from "../contract/generated/ColourRule";
import type { FilterNode } from "../contract/generated/FilterNode";
import type { RuleColour } from "../contract/generated/RuleColour";

/** The engine's cap on one page's rules, and on a rule's name. */
export const MAX_RULES = 50;
export const MAX_RULE_NAME = 80;

/** Every token the engine accepts, in the order the tokens are written in `theme/tokens.css`. */
export const RULE_COLOURS: readonly RuleColour[] = ["red", "orange", "amber", "green", "blue", "teal", "grey"];

/**
 * The colours the picker offers. Red and amber are the reserved malicious and suspicious mark colours, so a rule is not
 * given them by choice; the Hayabusa defaults (critical red, medium amber) already hold them and keep showing and
 * working, and the picker lists a rule's current colour even when it is one of these.
 */
export const OFFERED_COLOURS: readonly RuleColour[] = ["orange", "green", "blue", "teal", "grey"];

export const COLOUR_LABEL: Record<RuleColour, string> = {
  red: "Red", orange: "Orange", amber: "Amber", green: "Green", blue: "Blue", teal: "Teal", grey: "Grey",
};

// Literal maps, so the whole class set is visible here and in grid.css; no class is ever built from a string.
const ROW_CLASS: Record<RuleColour, string> = {
  red: "rule-row-red", orange: "rule-row-orange", amber: "rule-row-amber", green: "rule-row-green",
  blue: "rule-row-blue", teal: "rule-row-teal", grey: "rule-row-grey",
};
const CELL_CLASS: Record<RuleColour, string> = {
  red: "rule-cell-red", orange: "rule-cell-orange", amber: "rule-cell-amber", green: "rule-cell-green",
  blue: "rule-cell-blue", teal: "rule-cell-teal", grey: "rule-cell-grey",
};
const SWATCH_CLASS: Record<RuleColour, string> = {
  red: "swatch-red", orange: "swatch-orange", amber: "swatch-amber", green: "swatch-green",
  blue: "swatch-blue", teal: "swatch-teal", grey: "swatch-grey",
};

/** Every class this module can return. */
export const RULE_CLASSES: readonly string[] = [
  ...Object.values(ROW_CLASS), ...Object.values(CELL_CLASS), ...Object.values(SWATCH_CLASS),
];

const own = (m: Record<string, string>, k: unknown): string => (typeof k === "string" && Object.hasOwn(m, k) ? m[k] : "");

/** The class of a row a row-scope rule matched ("" for a scope or colour this build does not know). */
export const rowRuleClass = (rule: Pick<ColourRule, "colour" | "scope"> | undefined): string =>
  rule && rule.scope === "row" ? own(ROW_CLASS, rule.colour) : "";
/** The class of a cell a cell-scope rule tints. */
export const cellRuleClass = (rule: Pick<ColourRule, "colour" | "scope"> | undefined): string =>
  rule && rule.scope === "cell" ? own(CELL_CLASS, rule.colour) : "";
export const swatchClass = (colour: RuleColour): string => own(SWATCH_CLASS, colour);

// ---- building and editing ---------------------------------------------------------------------------------------------
let counter = 0;
/** A rule id no other rule of this window has: a counter and some randomness (never a clock). */
export const newRuleId = (): string => `rule-${(counter++).toString(36)}${Math.random().toString(36).slice(2, 8)}`;

/**
 * The filter a rule built from the current chips carries: the enabled chips ANDed (one chip is its own filter), or null
 * when none is enabled. A chip is at most 7 deep, so the AND makes 8, the engine's cap.
 */
export function filterFromChips(chips: readonly Chip[]): FilterNode | null {
  const on = chips.filter((c) => c.enabled).map((c) => c.filter);
  if (on.length === 0) return null;
  return on.length === 1 ? on[0] : { and: on };
}

/** A new enabled row rule, blue, named for its position, over `filter`. */
export function newRule(filter: FilterNode, existing: readonly ColourRule[]): ColourRule {
  return { id: newRuleId(), name: `Rule ${existing.length + 1}`, enabled: true, filter, colour: "blue", scope: "row" };
}

export const moveRule = (rules: readonly ColourRule[], from: number, to: number): ColourRule[] => {
  if (from === to || from < 0 || to < 0 || from >= rules.length || to >= rules.length) return [...rules];
  const next = [...rules];
  const [r] = next.splice(from, 1);
  next.splice(to, 0, r);
  return next;
};

export const patchRule = (rules: readonly ColourRule[], id: string, patch: Partial<ColourRule>): ColourRule[] =>
  rules.map((r) => (r.id === id ? { ...r, ...patch } : r));

export const removeRule = (rules: readonly ColourRule[], id: string): ColourRule[] => rules.filter((r) => r.id !== id);

/** The status line's words for the rules in force: the defaults named, a custom list counted, nothing when none is on. */
export function rulesStatus(rules: readonly ColourRule[], usingDefaults: boolean): string | null {
  const on = rules.filter((r) => r.enabled).length;
  if (on === 0) return null;
  if (usingDefaults) {
    return rules.every((r) => r.id.startsWith("hayabusa-")) ? "colour rule: Hayabusa level (on)" : "colour rules: defaults (on)";
  }
  return `colour ${on === 1 ? "rule" : "rules"}: ${on} on`;
}
