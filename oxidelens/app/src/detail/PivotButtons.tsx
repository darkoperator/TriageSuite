// *Around this row*: the rows of the same host within ±1 min, ±5 min and ±1 h of the row's time, each count from the
// engine (`around_totals`), each button opening the merged tab for that window. A row without a time says so instead.
import type { ReactNode } from "react";
import type { WindowTotal } from "../contract/generated/WindowTotal";

/** The windows asked for, in seconds. */
export const PIVOT_WINDOWS = [60, 300, 3600];

const LABEL: Record<number, string> = { 60: "±1 min", 300: "±5 min", 3600: "±1 h" };
export const windowLabel = (seconds: number): string => LABEL[seconds] ?? `±${seconds} s`;

export interface PivotButtonsProps {
  /** `null` while the counts are being read; `"untimed"` for a row with no primary time. */
  totals: WindowTotal[] | "untimed" | null;
  /** Why a window cannot be opened (no host, or a time the app cannot hand on); `null` when it can. */
  blocked: string | null;
  onOpen(seconds: number): void;
}

export default function PivotButtons({ totals, blocked, onOpen }: PivotButtonsProps): ReactNode {
  return (
    <section className="detail-section" aria-label="Around this row">
      <h3>Around this row</h3>
      {totals === "untimed" ? (
        <p className="detail-note">no time on this row</p>
      ) : totals === null ? (
        <p className="detail-note">Counting…</p>
      ) : (
        <div className="pivot">
          {totals.map((t) => (
            <button
              key={t.seconds}
              type="button"
              disabled={blocked !== null}
              title={blocked ?? "Open these rows in a merged tab"}
              onClick={() => onOpen(t.seconds)}
            >
              {`${windowLabel(t.seconds)} · ${t.rows.toLocaleString("en-US")}`}
            </button>
          ))}
        </div>
      )}
    </section>
  );
}
