// The status bar: the job's progress line (left), an error with its code and message, the display zone, and the grid's
// own status area (right; the grid fills it through `setGridStatus`).
import type { ReactNode } from "react";
import { displaySafe } from "../text";

export default function StatusBar({
  progress, error, zone, gridStatus, onDismissError,
}: {
  progress: string | null;
  error: string | null;
  zone: string;
  gridStatus: ReactNode;
  onDismissError(): void;
}) {
  return (
    <footer className="status-bar">
      <span className="status-progress" data-testid="progress">
        {progress === null ? null : `Loading ${displaySafe(progress)}…`}
      </span>
      {error !== null && (
        <span className="status-error" role="alert">
          {displaySafe(error)}
          <button type="button" className="icon-button" aria-label="Dismiss error" onClick={onDismissError}>
            ×
          </button>
        </span>
      )}
      <span className="status-spacer" />
      <span className="status-zone">Zone {displaySafe(zone)}</span>
      <span className="status-grid" data-testid="grid-status">
        {gridStatus}
      </span>
    </footer>
  );
}
