// The status bar: the job's progress line (left), an error with its code and message, the display zone, and the grid's
// own status area (right; the grid fills it through `setGridStatus`).
import type { ReactNode } from "react";
import { displaySafe } from "../text";

export default function StatusBar({
  progress, progressKind = null, onCancel = null, exportZone = null, note = null, error, zone, zoneOffset, gridStatus, onDismissError,
}: {
  /** The running job's kind (`export` says "Exporting"; any other says "Loading"). */
  progressKind?: string | null;
  /** Set while a cancellable job runs (an export): the status bar offers *Cancel*. */
  onCancel?: (() => void) | null;
  /** The zone a running export writes its times in; null when none runs or it writes UTC. */
  exportZone?: string | null;
  /** The engine's offset for the zone (`−05:00`); null when it has none to give. */
  zoneOffset: string | null;
  progress: string | null;
  /** A neutral line (not an error), such as "Export cancelled.". */
  note?: string | null;
  error: string | null;
  zone: string;
  gridStatus: ReactNode;
  onDismissError(): void;
}) {
  return (
    <footer className="status-bar">
      <span className="status-progress" data-testid="progress">
        {progress === null ? null : `${progressKind === "export" ? "Exporting" : "Loading"} ${displaySafe(progress)}…`}
      </span>
      {onCancel !== null && (
        <button type="button" className="status-cancel" aria-label="Cancel export" onClick={onCancel}>
          Cancel
        </button>
      )}
      {note !== null && <span className="status-note" role="status">{displaySafe(note)}</span>}
      {error !== null && (
        <span className="status-error" role="alert">
          {displaySafe(error)}
          <button type="button" className="icon-button" aria-label="Dismiss error" onClick={onDismissError}>
            ×
          </button>
        </span>
      )}
      <span className="status-spacer" />
      <span className="status-zone">
        {`TZ ${displaySafe(zone)}${zoneOffset === null ? "" : ` (${zoneOffset})`} · ${
          exportZone === null ? "storage and export stay UTC" : `storage stays UTC; this export's times are in ${displaySafe(exportZone)}`
        }`}
      </span>
      <span className="status-grid" data-testid="grid-status">
        {gridStatus}
      </span>
    </footer>
  );
}
