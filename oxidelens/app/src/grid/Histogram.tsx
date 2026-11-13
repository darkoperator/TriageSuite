// The histogram strip above the grid: the filtered rows per time bucket as plain SVG bars, a column picker, a legend and a
// brush. Dragging over the bars selects a run of buckets; on release the engine makes a date-range chip from the first
// bucket's `label` to the last bucket's `end_label` (`date_range_chip`), which replaces this column's earlier brush chip.
// Double-click clears it. The strip does no date arithmetic: bucket labels and the zone are the engine's text, a refused
// range (a bucket inside a repeated DST hour) shows the engine's message. Bars sit at the engine's bucket positions (at most one column per 6 px; buckets that share a column are summed),
// so a gap in time is a gap on screen.
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode } from "react";
import { dateRangeChip, isCmdError } from "../api";
import type { Chip } from "../contract/generated/Chip";
import type { Bucket } from "../contract/generated/Bucket";
import type { ColumnMeta } from "../contract/generated/ColumnMeta";
import type { GridState } from "../contract/generated/GridState";
import type { Histogram as HistogramData } from "../contract/generated/Histogram";
import { displaySafe } from "../text";
import { filterColumns } from "./chips";
import { stableKey } from "./useGridData";
import { useHistogram } from "./useHistogram";

/** One bucket per this many pixels of the strip, at most `MAX_BUCKETS` of them. */
const PX_PER_BUCKET = 6;
const MAX_BUCKETS = 240;
const PLOT_HEIGHT = 48;
/** A bar's slot is never wider than this, so a few buckets do not become a few slabs. */
const MAX_SLOT = 24;
const fmt = (n: number) => n.toLocaleString("en-US");

/** `bucket_seconds` as words (the ladder runs from a second to thirty days, a calendar month). */
export function widthText(seconds: number): string {
  const unit = (n: number, one: string) => `${fmt(n)} ${one}${n === 1 ? "" : "s"}`;
  if (seconds === 2_592_000) return "1 month";
  if (seconds === 604_800) return "1 week";
  if (seconds % 86_400 === 0) return unit(seconds / 86_400, "day");
  if (seconds % 3_600 === 0) return unit(seconds / 3_600, "hour");
  if (seconds % 60 === 0) return unit(seconds / 60, "minute");
  return unit(seconds, "second");
}

/** This column's chips made by the brush. */
const brushed = (chips: readonly Chip[], column: string) =>
  chips.filter((c) => c.origin === "histogram" && filterColumns(c.filter)[0] === column);

/** One drawn column: the buckets the engine put in it, summed. */
export interface Column {
  count: number;
  suspicious: number;
  first: Bucket;
  last: Bucket;
}

/**
 * The strip's columns: at most `maxColumns`, however many positions (`slots`) the engine counted (it can count more than
 * the width asked for when no bucket width fits the span). A bucket goes to column `floor(index * columns / slots)`:
 * whole-number scaling of the engine's positions, no dates. Buckets of one column are summed.
 */
export function columnsOf(h: HistogramData | null, maxColumns: number): { columns: Map<number, Column>; count: number } {
  if (h === null || h.buckets.length === 0) return { columns: new Map(), count: 1 };
  const slots = Math.max(1, h.slots, ...h.buckets.map((b) => b.index + 1)); // a bad answer cannot draw outside the strip
  const count = Math.max(1, Math.min(slots, maxColumns));
  const columns = new Map<number, Column>();
  for (const b of h.buckets) {
    const at = Math.floor((b.index * count) / slots);
    const c = columns.get(at);
    if (c) {
      c.count += b.count;
      c.suspicious += b.suspicious;
      c.last = b;
    } else columns.set(at, { count: b.count, suspicious: b.suspicious, first: b, last: b });
  }
  return { columns, count };
}

export interface HistogramProps {
  tabId: string;
  relation: string;
  /** Every column of the relation; the picker offers its TIMESTAMP ones. */
  columns: readonly ColumnMeta[];
  primaryTime: string | null;
  /** The grid's state: the strip counts what the grid counts. */
  state: GridState;
  zone: string;
  /** Rises when the case changed under the strip. */
  version: number;
  /** False while the tab is not shown. */
  enabled: boolean;
  applyChips(edit: (chips: Chip[]) => Chip[]): void;
  onError(e: unknown): void;
}

export default function Histogram(p: HistogramProps): ReactNode {
  const timeColumns = useMemo(() => p.columns.filter((c) => c.sql_type === "TIMESTAMP"), [p.columns]);
  const [picked, setPicked] = useState<string | null>(null);
  // The column shown is the column asked for: the picked one, else the primary time, else the first TIMESTAMP column (a
  // picked column the relation no longer has, or a primary time that is not a timestamp, is not used). Null only when
  // the relation has no TIMESTAMP column.
  const has = (n: string | null): n is string => n !== null && timeColumns.some((c) => c.name === n);
  const column: string | null = has(picked) ? picked : has(p.primaryTime) ? p.primaryTime : (timeColumns[0]?.name ?? null);

  const plot = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = plot.current;
    if (!el) return;
    const read = () => setWidth(el.offsetWidth);
    read();
    if (typeof ResizeObserver === "undefined") return;
    const watch = new ResizeObserver(read);
    watch.observe(el);
    return () => watch.disconnect();
  }, []);
  const maxBuckets = Math.min(MAX_BUCKETS, Math.max(1, Math.floor(width / PX_PER_BUCKET)));

  const { histogram, error } = useHistogram(
    p.tabId, p.state, column, p.zone, maxBuckets, p.enabled && width > 0 && timeColumns.length > 0, p.version, p.onError,
  );
  const shownColumn = column ?? "";

  // ---- the brush ----------------------------------------------------------------------------------------------------
  // The drag and the brush error remember the view (`viewKey`) they were made on and are shown only on that view.
  const [dragState, setDragState] = useState<{ view: string; from: number; to: number } | null>(null);
  const dragRef = useRef<{ view: string; from: number; to: number } | null>(null);
  const [brushState, setBrushState] = useState<{ view: string; message: string } | null>(null);
  const seq = useRef(0);
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);
  // What the brush needs when the button comes up, read from the latest render.
  // A brush still waiting for the engine, a drag under way and an error shown belong to the view they were made on: a new
  // zone, column or state (or a case change) ends them, so an old answer never lands on the new view. A layout effect, so
  // this happens before anything else can see the change; what is shown is judged against the view key.
  const viewKey = useMemo(() => stableKey([p.state, shownColumn, p.zone, p.version]), [p.state, shownColumn, p.zone, p.version]);
  const latest = useRef({ histogram, p, viewKey, maxColumns: maxBuckets });
  useEffect(() => {
    latest.current = { histogram, p, viewKey, maxColumns: maxBuckets };
  });
  useLayoutEffect(() => {
    seq.current += 1;
    dragRef.current = null;
  }, [viewKey]);
  const drag = dragState !== null && dragState.view === viewKey ? dragState : null;
  const brushError = brushState !== null && brushState.view === viewKey ? brushState.message : null;

  const release = useCallback(async () => {
    const d = dragRef.current;
    dragRef.current = null;
    setDragState(null);
    const { histogram: h, p: now, viewKey: view, maxColumns } = latest.current;
    if (!d || !h || d.view !== view) return;
    // The brush selects columns; the range is from the first to the last non-empty bucket inside them (the engine's
    // labels). A range with no bucket in it does nothing.
    const lo = Math.min(d.from, d.to);
    const hi = Math.max(d.from, d.to);
    const inside = [...columnsOf(h, maxColumns).columns]
      .filter(([at]) => at >= lo && at <= hi)
      .sort(([a], [b]) => a - b)
      .map(([, c]) => c);
    if (inside.length === 0) return;
    const mine = ++seq.current;
    setBrushState(null);
    try {
      const chip = await dateRangeChip(
        now.relation, h.column, inside[0].first.label, inside[inside.length - 1].last.end_label, now.zone, `tab:${now.tabId}:hist-brush`,
      );
      // A newer brush, a double-click or a closed strip ends this one: its chip is not applied.
      if (mine !== seq.current || !live.current) return;
      const made: Chip = { ...chip, origin: "histogram" };
      now.applyChips((chips) => [...chips.filter((c) => !brushed([c], h.column).length), made]);
    } catch (e) {
      if (mine !== seq.current || !live.current) return;
      if (isCmdError(e) && (e.code === "superseded" || e.code === "interrupted")) return;
      if (isCmdError(e) && e.code === "invalid") setBrushState({ view, message: e.message });
      else now.onError(e);
    }
  }, []);

  useEffect(() => {
    if (drag === null) return;
    const up = () => void release();
    window.addEventListener("mouseup", up, { once: true });
    return () => window.removeEventListener("mouseup", up);
    // the listener is for the whole drag, not for each move
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [drag === null, release]);

  const begin = (i: number) => {
    dragRef.current = { view: viewKey, from: i, to: i };
    setDragState(dragRef.current);
  };
  const extend = (i: number) => {
    if (!dragRef.current || dragRef.current.to === i) return;
    dragRef.current = { ...dragRef.current, to: i };
    setDragState(dragRef.current);
  };
  const clear = () => {
    seq.current += 1;
    setBrushState(null);
    const target = histogram?.column ?? shownColumn;
    p.applyChips((chips) => chips.filter((c) => !brushed([c], target).length));
  };

  // ---- drawing ------------------------------------------------------------------------------------------------------
  const buckets = histogram?.buckets ?? [];
  const { columns, count } = useMemo(() => columnsOf(histogram, maxBuckets), [histogram, maxBuckets]);
  const slot = Math.min(MAX_SLOT, width / count);
  const drawn = [...columns].sort(([x], [y]) => x - y);
  const tallest = Math.max(1, ...drawn.map(([, c]) => c.count));
  const marked = buckets.reduce((n, b) => n + b.suspicious, 0);
  const lo = drag ? Math.min(drag.from, drag.to) : -1;
  const hi = drag ? Math.max(drag.from, drag.to) : -1;
  const axis = [...new Set([0, Math.floor((drawn.length - 1) / 2), drawn.length - 1])].filter((i) => i >= 0 && i < drawn.length);
  /**
   * The column under the pointer, from its position in the strip (drawn at one pixel per unit); null outside the columns
   * that are drawn (the strip can be wider than they are). With `clamp`, a pointer outside gives the edge column it is
   * beyond, for a drag that started inside.
   */
  const columnAt = (e: MouseEvent<SVGSVGElement>, clamp: boolean): number | null => {
    const x = e.clientX - e.currentTarget.getBoundingClientRect().left;
    const at = Math.floor(x / slot);
    if (at >= 0 && at < count) return at;
    return clamp ? (at < 0 ? 0 : count - 1) : null;
  };
  const asText = histogram !== null && histogram.time_semantics !== "utc";
  const message = brushError ?? error;

  return (
    <section className="hist" role="region" aria-label="Histogram">
      <div className="hist-head">
        <select
          className="hist-picker"
          aria-label="Histogram column"
          value={shownColumn}
          disabled={timeColumns.length === 0}
          onChange={(e) => setPicked(e.target.value)}
        >
          {timeColumns.length === 0 && <option value="">no time column</option>}
          {timeColumns.map((c) => (
            <option key={c.name} value={c.name}>{displaySafe(c.name)}</option>
          ))}
        </select>
        <ul className="hist-legend" role="list" aria-label="Legend">
          {histogram !== null && (
            <>
              <li>{`bucket ${widthText(histogram.bucket_seconds)}`}</li>
              <li>{asText ? "as written (no zone)" : displaySafe(histogram.zone)}</li>
              <li>
                <span className="hist-amber" aria-hidden="true">●</span>
                <span>{`${fmt(marked)} marked`}</span>
              </li>
              <li>{`${fmt(histogram.untimed)} untimed`}</li>
            </>
          )}
        </ul>
      </div>
      <div ref={plot} className="hist-plot" style={{ height: PLOT_HEIGHT }}>
        {histogram !== null && buckets.length > 0 && (
          <svg
            width={width}
            height={PLOT_HEIGHT}
            viewBox={`0 0 ${width} ${PLOT_HEIGHT}`}
            role="img"
            aria-label={`Rows per ${widthText(histogram.bucket_seconds)} of ${displaySafe(histogram.column)}; drag to select a range, double-click to clear it`}
            onDoubleClick={clear}
            onMouseDown={(e) => {
              const at = e.button === 0 ? columnAt(e, false) : null;
              if (at !== null) begin(at);
            }}
            onMouseMove={(e) => {
              const at = dragRef.current ? columnAt(e, true) : null;
              if (at !== null) extend(at);
            }}
          >
            <rect className="hist-hit" x={0} y={0} width={count * slot} height={PLOT_HEIGHT} data-hit="" />
            {drawn.map(([at, c]) => {
              const h = Math.max(1, Math.round((c.count / tallest) * PLOT_HEIGHT));
              const sus = c.suspicious === 0 ? 0 : Math.min(h, Math.max(1, Math.round((c.suspicious / tallest) * PLOT_HEIGHT)));
              const x = at * slot;
              const bar = Math.max(1, slot - 1);
              const text = `${displaySafe(c.first.label)} to ${displaySafe(c.last.end_label)}: ${fmt(c.count)} rows${c.suspicious > 0 ? `, ${fmt(c.suspicious)} marked` : ""}`;
              return (
                <g key={at}>
                  <rect
                    className={at >= lo && at <= hi ? "hist-bar hist-sel" : "hist-bar"}
                    x={x + 0.5} y={PLOT_HEIGHT - h} width={bar} height={h}
                    data-bar="" data-count={c.count} aria-label={text}
                  >
                    <title>{text}</title>
                  </rect>
                  {sus > 0 && <rect className="hist-mark" x={x + 0.5} y={PLOT_HEIGHT - sus} width={bar} height={sus} />}
                </g>
              );
            })}
            {lo >= 0 && <rect className="hist-gap-sel" x={lo * slot} y={PLOT_HEIGHT - 2} width={(hi - lo + 1) * slot} height={2} />}
          </svg>
        )}
      </div>
      <div className="hist-axis" aria-hidden="true">
        {axis.map((i) => (
          <span key={i} className="hist-tick" style={{ left: Math.min(Math.max(0, width - 130), drawn[i][0] * slot) }}>
            {displaySafe(drawn[i][1].first.label)}
          </span>
        ))}
      </div>
      {message !== null && <div role="alert" className="form-error hist-error">{displaySafe(message)}</div>}
    </section>
  );
}
