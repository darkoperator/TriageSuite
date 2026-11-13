// The zone note the status bar states in brackets: the UTC offset of one row, and that row's local date. The front end
// computes nothing: it asks the engine for the first row of a timed dataset and reads both off a converted field's
// display text (the engine writes `YYYY-MM-DD HH:MM:SS.fff -05:00`, or `Z`). An offset is the offset *at that
// instant*, which differs across a daylight-saving change, so the note names the date it was read at. With no row, or
// no converted field, there is nothing to state and the bar names the zone alone. In UTC nothing is asked.
import { useEffect, useState } from "react";
import { gridPage, rowView } from "../api";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import { initialGridState } from "../grid/gridState";

const SHOWN = /^(\d{4}-\d{2}-\d{2}) \S* ?(Z|[+-]\d{2}:\d{2})$/;

/** `UTC−05:00 on 2026-01-04` from the engine's display text (a true minus sign; `Z` is `+00:00`); null when it is not that shape. */
export function offsetText(display: string): string | null {
  const m = SHOWN.exec(display);
  if (!m) return null;
  const offset = m[2] === "Z" ? "+00:00" : m[2].replace("-", "−");
  return `UTC${offset} on ${m[1]}`;
}

export function useZoneOffset(summary: CaseSummary | null, generation: number): string | null {
  const zone = summary?.display_zone ?? "UTC";
  const caseId = summary?.case_id ?? null;
  const relation = summary?.relations.find((r) => r.kind === "dataset" && r.primary_time !== null && r.rows > 0)?.name ?? null;
  // What was read is good for this case, zone, dataset and write generation only.
  const key = JSON.stringify([caseId, zone, relation, generation]);
  const [found, setFound] = useState<{ key: string; note: string | null } | null>(null);

  useEffect(() => {
    if (zone === "UTC" || relation === null) return;
    let live = true;
    const done = (note: string | null) => live && setFound({ key, note });
    gridPage({ ...initialGridState(relation), columns: null }, 1, 0, { zone, rules: [] }, "shell:zone:row")
      .then((page) => (page.row_ids.length === 0 ? null : rowView(page.row_ids[0], zone, "shell:zone:view")))
      .then(
        (view) => {
          const shown = view?.fields.find((f) => f.display !== null)?.display;
          done(shown ? offsetText(shown) : null);
        },
        () => done(null), // a refused or interrupted read leaves the zone stated without a note
      );
    return () => {
      live = false;
    };
  }, [key, zone, relation]);

  return found && found.key === key && zone !== "UTC" ? found.note : null;
}
