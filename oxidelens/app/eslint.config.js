import react from "eslint-plugin-react";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

// The front end holds no rules (no date arithmetic, no zone conversion: the engine renders every time as text) and
// treats every string from the case as text (no HTML injection path). The rules below are the guard rails; the
// `lint.test.ts` next to them proves they fire.
const HTML_SINKS = ["innerHTML", "outerHTML", "insertAdjacentHTML"];

export default tseslint.config(
  // `src/contract/generated/` is written by `cargo test -p oxl-app --test contract`: never linted, never edited.
  { ignores: ["dist/", "node_modules/", "src/contract/generated/"] },
  ...tseslint.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    plugins: { react, "react-hooks": reactHooks },
    settings: { react: { version: "18" } },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // Untrusted text (cells, names, paths, messages) is rendered as text only.
      "react/no-danger": "error",
      "react/no-danger-with-children": "error",
      "no-restricted-properties": [
        "error",
        ...HTML_SINKS.map((property) => ({
          property,
          message: "Untrusted text is text: set textContent or render it through React, never as markup.",
        })),
        { object: "document", property: "write", message: "Untrusted text is text: never document.write." },
        { object: "document", property: "writeln", message: "Untrusted text is text: never document.writeln." },
        { object: "Date", property: "parse", message: "The front end does no date arithmetic: show the engine's text." },
        { object: "Date", property: "UTC", message: "The front end does no date arithmetic: show the engine's text." },
        {
          object: "Intl",
          property: "DateTimeFormat",
          message: "The front end does no zone conversion: show the engine's text.",
        },
      ],
      "no-restricted-syntax": [
        "error",
        {
          selector: "NewExpression[callee.name='Date'][arguments.length>0]",
          message: "The front end does no date arithmetic: show the engine's text.",
        },
      ],
    },
  },
  {
    // Tests build fixtures and may compute with dates.
    files: ["**/*.test.{ts,tsx}", "src/test/**"],
    rules: { "no-restricted-properties": "off", "no-restricted-syntax": "off" },
  },
);
