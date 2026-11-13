// The top bar: app name, case name, what the case holds, the analyst, and the two load buttons.
import type { CaseSummary } from "../contract/generated/CaseSummary";
import { displaySafe } from "../text";

export default function TopBar({
  summary, analyst, busy, onLoadRun, onOpenCsv, onClose,
}: {
  summary: CaseSummary;
  analyst: string | null;
  busy: boolean;
  onLoadRun(): void;
  onOpenCsv(): void;
  onClose(): void;
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
      <button type="button" onClick={onClose}>Close case</button>
    </header>
  );
}
