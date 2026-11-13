import { describe, expect, it } from "vitest";
import type { Chip } from "../contract/generated/Chip";
import type { ColourRule } from "../contract/generated/ColourRule";
import { leaf } from "./chips";
import {
  OFFERED_COLOURS, RULE_CLASSES, RULE_COLOURS, cellRuleClass, filterFromChips, moveRule, newRule, rowRuleClass, rulesStatus, swatchClass,
} from "./ruleClasses";

const chip = (id: string, enabled: boolean, column = "Level"): Chip => ({
  id, label: id, enabled, origin: "advanced", filter: leaf(column, "eq", id),
});
const rule = (id: string): ColourRule => ({ id, name: id, enabled: true, colour: "blue", scope: "row", filter: leaf("Level", "eq", id) });

describe("rule classes", () => {
  it("rule_classes_come_from_a_fixed_set", () => {
    const seen = new Set<string>();
    for (const colour of RULE_COLOURS) {
      for (const scope of ["row", "cell"] as const) {
        const c = scope === "row" ? rowRuleClass({ colour, scope }) : cellRuleClass({ colour, scope });
        expect(c).toMatch(/^rule-(row|cell)-(red|orange|amber|green|blue|teal|grey)$/);
        seen.add(c);
      }
      seen.add(swatchClass(colour));
    }
    expect(seen.size).toBe(21);
    for (const c of seen) expect(RULE_CLASSES).toContain(c);
    // a colour that is not a token (a settings file edited by hand) is no class, never a class built from the string
    for (const bad of ["<img onerror=1>", "red; background:url(x)", "__proto__", "constructor", "toString", "", null, 7]) {
      expect(rowRuleClass({ colour: bad as never, scope: "row" })).toBe("");
      expect(cellRuleClass({ colour: bad as never, scope: "cell" })).toBe("");
      expect(swatchClass(bad as never)).toBe("");
    }
    // the scope decides which of the two a rule gives
    expect(rowRuleClass({ colour: "red", scope: "cell" })).toBe("");
    expect(cellRuleClass({ colour: "red", scope: "row" })).toBe("");
    expect(rowRuleClass(undefined)).toBe("");
  });

  it("the_reserved_mark_colours_are_not_offered", () => {
    expect(OFFERED_COLOURS).not.toContain("red");
    expect(OFFERED_COLOURS).not.toContain("amber");
    expect(RULE_COLOURS).toContain("red"); // the Hayabusa defaults still hold them
  });
});

describe("building rules", () => {
  it("rule_from_current_filter_ands_the_enabled_chips", () => {
    expect(filterFromChips([])).toBeNull();
    expect(filterFromChips([chip("a", false)])).toBeNull();
    expect(filterFromChips([chip("a", true)])).toEqual(leaf("Level", "eq", "a"));
    expect(filterFromChips([chip("a", true), chip("b", false), chip("c", true, "Channel")])).toEqual({
      and: [leaf("Level", "eq", "a"), leaf("Channel", "eq", "c")],
    });
    const r = newRule(leaf("Level", "eq", "x"), [rule("one")]);
    expect(r).toMatchObject({ name: "Rule 2", enabled: true, colour: "blue", scope: "row" });
    expect(r.id).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
  });

  it("move_keeps_every_rule", () => {
    const rs = [rule("a"), rule("b"), rule("c")];
    expect(moveRule(rs, 0, 2).map((r) => r.id)).toEqual(["b", "c", "a"]);
    expect(moveRule(rs, 2, 1).map((r) => r.id)).toEqual(["a", "c", "b"]);
    expect(moveRule(rs, 0, 3).map((r) => r.id)).toEqual(["a", "b", "c"]);
  });

  it("status_words", () => {
    const d = [{ ...rule("hayabusa-crit") }];
    expect(rulesStatus(d, true)).toBe("colour rule: Hayabusa level (on)");
    expect(rulesStatus([rule("a"), { ...rule("b"), enabled: false }], false)).toBe("colour rule: 1 on");
    expect(rulesStatus([{ ...rule("a"), enabled: false }], false)).toBeNull();
  });
});
