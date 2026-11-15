// N: a note on one row (the focused one). The text is sent as typed; the engine checks it and its refusal is shown here.
import { useState, type ReactNode } from "react";
import { noteAdd } from "../api";
import { isStale, messageOf } from "./outcomes";
import RecordDialog from "./RecordDialog";

export default function NoteDialog({ rowId, onClose }: { rowId: number; onClose(): void }): ReactNode {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async () => {
    if (busy || text === "") return;
    setBusy(true);
    try {
      await noteAdd(rowId, text);
      onClose();
    } catch (e) {
      setBusy(false);
      if (!isStale(e)) setError(messageOf(e));
    }
  };

  return (
    <RecordDialog label="Add note" onClose={onClose} error={error}>
      <p className="form-hint">{`Row ${rowId.toLocaleString("en-US")}`}</p>
      <label className="form-field">
        <span>Note</span>
        <textarea aria-label="Note" rows={5} value={text} onChange={(e) => setText(e.target.value)} />
      </label>
      <div className="form-buttons">
        <button type="button" disabled={busy || text === ""} onClick={() => void save()}>Add note</button>
        <button type="button" onClick={onClose}>Cancel</button>
      </div>
    </RecordDialog>
  );
}
