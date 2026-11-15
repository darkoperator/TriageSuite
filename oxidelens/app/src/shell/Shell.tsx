// The app shell: the welcome screen with no case open, and with one the top bar, the left and right panes (each
// foldable to a rail), the tab strip and its tab bodies, the status bar and the key strip. It owns what the window
// keeps: the settings, the case summary, the open tabs, the job line. Screens inside it read `useShell()`.
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  caseClose, caseSummary, exportReport, exportTimeline, exportWorkbook, isCmdError, newQueryId, loadFile, loadRun, onJob, onSettingsDamaged, queryCancel, recentClear, recentRemove,
  sessions as readSessions, settingsGet, settingsUpdate,
} from "../api";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import { sessionTabs } from "../views/sessions";
import type { ColumnLayout } from "../contract/generated/ColumnLayout";
import type { ColourRule } from "../contract/generated/ColourRule";
import type { DamagedSettings } from "../contract/generated/DamagedSettings";
import type { Session } from "../contract/generated/Session";
import type { Settings as SettingsData } from "../contract/generated/Settings";
import type { SettingsReply } from "../contract/generated/SettingsReply";
import { pickCsvFile, pickRecordExportPath, pickRunFolder, type RecordExport } from "../dialogs";
import Settings, { type SettingsPatch } from "../settings/Settings";
import { displaySafe } from "../text";
import { applyAppearance } from "../theme/appearance";
import Welcome from "../welcome/Welcome";
import { ShellCtx, type AppliedDelivery, type ExportJob, type ShellContext } from "./context";
import KeyStrip from "./KeyStrip";
import LeftPane from "./LeftPane";
import { useShellKeys } from "./keys";
import Rail from "./Rail";
import type { MarkState } from "../contract/generated/MarkState";
import type { RecordKind } from "../contract/generated/RecordKind";
import { MARK_NAME } from "../records/MarkGlyph";
import RightPane from "./RightPane";
import TabPanel from "./TabPanel";
import StatusBar from "./StatusBar";
import TabStrip from "./TabStrip";
import {
  activate, closeTab, datasetTabId, initialTabs, loadTabs, SHEETS, openDataset, openScoped, saveTabs, sheetTabId, OVERVIEW_ID, ScrollStore, type SheetKind, type Tab, type TabsState,
} from "./tabs";
import TopBar from "./TopBar";
import { useCaseChanged } from "./useCaseChanged";
import { useSheetCounts } from "./useSheetCounts";
import { useZoneOffset } from "./useZoneOffset";
import "./shell.css";

/** The pinned sheet each record kind lives on. */
const SHEET_OF: Record<RecordKind, SheetKind> = { host: "hosts", account: "accounts", ioc: "iocs" };

const describe = (e: unknown): string =>
  isCmdError(e) ? `${e.code}: ${e.message}` : e instanceof Error ? e.message : String(e);

/** An answer that lost to a newer query, or to the case closing, is not an error to show. */
const isStale = (e: unknown) => isCmdError(e) && (e.code === "superseded" || e.code === "interrupted");

/** Asks the engine to stop a running job by its query id; the job's own answer says how it ended. */
async function cancelJob(queryId: string, report: (e: unknown) => void): Promise<void> {
  try {
    await queryCancel([queryId]);
  } catch (e) {
    report(e);
  }
}

export default function Shell() {
  const [settings, setSettings] = useState<SettingsData | null>(null);
  const settingsRef = useRef<SettingsData | null>(null);
  const [damaged, setDamaged] = useState<DamagedSettings | null>(null);
  const [summary, setSummary] = useState<CaseSummary | null>(null);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [tabs, setTabs] = useState<TabsState>(initialTabs);
  const [tabsCase, setTabsCase] = useState<string | null>(null);
  const [change, setChange] = useState({ generation: 0, kinds: [] as string[] });
  const [job, setJob] = useState<{ kind: string; label: string } | null>(null);
  // A running export's query id: the status bar offers *Cancel* while one is set.
  const [exportJob, setExportJobState] = useState<ExportJob | null>(null);
  // The shell owns the one export slot. A ref is the truth (two starts in one tick cannot both win); the state is for drawing.
  const exportRef = useRef<ExportJob | null>(null);
  const exportRunning = useCallback(() => exportRef.current !== null, []);
  const beginExport = useCallback((job: ExportJob) => {
    if (exportRef.current !== null) return false;
    exportRef.current = job;
    setExportJobState(job);
    return true;
  }, []);
  const endExport = useCallback((queryId: string) => {
    if (exportRef.current?.queryId !== queryId) return;
    exportRef.current = null;
    setExportJobState(null);
  }, []);
  const [pendingApply, setPendingApply] = useState<AppliedDelivery | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // A neutral line beside the job line (an export was cancelled); not an error.
  const [note, setNote] = useState<string | null>(null);
  const [gridStatus, setGridStatus] = useState<ReactNode>(null);
  const [rightPane, setRightPane] = useState<ReactNode>(null);
  // Whether the shown dataset tab is grouped, as it says (`setGrouped`): the key strip follows it.
  const [grouped, setGrouped] = useState(false);
  const [store] = useState(() => new ScrollStore());
  const zoneOffset = useZoneOffset(summary, change.generation);
  const readSeq = useRef(0);

  const reportError = useCallback((e: unknown) => setError(describe(e)), []);

  // ---- settings ---------------------------------------------------------------------------------------------------
  const applySettings = useCallback((s: SettingsData) => {
    settingsRef.current = s;
    setSettings(s);
  }, []);

  // Settings writes go through one queue. `confirmed` is the latest reply's settings; `queued` holds changes not yet
  // sent. Each write is `confirmed` plus the queued changes, so two quick changes can never write an older object
  // last, and the UI follows the reply once the queue is empty (until then it shows the intended state).
  const confirmed = useRef<SettingsData | null>(null);
  // The queue is an ordered list. Consecutive plain changes coalesce; an awaited save (`saveSettings`) is its own entry,
  // written alone and in order, so its refusal drops only its own patch and its waiter gets its own write's outcome.
  type Waiter = { resolve(r: SettingsReply): void; reject(e: unknown): void };
  type Entry = { patch: SettingsPatch; waiter: Waiter | null };
  const queued = useRef<Entry[]>([]);
  const writing = useRef(false);
  const [actor, setActor] = useState("");
  // Counts the forget answers taken in. A settings answer to a request sent before the latest one may have been
  // computed before the forget, so its `recent` is stale: the confirmed list (the forget's, or newer) is kept instead.
  const recentGen = useRef(0);
  const withFreshRecent = useCallback((s: SettingsData, sentAt: number): SettingsData => {
    if (sentAt === recentGen.current || !confirmed.current) return s;
    return { ...s, recent: confirmed.current.recent };
  }, []);

  const pump = useCallback(async () => {
    if (writing.current) return;
    writing.current = true;
    try {
      while (queued.current.length > 0 && confirmed.current) {
        const { patch, waiter } = queued.current.shift()!;
        try {
          const sentAt = recentGen.current;
          const answer = await settingsUpdate({ ...confirmed.current, ...patch });
          const r = { ...answer, settings: withFreshRecent(answer.settings, sentAt) };
          confirmed.current = r.settings;
          setActor(r.actor);
          if (r.damaged === null) setDamaged(null);
          waiter?.resolve(r);
        } catch (e) {
          // The entry's change is dropped; the UI falls back to what the file holds. A caller that awaits shows the refusal itself.
          if (waiter) waiter.reject(e);
          else reportError(e);
        }
      }
      if (confirmed.current) applySettings(confirmed.current);
    } finally {
      writing.current = false;
    }
  }, [applySettings, reportError, withFreshRecent]);

  /** Saves a change to the settings, shown at once; the file follows through the queue. */
  const enqueue = useCallback(
    (patch: SettingsPatch, waiter: Waiter | null) => {
      const base = settingsRef.current;
      if (!base) return false;
      const last = queued.current[queued.current.length - 1];
      if (waiter === null && last && last.waiter === null) last.patch = { ...last.patch, ...patch };
      else queued.current.push({ patch, waiter });
      applySettings({ ...base, ...patch });
      void pump();
      return true;
    },
    [applySettings, pump],
  );
  const updateSettings = useCallback((patch: SettingsPatch) => void enqueue(patch, null), [enqueue]);

  /**
   * Like `updateSettings`, for a change whose refusal the caller shows: resolves with the reply of the write that carried
   * the patch, rejects with that write's error. It rides the same queue, so no older object is written last.
   */
  const saveSettings = useCallback(
    (patch: SettingsPatch) =>
      new Promise<SettingsReply>((resolve, reject) => {
        if (!enqueue(patch, { resolve, reject })) reject({ code: "internal", message: "the settings are not loaded yet" });
      }),
    [enqueue],
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

  /** One dataset's colour rules: the whole map is rebuilt from the latest (optimistic) settings, then queued. */
  const saveColourRules = useCallback(
    (relation: string, rules: ColourRule[] | null) => {
      const base = settingsRef.current;
      if (!base) return;
      const colour_rules = { ...base.colour_rules };
      if (rules === null) delete colour_rules[relation];
      else colour_rules[relation] = rules;
      updateSettings({ colour_rules });
    },
    [updateSettings],
  );

  const setHistogram = useCallback((on: boolean) => updateSettings({ histogram: on }), [updateSettings]);

  /**
   * Forgets one recent case or all of them through the backend's own command, outside the queue. The answer's list is
   * taken into both the confirmed and the shown settings. A settings answer already on its way may carry the list from
   * before the forget; `recentGen` makes it keep this one.
   */
  const forgetRecent = useCallback(
    (path: string | null) => {
      (path === null ? recentClear() : recentRemove(path)).then(
        (r) => {
          const recent = r.settings.recent;
          recentGen.current++;
          if (confirmed.current) confirmed.current = { ...confirmed.current, recent };
          if (settingsRef.current) applySettings({ ...settingsRef.current, recent });
          setActor(r.actor);
        },
        reportError,
      );
    },
    [applySettings, reportError],
  );

  /** Settings read from the backend: the new confirmed state, shown unless a change is still on its way. */
  const adoptSettings = useCallback(
    (s: SettingsData, sentAt: number) => {
      const fresh = withFreshRecent(s, sentAt);
      confirmed.current = fresh;
      if (!writing.current && queued.current.length === 0) applySettings(fresh);
    },
    [applySettings, withFreshRecent],
  );

  useEffect(() => {
    let live = true;
    const sentAt = recentGen.current;
    settingsGet().then(
      (r) => {
        if (!live) return;
        adoptSettings(r.settings, sentAt);
        setActor(r.actor);
        setDamaged(r.damaged);
      },
      (e) => live && reportError(e),
    );
    let stopDamaged: (() => void) | null = null;
    let stopJob: (() => void) | null = null;
    void onSettingsDamaged((d) => live && setDamaged(d)).then((s) => (live ? (stopDamaged = s) : s()));
    void onJob((e) => live && setJob(e.phase === "started" ? { kind: e.kind, label: e.label } : null)).then((s) => (live ? (stopJob = s) : s()));
    return () => {
      live = false;
      stopDamaged?.();
      stopJob?.();
    };
  }, [adoptSettings, reportError]);

  // The theme, the font size and the row height follow the settings (shown at once, ahead of the file).
  useEffect(() => {
    if (settings) applyAppearance(settings);
  }, [settings]);

  const [settingsOpen, setSettingsOpen] = useState<HTMLElement | null>(null);

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

  // Rises each time a case is entered (opened or made) or closed. An answer for the case shown before (its summary
  // after a write, its export's ending) carries the epoch it was asked under and is drawn only while that still holds,
  // on success and on failure: a late `no_case` must not close the next case, nor a late cancel note follow into it.
  const caseEpoch = useRef(0);

  const enterCase = useCallback(
    (s: CaseSummary) => {
      caseEpoch.current++;
      const restored = loadTabs(s.case_id, s.relations.map((r) => r.name));
      store.replace(restored.scroll);
      setTabs(restored.state);
      setTabsCase(s.case_id);
      setSummary(s);
      setError(null);
      setNote(null);
      setGridStatus(null);
      setRightPane(null);
      readSessionsNow();
      const sentAt = recentGen.current;
      settingsGet().then((r) => (adoptSettings(r.settings, sentAt), setActor(r.actor)), reportError); // the open put the case first in `recent`
    },
    [adoptSettings, readSessionsNow, reportError, store],
  );

  // A write changed the case: re-read what is shown. Tabs, filters, selection and scroll are not touched.
  const summarySeq = useRef(0);
  useCaseChanged((e) => {
    if (!summary) return;
    setChange({ generation: e.generation, kinds: e.kinds });
    const seq = ++summarySeq.current;
    const epoch = caseEpoch.current;
    const current = () => seq === summarySeq.current && epoch === caseEpoch.current;
    caseSummary("shell:summary").then(
      (s) => current() && setSummary(s),
      (err) => {
        if (!current()) return;
        if (isCmdError(err) && err.code === "no_case") setSummary(null);
        else if (!isStale(err)) reportError(err);
      },
    );
    readSessionsNow();
  });

  /** Asks for the file, then writes the case record as the export job the status bar can cancel. */
  const runRecordExport = async (kind: RecordExport) => {
    if (!summary || exportRunning()) return;
    const epoch = caseEpoch.current;
    let path: string | null;
    try {
      path = await pickRecordExportPath(kind, summary.name);
    } catch (e) {
      if (epoch === caseEpoch.current) reportError(e);
      return;
    }
    // The case was closed or another entered while the save dialog was up: the export was for the case gone.
    if (path === null || epoch !== caseEpoch.current) return;
    const queryId = newQueryId();
    setNote(null);
    if (!beginExport({ queryId, zoneTimes: false })) return setError("An export is running.");
    try {
      const write = kind === "timeline" ? exportTimeline : kind === "workbook" ? exportWorkbook : exportReport;
      await write(path, "case:export", queryId);
    } catch (e) {
      // An ending that comes after the case was closed or another entered is not drawn in the case shown now.
      if (epoch !== caseEpoch.current) return;
      // Cancel is a neutral line; a superseded read or a closed case shows nothing (APP.md). A refusal (the case
      // file, a source's evidence) is shown as the engine worded it, without its code.
      if (isCmdError(e) && e.code === "interrupted") setNote("Export cancelled.");
      else if (isCmdError(e) && (e.code === "superseded" || e.code === "no_case")) return;
      else if (isCmdError(e)) setError(e.message);
      else reportError(e);
    } finally {
      endExport(queryId);
    }
  };

  const close = async () => {
    try {
      await caseClose();
      caseEpoch.current++;
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
  const sheetCounts = useSheetCounts(caseId, reportError);
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
  const openScopedTab = useCallback((tab: Tab) => setTabs((t) => openScoped(t, tab)), []);
  // The merged tab's *open* link: the dataset tab opens and shows the row; the tab takes the request once it is mounted.
  const [pendingRow, setPendingRow] = useState<{ tabId: string; rowId: number } | null>(null);
  const openRow = useCallback((relation: string, rowId: number) => {
    setTabs((t) => openDataset(t, relation));
    setPendingRow({ tabId: datasetTabId(relation), rowId });
  }, []);
  const clearPendingRow = useCallback(() => setPendingRow(null), []);
  // A sheet asked to select a record or an entry: the sheet's tab is shown, and the sheet takes the request (it is always
  // mounted, being pinned).
  const [pendingSelect, setPendingSelect] = useState<{ sheet: SheetKind; id: number } | null>(null);
  const openSheet = useCallback((kind: RecordKind, id?: number) => {
    setTabs((t) => activate(t, sheetTabId(SHEET_OF[kind])));
    if (id !== undefined) setPendingSelect({ sheet: SHEET_OF[kind], id });
  }, []);
  const openEntry = useCallback((id: number) => {
    setTabs((t) => activate(t, sheetTabId("timeline")));
    setPendingSelect({ sheet: "timeline", id });
  }, []);
  const clearPendingSelect = useCallback(() => setPendingSelect(null), []);
  // A saved view applied from the left pane: opens the relation's tab when asked, then the tab takes the answer.
  const deliverApplied = useCallback((d: AppliedDelivery) => {
    if (d.openRelation) setTabs((t) => openDataset(t, d.relation));
    setPendingApply(d);
  }, []);
  const clearPendingApply = useCallback(() => setPendingApply(null), []);
  // A session opens one tab per relation, scoped to the hosts the session names for it; the first one is shown.
  const openSession = useCallback((s: Session) => {
    const made = sessionTabs(s);
    setTabs((t) => {
      let next = t;
      for (const tab of made) next = openScoped(next, tab);
      return made.length > 0 ? activate(next, made[0].id) : next;
    });
  }, []);

  // A mark count in the left pane: the worklist of every row with that mark, across the datasets (a merged tab).
  const openWorklist = useCallback((mark: MarkState) => {
    const source = { marked: { marks: [mark] } };
    setTabs((t) => openScoped(t, { id: `worklist:${mark}`, kind: "merged", relation: null, title: `${MARK_NAME[mark]} rows`, merged: { source } }));
  }, []);

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
            summary, settings, change, openDataset: openTab, openScoped: openScopedTab, openRow, openSheet, openEntry, pendingSelect, clearPendingSelect, pendingRow, clearPendingRow, setGridStatus, setRightPane, setGrouped, rightPane, scroll, reportError, saveLayout, saveColourRules, setHistogram,
            activeTab: tabs.tabs.find((t) => t.id === tabs.active) ?? null, pendingApply, clearPendingApply, deliverApplied, exportJob, exportRunning, beginExport, endExport,
          }
        : null,
    [summary, settings, change, openTab, openScopedTab, openRow, openSheet, openEntry, pendingSelect, clearPendingSelect, pendingRow, clearPendingRow, rightPane, scroll, reportError, saveLayout, saveColourRules, setHistogram, tabs, pendingApply, clearPendingApply, deliverApplied, exportJob, exportRunning, beginExport, endExport],
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
          onRemoveRecent={(p) => forgetRecent(p)}
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

  const activeTab = tabs.tabs.find((t) => t.id === tabs.active);
  const activeKind = activeTab?.kind ?? null;
  const titleOf = (id: string) => {
    const t = tabs.tabs.find((x) => x.id === id);
    if (id === OVERVIEW_ID) return "Overview";
    if (t?.sheet) return SHEETS.find((x) => x.kind === t.sheet)?.title ?? id;
    return t?.title ?? t?.relation ?? id;
  };
  const left = settings!.left_pane;
  const right = settings!.right_pane;

  return (
    <ShellCtx.Provider value={ctx}>
      <div className="shell">
        {dismissNote}
        <TopBar
          summary={summary}
          analyst={settings!.analyst}
          busy={loading || job !== null}
          exporting={exportJob !== null}
          onExport={(kind) => void runRecordExport(kind)}
          onLoadRun={() => void runLoad(pickRunFolder, loadRun)}
          onOpenCsv={() => void runLoad(pickCsvFile, (p) => loadFile(p))}
          onClose={() => void close()}
          onSettings={setSettingsOpen}
        />
        {settingsOpen !== null && (
          <Settings
            settings={settings!} actor={actor} update={updateSettings} save={saveSettings} onForgetRecent={() => forgetRecent(null)} returnTo={settingsOpen}
            onClose={() => setSettingsOpen(null)}
          />
        )}
        <div className="shell-main">
          {left === "open" ? (
            <LeftPane summary={summary} sessions={sessions} onOpenDataset={openTab} onOpenSession={openSession} onOpenWorklist={openWorklist} onCollapse={() => togglePane("left_pane")} />
          ) : (
            <Rail side="left" counts={summary.mark_counts} onExpand={() => togglePane("left_pane")} />
          )}
          <div className="shell-center">
            <TabStrip
              state={tabs}
              title={titleOf}
              count={(id) => {
                const kind = tabs.tabs.find((t) => t.id === id)?.sheet;
                return kind ? (sheetCounts[kind] ?? null) : null;
              }}
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
            <Rail side="right" counts={summary.mark_counts} onExpand={() => togglePane("right_pane")} />
          )}
        </div>
        <StatusBar progress={job?.label ?? null} progressKind={job?.kind ?? null} onCancel={exportJob === null ? null : () => void cancelJob(exportJob.queryId, reportError)} exportZone={exportJob?.zoneTimes ? summary.display_zone : null} note={note} error={error} zone={summary.display_zone} zoneOffset={zoneOffset} gridStatus={gridStatus} onDismissError={() => setError(null)} />
        <KeyStrip kind={activeKind === "dataset" && grouped ? "grouped" : activeTab?.sheet === "timeline" ? "timeline" : activeKind} />
      </div>
    </ShellCtx.Provider>
  );
}
