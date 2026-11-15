// The Timeline as Host lanes: one lane per host, on one shared axis, each entry a glyph in the lane of every host it
// links. The axis is the engine's order (by time), not a scale: the front end does no date arithmetic, so the k-th dated
// entry of n sits at k/(n-1) of the width in every lane, and the axis is labelled with the first and last times as the
// engine shows them. Undated entries ("needs a time") are not drawn; they are counted under the axis.
import type { ReactNode } from "react";
import type { Entry } from "../contract/generated/Entry";
import { displaySafe } from "../text";
import MarkGlyph from "./MarkGlyph";

const fmt = (n: number) => n.toLocaleString("en-US");
/** The lane of the entries that link no host. */
export const NO_HOST = "no host";

export default function HostLanes({
  entries, selected, onSelect,
}: {
  /** The entries, in the engine's order. */
  entries: Entry[];
  selected: number | null;
  onSelect(id: number): void;
}): ReactNode {
  const dated = entries.filter((e) => !e.needs_time);
  const undated = entries.length - dated.length;
  // Lanes in the order their hosts first appear in the engine's order.
  const lanes: string[] = [];
  for (const e of dated) for (const h of e.hosts.length === 0 ? [NO_HOST] : e.hosts) if (!lanes.includes(h)) lanes.push(h);
  const at = (k: number) => (dated.length === 1 ? 50 : (k * 100) / (dated.length - 1));
  const first = dated[0];
  const last = dated[dated.length - 1];
  return (
    <section className="host-lanes" aria-label="Host lanes">
      {lanes.length === 0 && <p className="pane-empty">No dated entries to draw.</p>}
      {lanes.map((host) => (
        <div key={host} className="lane" role="group" aria-label={`Host ${displaySafe(host)}`}>
          <span className="lane-name">{displaySafe(host)}</span>
          <div className="lane-track">
            {dated.map((e, k) =>
              (e.hosts.length === 0 ? host === NO_HOST : e.hosts.includes(host)) ? (
                <button
                  key={e.id}
                  type="button"
                  className={`lane-glyph${selected === e.id ? " lane-glyph-selected" : ""}`}
                  data-entry={e.id}
                  style={{ left: `${at(k)}%` }}
                  aria-label={`E-${e.id} ${displaySafe(e.title)}`}
                  title={`${displaySafe(e.time_shown ?? e.time ?? "")} · ${displaySafe(e.title)}`}
                  onClick={() => onSelect(e.id)}
                >
                  {e.mark !== null ? <MarkGlyph mark={e.mark} decorative /> : <span aria-hidden="true">○</span>}
                </button>
              ) : null,
            )}
          </div>
        </div>
      ))}
      {first && last && (
        <div className="lane-axis">
          <span>{displaySafe(first.time_shown ?? first.time ?? "")}</span>
          <span className="record-meta">in the Timeline's order</span>
          <span>{displaySafe(last.time_shown ?? last.time ?? "")}</span>
        </div>
      )}
      {undated > 0 && <p className="form-hint">{`${fmt(undated)} undated ${undated === 1 ? "entry" : "entries"}, not drawn`}</p>}
    </section>
  );
}
