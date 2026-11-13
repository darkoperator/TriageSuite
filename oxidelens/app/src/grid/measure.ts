// Text measurement for best fit, behind one function so tests can replace it (jsdom has no canvas). It measures with a
// canvas 2D context in the cell font (JetBrains Mono at the grid's size) or the header font (Inter 13 px); with no
// canvas it estimates from the character count.
let ctx: CanvasRenderingContext2D | null | undefined;

function context(): CanvasRenderingContext2D | null {
  if (ctx === undefined) {
    try {
      ctx = document.createElement("canvas").getContext("2d");
    } catch {
      ctx = null;
    }
  }
  return ctx;
}

/** Pixels `text` takes in the cell font (`size` px) or the header font. */
export function measureText(text: string, font: "cell" | "ui", size: number): number {
  const px = font === "cell" ? size : 13;
  const c = context();
  if (!c) return text.length * px * 0.6;
  c.font = font === "cell" ? `${px}px "JetBrains Mono Variable", "JetBrains Mono", monospace` : `${px}px "Inter Variable", Inter, sans-serif`;
  return c.measureText(text).width;
}
