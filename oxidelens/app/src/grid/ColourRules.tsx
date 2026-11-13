// The colour rules of one dataset: an ordered list (the first enabled rule a row matches tints it), each with a name, a
// colour from the fixed token set, a row or cell scope, an enabled switch, and move, drag and delete. The dialog holds
// no rules of its own: it hands the whole new list to its owner, which saves it and sends it with the next page.
// Names and the engine's error messages are case text and go through `displaySafe`.
import { useRef, useState, type DragEvent, type KeyboardEvent, type ReactNode } from "react";
import type { ColourRule } from "../contract/generated/ColourRule";
import type { RuleColour } from "../contract/generated/RuleColour";
import type { RuleScope } from "../contract/generated/RuleScope";
import { displaySafe } from "../text";
import { trapTab, useDialogFocus } from "./dialogFocus";
import {
  COLOUR_LABEL, MAX_RULES, MAX_RULE_NAME, OFFERED_COLOURS, RULE_COLOURS, moveRule, patchRule, removeRule, swatchClass,
} from "./ruleClasses";

export interface ColourRulesProps {
  rules: readonly ColourRule[];
  /** True while the relation has no saved entry and the engine's defaults apply. */
  usingDefaults: boolean;
  /** The engine's `rule_errors`: an index into `rules` and why that rule was skipped. */
  errors: readonly (readonly [number, string])[];
  /** Whether the grid has an enabled filter a new rule could be built from. */
  canFromFilter: boolean;
  onChange(rules: ColourRule[]): void;
  onFromFilter(): void;
  onReset(): void;
  onClose(): void;
}

const label = (r: ColourRule, i: number) => displaySafe(r.name) || `rule ${i + 1}`;

/** The colours offered for `current`: the fixed offer, plus the rule's own colour when it is a reserved one it already has. */
function colourOptions(current: RuleColour): RuleColour[] {
  return RULE_COLOURS.filter((c) => OFFERED_COLOURS.includes(c) || c === current);
}

function NameBox({ rule, index, onCommit }: { rule: ColourRule; index: number; onCommit(name: string): void }): ReactNode {
  const shown = displaySafe(rule.name);
  const [draft, setDraft] = useState(shown);
  const commit = () => {
    const next = draft.trim();
    if (next === "" || next === shown) {
      setDraft(shown);
      return;
    }
    onCommit(next);
  };
  return (
    <input
      type="text"
      className="rule-name"
      aria-label={`Name of rule ${index + 1}`}
      maxLength={MAX_RULE_NAME}
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          commit();
        }
      }}
    />
  );
}

export default function ColourRules(p: ColourRulesProps): ReactNode {
  const box = useRef<HTMLDivElement>(null);
  useDialogFocus(box);
  const dragFrom = useRef<number | null>(null);

  const onKey = (e: KeyboardEvent<HTMLElement>) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      p.onClose();
      return;
    }
    trapTab(e, box.current);
  };
  const errorsOf = (i: number) => p.errors.filter(([n]) => n === i).map(([, why]) => why);
  const full = p.rules.length >= MAX_RULES;

  return (
    <div className="viewer-scrim" onMouseDown={(e) => e.target === e.currentTarget && p.onClose()}>
      <div ref={box} role="dialog" aria-modal="true" aria-label="Colour rules" className="rules-dialog" tabIndex={-1} onKeyDown={onKey}>
        <div className="viewer-title">
          Colour rules · the first enabled rule a row matches tints it
          {p.usingDefaults && p.rules.length > 0 ? " · defaults" : ""}
        </div>
        {p.rules.length === 0 ? (
          <div className="rules-empty">No colour rules. Filter the grid, then make a rule from the current filter.</div>
        ) : (
          <ul className="rules-list">
            {p.rules.map((r, i) => {
              const name = label(r, i);
              const errs = errorsOf(i);
              return (
                <li
                  key={r.id}
                  className="rule-item"
                  draggable
                  onDragStart={(e: DragEvent) => {
                    dragFrom.current = i;
                    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
                  }}
                  onDragOver={(e) => {
                    if (dragFrom.current !== null) e.preventDefault();
                  }}
                  onDrop={(e) => {
                    e.preventDefault();
                    const from = dragFrom.current;
                    dragFrom.current = null;
                    if (from !== null && from !== i) p.onChange(moveRule(p.rules, from, i));
                  }}
                  onDragEnd={() => {
                    dragFrom.current = null;
                  }}
                >
                  <div className="rule-line">
                    <span className="rule-handle" aria-hidden="true">⋮⋮</span>
                    <input
                      type="checkbox"
                      aria-label={`${name} enabled`}
                      checked={r.enabled}
                      onChange={(e) => p.onChange(patchRule(p.rules, r.id, { enabled: e.target.checked }))}
                    />
                    <NameBox key={r.name} rule={r} index={i} onCommit={(n) => p.onChange(patchRule(p.rules, r.id, { name: n }))} />
                    <span className={`rule-swatch ${swatchClass(r.colour)}`} aria-hidden="true" />
                    <select
                      aria-label={`Colour of ${name}`}
                      value={r.colour}
                      onChange={(e) => p.onChange(patchRule(p.rules, r.id, { colour: e.target.value as RuleColour }))}
                    >
                      {colourOptions(r.colour).map((c) => (
                        <option key={c} value={c}>
                          {COLOUR_LABEL[c]}
                        </option>
                      ))}
                    </select>
                    <select
                      aria-label={`Scope of ${name}`}
                      value={r.scope}
                      onChange={(e) => p.onChange(patchRule(p.rules, r.id, { scope: e.target.value as RuleScope }))}
                    >
                      <option value="row">Row</option>
                      <option value="cell">Cell</option>
                    </select>
                    <button type="button" aria-label={`Move ${name} up`} disabled={i === 0} onClick={() => p.onChange(moveRule(p.rules, i, i - 1))}>
                      ↑
                    </button>
                    <button
                      type="button"
                      aria-label={`Move ${name} down`}
                      disabled={i === p.rules.length - 1}
                      onClick={() => p.onChange(moveRule(p.rules, i, i + 1))}
                    >
                      ↓
                    </button>
                    <button type="button" aria-label={`Delete ${name}`} onClick={() => p.onChange(removeRule(p.rules, r.id))}>
                      ✕
                    </button>
                  </div>
                  {errs.map((why, k) => (
                    <div key={k} role="alert" className="rule-error">
                      {`skipped: ${displaySafe(why)}`}
                    </div>
                  ))}
                </li>
              );
            })}
          </ul>
        )}
        <div className="rules-foot">
          <button type="button" disabled={!p.canFromFilter || full} onClick={p.onFromFilter}>
            New rule from the current filter
          </button>
          <button type="button" disabled={p.usingDefaults} onClick={p.onReset}>
            Reset to defaults
          </button>
          {full && <span className="rules-note">{`at most ${MAX_RULES} rules`}</span>}
          <button type="button" className="rules-close" onClick={p.onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
