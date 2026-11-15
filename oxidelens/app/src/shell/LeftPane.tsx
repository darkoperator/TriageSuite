// The left pane: marked-row counts, datasets, loose files, saved views and sessions of the open case. Each mark count
// opens the worklist of its rows across every dataset; a dataset with marked rows shows its name in the colour of its
// most severe mark, with how many of its rows are marked beside it (ruling S12). The counts are the engine's
// (`CaseSummary.mark_counts`).
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { MarkState } from "../contract/generated/MarkState";
import type { Session } from "../contract/generated/Session";
import MarkGlyph, { MARK_GLYPH, MARK_NAME, MARK_STATES, markClass, mostSevere } from "../records/MarkGlyph";
import { displaySafe } from "../text";
import { SavedViewsSection } from "./slots";

const fmt = (n: number) => n.toLocaleString("en-US");

/** The worklist order of the counts: Suspicious first, as the analyst works down from it. */
const WORKLIST_ORDER: readonly MarkState[] = ["suspicious", "malicious", "agent_malicious"];

export default function LeftPane({
  summary, sessions, onOpenDataset, onOpenSession, onOpenWorklist, onCollapse,
}: {
  summary: CaseSummary;
  sessions: Session[];
  onOpenDataset(relation: string): void;
  onOpenSession(session: Session): void;
  /** Opens the worklist of the rows with `mark`, across every dataset. */
  onOpenWorklist(mark: MarkState): void;
  onCollapse(): void;
}) {
  const marksOf = new Map(summary.mark_counts.by_relation.map((m) => [m.relation, m]));
  const datasets = summary.relations.filter((r) => r.kind === "dataset");
  const files = summary.relations.filter((r) => r.kind === "file");
  const list = (items: typeof datasets) =>
    items.length === 0 ? (
      <p className="pane-empty">None.</p>
    ) : (
      <ul className="pane-list">
        {items.map((r) => {
          const marks = marksOf.get(r.name);
          const worst = marks ? mostSevere(marks) : null;
          const marked = marks ? MARK_STATES.reduce((n, m) => n + marks[m], 0) : 0;
          return (
            <li key={r.name}>
              <button type="button" className="pane-item" onClick={() => onOpenDataset(r.name)}>
                <span className={["pane-item-name", markClass(worst)].filter(Boolean).join(" ")}>{displaySafe(r.name)}</span>
                {worst !== null && (
                  <span className={`pane-item-marks ${markClass(worst)}`} role="img" aria-label={`${fmt(marked)} marked rows`}>
                    {`${MARK_GLYPH[worst]} ${fmt(marked)}`}
                  </span>
                )}
                <span className="pane-item-count">{r.rows}</span>
              </button>
            </li>
          );
        })}
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
        <ul className="pane-list" role="list">
          {WORKLIST_ORDER.map((m) => (
            <li key={m}>
              <button
                type="button"
                className="pane-item"
                aria-label={`${MARK_NAME[m]} ${fmt(summary.mark_counts[m])}`}
                title={`Every ${MARK_NAME[m]} row, across the datasets`}
                onClick={() => onOpenWorklist(m)}
              >
                <span className="pane-item-name">
                  <MarkGlyph mark={m} decorative />
                  {" "}
                  <span>{MARK_NAME[m]}</span>
                </span>
                <span className={`pane-item-count ${markClass(m)}`}>{fmt(summary.mark_counts[m])}</span>
              </button>
            </li>
          ))}
        </ul>
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
