// Where a row came from: host and run, the evidence file and where the record sits in it, the output file and whether
// the run's manifest names it. The manifest records paths, not hashes, so the check says the run published the file;
// it never says anything about content, and it says nothing at all when the engine could not tell (`null`).
import type { ReactNode } from "react";
import type { Provenance as ProvenanceDoc } from "../contract/generated/Provenance";
import { displaySafe } from "../text";

/** The locator columns the engine knows, named as an analyst says them (`record 88213`); any other shows its column name. */
const LOCATOR_LABEL: Record<string, string> = { EventRecordId: "record", EntryNumber: "entry" };

const locatorText = ([column, value]: [string, string]): string => `${LOCATOR_LABEL[column] ?? column} ${value}`;

export default function Provenance({ provenance: p }: { provenance: ProvenanceDoc }): ReactNode {
  const line = (label: string, value: string | null, extra?: ReactNode) =>
    value === null && !extra ? null : (
      <div className="prov-line">
        <dt>{label}</dt>
        <dd>
          {value !== null && displaySafe(value)}
          {extra}
        </dd>
      </div>
    );
  return (
    <section className="detail-section" aria-label="Provenance">
      <h3>Provenance</h3>
      <dl className="prov">
        {line("Host", p.host)}
        {line("Run", p.run_id)}
        {line("Evidence file", p.evidence_path, p.locator && <span className="prov-locator">{` · ${displaySafe(locatorText(p.locator))}`}</span>)}
        {line(
          "Output file",
          p.output_file,
          p.output_file_listed === true ? (
            <span className="prov-ok">{" ✓ listed in the run manifest"}</span>
          ) : p.output_file_listed === false ? (
            <span className="prov-warn">{" not listed"}</span>
          ) : null,
        )}
      </dl>
    </section>
  );
}
