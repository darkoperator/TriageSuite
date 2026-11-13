// The status bar's zone statement: the offset comes from the engine's row view, never from the front end.
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as dialogs from "../dialogs";
import { mockCall } from "../mock/backend";
import { callsOf, overrideCommand } from "../test/backend";
import { rowViewFirst } from "../test/fixtures";
import Shell from "./Shell";

vi.mock("../dialogs", () => ({
  pickNewCasePath: vi.fn(),
  pickCaseFile: vi.fn(),
  pickRunFolder: vi.fn(),
  pickCsvFile: vi.fn(),
}));

afterEach(() => vi.restoreAllMocks());
void dialogs;

async function openCase() {
  const user = userEvent.setup();
  render(<Shell />);
  await user.click(await screen.findByRole("button", { name: /^\/cases\/acme\.case\.duckdb$/ }));
  await screen.findByRole("tab", { name: "Overview" });
}

const zoneText = () => document.querySelector(".status-zone")?.textContent ?? "";
const setZone = (zone: string) => act(() => mockCall("display_zone_set", { zone }));

/** The recorded row view with its timestamp fields as New York shows them (a stored 02:13:58Z is 21:13:58 the evening before). */
function newYorkRow() {
  const v = rowViewFirst();
  v.zone = "America/New_York";
  for (const f of v.fields) if (f.display !== null) f.display = "2026-01-04 21:13:58.2040000 -05:00";
  return v;
}

describe("status bar zone", () => {
  it("status_bar_states_zone_and_utc_storage", async () => {
    overrideCommand("row_view", () => newYorkRow());
    await openCase();
    // UTC: no offset is stated, and nothing is asked for one
    expect(zoneText()).toBe("TZ UTC · storage and export stay UTC");
    expect(callsOf("row_view")).toHaveLength(0);

    await setZone("America/New_York");
    await waitFor(() => expect(zoneText()).toBe("TZ America/New_York (UTC−05:00 on 2026-01-04) · storage and export stay UTC"));
    expect(callsOf("row_view").every((c) => (c.args as { zone: string }).zone === "America/New_York")).toBe(true);

    await setZone("UTC");
    await waitFor(() => expect(zoneText()).toBe("TZ UTC · storage and export stay UTC"));
  });

  it("states_the_zone_without_an_offset_when_no_row_gives_one", async () => {
    // no field has a converted display text: nothing to read an offset from
    overrideCommand("row_view", () => {
      const v = rowViewFirst();
      for (const f of v.fields) f.display = null;
      return v;
    });
    await openCase();
    await setZone("Asia/Tokyo");
    await waitFor(() => expect(callsOf("row_view").length).toBeGreaterThan(0));
    expect(zoneText()).toBe("TZ Asia/Tokyo · storage and export stay UTC");
  });
});
