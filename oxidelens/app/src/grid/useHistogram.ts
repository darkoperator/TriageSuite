// The histogram strip's data: `grid_histogram` for the grid's filter, on a slot of its own (`tab:<id>:hist`) so it never
// queues behind, or delays, the rows.
//
// One record per committed request (its key, and the ids of the queries running for it) is swapped in a layout effect, so
// from the moment a request is on screen an answer is judged against that request. An answer is drawn only when it was
// asked for the request on screen; the cleanup cancels what is still running (a settled id is forgotten, so the
// registry's cancel memory is not filled with ids that will never run). A superseded or interrupted answer is not an
// error.
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { gridHistogram, isCmdError, newQueryId, queryCancel } from "../api";
import type { GridState } from "../contract/generated/GridState";
import type { Histogram } from "../contract/generated/Histogram";
import { stableKey } from "./useGridData";

const stale = (e: unknown) => isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case");

export interface HistogramData {
  /** The answer for the request on screen; null while it is on its way (or failed). */
  histogram: Histogram | null;
  /** The engine's own refusal (`invalid`), as text; other failures go to `onError`. */
  error: string | null;
}

/**
 * `state` is the grid's state (columns and sort do not matter to the buckets and are dropped); `column` null means the
 * relation's primary time. `version` rises when the case changed under it. Nothing is asked while `enabled` is false.
 */
export function useHistogram(
  tabId: string, state: GridState, column: string | null, zone: string, maxBuckets: number, enabled: boolean,
  version: number, onError: (e: unknown) => void,
): HistogramData {
  const asked = useMemo(() => ({ ...state, columns: null, sort: [] }), [state]);
  const key = useMemo(() => stableKey([asked, column, zone, maxBuckets, version]), [asked, column, zone, maxBuckets, version]);
  const [got, setGot] = useState<{ key: string; histogram: Histogram | null; error: string | null } | null>(null);
  const onErrorRef = useRef(onError);
  useLayoutEffect(() => {
    onErrorRef.current = onError;
  });
  const record = useRef<{ key: string; running: Set<string> }>({ key, running: new Set() });

  useLayoutEffect(() => {
    const mine = { key, running: new Set<string>() };
    record.current = mine;
    if (!enabled) return;
    const id = newQueryId();
    mine.running.add(id);
    gridHistogram(asked, column, maxBuckets, zone, `tab:${tabId}:hist`, id)
      .then(
        (histogram) => {
          if (record.current === mine) setGot({ key, histogram, error: null });
        },
        (e: unknown) => {
          if (record.current !== mine || stale(e)) return;
          if (isCmdError(e) && e.code === "invalid") setGot({ key, histogram: null, error: e.message });
          else onErrorRef.current(e);
        },
      )
      .finally(() => mine.running.delete(id));
    return () => {
      if (mine.running.size > 0) queryCancel([...mine.running]).catch(() => {});
    };
    // `asked` is part of `key`: an equal state of a new identity is not a new request.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabId, key, enabled]);

  const here = enabled && got !== null && got.key === key ? got : null;
  return { histogram: here?.histogram ?? null, error: here?.error ?? null };
}
