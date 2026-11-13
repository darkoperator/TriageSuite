// What shows with no case open: New case…, Open case…, and the recent cases from the settings, each removable.
import { useState } from "react";
import { caseNew, caseOpen } from "../api";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import { pickCaseFile, pickNewCasePath } from "../dialogs";
import { displaySafe } from "../text";

/** The file name without its folder and its `.duckdb` extension: the default case name. */
export const stemOf = (path: string) => (path.split(/[\\/]/).pop() ?? path).replace(/\.duckdb$/i, "");

export default function Welcome({
  recent, onOpened, onRemoveRecent, onError,
}: {
  recent: string[];
  onOpened(summary: CaseSummary): void;
  onRemoveRecent(path: string): void;
  onError(e: unknown): void;
}) {
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<{ path: string; name: string } | null>(null);

  const run = async (work: () => Promise<CaseSummary | null>) => {
    setBusy(true);
    try {
      const s = await work();
      if (s) onOpened(s);
    } catch (e) {
      onError(e);
    } finally {
      setBusy(false);
    }
  };
  const newCase = async () => {
    try {
      let path = await pickNewCasePath();
      if (path === null) return;
      if (!/\.duckdb$/i.test(path)) path += ".duckdb";
      setPending({ path, name: stemOf(path) });
    } catch (e) {
      onError(e);
    }
  };
  const openCase = () =>
    run(async () => {
      const path = await pickCaseFile();
      return path === null ? null : caseOpen(path);
    });

  return (
    <main className="welcome" aria-label="OxideLens">
      <h1>OxideLens</h1>
      {pending === null ? (
        <div className="welcome-actions">
          <button type="button" disabled={busy} onClick={() => void newCase()}>New case…</button>
          <button type="button" disabled={busy} onClick={() => void openCase()}>Open case…</button>
        </div>
      ) : (
        <form
          className="welcome-actions"
          onSubmit={(e) => {
            e.preventDefault();
            const { path, name } = pending;
            void run(async () => {
              const s = await caseNew(path, name.trim() === "" ? stemOf(path) : name.trim());
              setPending(null);
              return s;
            });
          }}
        >
          <label>
            Case name{" "}
            <input type="text" aria-label="Case name" autoFocus value={pending.name} onChange={(e) => setPending({ ...pending, name: e.target.value })} />
          </label>
          <span className="welcome-path">{displaySafe(pending.path)}</span>
          <button type="submit" disabled={busy}>Create</button>
          <button type="button" onClick={() => setPending(null)}>Cancel</button>
        </form>
      )}
      <section aria-labelledby="recent-h">
        <h2 id="recent-h">Recent cases</h2>
        {recent.length === 0 ? (
          <p className="pane-empty">No recent cases.</p>
        ) : (
          <ul className="recent-list">
            {recent.map((p) => (
              <li key={p}>
                <button type="button" className="link" disabled={busy} onClick={() => void run(() => caseOpen(p))}>
                  {displaySafe(p)}
                </button>
                <button type="button" className="icon-button" aria-label={`Remove ${displaySafe(p)} from recent cases`} onClick={() => onRemoveRecent(p)}>
                  ×
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </main>
  );
}
