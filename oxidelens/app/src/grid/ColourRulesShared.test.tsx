// Two grids on one relation (a dataset tab and a group-scoped tab) share the shell's settings. The provider below keeps them
// the way the Shell does: an edit is shown at once, written through `settings_update`, and a rejected write falls back
// to the last confirmed settings. The grids derive their rules from those settings, never from a copy of their own.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useRef, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { caseOpen, caseSummary, settingsGet, settingsUpdate } from "../api";
import type { ColourRule } from "../contract/generated/ColourRule";
import type { PageOptions } from "../contract/generated/PageOptions";
import type { Settings } from "../contract/generated/Settings";
import { ShellCtx, type ShellContext } from "../shell/context";
import type { Tab } from "../shell/tabs";
import { callsOf, overrideCommand } from "../test/backend";
import { leaf } from "./chips";
import Grid from "./Grid";

vi.mock("./measure", () => ({ measureText: vi.fn(() => 50) }));

beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 440 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
});
afterEach(() => vi.restoreAllMocks());

const first: Tab = { id: "dataset:mock_hayabusa", kind: "dataset", relation: "mock_hayabusa" };
const second: Tab = { id: "scoped:two", kind: "dataset", relation: "mock_hayabusa", title: "scoped" };

const rule = (id: string, over: Partial<ColourRule> = {}): ColourRule => ({
  id, name: id, enabled: true, colour: "blue", scope: "row", filter: leaf("Level", "eq", "crit"), ...over,
});

function TwoGrids({ base, onError }: { base: ShellContext; onError: (e: unknown) => void }) {
  const [settings, setSettings] = useState<Settings>(base.settings);
  const confirmed = useRef<Settings>(base.settings);
  const latest = useRef<Settings>(base.settings);
  const saveColourRules = (relation: string, rules: ColourRule[] | null) => {
    const colour_rules = { ...latest.current.colour_rules };
    if (rules === null) delete colour_rules[relation];
    else colour_rules[relation] = rules;
    const next = { ...latest.current, colour_rules };
    latest.current = next;
    setSettings(next);
    settingsUpdate(next).then(
      (r) => {
        confirmed.current = r.settings;
        latest.current = r.settings;
        setSettings(r.settings);
      },
      (e: unknown) => {
        onError(e);
        latest.current = confirmed.current;
        setSettings(confirmed.current);
      },
    );
  };
  return (
    <ShellCtx.Provider value={{ ...base, settings, saveColourRules, setGridStatus: () => {} }}>
      <div data-testid="g1"><Grid tab={first} active scrollOffset={0} onScroll={() => {}} /></div>
      <div data-testid="g2"><Grid tab={second} active scrollOffset={0} onScroll={() => {}} /></div>
    </ShellCtx.Provider>
  );
}

async function mountTwo(saved: ColourRule[] | null) {
  await caseOpen("/cases/x.duckdb");
  const summary = await caseSummary("t");
  const base = (await settingsGet()).settings;
  const onError = vi.fn();
  const ctx: ShellContext = {
    summary, settings: saved ? { ...base, colour_rules: { mock_hayabusa: saved } } : base, change: { generation: 0, kinds: [] },
    openDataset: () => {}, openScoped: () => {}, setGridStatus: () => {}, setRightPane: () => {}, rightPane: null,
    scroll: { get: () => 0, set: () => {} }, reportError: onError, saveLayout: vi.fn(), setHistogram: vi.fn(), saveColourRules: vi.fn(),
  };
  render(<TwoGrids base={ctx} onError={onError} />);
  const g = (id: string) => within(screen.getByTestId(id));
  await waitFor(() => expect(screen.getAllByRole("columnheader").length).toBeGreaterThan(2));
  const open = async (user: ReturnType<typeof userEvent.setup>, id: string) => {
    await user.click(g(id).getByRole("button", { name: /^Colours/ }));
    return within(await g(id).findByRole("dialog", { name: "Colour rules" }));
  };
  return { g, open, onError, user: userEvent.setup() };
}

const tintedIn = (id: string) =>
  [...screen.getByTestId(id).querySelectorAll<HTMLElement>("[data-row-index]")].filter((r) => /rule-row/.test(r.className));
const lastRules = (): ColourRule[] =>
  (callsOf("grid_page").filter((c) => !/:(validate|search)$/.test(String(c.args!.slot))).at(-1)!.args!.options as PageOptions).rules;

describe("two grids on one relation", () => {
  it("an_edit_in_one_shows_in_the_other_and_the_second_edit_keeps_the_first", async () => {
    const { open, user } = await mountTwo([rule("a", { name: "First", colour: "teal" })]);
    await waitFor(() => expect(tintedIn("g2").length).toBeGreaterThan(0));
    expect(tintedIn("g2")[0].className).toContain("rule-row-teal");

    const d1 = await open(user, "g1");
    await user.selectOptions(d1.getByRole("combobox", { name: "Colour of First" }), "green");
    await waitFor(() => expect(tintedIn("g2")[0].className).toContain("rule-row-green"));

    // the second grid edits the list it now holds, which has the first's change
    const d2 = await open(user, "g2");
    expect((d2.getByRole("combobox", { name: "Colour of First" }) as HTMLSelectElement).value).toBe("green");
    await user.selectOptions(d2.getByRole("combobox", { name: "Scope of First" }), "cell");
    await waitFor(() => expect(lastRules()[0]).toMatchObject({ colour: "green", scope: "cell" }));
    await waitFor(async () => {
      const saved = (await settingsGet()).settings.colour_rules.mock_hayabusa;
      expect(saved).toHaveLength(1);
      expect(saved[0]).toMatchObject({ colour: "green", scope: "cell" });
    });
    expect((d1.getByRole("combobox", { name: "Scope of First" }) as HTMLSelectElement).value).toBe("cell");
  });

  it("two_grids_without_a_saved_entry_agree_on_the_defaults", async () => {
    const { open, user } = await mountTwo(null);
    await waitFor(() => expect(tintedIn("g1").length).toBeGreaterThan(0));
    await waitFor(() => expect(tintedIn("g2").length).toBeGreaterThan(0));
    const d1 = await open(user, "g1");
    await user.click(d1.getByRole("checkbox", { name: "Hayabusa level: critical enabled" }));
    // the edit stored the whole list; the other grid follows it and no longer counts the defaults
    await waitFor(() => expect(screen.getByTestId("g2").textContent).not.toContain("Colours 4"));
    const d2 = await open(user, "g2");
    expect(d2.getAllByRole("listitem")).toHaveLength(4);
    expect((d2.getByRole("checkbox", { name: "Hayabusa level: critical enabled" }) as HTMLInputElement).checked).toBe(false);
  });

  it("a_rejected_settings_write_reverts_the_shown_list", async () => {
    const { open, user, onError } = await mountTwo([rule("a", { name: "First", colour: "teal" })]);
    await waitFor(() => expect(tintedIn("g1").length).toBeGreaterThan(0));
    overrideCommand("settings_update", () => Promise.reject({ code: "invalid", message: "disk full" }));
    const d1 = await open(user, "g1");
    await user.selectOptions(d1.getByRole("combobox", { name: "Colour of First" }), "green");
    await waitFor(() => expect(onError).toHaveBeenCalled());
    await waitFor(() => expect((d1.getByRole("combobox", { name: "Colour of First" }) as HTMLSelectElement).value).toBe("teal"));
    await waitFor(() => expect(tintedIn("g2")[0].className).toContain("rule-row-teal"));
  });
});
