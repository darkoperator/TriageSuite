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
});
