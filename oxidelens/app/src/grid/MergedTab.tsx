// A merged tab: the rows of every dataset around one row (`merged_page` over `MergedSource::Around`), newest listing
// first as the engine orders them. Columns: mark, time (the zone's `display_time`, or the written value with a `?tz`
// badge), host, dataset, summary, and an *open* link that shows the row in its dataset's tab.
//
// One read per page, on slot `tab:<id>:merged:<offset>`; a read belongs to a key (zone and case generation) and is drawn
// only while that key is current. Reads pause while the tab is hidden.
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { isCmdError, mergedPage, newQueryId, queryCancel } from "../api";
import type { MergedView } from "../contract/generated/MergedView";
import { useShell } from "../shell/context";
import type { Tab } from "../shell/tabs";
import { displaySafe } from "../text";
import "./merged.css";

const PAGE = 200;

interface Loaded {
  key: string;
  items: MergedView[];
  truncated: boolean;
}

const obsolete = (e: unknown) => isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case");

export function MergedTab({ tab, active }: { tab: Tab; active: boolean }): ReactNode {
  const { summary, change, openRow, reportError } = useShell();
  const source = tab.merged?.source;
  const key = `${summary.display_zone}|${change.generation}`;
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [more, setMore] = useState(false);
  const keyRef = useRef(key);
  const inFlight = useRef(new Set<string>());
  useEffect(() => {
    keyRef.current = key;
  });

  /** Reads one page for `forKey`; the answer is dropped when the key moved on, and its id is cancelled by whoever leaves. */
  const read = useCallback(
    async (forKey: string, offset: number): Promise<Loaded | null> => {
      if (!source) return null;
      const id = newQueryId();
      inFlight.current.add(id);
      try {
        const page = await mergedPage(source, PAGE, offset, summary.display_zone, `tab:${tab.id}:merged:${offset}`, id);
        return keyRef.current === forKey ? { key: forKey, items: page.items, truncated: page.truncated } : null;
      } catch (e) {
        if (keyRef.current === forKey && !obsolete(e)) reportError(e);
        return null;
      } finally {
        inFlight.current.delete(id);
      }
    },
    [source, summary.display_zone, tab.id, reportError],
  );

  useEffect(() => {
    if (!active) return;
    let live = true;
    const ids = inFlight.current;
    void read(key, 0).then((first) => {
      if (live && first) setLoaded(first);
    });
    return () => {
      live = false;
      if (ids.size > 0) void queryCancel([...ids]).catch(() => {});
    };
  }, [active, key, read]);

  const current = loaded?.key === key ? loaded : null;
  const loadMore = async () => {
    if (!current || more) return;
    setMore(true);
    const next = await read(key, current.items.length);
    setMore(false);
    if (next) setLoaded((l) => (l && l.key === key ? { key, items: [...l.items, ...next.items], truncated: next.truncated } : l));
  };

  if (!source) return <p className="pane-empty">Nothing to list.</p>;
  return (
    <div className="merged">
      <table className="merged-table">
        <thead>
          <tr>
            <th aria-label="Mark" className="merged-mark" />
            <th>Time</th>
            <th>Host</th>
            <th>Dataset</th>
            <th>Summary</th>
            <th aria-label="Open" />
          </tr>
        </thead>
        <tbody>
          {(current?.items ?? []).map(({ row, display_time }) => (
            <tr key={row.row_id}>
              <td className="merged-mark">{row.mark === "suspicious" && <span className="merged-flag" role="img" aria-label="Suspicious">●</span>}</td>
              <td className="merged-time">
                {display_time !== null ? (
                  displaySafe(display_time)
                ) : row.time !== null ? (
                  <>
                    {displaySafe(row.time)}
                    <span className="badge" title="Shown as written: its zone is not known">?tz</span>
                  </>
                ) : null}
              </td>
              <td>{row.host !== null && displaySafe(row.host)}</td>
              <td>{displaySafe(row.relation)}</td>
              <td className="merged-summary">{displaySafe(row.summary)}</td>
              <td>
                <button type="button" className="merged-open" onClick={() => openRow?.(row.relation, row.row_id)}>open</button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {current === null && <p className="pane-empty">Loading…</p>}
      {current !== null && current.items.length === 0 && <p className="pane-empty">No rows in this window.</p>}
      {current?.truncated && (
        <button type="button" className="merged-more" disabled={more} onClick={() => void loadMore()}>
          Load more
        </button>
      )}
    </div>
  );
}
