// The Date range tab, for a TIMESTAMP column: from and to as `YYYY-MM-DD HH:MM:SS` in the display zone, either may be
// left out. The engine makes the chip (`date_range_chip`) and does the zone arithmetic; an ambiguous or nonexistent local
// time comes back as its message, shown under the boxes. The front end never parses or converts the text.
import { useState, type ReactNode } from "react";
import { dateRangeChip, isCmdError } from "../api";
import type { Chip } from "../contract/generated/Chip";
import { displaySafe } from "../text";
import { filterColumns, removeChip, upsertChip } from "./chips";

export interface DateTabProps {
  tabId: string;
  relation: string;
  column: string;
  zone: string;
  chips: readonly Chip[];
  applyChips(edit: (chips: Chip[]) => Chip[]): void;
  onDone(): void;
}

/** This column's date range chips (the engine's ids, found by origin and column). */
const rangesOf = (chips: readonly Chip[], column: string) =>
  chips.filter((c) => c.origin === "date_range" && filterColumns(c.filter)[0] === column);

export default function DateTab(p: DateTabProps): ReactNode {
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const existing = rangesOf(p.chips, p.column);

  const apply = async () => {
    setBusy(true);
    try {
      const chip = await dateRangeChip(
        p.relation, p.column, from.trim() === "" ? null : from.trim(), to.trim() === "" ? null : to.trim(), p.zone,
        `tab:${p.tabId}:daterange`,
      );
      p.applyChips((chips) => upsertChip(chips.filter((c) => !rangesOf([c], p.column).length || c.id === chip.id), chip));
      p.onDone();
    } catch (e) {
      setError(isCmdError(e) ? e.message : e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="ft-date"
      onSubmit={(e) => {
        e.preventDefault();
        if (!busy) void apply();
      }}
    >
      <label className="ft-field">
        From
        <input type="text" value={from} placeholder="YYYY-MM-DD HH:MM:SS" spellCheck={false} onChange={(e) => { setFrom(e.target.value); setError(null); }} />
      </label>
      <label className="ft-field">
        To
        <input type="text" value={to} placeholder="YYYY-MM-DD HH:MM:SS" spellCheck={false} onChange={(e) => { setTo(e.target.value); setError(null); }} />
      </label>
      <div className="ft-note">{`Times are in ${displaySafe(p.zone)}.`}</div>
      {error !== null && <div role="alert" className="form-error">{displaySafe(error)}</div>}
      <div className="ft-buttons">
        <button type="submit" disabled={busy}>Apply</button>
        {existing.length > 0 && (
          <button
            type="button"
            onClick={() => {
              p.applyChips((chips) => existing.reduce((acc, c) => removeChip(acc, c.id), chips));
              p.onDone();
            }}
          >
            Remove
          </button>
        )}
      </div>
    </form>
  );
}
