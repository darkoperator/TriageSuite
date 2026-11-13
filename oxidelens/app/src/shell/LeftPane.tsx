// The left pane: marked-row counts, datasets, loose files, saved views and sessions of the open case.
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { Session } from "../contract/generated/Session";
import { displaySafe } from "../text";
import { SavedViewsSection } from "./slots";

export default function LeftPane({
  summary, sessions, onOpenDataset, onOpenSession, onCollapse,
}: {
  summary: CaseSummary;
  sessions: Session[];
  onOpenDataset(relation: string): void;
  onOpenSession(session: Session): void;
  onCollapse(): void;
}) {
  const datasets = summary.relations.filter((r) => r.kind === "dataset");
  const files = summary.relations.filter((r) => r.kind === "file");
  const list = (items: typeof datasets) =>
    items.length === 0 ? (
      <p className="pane-empty">None.</p>
    ) : (
      <ul className="pane-list">
        {items.map((r) => (
          <li key={r.name}>
            <button type="button" className="pane-item" onClick={() => onOpenDataset(r.name)}>
              <span className="pane-item-name">{displaySafe(r.name)}</span>
              <span className="pane-item-count">{r.rows}</span>
            </button>
          </li>
        ))}
      </ul>
    );
  return (
    <aside className="pane pane-left" aria-label="Case">
      <header className="pane-head">
        <span>Case</span>
        <button type="button" className="icon-button" aria-label="Collapse left pane" title="Collapse left pane (Ctrl+[)" onClick={onCollapse}>
          «
        </button>
      </header>
      <section aria-labelledby="sec-marked">
        <h3 id="sec-marked">Marked rows</h3>
        <p className="pane-line">
          <span>Suspicious</span>
          <span className="pane-item-count mark-suspicious">{summary.mark_counts.suspicious}</span>
        </p>
      </section>
      <section aria-labelledby="sec-datasets">
        <h3 id="sec-datasets">Datasets</h3>
        {list(datasets)}
      </section>
      <section aria-labelledby="sec-files">
        <h3 id="sec-files">Loose files</h3>
        {list(files)}
      </section>
      <section aria-labelledby="sec-views">
        <h3 id="sec-views" tabIndex={-1}>Saved views</h3>
        <SavedViewsSection />
      </section>
      <section aria-labelledby="sec-sessions">
        <h3 id="sec-sessions">Sessions</h3>
        {sessions.length === 0 ? (
          <p className="pane-empty">None.</p>
        ) : (
          <ul className="pane-list">
            {sessions.map((s) => (
              <li key={`${s.source_id}:${s.name}`}>
                <button
                  type="button"
                  className="pane-item"
                  disabled={s.relations.length === 0}
                  title="Opens a tab for each dataset of the session, scoped to its hosts"
                  onClick={() => onOpenSession(s)}
                >
                  <span className="pane-item-name">{displaySafe(s.name)}</span>
                  <span className="pane-item-count">{new Set(s.relations.map((r) => r.relation)).size}</span>
                </button>
                {s.unmatched > 0 && (
                  <p className="pane-empty">
                    {`${s.unmatched} session file${s.unmatched === 1 ? "" : "s"} matched no loaded dataset.`}
                  </p>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>
    </aside>
  );
}
