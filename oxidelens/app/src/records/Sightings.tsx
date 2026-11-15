// IOC sightings (D21, ruling S5): where an IOC's value occurs in the loaded data, as the engine's exact,
// case-insensitive Find everywhere counts it (`ioc_sightings`). Computed when asked, never stored: an IOC's own
// *Find sightings*, or *Sweep all*, which runs them one IOC at a time on the one `sightings` slot with its progress
// shown and *Cancel* stopping it. What they found lives here, in the front end, until the next `case-changed`, when it
// is dropped and a sweep still running is stopped.
//
// Sightings are labelled as sightings and shown beside the entries' first and last seen, never copied into them;
// *Show hits* opens the merged tab of the find. A value the engine will not search (over 1,000 characters) says why.
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { iocSightings, newQueryId, queryCancel, recordsList, SIGHTINGS_SLOT } from "../api";
import type { IocRecord } from "../contract/generated/IocRecord";
import type { MergedSource } from "../contract/generated/MergedSource";
import type { Sightings } from "../contract/generated/Sightings";
import { clipLabel, hash } from "../grid/chips";
import { stableKey } from "../grid/useGridData";
import { useShell } from "../shell/context";
import { displaySafe } from "../text";
import { isStale, messageOf } from "./outcomes";
import { fmt } from "./RecordSheet";

export type Sighting = { state: "reading" } | { state: "found"; sightings: Sightings } | { state: "refused"; reason: string };

export interface SightingsStore {
  /** What has been found per IOC id since the last `case-changed`. */
  results: ReadonlyMap<number, Sighting>;
  /** A running sweep: how many IOCs are done of how many (`null` while they are being listed). */
  sweep: { done: number; total: number | null } | null;
  /** Reads one IOC's sightings (not while a sweep runs). */
  find(id: number): void;
  sweepAll(): void;
  cancel(): void;
}

const EMPTY: ReadonlyMap<number, Sighting> = new Map();
/** How many IOCs a sweep lists at once (the engine's `MAX_LIMIT`). */
const LIST = 1000;

/** The sightings of the IOCs sheet, for the case's write `generation`. */
export function useSightings(generation: number, report: (e: unknown) => void): SightingsStore {
  const [held, setHeld] = useState<{ generation: number; results: Map<number, Sighting> }>({ generation, results: new Map() });
  const [sweep, setSweep] = useState<{ generation: number; done: number; total: number | null } | null>(null);
  // One run at a time (a find or a sweep): `run` rises when one starts or is stopped, and an answer is kept only for
  // the run and the generation it was asked in. `inFlight` is the query id the run is waiting for.
  const run = useRef(0);
  const inFlight = useRef<string | null>(null);
  const gen = useRef(generation);
  const reportRef = useRef(report);
  useEffect(() => {
    reportRef.current = report;
  });

  const stop = useCallback(() => {
    run.current += 1;
    const id = inFlight.current;
    inFlight.current = null;
    if (id !== null) {
      void queryCancel([id]).catch((e: unknown) => {
        if (!isStale(e)) reportRef.current(e);
      });
    }
    setSweep(null);
    // What was being read is no longer: its "reading" goes.
    setHeld((h) => ({ ...h, results: new Map([...h.results].filter(([, s]) => s.state !== "reading")) }));
  }, []);

  // The case changed: what was found goes, and a sweep still running stops.
  useEffect(() => {
    if (gen.current === generation) return;
    gen.current = generation;
    stop();
    setHeld({ generation, results: new Map() });
  }, [generation, stop]);
  useEffect(() => () => stop(), [stop]);

  const put = useCallback((mine: number, at: number, id: number, s: Sighting | null) => {
    if (mine !== run.current || at !== gen.current) return;
    setHeld((h) => {
      if (h.generation !== at) return h;
      const results = new Map(h.results);
      if (s === null) results.delete(id);
      else results.set(id, s);
      return { generation: at, results };
    });
  }, []);

  /** One IOC's sightings in run `mine`; false when the run is over (stopped, superseded, or the case changed). */
  const readOne = useCallback(
    async (mine: number, at: number, id: number): Promise<boolean> => {
      put(mine, at, id, { state: "reading" });
      const q = newQueryId();
      inFlight.current = q;
      try {
        const sightings = await iocSightings(id, SIGHTINGS_SLOT, q);
        put(mine, at, id, { state: "found", sightings });
      } catch (e) {
        if (isStale(e)) {
          put(mine, at, id, null);
          return false;
        }
        put(mine, at, id, { state: "refused", reason: messageOf(e) });
      } finally {
        if (inFlight.current === q) inFlight.current = null;
      }
      return mine === run.current && at === gen.current;
    },
    [put],
  );

  const sweeping = useRef(false);
  useEffect(() => {
    sweeping.current = sweep !== null && sweep.generation === generation;
  });
  const find = useCallback(
    (id: number) => {
      if (sweeping.current) return;
      stop();
      void readOne(run.current, gen.current, id);
    },
    [stop, readOne],
  );

  const sweepAll = useCallback(() => {
    stop();
    const mine = run.current;
    const at = gen.current;
    const live = () => mine === run.current && at === gen.current;
    setSweep({ generation: at, done: 0, total: null });
    void (async () => {
      try {
        // Every live IOC, whatever the sheet shows, listed a page at a time.
        const ids: number[] = [];
        for (let offset = 0; ; offset += LIST) {
          const q = newQueryId();
          inFlight.current = q;
          const page = await recordsList({ kind: "ioc", ioc_type: null, tool: null, text: null }, LIST, offset, "sheet:iocs:sweep", q);
          if (!live()) return;
          ids.push(...page.items.map((r) => r.id));
          if (!page.truncated) break;
        }
        inFlight.current = null;
        setSweep({ generation: at, done: 0, total: ids.length });
        for (const [done, id] of ids.entries()) {
          if (!(await readOne(mine, at, id))) return;
          setSweep({ generation: at, done: done + 1, total: ids.length });
        }
      } catch (e) {
        if (live() && !isStale(e)) reportRef.current(e);
      }
      if (live()) setSweep(null);
    })();
  }, [stop, readOne]);

  const results = held.generation === generation ? held.results : EMPTY;
  const shown = sweep !== null && sweep.generation === generation ? sweep : null;
  const done = shown?.done ?? null;
  const total = shown?.total ?? null;
  return useMemo(
    () => ({ results, sweep: done === null ? null : { done, total }, find, sweepAll, cancel: stop }),
    [results, done, total, find, sweepAll, stop],
  );
}

/** The sheet's Sightings cell. */
export function sightingCell(s: Sighting | undefined): string {
  if (s === undefined) return "";
  if (s.state === "reading") return "…";
  if (s.state === "refused") return "not searched";
  return s.sightings.hits === 0 ? "not found" : fmt(s.sightings.hits);
}

/** The sweep's progress and its *Cancel*, or *Sweep all*. */
export function SweepControl({ store }: { store: SightingsStore }): ReactNode {
  if (store.sweep === null) {
    return (
      <button type="button" onClick={store.sweepAll}>
        Sweep all
      </button>
    );
  }
  const { done, total } = store.sweep;
  return (
    <>
      <span role="status" aria-label="Sweep" className="sweep-progress">
        {total === null ? "Sweeping: listing the IOCs…" : `Sweeping: ${fmt(done)} of ${fmt(total)} IOCs`}
      </span>
      <button type="button" onClick={store.cancel}>
        Cancel sweep
      </button>
    </>
  );
}

/** One IOC's sightings in its editor. */
export function SightingsPanel({ ioc, store }: { ioc: IocRecord; store: SightingsStore }): ReactNode {
  const { openScoped } = useShell();
  const s = store.results.get(ioc.id);
  const showHits = () => {
    const source: MergedSource = { find: { value: ioc.value, mode: "exact", hosts: [], from: null, to: null } };
    openScoped({ id: `hits:${hash(stableKey(source))}`, kind: "merged", relation: null, title: clipLabel(`Hits: ${ioc.value}`), merged: { source } });
  };
  const found = s?.state === "found" ? s.sightings : null;
  return (
    <section className="editor-section" aria-label="Sightings">
      <h3 className="editor-heading">Sightings</h3>
      <p className="form-hint">Where the value occurs in the loaded data now (exact, case ignored); not the entries' first and last seen.</p>
      <button type="button" className="link-button" disabled={store.sweep !== null || s?.state === "reading"} onClick={() => store.find(ioc.id)}>
        Find sightings
      </button>
      {s?.state === "reading" && <p className="form-hint">Searching…</p>}
      {s?.state === "refused" && <p className="form-error">{displaySafe(s.reason)}</p>}
      {found !== null && found.hits === 0 && <p className="form-hint">not found in the loaded data</p>}
      {found !== null && found.hits > 0 && (
        <>
          <dl className="entry-facts">
            <dt>hits</dt>
            <dd>{`${fmt(found.hits)} ${found.hits === 1 ? "hit" : "hits"}`}</dd>
            <dt>datasets</dt>
            <dd>
              <ul className="editor-list">
                {found.relations.map(([relation, n]) => (
                  <li key={relation}>{`${displaySafe(relation)} · ${fmt(n)}`}</li>
                ))}
              </ul>
            </dd>
            <dt>hosts</dt>
            <dd>{found.hosts.length === 0 ? "none" : displaySafe(found.hosts.join(", "))}</dd>
            <dt>first sighting</dt>
            <dd>{found.first === null ? "no UTC time" : displaySafe(found.first)}</dd>
            <dt>last sighting</dt>
            <dd>{found.last === null ? "no UTC time" : displaySafe(found.last)}</dd>
          </dl>
          <button type="button" className="link-button" aria-label="Show hits" onClick={showHits}>
            Show hits →
          </button>
        </>
      )}
    </section>
  );
}
