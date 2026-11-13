// Copying. What is copied is the stored text (`GridCell.text`, which is UTC for a timestamp), never the display text,
// and never the escaped text `displaySafe` makes. The clipboard is behind `writeClipboard` so tests can stand in for it.
import type { GridCell } from "../contract/generated/GridCell";

export async function writeClipboard(text: string): Promise<void> {
  await navigator.clipboard.writeText(text);
}

/** One cell's stored text; an empty cell is the empty string. */
export const cellText = (cell: GridCell): string => cell.text ?? "";

/**
 * One TSV field as Excel and LibreOffice read it: a field holding a tab, CR, LF or `"` is wrapped in `"` with the
 * inner `"` doubled; any other field is written as it is.
 */
export function tsvField(s: string): string {
  return /[\t\r\n"]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

/** Header line and one line per row, tab-separated and quoted as above. `rows` holds the stored text. */
export function rowsAsTsv(headers: readonly string[], rows: ReadonlyArray<readonly string[]>): string {
  return [headers, ...rows].map((r) => r.map(tsvField).join("\t")).join("\n");
}
