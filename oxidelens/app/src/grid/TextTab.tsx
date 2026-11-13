// The Text tab: an operator and a text (equals, not equals, contains, not contains, starts with, ends with, regex, is
// empty, is not empty), an Aa toggle for case, and for a typed column "did not convert". Apply makes or replaces this
// column's Text chip; the engine is asked first and its refusal (a bad regex, say) shows here, not as a chip.
import { useState, type ReactNode } from "react";
import type { Chip } from "../contract/generated/Chip";
import type { Op } from "../contract/generated/Op";
import { displaySafe } from "../text";
import { CONVERSION_FAILED, TEXT_OPS, opTakesValue, removeChip, textChip, textId, upsertChip } from "./chips";

export interface TextTabProps {
  column: string;
  typed: boolean;
  chips: readonly Chip[];
  applyChips(edit: (chips: Chip[]) => Chip[]): void;
  validate(chip: Chip): Promise<string | null>;
  onDone(): void;
}

export default function TextTab(p: TextTabProps): ReactNode {
  const existing = p.chips.find((c) => c.id === textId(p.column));
  const leaf = existing && "op" in existing.filter ? existing.filter : null;
  const [op, setOp] = useState<Op>(leaf?.op ?? "contains");
  const [text, setText] = useState(typeof leaf?.value === "string" ? leaf.value : "");
  const [caseSensitive, setCaseSensitive] = useState(leaf?.case_sensitive ?? false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const ops = p.typed ? [...TEXT_OPS, CONVERSION_FAILED] : TEXT_OPS;
  const takesValue = opTakesValue(op);

  const apply = async () => {
    const chip = textChip(p.column, op, text, caseSensitive);
    setBusy(true);
    const refusal = await p.validate(chip);
    setBusy(false);
    if (refusal !== null) {
      setError(refusal);
      return;
    }
    p.applyChips((chips) => upsertChip(chips, chip));
    p.onDone();
  };

  return (
    <form
      className="ft-text"
      onSubmit={(e) => {
        e.preventDefault();
        if (!busy) void apply();
      }}
    >
      <label className="ft-field">
        Operator
        <select value={op} onChange={(e) => { setOp(e.target.value as Op); setError(null); }}>
          {ops.map((o) => <option key={o.op} value={o.op}>{o.label}</option>)}
        </select>
      </label>
      {takesValue && (
        <div className="ft-field">
          <label className="ft-grow">
            Text
            <input type="text" value={text} autoFocus spellCheck={false} onChange={(e) => { setText(e.target.value); setError(null); }} />
          </label>
          <button
            type="button"
            className="ft-aa"
            aria-pressed={caseSensitive}
            aria-label="Match case"
            title="Match case"
            onClick={() => setCaseSensitive((c) => !c)}
          >
            Aa
          </button>
        </div>
      )}
      {error !== null && <div role="alert" className="form-error">{displaySafe(error)}</div>}
      <div className="ft-buttons">
        <button type="submit" disabled={busy}>Apply</button>
        {existing && (
          <button type="button" onClick={() => { p.applyChips((chips) => removeChip(chips, textId(p.column))); p.onDone(); }}>
            Remove
          </button>
        )}
      </div>
    </form>
  );
}
