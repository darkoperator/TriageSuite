// The bar above a grid that tells what applying a view or exporting did: the parts of a view that could not apply (each
// named, with the engine's message), what was written, and an export's outcome. It stays until dismissed. Every string
// goes through `displaySafe`: the names and messages come from the case.
import type { ReactNode } from "react";
import type { ViewWarning } from "../contract/generated/ViewWarning";
import { displaySafe } from "../text";

export interface ViewNotice {
  /** Plain statements (an applied view, a finished export). */
  notes: string[];
  /** A refusal or failure; shown as an alert. */
  errors: string[];
  /** The parts of a view that are inactive on this dataset. */
  warnings: ViewWarning[];
}

export const noNotice: ViewNotice = { notes: [], errors: [], warnings: [] };

export default function ApplyWarnings({ notice, onDismiss }: { notice: ViewNotice; onDismiss(): void }): ReactNode {
  return (
    <div className="view-notices" role="region" aria-label="View notices">
      <div className="view-notices-body">
        {notice.notes.map((n, i) => (
          <p key={`n${i}`} role="status" className="view-note">{displaySafe(n)}</p>
        ))}
        {notice.errors.map((n, i) => (
          <p key={`e${i}`} role="alert" className="view-error">{displaySafe(n)}</p>
        ))}
        {notice.warnings.length > 0 && (
          <>
            <p className="view-note">{`${notice.warnings.length} part${notice.warnings.length === 1 ? "" : "s"} of the view could not apply and ${notice.warnings.length === 1 ? "is" : "are"} switched off:`}</p>
            <ul className="view-warnings">
              {notice.warnings.map((w, i) => (
                <li key={`${w.part}:${w.name}:${i}`}>
                  <span className="view-warning-part">{displaySafe(w.part)}</span>{" "}
                  <span className="view-warning-name">{displaySafe(w.name)}</span>
                  {`: `}
                  <span>{displaySafe(w.message)}</span>
                </li>
              ))}
            </ul>
          </>
        )}
      </div>
      <button type="button" className="icon-button" aria-label="Dismiss view notices" onClick={onDismiss}>×</button>
    </div>
  );
}
