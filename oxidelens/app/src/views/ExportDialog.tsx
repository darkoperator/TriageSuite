// *Export…*: the grid's rows as a CSV file. Times are written in UTC unless the box is ticked; choosing the file starts
// the export, which runs as a job the status bar can cancel.
import { useRef, useState, type ReactNode, type RefObject } from "react";
import type { RecordExport } from "../dialogs";
import Popover from "../grid/Popover";
import { displaySafe } from "../text";

export default function ExportDialog({
  anchor, zone, onStart, onClose,
}: {
  anchor: RefObject<HTMLElement | null>;
  /** The display zone, named in the option's hint. */
  zone: string;
  /** Closes the dialog and goes on to the file dialog and the job. */
  onStart(zoneTimes: boolean): void;
  onClose(): void;
}): ReactNode {
  const [zoneTimes, setZoneTimes] = useState(false);
  return (
    <Popover label="Export" anchor={anchor} onClose={onClose} className="export-dialog">
      <p className="form-hint">The rows the grid shows now, as CSV.</p>
      <label className="form-check">
        <input type="checkbox" aria-label="Times in the display zone" checked={zoneTimes} onChange={(e) => setZoneTimes(e.target.checked)} />
        <span>{`Times in the display zone (${displaySafe(zone)}); off writes UTC`}</span>
      </label>
      <div className="form-buttons">
        <button type="button" onClick={() => onStart(zoneTimes)}>Choose file and export…</button>
        <button type="button" onClick={onClose}>Cancel</button>
      </div>
    </Popover>
  );
}

const KINDS: Array<[RecordExport, string]> = [["timeline", "timeline CSV"], ["workbook", "XLSX workbook"], ["report", "report"]];

/** The top bar's *Export ▾*: the case record as a timeline CSV, an XLSX workbook or a Markdown report. */
export function ExportMenu({ disabled, onChoose }: { disabled: boolean; onChoose(kind: RecordExport): void }): ReactNode {
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  return (
    <>
      <button
        ref={button} type="button" aria-label="Export" aria-haspopup="dialog" aria-expanded={open} disabled={disabled}
        onClick={() => setOpen(!open)}
      >
        Export ▾
      </button>
      {open && (
        <Popover label="Export case record" anchor={button} onClose={() => setOpen(false)} className="export-dialog">
          {KINDS.map(([kind, label]) => (
            <button key={kind} type="button" onClick={() => { setOpen(false); onChoose(kind); }}>{label}</button>
          ))}
        </Popover>
      )}
    </>
  );
}
