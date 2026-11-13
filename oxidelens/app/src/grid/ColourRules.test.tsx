// Colour rules in the grid: the Hayabusa defaults, first-match order, cell scope, rules from the current filter, saving per
// relation, marks under a tint, and the engine's rule errors. Against the mock backend, which matches rules like the
// engine does (first enabled rule whose filter the row matches) but filters approximately.
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState, type ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { caseOpen, caseSummary, settingsGet } from "../api";
import type { Settings } from "../contract/generated/Settings";
import type { ColourRule } from "../contract/generated/ColourRule";
import type { PageOptions } from "../contract/generated/PageOptions";
import { HOSTILE } from "../mock/generated";
import { mockCall } from "../mock/backend";
import { ShellCtx, type ShellContext } from "../shell/context";
import type { Tab } from "../shell/tabs";
import { callsOf, overrideCommand } from "../test/backend";
import { leaf } from "./chips";
import { readFileSync } from "node:fs";
import Grid from "./Grid";

vi.mock("./measure", () => ({ measureText: vi.fn(() => 50) }));

beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 440 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
});
afterEach(() => vi.restoreAllMocks());
const css = readFileSync("src/grid/grid.css", "utf8");

type Rel = "mock_hayabusa" | "mock_events";
const tabOf = (relation: Rel): Tab => ({ id: `dataset:${relation}`, kind: "dataset", relation });

function Host({ ctx, relation }: { ctx: ShellContext; relation: Rel }) {
  const [status, setStatus] = useState<ReactNode>(null);
  // The grid shows what the shell's settings hold, so a save must change them (the shell applies it at once).
  const [settings, setSettings] = useState<Settings>(ctx.settings);
  const saveColourRules = (rel: string, rules: ColourRule[] | null) => {
    ctx.saveColourRules(rel, rules);
    setSettings((s) => {
      const colour_rules = { ...s.colour_rules };
      if (rules === null) delete colour_rules[rel];
      else colour_rules[rel] = rules;
      return { ...s, colour_rules };
    });
  };
  return (
    <ShellCtx.Provider value={{ ...ctx, settings, saveColourRules, setGridStatus: setStatus }}>
      <div data-testid="status">{status}</div>
      <Grid tab={tabOf(relation)} active scrollOffset={0} onScroll={() => {}} />
    </ShellCtx.Provider>
  );
}

const rule = (id: string, over: Partial<ColourRule> = {}): ColourRule => ({
  id, name: id, enabled: true, colour: "blue", scope: "row", filter: leaf("Level", "eq", "crit"), ...over,
});

async function mount(relation: Rel = "mock_hayabusa", saved?: ColourRule[]) {
  await caseOpen("/cases/x.duckdb");
  const summary = await caseSummary("t");
  const base = (await settingsGet()).settings;
  const settings = saved ? { ...base, colour_rules: { [relation]: saved } } : base;
  const saveColourRules = vi.fn();
  const reportError = vi.fn();
  const ctx: ShellContext = {
    summary, settings, change: { generation: 0, kinds: [] }, openDataset: () => {}, openScoped: () => {}, setGridStatus: () => {},
    setRightPane: () => {}, rightPane: null, scroll: { get: () => 0, set: () => {} }, reportError, saveLayout: vi.fn(), setHistogram: vi.fn(),
    saveColourRules,
  };
  const utils = render(<Host ctx={ctx} relation={relation} />);
  await waitFor(() => expect(screen.getAllByRole("columnheader").length).toBeGreaterThan(1));
  await waitFor(() => expect(dataRows().length).toBeGreaterThan(0));
  return { ...utils, saveColourRules, reportError, user: userEvent.setup() };
}

const dataRows = () => screen.queryAllByRole("row").filter((r) => r.getAttribute("data-row-index") !== null);
const names = () => screen.getAllByRole("columnheader").filter((h) => h.querySelector(".col-name")).map((h) => h.querySelector(".col-name")!.textContent!);
const cellOf = (row: HTMLElement, column: string) => row.querySelectorAll<HTMLElement>("[role='gridcell']")[names().indexOf(column) + 1];
const levelOf = (row: HTMLElement) => cellOf(row, "Level").textContent ?? "";
const rowsWith = (level: string) => dataRows().filter((r) => levelOf(r) === level);
const lastRules = (): ColourRule[] => (callsOf("grid_page").filter((c) => !/:(validate|search)$/.test(String(c.args!.slot))).at(-1)!.args!.options as PageOptions).rules;
const status = () => screen.getByTestId("status").textContent ?? "";

describe("defaults", () => {
  it("hayabusa_defaults_colour_levels", async () => {
    await mount("mock_hayabusa"); // no saved entry: the engine's defaults
    await waitFor(() => expect(rowsWith("crit").length).toBeGreaterThan(0));
    expect(callsOf("default_colour_rules")).toHaveLength(1);
    expect(lastRules().map((r) => r.id)).toEqual(["hayabusa-crit", "hayabusa-high", "hayabusa-med", "hayabusa-low"]);
    // not one page was asked for before the defaults arrived: no flash of untinted rows
    expect(callsOf("grid_page").every((c) => (c.args!.options as PageOptions).rules.length === 4)).toBe(true);
    for (const r of rowsWith("crit")) expect(r.className).toContain("rule-row-red");
    for (const r of rowsWith("high")) expect(r.className).toContain("rule-row-orange");
    for (const r of rowsWith("med")) expect(r.className).toContain("rule-row-amber");
    for (const r of rowsWith("low")) expect(r.className).toContain("rule-row-green");
    for (const r of rowsWith("info")) expect(r.className).not.toMatch(/rule-/);
    expect(status()).toContain("colour rule: Hayabusa level (on)");
  });

  it("a_relation_with_a_saved_empty_list_gets_no_defaults", async () => {
    await mount("mock_hayabusa", []);
    expect(callsOf("default_colour_rules")).toHaveLength(0);
    expect(lastRules()).toEqual([]);
    expect(dataRows().every((r) => !/rule-/.test(r.className))).toBe(true);
    expect(status()).not.toContain("colour rule");
  });
});

describe("order", () => {
  it("first_rule_wins_and_order_changes_it", async () => {
    const { user, saveColourRules } = await mount("mock_hayabusa", [
      rule("a", { name: "First", colour: "teal" }), rule("b", { name: "Second", colour: "blue" }),
    ]);
    await waitFor(() => expect(rowsWith("crit").length).toBeGreaterThan(0));
    for (const r of rowsWith("crit")) expect(r.className).toContain("rule-row-teal");
    expect(status()).toContain("colour rules: 2 on");

    await user.click(screen.getByRole("button", { name: /^Colours/ }));
    const dialog = await screen.findByRole("dialog", { name: "Colour rules" });
    await user.click(within(dialog).getByRole("button", { name: "Move First down" }));
    await waitFor(() => expect(lastRules().map((r) => r.id)).toEqual(["b", "a"]));
    await waitFor(() => expect(rowsWith("crit")[0].className).toContain("rule-row-blue"));
    expect(rowsWith("crit")[0].className).not.toContain("rule-row-teal");
    expect(saveColourRules).toHaveBeenLastCalledWith("mock_hayabusa", expect.arrayContaining([expect.objectContaining({ id: "b" })]));
    expect((saveColourRules.mock.calls.at(-1)![1] as ColourRule[]).map((r) => r.id)).toEqual(["b", "a"]);

    // dragging does the same
    const items = within(dialog).getAllByRole("listitem");
    fireEvent.dragStart(items[1]);
    fireEvent.dragOver(items[0]);
    fireEvent.drop(items[0]);
    await waitFor(() => expect(lastRules().map((r) => r.id)).toEqual(["a", "b"]));

    // a disabled rule never matches: the next one takes over
    await user.click(within(dialog).getByRole("checkbox", { name: "First enabled" }));
    await waitFor(() => expect(rowsWith("crit")[0].className).toContain("rule-row-blue"));
    expect((saveColourRules.mock.calls.at(-1)![1] as ColourRule[])[0].enabled).toBe(false);
  });

  it("editing_a_rule_changes_scope_colour_and_name_and_can_delete", async () => {
    const { user, saveColourRules } = await mount("mock_hayabusa", [rule("a", { name: "First" })]);
    await user.click(screen.getByRole("button", { name: /^Colours/ }));
    const dialog = await screen.findByRole("dialog", { name: "Colour rules" });
    const colours = within(dialog).getByRole("combobox", { name: "Colour of First" });
    expect(within(colours).getAllByRole("option").map((o) => o.textContent)).toEqual(["Orange", "Green", "Blue", "Teal", "Grey"]);
    await user.selectOptions(colours, "green");
    await waitFor(() => expect(lastRules()[0].colour).toBe("green"));
    await user.selectOptions(within(dialog).getByRole("combobox", { name: "Scope of First" }), "cell");
    await waitFor(() => expect(lastRules()[0].scope).toBe("cell"));
    const name = within(dialog).getByRole("textbox", { name: "Name of rule 1" });
    await user.clear(name);
    await user.type(name, "Criticals{Enter}");
    await waitFor(() => expect(lastRules()[0].name).toBe("Criticals"));
    await user.click(within(dialog).getByRole("button", { name: "Delete Criticals" }));
    await waitFor(() => expect(lastRules()).toEqual([]));
    expect(saveColourRules).toHaveBeenLastCalledWith("mock_hayabusa", []);
  });

  it("a_default_red_stays_shown_in_the_picker_but_is_not_offered_for_other_rules", async () => {
    const { user } = await mount("mock_hayabusa");
    await user.click(screen.getByRole("button", { name: /^Colours/ }));
    const dialog = await screen.findByRole("dialog", { name: "Colour rules" });
    const crit = within(dialog).getByRole("combobox", { name: "Colour of Hayabusa level: critical" }) as HTMLSelectElement;
    expect(crit.value).toBe("red");
    expect(within(crit).getAllByRole("option").map((o) => o.textContent)).toEqual(["Red", "Orange", "Green", "Blue", "Teal", "Grey"]);
    const high = within(dialog).getByRole("combobox", { name: "Colour of Hayabusa level: high" });
    expect(within(high).queryByRole("option", { name: "Red" })).toBeNull();
    expect(within(high).queryByRole("option", { name: "Amber" })).toBeNull();
  });
});

describe("scope", () => {
  it("cell_scope_tints_only_named_columns", async () => {
    await mount("mock_hayabusa", [rule("c", { scope: "cell", colour: "teal", filter: { and: [leaf("Level", "eq", "crit"), leaf("Channel", "ne", "zzz")] } })]);
    await waitFor(() => expect(rowsWith("crit").length).toBeGreaterThan(0));
    const row = rowsWith("crit")[0];
    expect(row.className).not.toMatch(/rule-/);
    const tinted = [...row.querySelectorAll<HTMLElement>("[role='gridcell']")].filter((c) => /rule-cell-teal/.test(c.className));
    expect(tinted.map((c) => names()[Number(c.getAttribute("aria-colindex")) - 2]).sort()).toEqual(["Channel", "Level"]);
    expect(cellOf(row, "RuleTitle").className).not.toMatch(/rule-/);
    // the mark cell is never tinted
    expect(row.querySelector(".cell-mark")!.className).not.toMatch(/rule-/);
    // a row the rule does not match has none
    expect(rowsWith("low")[0].querySelectorAll("[class*='rule-']")).toHaveLength(0);
  });
});

describe("rule from the current filter", () => {
  it("rule_from_current_filter", async () => {
    const { user, saveColourRules } = await mount("mock_hayabusa", []);
    await user.type(screen.getByRole("textbox", { name: "Match Level" }), "crit");
    await user.type(screen.getByRole("textbox", { name: "Match Channel" }), "Sec");
    await waitFor(() => expect(callsOf("grid_page").some((c) => (c.args!.state as { chips: unknown[] }).chips.length === 2)).toBe(true), { timeout: 3000 });
    await user.click(screen.getByRole("button", { name: /^Colours/ }));
    const dialog = await screen.findByRole("dialog", { name: "Colour rules" });
    await user.click(within(dialog).getByRole("button", { name: "New rule from the current filter" }));
    await waitFor(() => expect(saveColourRules).toHaveBeenCalled());
    const saved = saveColourRules.mock.calls.at(-1)![1] as ColourRule[];
    expect(saved).toHaveLength(1);
    expect(saved[0]).toMatchObject({ enabled: true, scope: "row", colour: "blue" });
    expect(saved[0].filter).toEqual({ and: [leaf("Level", "contains", "crit"), leaf("Channel", "contains", "Sec")] });
    await waitFor(() => expect(lastRules()).toHaveLength(1));
    expect(within(dialog).getByRole("textbox", { name: "Name of rule 1" })).toBeInTheDocument();
  });

  it("the_button_is_disabled_without_an_enabled_filter", async () => {
    const { user } = await mount("mock_hayabusa", []);
    await user.click(screen.getByRole("button", { name: /^Colours/ }));
    const dialog = await screen.findByRole("dialog", { name: "Colour rules" });
    expect(within(dialog).getByRole("button", { name: "New rule from the current filter" })).toBeDisabled();
  });
});

describe("persistence", () => {
  it("rules_persist_per_relation", async () => {
    const mine = [rule("only", { name: "Only", colour: "grey" })];
    const { user, saveColourRules } = await mount("mock_hayabusa", mine);
    // the saved list is what is sent; the defaults are not asked for
    expect(callsOf("default_colour_rules")).toHaveLength(0);
    expect(lastRules()).toEqual(mine);
    // an edit stores the whole list under this relation
    await user.click(screen.getByRole("button", { name: /^Colours/ }));
    const dialog = await screen.findByRole("dialog", { name: "Colour rules" });
    await user.click(within(dialog).getByRole("checkbox", { name: "Only enabled" }));
    await waitFor(() => expect(saveColourRules).toHaveBeenCalledTimes(1));
    expect(saveColourRules).toHaveBeenCalledWith("mock_hayabusa", [{ ...mine[0], enabled: false }]);
  });

  it("another_relation_asks_for_its_own_defaults", async () => {
    await mount("mock_events"); // nothing saved for it
    await waitFor(() => expect(callsOf("default_colour_rules")).toHaveLength(1));
    expect((callsOf("default_colour_rules")[0].args as { relation: string }).relation).toBe("mock_events");
    expect(lastRules()).toEqual([]); // the engine gives defaults only to a relation with Level and RuleTitle
  });

  it("reset_to_defaults_asks_the_engine_again", async () => {
    const { user, saveColourRules } = await mount("mock_hayabusa", [rule("x")]);
    await user.click(screen.getByRole("button", { name: /^Colours/ }));
    const dialog = await screen.findByRole("dialog", { name: "Colour rules" });
    await user.click(within(dialog).getByRole("button", { name: "Reset to defaults" }));
    await waitFor(() => expect(lastRules().map((r) => r.id)).toEqual(["hayabusa-crit", "hayabusa-high", "hayabusa-med", "hayabusa-low"]));
    // back to "no saved entry": the file forgets the relation, so it follows the defaults again
    expect(saveColourRules).toHaveBeenLastCalledWith("mock_hayabusa", null);
    expect(status()).toContain("colour rule: Hayabusa level (on)");
  });
});

describe("marks and errors", () => {
  it("mark_glyph_stays_visible_under_a_rule", async () => {
    // rule 0 matches every row; mock_events rows with ids 5, 41, 902 are marked suspicious
    await mount("mock_events", [rule("all", { colour: "orange", filter: leaf("_mark", "in", ["suspicious", "none"]) })]);
    await waitFor(() => expect(document.querySelector(".mark-suspicious")).not.toBeNull());
    const glyph = document.querySelector(".mark-suspicious")!;
    expect(glyph.textContent).toBe("●");
    expect(glyph).toHaveAccessibleName("suspicious");
    const row = glyph.closest<HTMLElement>("[role='row']")!;
    expect(row.className).toContain("rule-row-orange"); // the row is tinted...
    expect(glyph.closest(".cell-mark")!.className).not.toMatch(/rule-/); // ...the mark cell is not
    // the CSS keeps it that way: the wash is a background-image on the row, the mark cell has its own opaque background,
    // and the glyph is never given a rule colour
    expect(css).toMatch(/\.grid-row \.cell-mark\s*\{[^}]*background:/);
    expect(css).not.toMatch(/\.mark-suspicious[^{]*\{[^}]*--rule-/);
    expect(css).not.toMatch(/\.rule-row-[a-z]+[^{]*\{[^}]*(?<!-image)\bbackground\s*:/);
  });

  it("rule_errors_are_shown", async () => {
    const hostile = HOSTILE.html;
    const { user } = await mount("mock_hayabusa", [
      rule("ok", { name: "Fine" }),
      rule("bad", { name: hostile, filter: leaf("NoSuchColumn", "eq", "x") }),
    ]);
    await user.click(screen.getByRole("button", { name: /^Colours/ }));
    const dialog = await screen.findByRole("dialog", { name: "Colour rules" });
    const alert = await within(dialog).findByRole("alert");
    expect(alert.textContent).toContain("the relation has no column NoSuchColumn");
    // it sits under the rule it names, and the other rule has none
    const items = within(dialog).getAllByRole("listitem");
    expect(items[1]).toContainElement(alert);
    expect(within(items[0]).queryByRole("alert")).toBeNull();
    // the hostile name is text
    expect(dialog.querySelector("img")).toBeNull();
    expect((within(items[1]).getByRole("textbox", { name: "Name of rule 2" }) as HTMLInputElement).value).toBe(hostile);
  });

  it("an_engine_error_in_a_rule_message_is_shown_as_text", async () => {
    overrideCommand("grid_page", async (args) => {
      const page = (await mockCall("grid_page", args)) as { rule_errors: [number, string][] };
      return { ...page, rule_errors: [[0, HOSTILE.html]] };
    });
    const { user } = await mount("mock_hayabusa", [rule("a")]);
    await user.click(screen.getByRole("button", { name: /^Colours/ }));
    const dialog = await screen.findByRole("dialog", { name: "Colour rules" });
    const alert = await within(dialog).findByRole("alert");
    expect(alert.textContent).toContain("<img");
    expect(dialog.querySelector("img")).toBeNull();
  });
});

describe("the colours button", () => {
  it("shows_how_many_rules_are_on_and_the_dialog_closes_on_escape", async () => {
    const { user } = await mount("mock_hayabusa", [rule("a"), rule("b", { enabled: false })]);
    const button = screen.getByRole("button", { name: /^Colours/ });
    expect(button.textContent).toBe("Colours 1");
    await user.click(button);
    await screen.findByRole("dialog", { name: "Colour rules" });
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Colour rules" })).toBeNull());
    expect(button).toHaveFocus();
  });
});
