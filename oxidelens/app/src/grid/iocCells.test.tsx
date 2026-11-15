import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { GridCell } from "../contract/generated/GridCell";
import { CellText } from "./GridBody";

const cell = (over: Partial<GridCell>): GridCell => ({ text: "203.0.113.7", display: null, failed: false, clipped: false, ioc: false, ...over });

describe("IOC cells", () => {
  it("ioc_cells_are_underlined", () => {
    const on = render(<CellText cell={cell({ ioc: true })} />).container.querySelector(".cell-text");
    expect(on).toHaveClass("cell-ioc");
    const off = render(<CellText cell={cell({})} />).container.querySelector(".cell-text");
    expect(off).not.toHaveClass("cell-ioc");
    // A flagged cell that is also clipped or failed keeps both signs.
    const both = render(<CellText cell={cell({ ioc: true, clipped: true })} />).container.querySelector(".cell-text");
    expect(both).toHaveClass("cell-ioc");
    expect(both).toHaveAttribute("data-clipped", "true");
    expect(render(<CellText cell={cell({ ioc: true, failed: true })} />).container.querySelector(".cell-text")).toHaveClass("cell-ioc", "cell-failed");
  });

  it("a failed cell that is also an IOC keeps the accent underline", () => {
    const css = readFileSync(join(process.cwd(), "src", "grid", "grid.css"), "utf-8").replace(/\/\*[\s\S]*?\*\//g, "");
    const rules = [...css.matchAll(/([^{}]+)\{([^}]*)\}/g)].map((m) => ({ sel: m[1].trim(), body: m[2] }));
    const at = (pred: (sel: string) => boolean) => rules.findIndex((r) => pred(r.sel));
    const failed = at((s) => s === ".cell-failed");
    const both = at((s) => /^\.cell-ioc\.cell-failed$|^\.cell-failed\.cell-ioc$/.test(s));
    expect(both).toBeGreaterThan(failed); // a later rule of equal or higher specificity wins
    expect(rules[both].body).toMatch(/text-decoration-color:\s*var\(--accent\)/);
  });
});
