// The left pane's *Saved views*: the views and presets that match any relation of the case, each marked with its kind
// (and its author when an agent made it). Clicking one applies it to the tab being shown when it matches that tab's
// relation, else opens the first relation it matches. Right-click: Export… and Delete. *Import view…* reads a file.
import { useCallback, useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { isCmdError, viewApply, viewDelete, viewExport, viewImport, viewsMatching } from "../api";
import type { MatchingView } from "../contract/generated/MatchingView";
import type { SavedView } from "../contract/generated/SavedView";
import { pickViewFile, pickViewSavePath } from "../dialogs";
import { useDialogFocus } from "../grid/dialogFocus";
import { useShell } from "../shell/context";
import { datasetTabId } from "../shell/tabs";
import { displaySafe } from "../text";

/** Views read per page. */
const PAGE = 200;

const stale = (e: unknown) => isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case");

/** A view with the case's relations it matches, sorted by name. */
type Entry = MatchingView;

/** The author to show: an agent's view names it; an analyst's view (the common case) shows nothing. */
const agentAuthor = (v: SavedView): string | null => [v.updated_by, v.created_by].find((a) => a.startsWith("agent:")) ?? null;

export default function ViewList(): ReactNode {
  const { summary, change, activeTab, deliverApplied, reportError } = useShell();
  const [entries, setEntries] = useState<Entry[] | null>(null);
  const [more, setMore] = useState(false);
  const [version, setVersion] = useState(0);
  const [menu, setMenu] = useState<{ entry: Entry; x: number; y: number; opener: HTMLElement } | null>(null);

  // A write that touched the views re-reads the list; other changes leave it alone.
  const [seen, setSeen] = useState(change.generation);
  if (change.generation !== seen) {
    setSeen(change.generation);
    if (change.kinds.includes("views")) setVersion((v) => v + 1);
  }

  // The engine matches the patterns against the case's relations and answers once, each view with the relations it
  // matches. A page is PAGE views; *More…* reads the next one at the offset of what is loaded. The list is read again
  // when a write touched the views or the relations changed; until page 0 of that reload is back the old list stays
  // but *More…* is off, and an answer that belongs to an older read (a More page a reload overtook) is dropped. The
  // pages are read at different moments, so a view saved between two of them can shift a page boundary (a view seen
  // twice or missed) until the next reload.
  const readKey = `${summary.relations.map((r) => r.name).join(",")}|${version}`;
  const [loadedKey, setLoadedKey] = useState<string | null>(null);
  const reloading = loadedKey !== readKey;
  const readSeq = useRef(0);
  const readPage = useCallback(
    (offset: number, replace: boolean, key: string) => {
      const seq = ++readSeq.current;
      viewsMatching(PAGE, offset, replace ? "views:list" : "views:more").then(
        (l) => {
          if (seq !== readSeq.current) return;
          setEntries((old) => (replace || old === null ? l.items : [...old, ...l.items]));
          setMore(l.truncated);
          if (replace) setLoadedKey(key);
        },
        (e: unknown) => {
          if (seq === readSeq.current && !stale(e)) reportError(e);
        },
      );
    },
    [reportError],
  );
  useEffect(() => {
    readPage(0, true, readKey);
    return () => {
      readSeq.current += 1;
    };
  }, [readKey, readPage]);

  // After a confirmed delete the view's button is gone: focus goes to the next view, else the previous, else the heading.
  const itemRefs = useRef(new Map<number, HTMLButtonElement>());
  const afterDelete = useRef<{ gone: number; to: number | null } | null>(null);
  useEffect(() => {
    const pending = afterDelete.current;
    if (!pending || entries === null || entries.some((e) => e.view.id === pending.gone)) return;
    afterDelete.current = null;
    const target = pending.to === null ? null : itemRefs.current.get(pending.to);
    if (target) target.focus();
    else document.getElementById("sec-views")?.focus();
  }, [entries]);

  const apply = useCallback(
    async (entry: Entry) => {
      const here = activeTab?.kind === "dataset" && activeTab.relation !== null && entry.relations.includes(activeTab.relation) ? activeTab : null;
      const relation = here?.relation ?? entry.relations[0];
      if (relation === undefined) return;
      try {
        const applied = await viewApply(entry.view.id, relation, "views:apply");
        deliverApplied?.({
          tabId: here ? here.id : datasetTabId(relation), relation, name: entry.view.name, applied, openRelation: here === null,
        });
      } catch (e) {
        if (!stale(e)) reportError(e);
      }
    },
    [activeTab, deliverApplied, reportError],
  );

  const importView = async () => {
    try {
      const path = await pickViewFile();
      if (path !== null) await viewImport(path);
    } catch (e) {
      reportError(e);
    }
  };

  const exportView = async (entry: Entry) => {
    try {
      const path = await pickViewSavePath();
      if (path !== null) await viewExport(entry.view.id, path, "views:export");
    } catch (e) {
      reportError(e);
    }
  };

  const remove = async (entry: Entry) => {
    const list = entries ?? [];
    const at = list.findIndex((e) => e.view.id === entry.view.id);
    afterDelete.current = { gone: entry.view.id, to: (list[at + 1] ?? list[at - 1])?.view.id ?? null };
    try {
      await viewDelete(entry.view.id);
    } catch (e) {
      afterDelete.current = null;
      reportError(e); // the engine refuses when it cannot be done (an analyst's view is not an agent's to delete)
    }
  };

  const closeMenu = useCallback(() => setMenu(null), []);

  return (
    <>
      <div className="pane-actions">
        <button type="button" className="pane-action" onClick={() => void importView()}>Import view…</button>
      </div>
      {entries === null ? null : entries.length === 0 ? (
        <p className="pane-empty">No saved views yet.</p>
      ) : (
        <ul className="pane-list" aria-label="Saved views">
          {entries.map((entry) => {
            const author = agentAuthor(entry.view);
            return (
              <li key={entry.view.id}>
                <button
                  type="button"
                  className="pane-item"
                  ref={(el) => {
                    if (el) itemRefs.current.set(entry.view.id, el);
                    else itemRefs.current.delete(entry.view.id);
                  }}
                  title={`Applies to ${entry.relations.length} dataset${entry.relations.length === 1 ? "" : "s"}`}
                  onClick={() => void apply(entry)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    setMenu({ entry, x: e.clientX, y: e.clientY, opener: e.currentTarget });
                  }}
                >
                  <span className="pane-item-name">{displaySafe(entry.view.name)}</span>
                  <span className="pane-item-count">
                    {entry.view.kind === "preset" ? "preset" : ""}
                    {author !== null && ` by ${displaySafe(author)}`}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
      {more && (
        <div className="pane-actions">
          <button type="button" className="pane-action" disabled={reloading} onClick={() => readPage(entries?.length ?? 0, false, readKey)}>More…</button>
        </div>
      )}
      {menu && <ViewMenu opener={menu.opener} x={menu.x} y={menu.y} name={menu.entry.view.name} onExport={() => void exportView(menu.entry)} onDelete={() => void remove(menu.entry)} onClose={closeMenu} />}
    </>
  );
}

/** The right-click menu of a view. Delete asks once more, in the menu, before it deletes. */
function ViewMenu(p: { opener: HTMLElement; x: number; y: number; name: string; onExport(): void; onDelete(): void; onClose(): void }): ReactNode {
  const box = useRef<HTMLDivElement>(null);
  const [asking, setAsking] = useState(false);
  const { onClose } = p;

  // Focus goes to the first entry, and back to the view that opened the menu when it closes.
  useDialogFocus(box, p.opener);
  useEffect(() => {
    if (asking) box.current?.querySelector<HTMLElement>("button")?.focus();
  }, [asking]);
  useEffect(() => {
    const away = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", away);
    return () => document.removeEventListener("mousedown", away);
  }, [onClose, asking]);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const items = [...(box.current?.querySelectorAll<HTMLElement>("button") ?? [])];
      const at = items.indexOf(document.activeElement as HTMLElement);
      items[(at + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
    }
  };

  return (
    <div ref={box} className="cell-menu" role="menu" aria-label="Saved view" style={{ left: p.x, top: p.y }} onKeyDown={onKeyDown}>
      {asking ? (
        <>
          <p className="menu-ask">{`Delete “${displaySafe(p.name)}”?`}</p>
          <button type="button" onClick={() => { onClose(); p.onDelete(); }}>Delete view</button>
          <button type="button" onClick={onClose}>Cancel</button>
        </>
      ) : (
        <>
          <button type="button" role="menuitem" onClick={() => { onClose(); p.onExport(); }}>Export…</button>
          <button type="button" role="menuitem" onClick={() => setAsking(true)}>Delete</button>
        </>
      )}
    </div>
  );
}
