// The app shell: the welcome screen with no case open, and with one the top bar, the left and right panes (each
// foldable to a rail), the tab strip and its tab bodies, the status bar and the key strip. It owns what the window
// keeps: the settings, the case summary, the open tabs, the job line. Screens inside it read `useShell()`.
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  caseClose, caseSummary, isCmdError, loadFile, loadRun, onJob, onSettingsDamaged, sessions as readSessions,
  settingsGet, settingsUpdate,
} from "../api";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { ColumnLayout } from "../contract/generated/ColumnLayout";
import type { DamagedSettings } from "../contract/generated/DamagedSettings";
import type { Session } from "../contract/generated/Session";
import type { Settings } from "../contract/generated/Settings";
import { pickCsvFile, pickRunFolder } from "../dialogs";
import { displaySafe } from "../text";
import Welcome from "../welcome/Welcome";
import { ShellCtx, type ShellContext } from "./context";
import KeyStrip from "./KeyStrip";
import LeftPane from "./LeftPane";
import { useShellKeys } from "./keys";
import Rail from "./Rail";
import RightPane from "./RightPane";
import TabPanel from "./TabPanel";
import StatusBar from "./StatusBar";
import TabStrip from "./TabStrip";
import {
  activate, closeTab, initialTabs, loadTabs, openDataset, saveTabs, OVERVIEW_ID, ScrollStore, type TabsState,
} from "./tabs";
import TopBar from "./TopBar";
import { useCaseChanged } from "./useCaseChanged";
import "./shell.css";

const describe = (e: unknown): string =>
  isCmdError(e) ? `${e.code}: ${e.message}` : e instanceof Error ? e.message : String(e);

/** An answer that lost to a newer query, or to the case closing, is not an error to show. */
const isStale = (e: unknown) => isCmdError(e) && (e.code === "superseded" || e.code === "interrupted");

export default function Shell() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const settingsRef = useRef<Settings | null>(null);
  const [damaged, setDamaged] = useState<DamagedSettings | null>(null);
  const [summary, setSummary] = useState<CaseSummary | null>(null);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [tabs, setTabs] = useState<TabsState>(initialTabs);
  const [tabsCase, setTabsCase] = useState<string | null>(null);
  const [change, setChange] = useState({ generation: 0, kinds: [] as string[] });
  const [job, setJob] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [gridStatus, setGridStatus] = useState<ReactNode>(null);
  const [rightPane, setRightPane] = useState<ReactNode>(null);
  const [store] = useState(() => new ScrollStore());
  const readSeq = useRef(0);

  const reportError = useCallback((e: unknown) => setError(describe(e)), []);

  // ---- settings ---------------------------------------------------------------------------------------------------
  const applySettings = useCallback((s: Settings) => {
    settingsRef.current = s;
    setSettings(s);
  }, []);

  // Settings writes go through one queue. `confirmed` is the latest reply's settings; `queued` holds changes not yet
  // sent. Each write is `confirmed` plus the queued changes, so two quick changes can never write an older object
  // last, and the UI follows the reply once the queue is empty (until then it shows the intended state).
  const confirmed = useRef<Settings | null>(null);
  const queued = useRef<Partial<Settings>>({});
  const writing = useRef(false);

  const pump = useCallback(async () => {
    if (writing.current) return;
    writing.current = true;
    try {
      while (Object.keys(queued.current).length > 0 && confirmed.current) {
        const patch = queued.current;
        queued.current = {};
        try {
          const r = await settingsUpdate({ ...confirmed.current, ...patch });
          confirmed.current = r.settings;
          if (r.damaged === null) setDamaged(null);
        } catch (e) {
          reportError(e); // the change is dropped; the UI falls back to what the file holds
        }
      }
      if (confirmed.current) applySettings(confirmed.current);
    } finally {
      writing.current = false;
    }
  }, [applySettings, reportError]);

  /** Saves a change to the settings, shown at once; the file follows through the queue. */
  const updateSettings = useCallback(
    (patch: Partial<Settings>) => {
      const base = settingsRef.current;
      if (!base) return;
      queued.current = { ...queued.current, ...patch };
      applySettings({ ...base, ...patch });
      void pump();
    },
    [applySettings, pump],
  );

  /** One dataset's column layout: the layouts map is rebuilt from the latest (optimistic) settings, then queued. */
  const saveLayout = useCallback(
    (relation: string, layout: ColumnLayout | null) => {
      const base = settingsRef.current;
      if (!base) return;
      const layouts = { ...base.layouts };
      if (layout === null) delete layouts[relation];
      else layouts[relation] = layout;
      updateSettings({ layouts });
    },
    [updateSettings],
  );

  /** Settings read from the backend: the new confirmed state, shown unless a change is still on its way. */
  const adoptSettings = useCallback(
    (s: Settings) => {
      confirmed.current = s;
      if (!writing.current && Object.keys(queued.current).length === 0) applySettings(s);
    },
    [applySettings],
  );

  useEffect(() => {
    let live = true;
    settingsGet().then(
      (r) => {
        if (!live) return;
        adoptSettings(r.settings);
        setDamaged(r.damaged);
      },
      (e) => live && reportError(e),
    );
    let stopDamaged: (() => void) | null = null;
    let stopJob: (() => void) | null = null;
    void onSettingsDamaged((d) => live && setDamaged(d)).then((s) => (live ? (stopDamaged = s) : s()));
    void onJob((e) => live && setJob(e.phase === "started" ? e.label : null)).then((s) => (live ? (stopJob = s) : s()));
    return () => {
      live = false;
      stopDamaged?.();
      stopJob?.();
    };
  }, [adoptSettings, reportError]);

  // ---- the case ---------------------------------------------------------------------------------------------------
  const readSessionsNow = useCallback(() => {
    const seq = ++readSeq.current;
    readSessions("shell:sessions").then(
      (s) => seq === readSeq.current && setSessions(s),
      (e) => {
        if (!isStale(e) && !(isCmdError(e) && e.code === "no_case")) reportError(e);
      },
    );
  }, [reportError]);

  const enterCase = useCallback(
    (s: CaseSummary) => {
      const restored = loadTabs(s.case_id, s.relations.map((r) => r.name));
      store.replace(restored.scroll);
      setTabs(restored.state);
      setTabsCase(s.case_id);
      setSummary(s);
      setError(null);
      setGridStatus(null);
      setRightPane(null);
      readSessionsNow();
      settingsGet().then((r) => adoptSettings(r.settings), reportError); // the open put the case first in `recent`
    },
    [adoptSettings, readSessionsNow, reportError, store],
  );

  // A write changed the case: re-read what is shown. Tabs, filters, selection and scroll are not touched.
  const summarySeq = useRef(0);
  useCaseChanged((e) => {
    if (!summary) return;
    setChange({ generation: e.generation, kinds: e.kinds });
    const seq = ++summarySeq.current;
    caseSummary("shell:summary").then(
      (s) => seq === summarySeq.current && setSummary(s),
      (err) => {
        if (isCmdError(err) && err.code === "no_case") setSummary(null);
        else if (!isStale(err)) reportError(err);
      },
    );
    readSessionsNow();
  });

  const close = async () => {
    try {
      await caseClose();
      if (summary) saveTabs(summary.case_id, tabs, store.offsets); // a pending scroll save must not be lost
      setSummary(null);
      setSessions([]);
      setTabsCase(null);
    } catch (e) {
      reportError(e);
    }
  };

  const runLoad = async (pick: () => Promise<string | null>, load: (path: string) => Promise<unknown>) => {
    setLoading(true);
    setError(null);
    try {
      const path = await pick();
      if (path !== null) await load(path);
    } catch (e) {
      reportError(e);
    } finally {
      setLoading(false);
    }
  };

  // ---- tabs -------------------------------------------------------------------------------------------------------
  useEffect(() => {
    if (summary && tabsCase === summary.case_id) saveTabs(summary.case_id, tabs, store.offsets);
  }, [tabs, tabsCase, summary, store]);

  const caseId = summary?.case_id ?? null;
  const scroll = useMemo(
    () => ({
      get: (id: string) => store.get(id),
      // Saved a moment after the last change, through the tabs effect (which drops a closed tab's offset).
      set: (id: string, offset: number) => store.set(id, offset, () => setTabs((t) => ({ ...t }))),
    }),
    [store],
  );
  useEffect(() => () => store.dispose(), [caseId, store]);

  const openTab = useCallback((relation: string) => setTabs((t) => openDataset(t, relation)), []);

  // ---- panes ------------------------------------------------------------------------------------------------------
  const togglePane = useCallback(
    (side: "left_pane" | "right_pane") => {
      const cur = settingsRef.current?.[side];
      if (cur) updateSettings({ [side]: cur === "open" ? "rail" : "open" });
    },
    [updateSettings],
  );
  useShellKeys(
    { toggleLeft: () => togglePane("left_pane"), toggleRight: () => togglePane("right_pane") },
    summary !== null,
  );

  const ctx: ShellContext | null = useMemo(
    () =>
      summary && settings
        ? {
            summary, settings, change, openDataset: openTab, setGridStatus, setRightPane, rightPane, scroll, reportError, saveLayout,
          }
        : null,
    [summary, settings, change, openTab, rightPane, scroll, reportError, saveLayout],
  );

  const dismissNote = damaged && (
    <div className="settings-note" role="status" aria-label="Settings notice">
      <span>
        {`The settings file could not be read (${displaySafe(damaged.reason)}); defaults are in use.`}
        {damaged.file !== null && ` The old file was kept as ${displaySafe(damaged.file)}.`}
      </span>
      <button type="button" className="icon-button" aria-label="Dismiss" onClick={() => setDamaged(null)}>×</button>
    </div>
  );

  if (!summary || !ctx) {
    return (
      <div className="shell shell-welcome">
        {dismissNote}
        <Welcome
          recent={settings?.recent ?? []}
          onOpened={enterCase}
          onRemoveRecent={(p) => updateSettings({ recent: (settingsRef.current?.recent ?? []).filter((r) => r !== p) })}
          onError={reportError}
        />
        {error !== null && (
          <div className="status-error welcome-error" role="alert">
            {displaySafe(error)}
          </div>
        )}
      </div>
    );
  }

  const titleOf = (id: string) => (id === OVERVIEW_ID ? "Overview" : (tabs.tabs.find((t) => t.id === id)?.relation ?? id));
  const left = settings!.left_pane;
  const right = settings!.right_pane;
  const suspicious = summary.mark_counts.suspicious;

  return (
    <ShellCtx.Provider value={ctx}>
      <div className="shell">
        {dismissNote}
        <TopBar
          summary={summary}
          analyst={settings!.analyst}
          busy={loading || job !== null}
          onLoadRun={() => void runLoad(pickRunFolder, loadRun)}
          onOpenCsv={() => void runLoad(pickCsvFile, (p) => loadFile(p))}
          onClose={() => void close()}
        />
        <div className="shell-main">
          {left === "open" ? (
            <LeftPane summary={summary} sessions={sessions} onOpenDataset={openTab} onCollapse={() => togglePane("left_pane")} />
          ) : (
            <Rail side="left" suspicious={suspicious} onExpand={() => togglePane("left_pane")} />
          )}
          <div className="shell-center">
            <TabStrip
              state={tabs}
              title={titleOf}
              onSelect={(id) => setTabs((t) => activate(t, id))}
              onClose={(id) => setTabs((t) => closeTab(t, id))}
            />
            <div className="tab-bodies">
              {tabs.tabs.map((t) => (
                <TabPanel
                  key={t.id}
                  tab={t}
                  active={t.id === tabs.active}
                  scrollOffset={scroll.get(t.id)}
                  onScroll={(offset) => scroll.set(t.id, offset)}
                />
              ))}
            </div>
          </div>
          {right === "open" ? (
            <RightPane onCollapse={() => togglePane("right_pane")} />
          ) : (
            <Rail side="right" suspicious={suspicious} onExpand={() => togglePane("right_pane")} />
          )}
        </div>
        <StatusBar progress={job} error={error} zone={summary.display_zone} gridStatus={gridStatus} onDismissError={() => setError(null)} />
        <KeyStrip />
      </div>
    </ShellCtx.Provider>
  );
}
