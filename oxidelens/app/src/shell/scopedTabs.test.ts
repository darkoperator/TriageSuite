import { beforeEach, describe, expect, it } from "vitest";
import { groupChip, groupChips } from "../grid/chips";
import { initialTabs, loadTabs, openDataset, openScoped, saveTabs, type Tab } from "./tabs";

const seeded = (id: string): Tab => ({
  id, kind: "dataset", relation: "mock_events", title: "mock_events · Security",
  seed: { by: [], state: { relation: "mock_events", chips: [], search: { text: "", regex: false, case_sensitive: false, mode: "filter" }, hosts: [], sort: [], columns: null } },
});

describe("tabs opened from a group", () => {
  beforeEach(() => localStorage.clear());

  it("opens once per id, and is restored in order with its seed; an invalid seed is dropped", () => {
    let s = openDataset(initialTabs(), "mock_events");
    s = openScoped(s, seeded("scoped:a"));
    s = openScoped(s, seeded("scoped:a"));
    expect(s.tabs.map((t) => t.id)).toEqual(["overview", "dataset:mock_events", "scoped:a"]);
    saveTabs("c1", s, {});
    const back = loadTabs("c1", ["mock_events"]);
    expect(back.state.tabs.map((t) => t.id)).toEqual(["overview", "dataset:mock_events", "scoped:a"]);
    expect(back.state.active).toBe("scoped:a");
    expect(back.state.tabs[2].seed).toEqual(seeded("scoped:a").seed);
    expect(back.state.tabs[2].title).toBe("mock_events · Security");

    // A seed that is not valid (a chip id outside the alphabet, a relation that no longer exists) is dropped, the rest kept.
    const bad = seeded("scoped:b");
    bad.seed!.state.chips = [{ id: "bad id!", label: "x", enabled: true, origin: "group", filter: { column: "a", op: "eq", value: "b", case_sensitive: true } }];
    const gone = { ...seeded("scoped:c"), relation: "nope" };
    localStorage.setItem(
      "oxl.tabs.v1.c2",
      JSON.stringify({ v: 1, active: "scoped:a", scroll: {}, tabs: [{ relation: "mock_events", id: "scoped:b", title: "t", seed: bad.seed }, { relation: gone.relation, id: "scoped:c", title: "t", seed: gone.seed }, { relation: "mock_events", id: "scoped:a", title: "t", seed: seeded("scoped:a").seed }] }),
    );
    expect(loadTabs("c2", ["mock_events"]).state.tabs.map((t) => t.id)).toEqual(["overview", "scoped:a"]);
  });
});

describe("restoring a seed", () => {
  const withChip = (id: string, chip: unknown) => {
    const t = seeded(id);
    return { relation: "mock_events", id, title: "t", seed: { ...t.seed, state: { ...t.seed!.state, chips: [chip] } } };
  };
  const good = { id: "g1", label: "x", enabled: true, origin: "group", filter: { column: "a", op: "eq", value: "b", case_sensitive: true } };
  const leaf = (over: object) => ({ ...good, filter: { ...good.filter, ...over } });

  it("drops a seed with an unknown operator, origin or value shape, and loads the others", () => {
    const cases: [string, unknown][] = [
      ["bad-op", leaf({ op: "bogus" })],
      ["bad-origin", { ...good, origin: "bogus" }],
      ["in-not-list", leaf({ op: "in", value: "a" })],
      ["between-three", leaf({ op: "between", value: [1, 2, 3] })],
      ["empty-with-value", leaf({ op: "is_empty", value: "x" })],
      ["eq-number", leaf({ op: "eq", value: 5 })],
      ["mark-contains", leaf({ column: "_mark", op: "contains", value: "suspicious" })],
      ["mark-bogus", leaf({ column: "_mark", op: "eq", value: "bogus" })],
      ["mark-in-bogus", leaf({ column: "_mark", op: "in", value: ["none", "bogus"] })],
    ];
    localStorage.setItem(
      "oxl.tabs.v1.c3",
      JSON.stringify({
        v: 1, active: "scoped:ok", scroll: {},
        tabs: [
          ...cases.map(([id, chip]) => withChip(`scoped:${id}`, chip)),
          withChip("scoped:ok", leaf({ op: "between", value: [1, "z"] })),
          withChip("scoped:okmark", leaf({ column: "_mark", op: "eq", value: "none" })),
          withChip("scoped:ok2",{ ...good, id: "g2", filter: { not: leaf({ op: "is_empty", value: null }).filter } }),
        ],
      }),
    );
    const back = loadTabs("c3", ["mock_events"]);
    expect(back.state.tabs.map((t) => t.id)).toEqual(["overview", "scoped:ok", "scoped:okmark", "scoped:ok2"]);
    expect(back.state.active).toBe("scoped:ok");
  });

  it("builds the seed from validated fields only", () => {
    const t = withChip("scoped:x", leaf({}));
    (t.seed.state as Record<string, unknown>).extra = "junk";
    (t.seed as Record<string, unknown>).other = 1;
    localStorage.setItem("oxl.tabs.v1.c4", JSON.stringify({ v: 1, active: "overview", scroll: {}, tabs: [t] }));
    const seed = loadTabs("c4", ["mock_events"]).state.tabs[1].seed as unknown as Record<string, unknown>;
    expect(seed.other).toBeUndefined();
    expect((seed.state as Record<string, unknown>).extra).toBeUndefined();
  });
});

describe("group chips", () => {
  it("select exactly the group: eq on the whole key, is_empty for none, and _mark by the engine's spelling", () => {
    expect(groupChip("Channel", "Security", new Set())).toMatchObject({ origin: "group", filter: { column: "Channel", op: "eq", value: "Security", case_sensitive: true } });
    expect(groupChip("Channel", null, new Set()).filter).toMatchObject({ op: "is_empty" });
    expect(groupChip("_mark", "suspicious", new Set()).filter).toMatchObject({ column: "_mark", op: "eq", value: "suspicious" });
    expect(groupChip("_mark", null, new Set()).filter).toMatchObject({ column: "_mark", op: "eq", value: "none" });
  });

  it("have ids of the allowed alphabet, unique even for a repeated column, and a label within the cap", () => {
    const hostile = "<img src=x onerror=alert(1)>‮" + "y".repeat(5000);
    const [a, b] = groupChips(["Channel", "Channel"], [hostile, "x"], []);
    expect(a.id).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
    expect(b.id).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
    expect(a.id).not.toBe(b.id);
    expect([...a.label].length).toBeLessThanOrEqual(200);
    expect(a.label).not.toContain("‮");
    const [c] = groupChips(["Channel"], ["x"], [a]);
    expect(c.id).not.toBe(a.id);
  });
});
