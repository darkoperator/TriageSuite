import { describe, expect, it } from "vitest";
import { displaySafe, displaySafeMultiline } from "./text";

describe("displaySafe", () => {
  it("escapes the bidi override, the zero-width joiner, BEL and the BOM", () => {
    expect(displaySafe("a‮b")).toBe("a\\u{202E}b");
    expect(displaySafe("a‍b")).toBe("a\\u{200D}b");
    expect(displaySafe("a\u0007b")).toBe("a\\u{0007}b");
    expect(displaySafe("﻿a")).toBe("\\u{FEFF}a");
  });

  it("escapes tag characters whole (they are above the BMP)", () => {
    expect(displaySafe("a\u{E0001}\u{E0041}b")).toBe("a\\u{E0001}\\u{E0041}b");
  });

  it("escapes line breaks and other C0 and C1 controls on one line", () => {
    expect(displaySafe("a\nb\rc")).toBe("a\\u{000A}b\\u{000D}c");
    expect(displaySafe("a\u007Fb\u0085c")).toBe("a\\u{007F}b\\u{0085}c");
  });

  it("leaves tab, markup, a formula and emoji alone", () => {
    expect(displaySafe("a\tb")).toBe("a\tb");
    expect(displaySafe("<img src=x onerror=alert(1)>")).toBe("<img src=x onerror=alert(1)>");
    expect(displaySafe('=HYPERLINK("http://x")')).toBe('=HYPERLINK("http://x")');
    expect(displaySafe("ok \u{1F44D} é")).toBe("ok \u{1F44D} é");
  });

  it("is the identity on a 5,000-character plain value", () => {
    const long = "x".repeat(5000);
    expect(displaySafe(long)).toBe(long);
  });
});

describe("displaySafeMultiline", () => {
  it("keeps line breaks and escapes everything else", () => {
    expect(displaySafeMultiline("a\r\nb\nc‮d\u0007")).toBe("a\nb\nc\\u{202E}d\\u{0007}");
  });
});
