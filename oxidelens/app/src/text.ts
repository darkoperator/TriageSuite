// Untrusted text (cells, column and relation names, host names, chip labels, view names, paths, error messages) for
// display on one line. Every control character except tab (general category Cc) and every format character (Cf: bidi
// overrides and isolates, zero-width characters, the BOM, tag characters) becomes `\u{XXXX}`, so a value cannot
// reorder what is shown, hide characters or forge an extra line. Markup is not touched: React renders it as text.
//
// Copy, export and the value viewer work on the original string, never on this one.

// `for..of` and the `u` flag both walk code points, so astral format characters (U+E0001 tags) are seen whole.
const UNSAFE = /[\p{Cc}\p{Cf}]/u;

function escape(ch: string): string {
  return "\\u{" + ch.codePointAt(0)!.toString(16).toUpperCase().padStart(4, "0") + "}";
}

export function displaySafe(s: string): string {
  let out = "";
  for (const ch of s) out += ch !== "\t" && UNSAFE.test(ch) ? escape(ch) : ch;
  return out;
}

/**
 * [`displaySafe`] for multi-line text (a note, a long value): line breaks are kept (render with
 * `white-space: pre-wrap`), every other control or format character is escaped.
 */
export function displaySafeMultiline(s: string): string {
  return s.split(/\r?\n/).map(displaySafe).join("\n");
}
