// The count beside each pinned case-sheet tab: the Timeline's entries and the host, account and IOC records. The
// listing has no total, so a count is what one page of `LIMIT` holds, shown as `1000+` when a page was cut short.
// A read is keyed by the case and its `case-changed` generation; an answer for an older one is not drawn.
import { useCallback, useEffect, useRef, useState } from "react";
import { entriesList, isCmdError, newQueryId, queryCancel, recordsList } from "../api";
import type { EntryQuery } from "../contract/generated/EntryQuery";
import type { Listing } from "../contract/generated/Listing";
import type { RecordQuery } from "../contract/generated/RecordQuery";
import type { SheetKind } from "./tabs";
import { useCaseChanged } from "./useCaseChanged";

const LIMIT = 1000; // the engine's MAX_LIMIT
const ALL_ENTRIES: EntryQuery = { status: [], host: null, tactic: null, text: null, row: null, order: null };
const QUERIES: Record<SheetKind, (slot: string, id: string) => Promise<Listing<unknown>>> = {
  timeline: (slot, id) => entriesList(ALL_ENTRIES, LIMIT, 0, slot, id),
  hosts: (slot, id) => recordsList({ kind: "host" } satisfies RecordQuery, LIMIT, 0, slot, id),
  accounts: (slot, id) => recordsList({ kind: "account" } satisfies RecordQuery, LIMIT, 0, slot, id),
  iocs: (slot, id) => recordsList({ kind: "ioc", ioc_type: null, tool: null, text: null } satisfies RecordQuery, LIMIT, 0, slot, id),
};
const KINDS = Object.keys(QUERIES) as SheetKind[];
const isStale = (e: unknown) => isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case");

export type SheetCounts = Partial<Record<SheetKind, string>>;

/** Counts for the open case `caseId` (`null`: none open); re-read on every `case-changed`. */
export function useSheetCounts(caseId: string | null, onError: (e: unknown) => void): SheetCounts {
  const [counts, setCounts] = useState<{ caseId: string | null; counts: SheetCounts }>({ caseId: null, counts: {} });
  const version = useRef(0);
  const inFlight = useRef(new Set<string>());
  const errorRef = useRef(onError);
  useEffect(() => {
    errorRef.current = onError;
  });

  const read = useCallback((id: string | null) => {
    const mine = ++version.current;
    // Whatever the older version still has in flight is no longer wanted.
    if (inFlight.current.size > 0) void queryCancel([...inFlight.current]).catch(() => {});
    inFlight.current = new Set();
    if (id === null) return; // `counts` is read only for the case it was made for, so nothing is left to clear
    for (const kind of KINDS) {
      const queryId = newQueryId();
      inFlight.current.add(queryId);
      QUERIES[kind](`shell:count:${kind}`, queryId).then(
        (l) => {
          if (mine !== version.current) return;
          inFlight.current.delete(queryId);
          const text = l.truncated ? `${LIMIT}+` : String(l.items.length);
          setCounts((c) => ({ caseId: id, counts: { ...(c.caseId === id ? c.counts : {}), [kind]: text } }));
        },
        (e) => {
          if (mine !== version.current) return;
          inFlight.current.delete(queryId);
          if (!isStale(e)) errorRef.current(e);
        },
      );
    }
  }, []);

  useEffect(() => {
    read(caseId);
  }, [caseId, read]);
  useEffect(() => {
    const v = version;
    return () => {
      v.current++; // the screen is gone: nothing in flight is drawn
    };
  }, []);
  useCaseChanged(() => {
    if (caseId !== null) read(caseId);
  });
  return counts.caseId === caseId ? counts.counts : {};
}
