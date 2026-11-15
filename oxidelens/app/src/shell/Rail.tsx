// A folded pane: a 26 px strip that keeps the mark counts visible (each in its mark's glyph and colour) and a button to
// unfold the pane.
import type { MarkCounts } from "../contract/generated/MarkCounts";
import { MARK_GLYPH, MARK_LABEL, MARK_STATES, markClass } from "../records/MarkGlyph";

export const RAIL_WIDTH = 26;

export default function Rail({
  side, counts, onExpand,
}: {
  side: "left" | "right";
  counts: MarkCounts;
  onExpand(): void;
}) {
  return (
    <div className="rail" data-side={side} data-testid={`${side}-rail`} style={{ width: RAIL_WIDTH }}>
      <button type="button" className="rail-button" aria-label={`Expand ${side} pane`} title={`Expand ${side} pane (Ctrl+${side === "left" ? "[" : "]"})`} onClick={onExpand}>
        {side === "left" ? "»" : "«"}
      </button>
      {MARK_STATES.map((m) => (
        <span key={m} className={`rail-count ${markClass(m)}`} title={`${MARK_LABEL[m]} rows`} role="img" aria-label={`${counts[m]} ${MARK_LABEL[m]}`}>
          <span aria-hidden="true">{MARK_GLYPH[m]}</span>
          <span>{counts[m]}</span>
        </span>
      ))}
    </div>
  );
}
