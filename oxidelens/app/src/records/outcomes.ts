// What the screens say about a row write: the engine's own reasons, shown in the status bar without the error code (a
// refusal is the analyst's answer, not a fault), and the 10,000-row cap checked before any round trip (ruling S10).
import { isCmdError } from "../api";
import type { PromoteOutcome } from "../contract/generated/PromoteOutcome";
import type { RowOutcome } from "../contract/generated/RowOutcome";
import { MAX_ACTION_ROWS } from "../grid/selection";

const fmt = (n: number) => n.toLocaleString("en-US");

/** The refusal of a selection past the cap, worded as the engine words it. */
export const tooManyRows = (n: number): string => `select at most ${fmt(MAX_ACTION_ROWS)} rows (you have ${fmt(n)})`;

/** An answer that lost to a newer query, or to the case closing: not drawn, never an error. */
export const isStale = (e: unknown): boolean =>
  isCmdError(e) && (e.code === "superseded" || e.code === "interrupted" || e.code === "no_case");

/**
 * Hands `report` what to show for a failed write or read: the engine's message alone for a refusal of what was asked
 * (`invalid`, `refused`, `not_found`), the code and message for anything else, nothing for a stale answer.
 */
export function reportFailure(e: unknown, report: (e: unknown) => void): void {
  if (isStale(e)) return;
  if (isCmdError(e) && (e.code === "invalid" || e.code === "refused" || e.code === "not_found")) report(new Error(e.message));
  else report(e);
}

/** A failure as a dialog shows it: the engine's message for a refusal of what was asked, else the code with it. */
export function messageOf(e: unknown): string {
  if (isCmdError(e)) return e.code === "invalid" || e.code === "refused" || e.code === "not_found" ? e.message : `${e.code}: ${e.message}`;
  return e instanceof Error ? e.message : String(e);
}

/** The engine's text for the rows it did not take: the first reason, and how many more there were. Null for none. */
function describe(reasons: string[]): string | null {
  if (reasons.length === 0) return null;
  const more = reasons.length - 1;
  return more === 0 ? reasons[0] : `${reasons[0]} (and ${fmt(more)} more ${more === 1 ? "row" : "rows"} not changed)`;
}

/** The rows a batch write (S, U, T) did not take. A row already as asked is done and says nothing. */
export const rowRefusals = (outcomes: readonly RowOutcome[]): string | null =>
  describe(outcomes.filter((o) => !o.done).map((o) => o.reason ?? `row ${o.row_id} was not changed`));

/** The rows M made no entry for and found none on. */
export const promoteRefusals = (outcomes: readonly PromoteOutcome[]): string | null =>
  describe(outcomes.filter((o) => o.entry_id === null).map((o) => o.reason ?? `row ${o.row_id} was not promoted`));

/** The entry M answers with for the first row that has one: the new entry, or the one the row is already evidence on. */
export const promotedEntry = (outcomes: readonly PromoteOutcome[]): number | null =>
  outcomes.find((o) => o.entry_id !== null)?.entry_id ?? null;
