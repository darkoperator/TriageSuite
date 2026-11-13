// A folded pane: a 26 px strip that keeps the Suspicious mark count visible and a button to unfold the pane.
export const RAIL_WIDTH = 26;

export default function Rail({
  side, suspicious, onExpand,
}: {
  side: "left" | "right";
  suspicious: number;
  onExpand(): void;
}) {
  return (
    <div className="rail" data-side={side} data-testid={`${side}-rail`} style={{ width: RAIL_WIDTH }}>
      <button type="button" className="rail-button" aria-label={`Expand ${side} pane`} title={`Expand ${side} pane (Ctrl+${side === "left" ? "[" : "]"})`} onClick={onExpand}>
        {side === "left" ? "»" : "«"}
      </button>
      <span className="rail-count" title="Suspicious rows" aria-label={`${suspicious} suspicious`}>
        {suspicious}
      </span>
    </div>
  );
}
