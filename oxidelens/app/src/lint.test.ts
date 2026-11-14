// @vitest-environment node
import { fileURLToPath } from "node:url";
import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";

// The guard rails of eslint.config.js, proved by ESLint's own Node API on snippets that break each one.
const appDir = fileURLToPath(new URL("..", import.meta.url));
const eslint = new ESLint({ cwd: appDir });

async function errors(code: string, file = "src/snippet.tsx") {
  const [result] = await eslint.lintText(code, { filePath: `${appDir}/${file}` });
  return result.messages.filter((m) => m.severity === 2);
}

describe("eslint config", () => {
  it("fails dangerouslySetInnerHTML", async () => {
    const found = await errors("export const A = (x: string) => <div dangerouslySetInnerHTML={{ __html: x }} />;\n");
    expect(found.map((m) => m.ruleId)).toEqual(["react/no-danger"]);
  });

  it("fails an innerHTML assignment", async () => {
    const found = await errors("export function f(el: HTMLElement, x: string) {\n  el.innerHTML = x;\n}\n");
    expect(found.map((m) => m.ruleId)).toEqual(["no-restricted-properties"]);
  });

  it("fails new Date(s)", async () => {
    const found = await errors("export const d = (s: string) => new Date(s);\n");
    expect(found.map((m) => m.ruleId)).toEqual(["no-restricted-syntax"]);
  });

  it("fails the other ways in: outerHTML, insertAdjacentHTML, document.write, Date.parse, Date.UTC, Intl.DateTimeFormat", async () => {
    const code = [
      "export function f(el: HTMLElement, x: string) {",
      "  el.outerHTML = x;",
      "  el.insertAdjacentHTML('beforeend', x);",
      "  document.write(x);",
      "  return [Date.parse(x), Date.UTC(1), new Intl.DateTimeFormat('en')];",
      "}",
      "",
    ].join("\n");
    expect((await errors(code)).length).toBe(6);
  });

  it("fails a clickable div with no keyboard path", async () => {
    const found = await errors("export const A = (f: () => void) => <div onClick={f}>x</div>;\n");
    expect(found.map((m) => m.ruleId).sort()).toEqual([
      "jsx-a11y/click-events-have-key-events",
      "jsx-a11y/no-static-element-interactions",
    ]);
  });

  it("allows key handlers on a dialog and a focusable menu", async () => {
    const code = [
      "export const A = (k: () => void) => (",
      "  <>",
      '    <div role="dialog" aria-label="d" onKeyDown={k}><button type="button">x</button></div>',
      '    <div role="menu" tabIndex={-1} onKeyDown={k}><button type="button" role="menuitem">y</button></div>',
      "  </>",
      ");",
      "",
    ].join("\n");
    expect(await errors(code)).toEqual([]);
  });

  it("fails a mouse handler on a dialog", async () => {
    const found = await errors('export const A = (f: () => void) => <div role="dialog" aria-label="d" onClick={f} />;\n');
    expect(found.map((m) => m.ruleId)).toContain("jsx-a11y/no-noninteractive-element-interactions");
  });

  it("allows a bare new Date() and a test file's dates", async () => {
    expect(await errors("export const now = () => new Date();\n")).toEqual([]);
    expect(await errors("export const d = (s: string) => new Date(s);\n", "src/x.test.ts")).toEqual([]);
  });
});
