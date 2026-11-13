// Find mode's matches: how many rows match (`grid_match_count`) and stepping to the next or previous one
// (`grid_matches`, one position per step, carrying its rank among the matches). The engine finds the neighbour of an
// offset in the grid's order, so a step is one small query however many rows match; nothing here lists or caches
// matches.
//
// Both kinds of query are cancelled when the state changes (only while they are still running: a settled id is
// forgotten, so the registry's pending-cancel memory is not filled with ids that will never run), and a step whose
// state changed while it ran moves nothing.
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { gridMatchCount, gridMatches, isCmdError, newQueryId, queryCancel } from "../api";
import type { GridState } from "../contract/generated/GridState";
import { stableKey } from "./useGridData";

const stale = (e: unknown) => isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case");

export interface Matches {
  /** How many rows match; null until the engine has said. */
  count: number | null;
  /** The offset of the match the analyst is on, null before the first step. */
  current: number | null;
  /** 1-based rank of `current` among the matches (the engine says), 0 before the first step. */
  position: number;
  /**
   * The next (1) or previous (-1) match after `anchor` (default: the current one), wrapping at the ends; null when
   * there is none, or when the state changed while it ran (the caller then must not move anything).
   */
  step(direction: 1 | -1, anchor?: number | null): Promise<number | null>;
}

/**
 * `state` is the grid's state (its columns do not matter, so pass it without them); `active` is true in find mode with a
 * text. `version` rises when the case changed under it. A new state or version forgets the position.
 */
export function useMatches(tabId: string, state: GridState, active: boolean, version: number, onError: (e: unknown) => void): Matches {
  const key = useMemo(() => stableKey([state, version]), [state, version]);
  const [count, setCount] = useState<{ key: string; n: number } | null>(null);
  const [place, setPlace] = useState<{ key: string; offset: number; position: number } | null>(null);
  const onErrorRef = useRef(onError);
  useEffect(() => {
    onErrorRef.current = onError;
  });

  // One record per committed state: its key, the state itself and the ids of the queries running for it. It is swapped
  // as a unit in a layout effect, so from the moment a state is on screen every step reads that state, registers its
  // ids with that state, and is judged against that state. (A passive effect would leave a window in which a step
  // started after the commit queried the old state, or was cancelled by the old state's cleanup.)
  const record = useRef<{ key: string; state: GridState; running: Set<string> }>({ key, state, running: new Set() });
  useLayoutEffect(() => {
    const mine = { key, state, running: new Set<string>() };
    record.current = mine;
    let live = true;
    if (active) {
      const id = newQueryId();
      mine.running.add(id);
      gridMatchCount(state, `tab:${tabId}:matches:count`, id)
        .then(
          (n) => live && setCount({ key, n }),
          (e: unknown) => {
            if (live && !stale(e) && !(isCmdError(e) && e.code === "invalid")) onErrorRef.current(e);
          },
        )
        .finally(() => mine.running.delete(id));
    }
    return () => {
      live = false;
      // Whatever this state still has running is of no use to the next one.
      if (mine.running.size > 0) queryCancel([...mine.running]).catch(() => {});
    };
    // `state` is part of `key`: an equal state of a new identity is not a new record.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabId, key, active]);

  const step = useCallback(
    async (direction: 1 | -1, anchor?: number | null): Promise<number | null> => {
      const rec = record.current;
      const from = anchor ?? (place && place.key === rec.key ? place.offset : null);
      const backward = direction === -1;
      const ask = async (at: number | null) => {
        const id = newQueryId();
        rec.running.add(id);
        try {
          const got = await gridMatches(rec.state, at, backward, 1, `tab:${tabId}:matches:step`, id);
          return got.items.length > 0 ? got.items[0] : null;
        } finally {
          rec.running.delete(id);
        }
      };
      try {
        let found = await ask(from);
        // Past the last (or before the first): the other end.
        if (found === null && from !== null && record.current === rec) found = await ask(null);
        if (record.current !== rec || found === null) return null; // the state changed: nothing is moved
        setPlace({ key: rec.key, offset: found.offset, position: found.ordinal });
        return found.offset;
      } catch (e) {
        if (record.current === rec && !stale(e)) onErrorRef.current(e);
        return null;
      }
    },
    [place, tabId],
  );


  const here = place && place.key === key ? place : null;
  return {
    count: active && count && count.key === key ? count.n : null,
    current: here?.offset ?? null,
    position: here?.position ?? 0,
    step,
  };
}
