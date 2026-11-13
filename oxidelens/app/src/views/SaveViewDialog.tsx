// *Save view…*: a name and the datasets the view is offered for (a pattern, defaulting to this relation's name; `*`
// stands for any run of characters, as in `evtxtriage_*`). The engine checks both; its refusal is shown here.
import { useState, type KeyboardEvent, type ReactNode, type RefObject } from "react";
import { displaySafe } from "../text";
import Popover from "../grid/Popover";

export default function SaveViewDialog({
  anchor, relation, onSave, onClose,
}: {
  anchor: RefObject<HTMLElement | null>;
  relation: string;
  /** Saves the view; resolves to the refusal's text, or `null` when it was saved. */
  onSave(name: string, pattern: string): Promise<string | null>;
  onClose(): void;
}): ReactNode {
  const [name, setName] = useState("");
  const [pattern, setPattern] = useState(relation);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async () => {
    if (busy) return;
    if (name.trim() === "") {
      setError("Give the view a name.");
      return;
    }
    setBusy(true);
    const refusal = await onSave(name, pattern);
    setBusy(false);
    if (refusal === null) onClose();
    else setError(refusal);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Enter") void save();
  };

  return (
    <Popover label="Save view" anchor={anchor} onClose={onClose} className="save-view">
      <label className="form-field">
        <span>Name</span>
        <input type="text" aria-label="View name" value={name} maxLength={128} onChange={(e) => setName(e.target.value)} onKeyDown={onKey} />
      </label>
      <label className="form-field">
        <span>Datasets</span>
        <input type="text" aria-label="Dataset pattern" value={pattern} maxLength={128} onChange={(e) => setPattern(e.target.value)} onKeyDown={onKey} />
      </label>
      <p className="form-hint">{`Offered for relations matching this name; * stands for any run of characters, as in evtxtriage_*.`}</p>
      <div className="form-buttons">
        <button type="button" disabled={busy} onClick={() => void save()}>Save</button>
        <button type="button" onClick={onClose}>Cancel</button>
      </div>
      {error !== null && <div role="alert" className="form-error">{displaySafe(error)}</div>}
    </Popover>
  );
}
