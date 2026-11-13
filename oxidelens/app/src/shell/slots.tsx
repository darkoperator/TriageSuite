// The mount points later tasks fill. Each is a small component the shell renders; replace its body, keep its name.
//
//   TabBody             a tab's content. The shell keeps every open tab mounted and hides the inactive ones (`active`
//                       is false) so a grid keeps its filters, selection and scroll across tab switches; a body
//                       should pause its own reads while inactive. Task 15 renders the grid for `tab.kind === "dataset"`.
//                       Scroll: the body gets `scrollOffset` (restore it) and `onScroll(offset)` (report it).
//   SavedViewsSection   the left pane's *Saved views* section (Task 23).
//   RightPaneContent    the right pane's content (row detail, Task 22); `useShell().setRightPane(node)` sets it.
//
// The status bar's areas are fed through the context: `setGridStatus(node)` for the grid-status area; the progress
// area is the shell's own job line.
import type { ReactNode } from "react";
import { displaySafe } from "../text";
import Overview from "./Overview";
import { useShell } from "./context";
import type { Tab } from "./tabs";

export interface TabBodyProps {
  tab: Tab;
  active: boolean;
  /** The saved scroll offset of this tab: put it on your scroll container when you mount and when `active` turns true. */
  scrollOffset: number;
  /** Report your scroll container's offset here as it scrolls (the shell debounces the save per case). */
  onScroll(offset: number): void;
}

export function TabBody({ tab }: TabBodyProps): ReactNode {
  if (tab.kind === "overview") return <Overview />;
  return (
    <div className="tab-placeholder">
      The grid for <span className="mono">{displaySafe(tab.relation ?? "")}</span> arrives with the grid task.
    </div>
  );
}

export function SavedViewsSection(): ReactNode {
  return <p className="pane-empty">No saved views yet.</p>;
}

export function RightPaneContent(): ReactNode {
  const { rightPane } = useShell();
  return rightPane ?? <p className="pane-empty">Select a row and press Enter to see its detail.</p>;
}
