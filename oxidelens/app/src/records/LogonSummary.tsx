// An account's logon summary (D21, ruling S6), in the account's editor: its successful and failed logons by host and
// logon type, as the engine reads them from the event-log dataset. Each number opens the rows behind it as a dataset
// tab filtered by the document the engine gives with the count, so the tab shows exactly those rows; its chip reads as
// the account, the type and the host (`Logons: ACME\jdoe · type 10 · WS01 · success`), not as the regex it holds.
//
// A computed panel, not a judgement: no flag, no logon path. "No event-log dataset in this case" when the engine read
// none. The rows that do not read as an account and a logon type are counted, never guessed.
import { useEffect, type ReactNode } from "react";
import { logonSummary } from "../api";
import type { AccountRecord } from "../contract/generated/AccountRecord";
import type { LogonCount } from "../contract/generated/LogonCount";
import type { LogonSummary } from "../contract/generated/LogonSummary";
import { clipLabel, hash } from "../grid/chips";
import { initialGridState } from "../grid/gridState";
import { stableKey } from "../grid/useGridData";
import { useShell } from "../shell/context";
import { displaySafe } from "../text";
import { reportFailure } from "./outcomes";
import { accountText } from "./RecordEditor";
import { fmt } from "./RecordSheet";
import { useCaseRead } from "./useCaseRead";

const SLOT = "sheet:accounts:logons";
const NO_HOST = "no host";

/** One line of the table: a host and a logon type, with its success and failure counts (either may be absent). */
interface Line {
  host: string | null;
  type: number;
  success: LogonCount | null;
  failure: LogonCount | null;
}

/** The engine's rows as lines, in its order (by host, type, success before failure): only put side by side. */
function linesOf(s: LogonSummary): Line[] {
  const lines = new Map<string, Line>();
  for (const c of s.rows) {
    const key = JSON.stringify([c.host, c.logon_type]);
    const line = lines.get(key) ?? { host: c.host, type: c.logon_type, success: null, failure: null };
    line[c.outcome] = c;
    lines.set(key, line);
  }
  return [...lines.values()];
}

/** What a count's chip and tab say: the account, the type, the host and the outcome. Raw text: the tab and the chip
 *  draw it through `displaySafe`. */
export const logonLabel = (account: string, c: LogonCount): string =>
  `Logons: ${account} · type ${c.logon_type} · ${c.host ?? NO_HOST} · ${c.outcome}`;

export default function LogonSummaryPanel({ account }: { account: { kind: "account" } & AccountRecord }): ReactNode {
  const { change, reportError, openScoped } = useShell();
  const read = useCaseRead<LogonSummary>(`logons:${account.id}`, change.generation, (next) => logonSummary(account.id, SLOT, next()), reportError);
  useEffect(() => {
    if (read?.error !== undefined) reportFailure(read.error, reportError);
  }, [read, reportError]);
  const summary = read?.value;
  const who = accountText(account);

  const open = (source: string, c: LogonCount) => {
    const label = clipLabel(logonLabel(who, c));
    const chip = { id: "logons", label, enabled: true, origin: "advanced" as const, filter: c.filter };
    const seed = { state: { ...initialGridState(source), chips: [chip] }, by: [] as string[] };
    openScoped({ id: `logons:${hash(stableKey([source, c.filter]))}`, kind: "dataset", relation: source, title: label, seed });
  };
  const count = (source: string, c: LogonCount | null) => {
    if (c === null || c.count === 0) return "0";
    return (
      <button
        type="button"
        className="link-button"
        aria-label={`Open the ${fmt(c.count)} ${c.outcome} logons, type ${c.logon_type}, ${displaySafe(c.host ?? NO_HOST)}`}
        onClick={() => open(source, c)}
      >
        {fmt(c.count)}
      </button>
    );
  };

  return (
    <section className="editor-section" aria-label="Logon summary">
      <h3 className="editor-heading">Logon summary</h3>
      {read === null && <p className="form-hint">Reading the event log…</p>}
      {summary && summary.source === null && <p className="form-hint">No event-log dataset in this case.</p>}
      {summary && summary.source !== null && (
        <>
          <p className="form-hint">{`From ${displaySafe(summary.source)}: Security 4624 (success) and 4625 (failure).`}</p>
          {summary.rows.length === 0 ? (
            <p className="form-hint">{`No logons of this account in ${displaySafe(summary.source)}.`}</p>
          ) : (
            <table className="logon-table" aria-label="Logons">
              <thead>
                <tr>
                  <th>Host</th>
                  <th>Logon type</th>
                  <th className="col-num">Success</th>
                  <th className="col-num">Failure</th>
                </tr>
              </thead>
              <tbody>
                {linesOf(summary).map((l) => (
                  <tr key={JSON.stringify([l.host, l.type])}>
                    <td>{displaySafe(l.host ?? NO_HOST)}</td>
                    <td>{l.type}</td>
                    <td className="col-num">{count(summary.source!, l.success)}</td>
                    <td className="col-num">{count(summary.source!, l.failure)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {summary.unparsed > 0 && (
            <p className="form-hint">{`${fmt(summary.unparsed)} logon events did not read as an account and a logon type, and are not counted.`}</p>
          )}
        </>
      )}
    </section>
  );
}
