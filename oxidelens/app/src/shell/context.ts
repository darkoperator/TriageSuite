// What a screen inside the shell can ask of it. A tab body, the right pane's content and the saved-views section read
// this through `useShell()`.
import { createContext, useContext, type ReactNode } from "react";
import type { CaseSummary } from "../contract/generated/CaseSummary";
import type { Settings } from "../contract/generated/Settings";

export interface ShellContext {
  summary: CaseSummary;
  settings: Settings;
  /** Rises with every `case-changed`; `kinds` says what the write touched. A screen re-reads when its kind is listed. */
  change: { generation: number; kinds: string[] };
  /** Opens (or switches to) the dataset tab of a relation. */
  openDataset(relation: string): void;
  /** What the status bar's grid-status area shows (rows, filters, hidden columns...); `null` clears it. */
  setGridStatus(node: ReactNode): void;
  /** What the right pane shows (row detail); `null` shows its empty hint. */
  setRightPane(node: ReactNode): void;
  /** What the right pane currently holds. */
  rightPane: ReactNode;
  /** A tab's scroll offset, kept per case in localStorage. */
  scroll: { get(tabId: string): number; set(tabId: string, offset: number): void };
  /** Shows a command error (code and message) in the status bar. */
  reportError(e: unknown): void;
}

export const ShellCtx = createContext<ShellContext | null>(null);

export function useShell(): ShellContext {
  const c = useContext(ShellCtx);
  if (!c) throw new Error("useShell outside the shell");
  return c;
}
