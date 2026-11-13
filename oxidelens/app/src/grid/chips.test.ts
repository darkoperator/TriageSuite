// The pure chip rules: ids, labels, the shapes the engine accepts, the Values chip's round trip.
import { describe, expect, it } from "vitest";
import type { Chip } from "../contract/generated/Chip";
import {
  MAX_CHIP_LABEL, cellChip, clipLabel, filterRowChip, filterRowText, markChip, markSelection, readValuesChip, removeChip,
  orderedValue, textChip, toggleChip, upsertChip, valuesChip, valuesId,
} from "./chips";

const ID = /^[A-Za-z0-9_-]{1,64}$/;

describe("chips", () => {
  it("ids_are_valid_whatever_the_column_is_called", () => {
    for (const column of ["Channel", "a b/c", "<img src=x onerror=alert(1)>", "x".repeat(500), "ünï‮", ""]) {
      for (const chip of [filterRowChip(column, "v"), valuesChip(column, ["a"], "include"), textChip(column, "eq", "a", false), cellChip(column, "eq", "a")]) {
        expect(chip.id).toMatch(ID);
      }
    }
    expect(valuesId("Channel")).toBe(valuesId("Channel"));
    expect(valuesId("Channel")).not.toBe(valuesId("Provider"));
    expect(cellChip("A", "eq", "x").id).not.toBe(cellChip("A", "eq", "x").id);
  });

  it("labels_never_pass_the_engines_cap", () => {
    const long = "é".repeat(5000);
    for (const chip of [filterRowChip("C", long), valuesChip("C", [long], "exclude"), textChip("C", "contains", long, true), cellChip("C", "ne", long)]) {
      expect([...chip.label].length).toBeLessThanOrEqual(MAX_CHIP_LABEL);
    }
    expect(clipLabel("short")).toBe("short");
  });

  it("values_chip_shapes_and_round_trip", () => {
    const inc = valuesChip("C", ["a", "b"], "include");
    expect(inc.filter).toEqual({ column: "C", op: "in", value: ["a", "b"], case_sensitive: true });
    expect(inc.origin).toBe("values");
    // empty cells are not a string in the list: they are an is_empty leaf beside it
    const withEmpty = valuesChip("C", ["a", null], "include");
    expect(withEmpty.filter).toEqual({ or: [{ column: "C", op: "in", value: ["a"], case_sensitive: true }, { column: "C", op: "is_empty", value: null, case_sensitive: false }] });
    const outEmpty = valuesChip("C", ["a", null], "exclude");
    expect(outEmpty.filter).toEqual({ and: [{ column: "C", op: "not_in", value: ["a"], case_sensitive: true }, { column: "C", op: "not_empty", value: null, case_sensitive: false }] });
    for (const chip of [inc, withEmpty, outEmpty, valuesChip("C", [null], "include"), valuesChip("C", [null], "exclude")]) {
      const back = readValuesChip(chip)!;
      expect(valuesChip("C", back.values, back.mode).filter).toEqual(chip.filter);
    }
    expect(readValuesChip(filterRowChip("C", "x"))).toBeNull();
  });

  it("filter_row_chip_is_case_insensitive_contains_and_reads_back", () => {
    const c = filterRowChip("UserName", "adm");
    expect(c).toMatchObject({ origin: "filter_row", enabled: true, filter: { column: "UserName", op: "contains", value: "adm", case_sensitive: false } });
    expect(filterRowText([c], "UserName")).toBe("adm");
    expect(filterRowText([c], "Other")).toBe("");
  });

  it("value_less_operators_carry_null", () => {
    expect(textChip("C", "is_empty", "ignored", true).filter).toMatchObject({ op: "is_empty", value: null });
    expect(textChip("C", "conversion_failed", "", false).filter).toMatchObject({ op: "conversion_failed", value: null });
    expect(cellChip("C", "eq", null).filter).toMatchObject({ op: "is_empty", value: null });
    expect(cellChip("C", "ne", null).filter).toMatchObject({ op: "not_empty", value: null });
    expect(cellChip("C", "ne", "x").filter).toMatchObject({ op: "ne", value: "x", case_sensitive: true });
  });

  it("an_empty_string_cell_filters_by_eq_and_ne_not_by_emptiness", () => {
    // The engine's is_empty matches NULL and '' together, so only a NULL cell may use it.
    expect(cellChip("C", "eq", "").filter).toEqual({ column: "C", op: "eq", value: "", case_sensitive: true });
    expect(cellChip("C", "ne", "").filter).toEqual({ column: "C", op: "ne", value: "", case_sensitive: true });
    expect(cellChip("C", "eq", null).filter).toMatchObject({ op: "is_empty" });
  });

  it("values_empty_entry_is_null_and_empty_together_so_it_is_is_empty", () => {
    // oxl-engine query/group.rs key_sql is NULLIF(cell, ''): the null entry of grid_values counts NULL and '' as one
    // group. filter/compile.rs: is_empty is `x IS NULL OR x = ''`, not_empty its exact complement. Same rows.
    expect(valuesChip("C", [null], "include").filter).toMatchObject({ op: "is_empty", value: null });
    expect(valuesChip("C", [null], "exclude").filter).toMatchObject({ op: "not_empty", value: null });
    // and no real entry of that listing is the empty string, so the list never needs an eq ""
    expect(JSON.stringify(valuesChip("C", [null, "a"], "include").filter)).not.toContain('"value":""');
  });

  it("ordered_values_are_typed_by_the_column", () => {
    expect(orderedValue("BOOLEAN", "true")).toBe(true);
    expect(orderedValue("BOOLEAN", "false")).toBe(false);
    expect(orderedValue("BIGINT", "-42")).toBe(-42);
    expect(orderedValue("UBIGINT", "42")).toBe(42);
    expect(orderedValue("UBIGINT", "-1")).toBe("-1"); // left for the engine to refuse by name
    expect(orderedValue("BIGINT", "12x")).toBe("12x");
    expect(orderedValue("BIGINT", "9007199254740993")).toBe("9007199254740993"); // not exact as a JS number
    expect(orderedValue("TIMESTAMP", "2026-01-05T03:00:00Z")).toBe("2026-01-05T03:00:00Z");
    expect(orderedValue(null, "10")).toBe("10"); // untyped: text compare, as the user typed it
  });

  it("mark_chip_is_in_with_one_value_or_nothing", () => {
    expect(markChip(["suspicious"])!.filter).toEqual({ column: "_mark", op: "in", value: ["suspicious"], case_sensitive: false });
    expect(markChip(["none"])!.filter).toMatchObject({ value: ["none"] });
    expect(markChip([])).toBeNull();
    expect(markChip(["suspicious", "none"])).toBeNull();
    expect(markSelection([markChip(["none"])!])).toEqual(["none"]);
  });

  it("upsert_remove_toggle", () => {
    const a = filterRowChip("A", "1");
    const b = filterRowChip("B", "2");
    const a2 = filterRowChip("A", "3");
    const list = upsertChip(upsertChip([], a), b);
    expect(upsertChip(list, a2).map((c) => c.filter)).toEqual([a2.filter, b.filter]);
    expect(removeChip(list, a.id)).toEqual([b]);
    const off = toggleChip(list, a.id);
    expect(off[0].enabled).toBe(false);
    expect(toggleChip(off, a.id)[0].enabled).toBe(true);
    expect(list[0].enabled).toBe(true); // never mutated
    const _typed: Chip[] = list;
    void _typed;
  });
});
