// T: a tag added to, or removed from, the rows the selection was resolved to when the key was pressed. The case's tags
// are offered to pick from; the name is the engine's to check, and its refusal is shown here. Rows it did not take are
// said in the status bar.
import { useEffect, useState, type KeyboardEvent, type ReactNode } from "react";
import { tagAdd, tagRemove, tagsList } from "../api";
import type { TagInfo } from "../contract/generated/TagInfo";
import { displaySafe } from "../text";
import { isStale, messageOf, rowRefusals } from "./outcomes";
import RecordDialog from "./RecordDialog";

const fmt = (n: number) => n.toLocaleString("en-US");
/** How many of the case's tags are offered. */
const OFFERED = 50;

export default function TagDialog({
  rows, report, onClose,
}: {
  rows: number[];
  /** Shows a message in the status bar (the rows the engine did not take). */
  report(e: unknown): void;
  onClose(): void;
}): ReactNode {
  const [tag, setTag] = useState("");
  const [known, setKnown] = useState<TagInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    tagsList(OFFERED, 0, "dialog:tags").then(
      (page) => live && setKnown(page.items),
      (e: unknown) => {
        if (live && !isStale(e)) report(e);
      },
    );
    return () => {
      live = false;
    };
  }, [report]);

  const run = async (add: boolean) => {
    if (busy || tag === "") return;
    setBusy(true);
    try {
      const outcomes = await (add ? tagAdd(rows, tag) : tagRemove(rows, tag));
      const refused = rowRefusals(outcomes);
      if (refused !== null) report(new Error(refused));
      onClose();
    } catch (e) {
      setBusy(false);
      if (!isStale(e)) setError(messageOf(e));
    }
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Enter") void run(true);
  };

  return (
    <RecordDialog label="Tag rows" onClose={onClose} error={error}>
      <p className="form-hint">{`${fmt(rows.length)} ${rows.length === 1 ? "row" : "rows"}`}</p>
      <label className="form-field">
        <span>Tag</span>
        <input type="text" aria-label="Tag" value={tag} maxLength={200} onChange={(e) => setTag(e.target.value)} onKeyDown={onKey} />
      </label>
      {known.length > 0 && (
        <div className="record-picks" role="group" aria-label="Tags in the case">
          {known.map((t) => (
            <button key={t.name} type="button" className="tag" onClick={() => setTag(t.name)}>
              {displaySafe(t.name)}
            </button>
          ))}
        </div>
      )}
      <div className="form-buttons">
        <button type="button" disabled={busy || tag === ""} onClick={() => void run(true)}>Add tag</button>
        <button type="button" disabled={busy || tag === ""} onClick={() => void run(false)}>Remove tag</button>
        <button type="button" onClick={onClose}>Cancel</button>
      </div>
    </RecordDialog>
  );
}
