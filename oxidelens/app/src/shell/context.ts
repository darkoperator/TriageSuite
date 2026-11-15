// What a screen inside the shell can ask of it. A tab body, the right pane's content and the saved-views section read
// this through `useShell()`.
import { createContext, useContext, type ReactNode } from "react";
import type { AppliedView } from "../contract/generated/AppliedView";
import type { ColourRule } from "../contract/generated/ColourRule";
import type { ColumnLayout } from "../contract/generated/ColumnLayout";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { Settings } from "../contract/generated/Settings";
import type { Tab } from "./tabs";

/** A saved view the engine resolved for one relation, on its way to the tab that takes it. */
export interface AppliedDelivery {
  /** The tab that takes it (a dataset tab, which `deliverApplied` opens first when `openRelation` is set). */
  tabId: string;
  relation: string;
  /** The view's name, for the confirmation. */
  name: string;
  applied: AppliedView;
  openRelation: boolean;
}

/** A running CSV export: the query id `query_cancel` names, and whether its times are written in the display zone. */
export interface ExportJob {
  queryId: string;
  zoneTimes: boolean;
}

export interface ShellContext {
  /** The tab being shown (the saved views section applies a view to it when the view matches its relation). */
  activeTab?: Tab | null;
  /** A view's answer for a tab: the tab replaces its state and group-by with it and writes its layout and colour rules. */
  pendingApply?: AppliedDelivery | null;
  clearPendingApply?(): void;
  deliverApplied?(d: AppliedDelivery): void;
  /** The export that is running, if any (one at a time, across tabs): the status bar offers *Cancel* while it is set. */
  exportJob?: ExportJob | null;
  /** Whether an export is running right now (read at the moment of asking, not at the last render). */
  exportRunning?(): boolean;
  /** Takes the one export slot for `job`; false (and nothing changes) when an export is already running. */
  beginExport?(job: ExportJob): boolean;
  /** Frees the export slot, only if `queryId` still holds it, so a job never frees another's. */
  endExport?(queryId: string): void;
  summary: CaseSummary;
  settings: Settings;
  /** Rises with every `case-changed`; `kinds` says what the write touched. A screen re-reads when its kind is listed. */
  change: { generation: number; kinds: string[] };
  /** Opens (or switches to) the dataset tab of a relation. */
  openDataset(relation: string): void;
  /** Opens a tab the grid made (a group's rows) or the row detail made (a merged tab); a tab with the same id is switched to. */
  openScoped(tab: Tab): void;
  /** Opens (or switches to) a relation's dataset tab and shows one row of it in the right pane (the merged tab's *open* link). */
  openRow?(relation: string, rowId: number): void;
  /** A row a tab was asked to show (`openRow`); the dataset tab it names takes it and calls `clearPendingRow`. */
  pendingRow?: { tabId: string; rowId: number } | null;
  clearPendingRow?(): void;
  /** What the status bar's grid-status area shows (rows, filters, hidden columns...); `null` clears it. */
  setGridStatus(node: ReactNode): void;
  /** The shown dataset tab is grouped (true) or flat: the key strip lists only the keys live on it. */
  setGrouped?(grouped: boolean): void;
  /** What the right pane shows (row detail); `null` shows its empty hint. */
  setRightPane(node: ReactNode): void;
  /** What the right pane currently holds. */
  rightPane: ReactNode;
  /** A tab's scroll offset, kept per case in localStorage. */
  scroll: { get(tabId: string): number; set(tabId: string, offset: number): void };
  /**
   * Saves one dataset's column layout in the settings (`null` forgets it). It goes through the shell's settings write
   * queue, built on the latest layouts, so a change made while another write is in flight is never lost.
   */
  saveLayout(relation: string, layout: ColumnLayout | null): void;
  /**
   * Saves one dataset's colour rules (the whole list, in order) in the settings; `null` forgets the entry, so the
   * relation follows the engine's defaults again. Through the same write queue as the layouts.
   */
  saveColourRules(relation: string, rules: ColourRule[] | null): void;
  /** Switches the histogram strip on or off; shown at once, written through the settings queue. */
  setHistogram(on: boolean): void;
  /** Shows a command error (code and message) in the status bar. */
  reportError(e: unknown): void;
}

export const ShellCtx = createContext<ShellContext | null>(null);

export function useShell(): ShellContext {
  const c = useContext(ShellCtx);
  if (!c) throw new Error("useShell outside the shell");
  return c;
}
