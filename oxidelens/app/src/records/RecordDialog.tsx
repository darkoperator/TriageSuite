// The frame of the small dialogs the row keys open (T, N, A, I): modal, focus on its first control when it opens and
// back to the opener (the grid) when it closes, Tab kept inside, Escape closes it with nothing written.
import { useRef, type KeyboardEvent, type ReactNode } from "react";
import { trapTab, useDialogFocus } from "../grid/dialogFocus";
import { displaySafe } from "../text";
import "./records.css";

export default function RecordDialog({
  label, onClose, error, children,
}: {
  label: string;
  onClose(): void;
  /** The engine's refusal of the last try, shown in the dialog (as text). */
  error?: string | null;
  children: ReactNode;
}): ReactNode {
  const box = useRef<HTMLDivElement>(null);
  useDialogFocus(box);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
      return;
    }
    trapTab(e, box.current);
  };
  return (
    <div className="modal-scrim">
      <div ref={box} className="record-dialog" role="dialog" aria-label={label} aria-modal="true" onKeyDown={onKeyDown}>
        <div className="viewer-title">{label}</div>
        <div className="record-dialog-body">{children}</div>
        {error != null && <div role="alert" className="form-error">{displaySafe(error)}</div>}
      </div>
    </div>
  );
}
