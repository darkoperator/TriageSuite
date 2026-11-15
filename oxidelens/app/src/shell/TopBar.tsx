// The top bar: app name, case name, what the case holds, the analyst, and the two load buttons.
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { RecordExport } from "../dialogs";
import { displaySafe } from "../text";
import { ExportMenu } from "../views/ExportDialog";

export default function TopBar({
  summary, analyst, busy, exporting, onExport, onLoadRun, onOpenCsv, onClose, onSettings,
}: {
  summary: CaseSummary;
  analyst: string | null;
  busy: boolean;
  /** An export is running: the menu is off until it ends (one at a time). */
  exporting: boolean;
  onExport(kind: RecordExport): void;
  onLoadRun(): void;
  onOpenCsv(): void;
  onClose(): void;
  /** Opens the settings dialog; gets the gear button so focus can return to it. */
  onSettings(gear: HTMLButtonElement): void;
}) {
  const hosts = summary.hosts.length;
  const sources = summary.sources.filter((s) => !s.removed).length;
  const rows = summary.relations.reduce((n, r) => n + r.rows, 0);
  return (
    <header className="top-bar">
      <strong className="app-name">OxideLens</strong>
      <span className="case-name">{displaySafe(summary.name)}</span>
      <span className="case-counts">{`${hosts} hosts · ${sources} sources · ${rows} rows`}</span>
      <span className="top-spacer" />
      <span className="analyst" title="Analyst">{analyst === null ? "no analyst name" : displaySafe(analyst)}</span>
      <button type="button" disabled={busy} onClick={onLoadRun}>Load run…</button>
      <button type="button" disabled={busy} onClick={onOpenCsv}>Open CSV…</button>
      <ExportMenu disabled={exporting} onChoose={onExport} />
      <button type="button" onClick={onClose}>Close case</button>
      <button type="button" className="icon-button" aria-label="Settings" title="Settings" onClick={(e) => onSettings(e.currentTarget)}>⚙</button>
    </header>
  );
}
