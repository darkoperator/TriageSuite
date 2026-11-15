// One read of the case for what a record screen shows, by the late-answer rules (docs/APP.md): the read is keyed by
// `base` (what it reads) and the case's write generation; a new key cancels the read still in flight, and an answer is
// kept only for the key it was asked for. `superseded`, `interrupted` and `no_case` are never errors. While a newer
// generation is being read the last answer for the same `base` stays up, so a write does not blank the screen; an answer
// for another `base` is never shown. A read is handed that last answer, so one that asks for more of the same (the next
// page) can add to it.
import { useEffect, useRef, useState } from "react";
import { newQueryId, queryCancel } from "../api";
import { isStale } from "./outcomes";

export type ReadState<T> = { base: string; value: T; error?: undefined } | { base: string; value?: undefined; error: unknown };

/**
 * Reads `read(nextId)` for `base` (null reads nothing) at `generation` (the case's write generation, or any version
 * that asks for a re-read of the same `base` while its last answer stays up): `read` takes a query id from `nextId()` for each
 * backend call it makes, one after another, and the one in flight is cancelled when the key moves on. `had` is the
 * answer now shown for `base` (undefined when there is none, or it was an error), whatever generation it was read at.
 * Returns the answer, or the read's error, for `base` only; null while there is none. A failed cancel is passed to
 * `report` unless it is stale.
 */
export function useCaseRead<T>(
  base: string | null,
  generation: number | string,
  read: (nextId: () => string, had: T | undefined) => Promise<T>,
  report: (e: unknown) => void,
): ReadState<T> | null {
  const [got, setGot] = useState<ReadState<T> | null>(null);
  // What `got` holds, for the effect to hand to the next read: only an answer that was kept is ever in it.
  const kept = useRef<ReadState<T> | null>(null);
  const readRef = useRef(read);
  const reportRef = useRef(report);
  useEffect(() => {
    readRef.current = read;
    reportRef.current = report;
  });

  useEffect(() => {
    if (base === null) return;
    let live = true;
    let settled = false;
    let current: string | null = null;
    // A read that moved on starts no further call: its next step stops as a superseded read would.
    const nextId = () => {
      if (!live) throw { code: "superseded", message: "the read moved on" };
      return (current = newQueryId());
    };
    const keep = (answer: ReadState<T>) => {
      kept.current = answer;
      setGot(answer);
    };
    readRef.current(nextId, kept.current?.base === base ? kept.current.value : undefined).then(
      (value) => {
        settled = true;
        if (live) keep({ base, value });
      },
      (error: unknown) => {
        settled = true;
        if (live && !isStale(error)) keep({ base, error });
      },
    );
    return () => {
      live = false;
      if (!settled && current !== null) {
        void queryCancel([current]).catch((e: unknown) => {
          if (!isStale(e)) reportRef.current(e);
        });
      }
    };
  }, [base, generation]);

  return got !== null && got.base === base ? got : null;
}
