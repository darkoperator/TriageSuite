// What a screen inside the shell can ask of it. A tab body, the right pane's content and the saved-views section read
// this through `useShell()`.
import { createContext, useContext, type ReactNode } from "react";
import type { ColumnLayout } from "../contract/generated/ColumnLayout";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { Settings } from "../contract/generated/Settings";
import type { Tab } from "./tabs";

export interface ShellContext {
  summary: CaseSummary;
  settings: Settings;
  /** Rises with every `case-changed`; `kinds` says what the write touched. A screen re-reads when its kind is listed. */
  change: { generation: number; kinds: string[] };
  /** Opens (or switches to) the dataset tab of a relation. */
  openDataset(relation: string): void;
  /** Opens a tab the grid made (a group's rows); a tab with the same id is switched to. */
  openScoped(tab: Tab): void;
  /** What the status bar's grid-status area shows (rows, filters, hidden columns...); `null` clears it. */
  setGridStatus(node: ReactNode): void;
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
  /** Shows a command error (code and message) in the status bar. */
  reportError(e: unknown): void;
}

export const ShellCtx = createContext<ShellContext | null>(null);

export function useShell(): ShellContext {
  const c = useContext(ShellCtx);
  if (!c) throw new Error("useShell outside the shell");
  return c;
}
