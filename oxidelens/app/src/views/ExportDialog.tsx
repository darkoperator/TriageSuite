// *Export…*: the grid's rows as a CSV file. Times are written in UTC unless the box is ticked; choosing the file starts
// the export, which runs as a job the status bar can cancel.
import { useState, type ReactNode, type RefObject } from "react";
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
