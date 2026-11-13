// One tab's panel. It is the scroll container of a body that does not scroll itself (the Overview): it reports its
// scroll offset and restores the saved one whenever the tab becomes visible. A body that owns its own scroll container
// (the grid) takes `scrollOffset` and `onScroll` from `TabBody`'s props instead.
import { useEffect, useLayoutEffect, useRef } from "react";
import { TabBody } from "./slots";
import { panelDomId, tabDomId } from "./TabStrip";
import type { Tab } from "./tabs";

export default function TabPanel({
  tab, active, scrollOffset, onScroll,
}: {
  tab: Tab;
  active: boolean;
  scrollOffset: number;
  onScroll(offset: number): void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const latest = useRef(scrollOffset);
  useEffect(() => {
    latest.current = scrollOffset;
  });
  // A hidden element cannot be scrolled, so the offset is put back each time the tab is shown (and on first mount).
  useLayoutEffect(() => {
    if (active && ref.current) ref.current.scrollTop = latest.current;
  }, [active]);
  return (
    <div
      ref={ref}
      role="tabpanel"
      id={panelDomId(tab.id)}
      aria-labelledby={tabDomId(tab.id)}
      className="tab-body"
      hidden={!active}
      onScroll={(e) => active && onScroll(e.currentTarget.scrollTop)}
    >
      <TabBody tab={tab} active={active} scrollOffset={scrollOffset} onScroll={onScroll} />
    </div>
  );
}
