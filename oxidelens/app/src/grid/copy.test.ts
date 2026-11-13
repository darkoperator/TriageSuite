import { describe, expect, it } from "vitest";
import { rowsAsTsv } from "./copy";

/** A reader of what Excel and LibreOffice accept: tab-separated, a quoted field may hold tabs, newlines and "". */
function parseTsv(text: string): string[][] {
  const rows: string[][] = [[]];
  let field = "";
  let quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') {
        field += '"';
        i++;
      } else if (c === '"') quoted = false;
      else field += c;
    } else if (c === '"' && field === "") quoted = true;
    else if (c === "\t") {
      rows[rows.length - 1].push(field);
      field = "";
    } else if (c === "\n") {
      rows[rows.length - 1].push(field);
      field = "";
      rows.push([]);
    } else field += c;
  }
  rows[rows.length - 1].push(field);
  return rows;
}

describe("rowsAsTsv", () => {
  it("quotes_fields_with_tab_newline_or_quote_and_round_trips", () => {
    const headers = ["a", 'b"quote', "c\td"];
    const rows = [
      ["plain", "has\ttab", "two\nlines"],
      ['say "hi"', "cr\rhere", ""],
      ["=HYPERLINK(\"http://x\")", "report‮gpj.exe", "ring\u0007ring"],
    ];
    const text = rowsAsTsv(headers, rows);
    expect(text.split("\n")[1].startsWith('plain\t"has\ttab"\t"two')).toBe(true);
    expect(text).toContain('"say ""hi"""');
    expect(parseTsv(text)).toEqual([headers, ...rows]);
  });

  it("leaves_ordinary_fields_alone", () => {
    expect(rowsAsTsv(["a", "b"], [["1", "x y"]])).toBe("a\tb\n1\tx y");
  });
});
