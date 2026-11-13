import { readFileSync } from "node:fs";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as dialogs from "../dialogs";
import { mockCall } from "../mock/backend";
import { callsOf, overrideCommand } from "../test/backend";
import Shell from "../shell/Shell";

vi.mock("../dialogs", () => ({
  pickNewCasePath: vi.fn(),
  pickCaseFile: vi.fn(),
  pickRunFolder: vi.fn(),
  pickCsvFile: vi.fn(),
}));

const root = document.documentElement;
beforeEach(() => {
  localStorage.clear();
  root.setAttribute("data-theme", "dark");
  root.style.removeProperty("--row-h");
  root.style.removeProperty("--cell-size");
  vi.mocked(dialogs.pickCsvFile).mockReset();
  // jsdom has no layout; the grid's scroller needs a size to draw rows.
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 440 });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
});

/** Holds the first `settings_update` until `release()`; later ones go straight to the mock backend. */
function holdFirstWrite() {
  let release!: () => void;
  const gate = new Promise<void>((r) => (release = r));
  let first = true;
  overrideCommand("settings_update", async (args) => {
    if (first) {
      first = false;
      await gate;
    }
    return mockCall("settings_update", args);
  });
  return release;
}

async function openSettings() {
  const user = userEvent.setup();
  render(<Shell />);
  await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
  await screen.findByRole("tab", { name: "Overview" });
  const gear = screen.getByRole("button", { name: "Settings" });
  await user.click(gear);
  const dialog = await screen.findByRole("dialog", { name: "Settings" });
  return { user, gear, dialog };
}

const lastSent = () => {
  const calls = callsOf("settings_update");
  return (calls[calls.length - 1].args as { settings: Record<string, unknown> }).settings;
};

describe("settings dialog", () => {
  it("theme_toggle_sets_data_theme_and_persists", async () => {
    const { user } = await openSettings();
    await user.click(screen.getByRole("radio", { name: "Light" }));
    expect(root.getAttribute("data-theme")).toBe("light");
    await waitFor(() => expect(callsOf("settings_update")).toHaveLength(1));
    expect(lastSent().theme).toBe("light");
    await user.click(screen.getByRole("radio", { name: "Dark" }));
    expect(root.getAttribute("data-theme")).toBe("dark");
    await waitFor(() => expect(lastSent().theme).toBe("dark"));
  });

  it("font_size_scales_rows", async () => {
    const { user, dialog } = await openSettings();
    // A dataset grid behind the dialog: its virtual rows follow the font size, not a fixed 22 px.
    await user.click(within(screen.getByRole("tabpanel", { name: "Overview" })).getByRole("button", { name: "mock_events" }));
    const metrics = async (rowH: number) => {
      const el = await waitFor(() => {
        const found = document.querySelector<HTMLElement>(".grid-body");
        expect(found).not.toBeNull();
        expect(found!.querySelectorAll(".grid-row").length).toBeGreaterThan(1);
        expect(parseFloat(found!.style.height) % rowH).toBe(0);
        return found!;
      });
      const second = el.querySelectorAll<HTMLElement>(".grid-row")[1];
      return { total: parseFloat(el.style.height), secondTop: second.style.transform, rowHeight: second.style.height };
    };
    const size = screen.getByRole("combobox", { name: "Cell font size" });
    expect(root.style.getPropertyValue("--row-h")).toBe("22px");
    const at12 = await metrics(22);
    expect(at12.rowHeight).toBe("22px");
    expect(at12.secondTop).toBe("translateY(22px)");

    await user.selectOptions(size, "16");
    expect(root.style.getPropertyValue("--cell-size")).toBe("16px");
    expect(root.style.getPropertyValue("--row-h")).toBe("26px");
    await waitFor(() => expect(lastSent().font_size).toBe(16));
    await waitFor(() => expect(document.querySelector<HTMLElement>(".grid-row")!.style.height).toBe("26px"));
    const at16 = await metrics(26);
    expect(at16.secondTop).toBe("translateY(26px)");
    expect(at16.total * 22).toBe(at12.total * 26);

    await user.selectOptions(size, "10");
    expect(root.style.getPropertyValue("--row-h")).toBe("20px");
    expect(dialog).toBeInTheDocument();
  });

  it("analyst_name_is_saved_and_rejected_names_show_the_reason", async () => {
    const { user, dialog } = await openSettings();
    const field = screen.getByRole("textbox", { name: "Analyst name" });
    expect(field).toHaveValue("analyst01");
    expect(dialog).toHaveTextContent("analyst:analyst01");

    await user.clear(field);
    await user.type(field, "jdoe");
    await user.click(screen.getByRole("button", { name: "Save name" }));
    await waitFor(() => expect(lastSent().analyst).toBe("jdoe"));
    await waitFor(() => expect(dialog).toHaveTextContent("analyst:jdoe"));
    expect(screen.queryByRole("alert")).toBeNull();

    // A refusal is shown next to the field, through displaySafe, and the file keeps the old name.
    overrideCommand("settings_update", () => {
      throw { code: "invalid", message: "analyst name is not valid‮" };
    });
    await user.clear(field);
    await user.type(field, "bad name");
    await user.click(screen.getByRole("button", { name: "Save name" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("analyst name is not valid\\u{202E}");
    expect(dialog).toHaveTextContent("analyst:jdoe");
  });

  it("a_refused_name_does_not_drop_a_plain_change_queued_beside_it", async () => {
    const { user } = await openSettings();
    const release = holdFirstWrite();
    await user.click(screen.getByRole("radio", { name: "Light" })); // write 1, held
    const field = screen.getByRole("textbox", { name: "Analyst name" });
    await user.clear(field);
    await user.type(field, "bad name");
    await user.click(screen.getByRole("button", { name: "Save name" })); // queued behind it, alone
    await user.click(screen.getByRole("checkbox", { name: "Show the histogram" })); // a plain entry after it
    release();
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("an analyst name is 1 to 64 characters");
    await waitFor(() => expect(callsOf("settings_update")).toHaveLength(3));
    const sent = callsOf("settings_update").map((c) => (c.args as { settings: { theme: string; histogram: boolean; analyst: string | null } }).settings);
    expect(sent[0].theme).toBe("light");
    expect(sent[1].analyst).toBe("bad name");
    expect(sent[2]).toMatchObject({ theme: "light", histogram: false, analyst: "analyst01" });
    expect(screen.queryByText(/^invalid:/)).toBeNull(); // not also reported as a global error
  });

  it("queued_name_saves_each_get_their_own_writes_outcome", async () => {
    const { user, dialog } = await openSettings();
    const release = holdFirstWrite();
    await user.click(screen.getByRole("radio", { name: "Light" }));
    const field = screen.getByRole("textbox", { name: "Analyst name" });
    await user.clear(field);
    await user.type(field, "alice");
    await user.click(screen.getByRole("button", { name: "Save name" }));
    await user.clear(field);
    await user.type(field, "bad name");
    await user.click(screen.getByRole("button", { name: "Save name" }));
    release();
    await screen.findByRole("alert");
    await waitFor(() => expect(callsOf("settings_update")).toHaveLength(3));
    expect(dialog).toHaveTextContent("analyst:alice"); // alice's write stood; the refused one dropped only itself
    const names = callsOf("settings_update").map((c) => (c.args as { settings: { analyst: string | null } }).settings.analyst);
    expect(names).toEqual(["analyst01", "alice", "bad name"]);
  });

  it("only_the_latest_name_save_decides_the_alert", async () => {
    const { user, dialog } = await openSettings();
    const release = holdFirstWrite();
    await user.click(screen.getByRole("radio", { name: "Light" }));
    const field = screen.getByRole("textbox", { name: "Analyst name" });
    await user.clear(field);
    await user.type(field, "bad name");
    await user.click(screen.getByRole("button", { name: "Save name" }));
    await user.clear(field);
    await user.type(field, "carol");
    await user.click(screen.getByRole("button", { name: "Save name" }));
    release();
    await waitFor(() => expect(dialog).toHaveTextContent("analyst:carol"));
    await waitFor(() => expect(callsOf("settings_update")).toHaveLength(3));
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(field).not.toHaveAttribute("aria-invalid", "true");
  });

  it("the_mock_refuses_a_name_with_a_trailing_newline", async () => {
    const { mockCall: call } = await import("../mock/backend");
    const s = (await call("settings_get", {})) as { settings: Record<string, unknown> };
    await expect(call("settings_update", { settings: { ...s.settings, analyst: "alice\n" } })).rejects.toMatchObject({ code: "invalid" });
  });

  it("the_name_is_sent_as_typed_and_the_backends_reason_is_shown", async () => {
    const { user } = await openSettings();
    const field = screen.getByRole("textbox", { name: "Analyst name" });
    for (const typed of [" jdoe ", "a".repeat(65)]) {
      await user.clear(field);
      await user.type(field, typed);
      await user.click(screen.getByRole("button", { name: "Save name" }));
      await waitFor(() => expect(lastSent().analyst).toBe(typed));
      expect(await screen.findByRole("alert")).toHaveTextContent("1 to 64 characters");
      await user.clear(field);
      await user.type(field, "ok");
      await user.click(screen.getByRole("button", { name: "Save name" }));
      await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    }
  });

  it("an_empty_analyst_name_clears_it", async () => {
    const { user } = await openSettings();
    await user.clear(screen.getByRole("textbox", { name: "Analyst name" }));
    await user.click(screen.getByRole("button", { name: "Save name" }));
    await waitFor(() => expect(lastSent().analyst).toBeNull());
  });

  it("the_histogram_switch_is_written", async () => {
    const { user } = await openSettings();
    const box = screen.getByRole("checkbox", { name: "Show the histogram" });
    expect(box).toBeChecked();
    await user.click(box);
    await waitFor(() => expect(lastSent().histogram).toBe(false));
  });

  it("forget_recent_cases_writes_an_empty_list_through_the_queue", async () => {
    const { user } = await openSettings();
    await user.click(screen.getByRole("button", { name: "Forget recent cases" }));
    await waitFor(() => expect(lastSent().recent).toEqual([]));
  });

  it("focus_moves_in_returns_to_the_gear_and_escape_closes", async () => {
    const { user, gear, dialog } = await openSettings();
    expect(dialog.contains(document.activeElement)).toBe(true);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
    expect(document.activeElement).toBe(gear);
  });
});

describe("the light theme", () => {
  const css = readFileSync("src/theme/tokens.css", "utf8");
  const block = (sel: string) => {
    const m = css.match(new RegExp(`:root\\[data-theme="${sel}"\\]\\s*\\{([^}]*)\\}`));
    if (!m) throw new Error(`no ${sel} block`);
    const props = new Map<string, string>();
    for (const x of m[1].matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) props.set(x[1], x[2].trim());
    return props;
  };

  it("light_tokens_define_every_dark_token", () => {
    const dark = [...block("dark").keys()].sort();
    const light = [...block("light").keys()].sort();
    expect(dark.length).toBeGreaterThan(30);
    expect(light).toEqual(dark);
  });

  it("accent_and_reserved_colours_hold_in_light", () => {
    const light = block("light");
    const lum = (hex: string) => {
      const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
      return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    };
    const ratio = (a: string, b: string) => {
      const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
      return (x + 0.05) / (y + 0.05);
    };
    for (const name of ["--suspicious", "--malicious", "--agent"]) {
      for (const bg of ["--bg", "--surface", "--card"]) {
        expect(ratio(light.get(name)!, light.get(bg)!), `${name} on ${bg}`).toBeGreaterThanOrEqual(3);
      }
    }
    expect(light.get("--accent")).toBe(block("dark").get("--accent"));
    // The marks (suspicious, malicious) and the agent keep their hue; only lightness changes.
    const hue = (hex: string) => {
      const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
      const max = Math.max(r, g, b), d = max - Math.min(r, g, b);
      const h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
      return (h * 60 + 360) % 360;
    };
    const dark = block("dark");
    for (const name of ["--suspicious", "--malicious", "--agent"]) {
      const diff = Math.abs(hue(light.get(name)!) - hue(dark.get(name)!));
      expect(Math.min(diff, 360 - diff), `${name} hue`).toBeLessThanOrEqual(12);
    }
  });
});
