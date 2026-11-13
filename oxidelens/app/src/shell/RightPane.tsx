// The right pane: row detail (Task 22 puts it in `RightPaneContent`).
import { RightPaneContent } from "./slots";

export default function RightPane({ onCollapse }: { onCollapse(): void }) {
  return (
    <aside className="pane pane-right" aria-label="Row detail">
      <header className="pane-head">
        <span>Row detail</span>
        <button type="button" className="icon-button" aria-label="Collapse right pane" title="Collapse right pane (Ctrl+])" onClick={onCollapse}>
          »
        </button>
      </header>
      <div className="pane-body">
        <RightPaneContent />
      </div>
    </aside>
  );
}
