import { describe, expect, it } from "vitest";
import * as fx from "./fixtures";

// The recorded answers (src/test/fixtures/*.json, written by `cargo test -p oxl-app --test record_fixtures --
// --ignored`) must still have the shape the generated TypeScript types name; the loaders check it at runtime, so a
// change in the Rust serde shapes fails here instead of hiding behind a cast.
describe("recorded fixtures", () => {
  it("covers every file in the fixtures folder", () => {
    const onDisk = Object.keys(import.meta.glob("./fixtures/*.json"))
      .map((p) => p.replace("./fixtures/", "").replace(".json", ""))
      .sort();
    expect(Object.keys(fx.FIXTURES).sort()).toEqual(onDisk);
    expect(onDisk.length).toBeGreaterThanOrEqual(12);
  });

  it("loads every fixture (its shape is checked by tsc against the generated types: see fixtures.ts)", () => {
    for (const [name, load] of Object.entries(fx.FIXTURES)) expect(load(), name).toBeTruthy();
  });

  it("returns independent copies", () => {
    fx.caseSummary().name = "changed";
    expect(fx.caseSummary().name).toBe("WS01 triage");
  });

  it("holds what the screen tests rely on", () => {
    const summary = fx.caseSummary();
    expect(summary.format).toBe(2);
    expect(summary.hosts).toEqual(["SRV01", "WS01"]);
    const page = fx.gridPageEvents();
    expect(page.cells.length).toBe(page.row_ids.length);
    for (const row of page.cells) expect(row.length).toBe(page.columns.length);
    expect(page.marks).toContain("suspicious");
    expect(fx.gridPageEventsNy().zone).toBe("America/New_York");
    expect(fx.hostZones().map((z) => z.iana)).toEqual(["Etc/UTC", "America/New_York"]);
    expect(fx.rowViewFirst().fields.length).toBe(fx.rowViewFirst().detail.fields.length);
    expect(fx.viewsList().items.length).toBeGreaterThan(0);
    expect(fx.sessions()[0].relations.length).toBeGreaterThan(0);
  });
});
