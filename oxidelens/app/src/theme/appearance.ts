// What the theme and font-size settings do to the page. The row height is one rule, used here for the `--row-h`
// custom property and by the grid's virtualiser, so the drawn rows and the scroll arithmetic cannot disagree.
import type { Settings } from "../contract/generated/Settings";

/** A row is the cell font plus 10 px: 22 px at the default 12 px. */
export const rowHeightFor = (fontSize: number): number => fontSize + 10;

/** Puts the theme on the document root and the font size and row height on it as custom properties. */
export function applyAppearance(s: Pick<Settings, "theme" | "font_size">): void {
  const root = document.documentElement;
  root.setAttribute("data-theme", s.theme);
  root.style.setProperty("--cell-size", `${s.font_size}px`);
  root.style.setProperty("--row-h", `${rowHeightFor(s.font_size)}px`);
}
