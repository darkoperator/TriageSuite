// The Timeline as Host lanes: one lane per host, on one shared time axis, each entry a glyph in the lane of every host
// it links. The axis runs from the earliest to the latest shown entry by the engine's `time_us` (microseconds since the
// epoch): a glyph sits at (t - first) / (last - first) of the width, plain arithmetic on the engine's numbers, no date
// parsed. The axis is labelled with the first and last times as the engine shows them. Undated entries ("needs a
// time") are not drawn; they are counted under the axis.
import type { KeyboardEvent, ReactNode } from "react";
import type { Entry } from "../contract/generated/Entry";
import { displaySafe } from "../text";
import MarkGlyph from "./MarkGlyph";

const fmt = (n: number) => n.toLocaleString("en-US");
/** The lane of the entries that link no host. */
export const NO_HOST = "no host";

export default function HostLanes({
  entries, selected, onSelect, onKeyDown,
}: {
  /** The entries, in the engine's order. */
  entries: Entry[];
  selected: number | null;
  onSelect(id: number): void;
  /** The sheet's keys (N, Enter, R, Del), the same as on the table; a glyph names its entry by `data-entry`. */
  onKeyDown(e: KeyboardEvent<HTMLElement>): void;
}): ReactNode {
  const dated = entries.filter((e) => !e.needs_time && e.time_us !== null);
  const undated = entries.length - dated.length;
  // Lanes in the order their hosts first appear in the engine's order.
  const lanes: string[] = [];
  for (const e of dated) for (const h of e.hosts.length === 0 ? [NO_HOST] : e.hosts) if (!lanes.includes(h)) lanes.push(h);
  // The axis's ends: the earliest and latest shown entry (the engine lists by time, but the ends are read, not assumed).
  let first: Entry | undefined;
  let last: Entry | undefined;
  for (const e of dated) {
    if (first === undefined || e.time_us! < first.time_us!) first = e;
    if (last === undefined || e.time_us! > last.time_us!) last = e;
  }
  const span = first && last ? last.time_us! - first.time_us! : 0;
  const at = (e: Entry) => (span === 0 ? 50 : ((e.time_us! - first!.time_us!) * 100) / span);
  return (
    <section className="host-lanes" aria-label="Host lanes" onKeyDown={onKeyDown}>
      {lanes.length === 0 && <p className="pane-empty">No dated entries to draw.</p>}
      {lanes.map((host) => (
        <div key={host} className="lane" role="group" aria-label={`Host ${displaySafe(host)}`}>
          <span className="lane-name">{displaySafe(host)}</span>
          <div className="lane-track">
            {dated.map((e) =>
              (e.hosts.length === 0 ? host === NO_HOST : e.hosts.includes(host)) ? (
                <button
                  key={e.id}
                  type="button"
                  className={`lane-glyph${selected === e.id ? " lane-glyph-selected" : ""}`}
                  data-entry={e.id}
                  style={{ left: `${at(e)}%` }}
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

          <span>{displaySafe(last.time_shown ?? last.time ?? "")}</span>
        </div>
      )}
      {undated > 0 && <p className="form-hint">{`${fmt(undated)} undated ${undated === 1 ? "entry" : "entries"}, not drawn`}</p>}
    </section>
  );
}
